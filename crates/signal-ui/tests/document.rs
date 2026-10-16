//! M2.8-12 (auditor-1-edited-metadata, AC17): a `.signal` project's
//! working folder is edited when its content differs from the file, whatever
//! its files' sizes and times say, and closing never removes such a folder
//! unless its edits are let go. Audit P2: the display's comparison never
//! holds up a refresh, and a stale one never clears a newer edit.

use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};

use signal_project::archive;
use signal_ui::document::{COMPARE_EVERY, Document};

fn base(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("signal-ui-document-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A same-length edit of `project.toml`, its modification time put back.
fn hidden_edit(dir: &Path) {
    hidden_edit_of(dir, "gain = 0.33", "gain = 0.44");
}

/// `from` replaced by `to` (as long) in `project.toml`, its modification
/// time put back.
fn hidden_edit_of(dir: &Path, from: &str, to: &str) {
    let toml = dir.join("project.toml");
    let modified = fs::metadata(&toml).unwrap().modified().unwrap();
    let text = fs::read_to_string(&toml).unwrap();
    let edited = text.replace(from, to);
    assert_ne!(text, edited);
    assert_eq!(text.len(), edited.len());
    fs::write(&toml, edited).unwrap();
    fs::File::options()
        .write(true)
        .open(&toml)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(fs::metadata(&toml).unwrap().modified().unwrap(), modified);
}

#[test]
fn an_edit_that_keeps_sizes_and_times_is_still_an_edit() {
    let base = base("hidden");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();

    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    assert!(!document.check());
    hidden_edit(&working);
    assert!(document.check(), "the content differs from the file");
    assert_eq!(
        document.close(false).as_deref(),
        Some(working.as_path()),
        "kept, with its edits"
    );
    assert!(
        fs::read_to_string(working.join("project.toml"))
            .unwrap()
            .contains("gain = 0.44")
    );

    // The same edit let go: removed.
    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    hidden_edit(&working);
    assert!(document.check());
    assert_eq!(document.close(true), None);
    assert!(!working.exists());
    fs::remove_dir_all(&base).unwrap();
}

/// auditor-1-edited-metadata (AC17, the display): the poll behind the
/// toolbar's Edited and the window title shows an edit that keeps sizes and
/// times within COMPARE_EVERY (and one look, 250 ms); an unchanged folder
/// stays not edited.
#[test]
fn the_display_shows_a_hidden_edit_within_the_bound() {
    let base = base("display");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    let bound = COMPARE_EVERY + std::time::Duration::from_millis(250);

    let mut clean = Document::open(&file, &base.join("Open")).unwrap();
    let mut edited = Document::open(&file, &base.join("Open")).unwrap();
    hidden_edit(edited.working());
    let started = std::time::Instant::now();
    let mut shown = None;
    while started.elapsed() <= bound + std::time::Duration::from_millis(500) {
        assert!(!clean.refresh(), "an unchanged folder is not edited");
        if shown.is_none() && edited.refresh() {
            shown = Some(started.elapsed());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let shown = shown.expect("the hidden edit is shown");
    assert!(shown <= bound, "shown after {shown:?}, bound {bound:?}");
    assert!(!clean.refresh());
    clean.close(true);
    edited.close(true);
    fs::remove_dir_all(&base).unwrap();
}

/// Refreshes until `done` holds of the document (each refresh a look
/// apart), at most 20 s.
fn refresh_until(document: &mut Document, what: &str, done: impl Fn(&mut Document) -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < until {
        document.refresh();
        if done(document) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// `project.toml` touched: a new modification time, the same content.
fn touch(dir: &Path) {
    let toml = dir.join("project.toml");
    let modified = fs::metadata(&toml).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&toml)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified - std::time::Duration::from_secs(60)))
        .unwrap();
}

/// Audit P2 (document-check-stall): the display's comparison runs on its
/// own thread and a refresh never waits for it; a result started before a
/// newer edit is stale and never clears that edit; a newer comparison's
/// result counts, either way.
#[test]
fn a_stale_comparison_never_clears_a_newer_edit() {
    let base = base("stale");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    let toml = working.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();

    // A touch starts a comparison of a folder that is not edited; its
    // result held, as a large project's comparison would take long.
    document.hold_comparisons(true);
    touch(&working);
    refresh_until(&mut document, "a held comparison", |d| {
        d.comparisons_held() == 1
    });
    assert!(document.edited(), "changed files count until compared");

    // An edit while it is held: shown at once, the refresh not waiting.
    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.4")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(document.refresh());
    assert_eq!(document.comparisons_held(), 1, "the refresh did not wait");

    // The stale result ("not edited") arrives; the next one is held.
    document.hold_comparisons(false);
    while document.comparisons_held() > 0 {
        std::thread::yield_now();
    }
    document.hold_comparisons(true);
    refresh_until(&mut document, "the next comparison", |d| {
        assert!(d.edited(), "a stale result never clears a newer edit");
        d.comparisons_held() == 1
    });
    document.hold_comparisons(false);
    refresh_until(&mut document, "its result", |d| !d.comparing());
    assert!(document.edited(), "compared: edited");

    // Put back: a comparison of the folder as it is now clears it.
    fs::write(&toml, &text).unwrap();
    refresh_until(&mut document, "not edited", |d| !d.edited());
    assert!(!document.check());
    document.close(true);
    fs::remove_dir_all(&base).unwrap();
}

/// Audit P2: Save and close decide on the folder as it is, whatever
/// comparison is pending; a pending comparison of the content before a Save
/// never marks the saved project edited.
#[test]
fn save_and_close_do_not_depend_on_a_pending_comparison() {
    let base = base("pending");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    let toml = working.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();

    document.hold_comparisons(true);
    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.4")).unwrap();
    refresh_until(&mut document, "a held comparison", |d| {
        d.comparisons_held() == 1
    });
    assert!(document.check(), "a decision compares at once");
    document.save().unwrap();
    assert!(!document.edited());
    // The held result (the content before the Save: edited) is let go.
    document.hold_comparisons(false);
    refresh_until(&mut document, "its result", |d| {
        assert!(
            !d.edited(),
            "a stale result never marks a saved folder edited"
        );
        !d.comparing()
    });
    let reopened = Document::open(&file, &base.join("Open")).unwrap();
    assert!(
        fs::read_to_string(reopened.working().join("project.toml"))
            .unwrap()
            .contains("gain = 0.4")
    );
    reopened.close(true);

    // A hidden edit while a result ("not edited") is held: close keeps it.
    document.hold_comparisons(true);
    touch(&working);
    refresh_until(&mut document, "a held comparison", |d| {
        d.comparisons_held() == 1
    });
    hidden_edit_of(&working, "gain = 0.4", "gain = 0.5");
    assert_eq!(
        document.close(false).as_deref(),
        Some(working.as_path()),
        "kept, with its edits"
    );
    fs::remove_dir_all(&base).unwrap();
}

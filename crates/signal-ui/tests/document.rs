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

/// What the `.signal` file `file` holds in its `project.toml`, unpacked
/// beside it.
fn saved_toml(file: &Path) -> String {
    let dir = file.with_extension("check");
    let _ = fs::remove_dir_all(&dir);
    archive::unpack(&fs::read(file).unwrap(), &dir).unwrap();
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    text
}

/// Refreshes until `held` save threads are held, at most 20 s.
fn until_saves_held(document: &mut Document, held: usize) {
    refresh_until(document, "a held save", |d| d.saves_held() == held);
}

/// Audit (save-blocks-control): a Save packs and writes on a thread of its
/// own, so a refresh never waits for it (deterministic: the save is held
/// after packing, and refreshes go on); its result is told by its ticket.
/// A save that finishes after a newer edit (one it did not pack) never
/// clears that edit; the next save does.
#[test]
fn a_save_never_holds_up_a_refresh_and_a_stale_one_keeps_newer_edits() {
    let base = base("save-stale");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    let toml = working.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();

    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.4")).unwrap();
    assert!(document.check());
    document.hold_saves(true);
    let ticket = document.start_save();
    until_saves_held(&mut document, 1);
    for _ in 0..100 {
        assert!(document.refresh(), "edited until the save is written");
    }
    assert!(document.saving(), "no refresh waited for the save");
    assert_eq!(document.saves_held(), 1);
    assert!(document.take_saved().is_empty(), "not written yet");
    assert!(saved_toml(&file).contains("gain = 0.33"));

    // An edit after it packed: not in what it writes.
    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.45")).unwrap();
    document.hold_saves(false);
    refresh_until(&mut document, "the save written", |d| !d.saving());
    assert_eq!(document.take_saved(), vec![(ticket, Ok(file.clone()))]);
    assert!(saved_toml(&file).contains("gain = 0.4 }"));
    assert!(document.edited(), "a stale save never clears a newer edit");
    assert!(document.check(), "the newer edit is still to save");

    // The next save writes it, and clears it.
    let next = document.start_save();
    assert_ne!(next, ticket);
    refresh_until(&mut document, "the next save written", |d| !d.saving());
    assert_eq!(document.take_saved(), vec![(next, Ok(file.clone()))]);
    assert!(!document.edited());
    assert!(!document.check());
    assert!(saved_toml(&file).contains("gain = 0.45"));
    assert_eq!(document.close(false), None);
    fs::remove_dir_all(&base).unwrap();
}

/// Saves asked while one runs wait for it, in order; Saves asked while
/// another waits to start share it (it packs the folder later than both
/// were asked); a failed save is told, and the folder stays edited.
#[test]
fn saves_asked_while_one_runs_wait_and_share() {
    let base = base("save-queue");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    let mut document = Document::open(&file, &base.join("Open")).unwrap();
    let working = document.working().to_owned();
    let toml = working.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();

    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.4")).unwrap();
    document.hold_saves(true);
    let first = document.start_save();
    until_saves_held(&mut document, 1);
    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.45")).unwrap();
    let second = document.start_save();
    let third = document.start_save();
    assert_ne!(first, second);
    assert_eq!(second, third, "a Save waiting to start is shared");
    document.hold_saves(false);
    document.finish_saves();
    assert!(!document.saving());
    assert_eq!(
        document.take_saved(),
        vec![(first, Ok(file.clone())), (second, Ok(file.clone()))]
    );
    assert!(!document.edited(), "the last save had every edit");
    assert!(saved_toml(&file).contains("gain = 0.45"));

    // A Save As into a folder that is not there fails, and is told.
    fs::write(&toml, text.replace("gain = 0.33", "gain = 0.5")).unwrap();
    let nowhere = base.join("missing/pulse.signal");
    let failed = document.start_save_as(&nowhere);
    document.finish_saves();
    let told = document.take_saved();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].0, failed);
    let message = told[0].1.as_ref().unwrap_err();
    assert!(message.contains("was not saved"), "{message}");
    assert!(document.check(), "still edited");
    assert_eq!(document.file(), file, "still the file it was");
    document.close(true);
    fs::remove_dir_all(&base).unwrap();
}

/// Save, then close at once: close waits for the save, so the edits reach
/// the file and the folder, saved, is removed; with Don't Save after it,
/// the save still writes what it packed before the folder goes.
#[test]
fn close_finishes_a_pending_save() {
    let base = base("save-close");
    let pulse = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/pulse");
    let file = base.join("pulse.signal");
    archive::write_atomic(&file, &archive::pack(&pulse).unwrap()).unwrap();
    for (discard, from, to) in [
        (false, "gain = 0.33", "gain = 0.5"),
        (true, "gain = 0.5", "gain = 0.6"),
    ] {
        let mut document = Document::open(&file, &base.join("Open")).unwrap();
        let working = document.working().to_owned();
        let toml = working.join("project.toml");
        let text = fs::read_to_string(&toml).unwrap();
        fs::write(&toml, text.replace(from, to)).unwrap();
        document.hold_saves(true);
        document.start_save();
        until_saves_held(&mut document, 1);
        // Released from another thread while close waits for the save.
        let release = document.save_releaser();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            release();
        });
        assert_eq!(document.close(discard), None, "saved: removed");
        releaser.join().unwrap();
        assert!(!working.exists());
        assert!(saved_toml(&file).contains(to), "discard {discard}");
    }
    fs::remove_dir_all(&base).unwrap();
}

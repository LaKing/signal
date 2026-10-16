//! M2.8-12 (auditor-1-edited-metadata, AC17): a `.signal` project's
//! working folder is edited when its content differs from the file, whatever
//! its files' sizes and times say, and closing never removes such a folder
//! unless its edits are let go.

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
    let toml = dir.join("project.toml");
    let modified = fs::metadata(&toml).unwrap().modified().unwrap();
    let text = fs::read_to_string(&toml).unwrap();
    let edited = text.replace("gain = 0.33", "gain = 0.44");
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

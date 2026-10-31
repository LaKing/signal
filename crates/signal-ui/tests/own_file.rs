//! Second review of batch 2, item 4: `read_own_file` reads only a regular
//! file of the project's own: a FIFO at a panel's path is refused without
//! being opened (an open of a FIFO with no writer blocks, and the reader is
//! the control thread); a link and a hard link are refused; a missing file
//! is absent; a plain file is read.

use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use signal_ui::read_own_file;

#[test]
fn only_a_regular_file_of_the_project_s_own_is_read() {
    let dir = std::env::temp_dir().join(format!("signal-ui-own-file-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ui")).unwrap();
    fs::write(dir.join("ui/main.html"), "<p>own</p>").unwrap();
    assert_eq!(
        read_own_file(&dir, "ui/main.html"),
        Ok(Some("<p>own</p>".to_owned()))
    );
    assert_eq!(read_own_file(&dir, "ui/missing.html"), Ok(None));
    assert_eq!(read_own_file(&dir, "ui/../ui/main.html"), Err(()));
    // A link and a hard link.
    std::os::unix::fs::symlink(dir.join("ui/main.html"), dir.join("ui/link.html")).unwrap();
    assert_eq!(read_own_file(&dir, "ui/link.html"), Err(()));
    fs::hard_link(dir.join("ui/main.html"), dir.join("ui/hard.html")).unwrap();
    assert_eq!(read_own_file(&dir, "ui/hard.html"), Err(()));
    // A FIFO with no writer: refused at once, never waited on.
    let fifo = dir.join("ui/fifo.html");
    let made = Command::new("mkfifo").arg(&fifo).status();
    if made.is_ok_and(|s| s.success()) {
        let (done, answer) = mpsc::channel();
        let at = dir.clone();
        std::thread::spawn(move || {
            let _ = done.send(read_own_file(&at, "ui/fifo.html"));
        });
        let read = answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the read of a FIFO blocked");
        assert_eq!(read, Err(()));
    }
    let _ = fs::remove_dir_all(&dir);
}

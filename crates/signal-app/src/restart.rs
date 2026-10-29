//! A developer restart (`./start.sh`): the running app is asked to quit
//! without the "Save changes?" prompt.
//!
//! `./start.sh` writes the running app's process id into [`FILE`] beside the
//! Setup file (`~/Library/Application Support/Signal/restart-quit`), then
//! asks it to quit through AppleScript as the Dock's Quit does. When that
//! termination arrives (`Event::LoopDestroyed`), the app looks for the file:
//! a fresh one naming its own process id makes the route
//! [`QuitRoute::Restart`](crate::window::QuitRoute::Restart), which asks
//! nothing and closes as if the prompt went unanswered (`ExitPlan::Keep`:
//! the working folder keeps the edits, nothing is written into the project).
//! The file is removed once read.
//!
//! Only the person's own account can write there (their `~/Library` is
//! theirs alone), and a page or another user cannot; a process of the same
//! account could quit the app anyway. A stale file (another process id, or
//! older than [`FRESH`]) changes nothing, so a later Dock Quit still asks.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The request's file name, in the Setup file's folder.
pub const FILE: &str = "restart-quit";

/// How old a request may be: `./start.sh` writes it just before it asks the
/// app to quit, and removes it once the app is gone or has not gone.
pub const FRESH: Duration = Duration::from_secs(30);

/// The request file for the Setup at `setup_path`.
pub fn request_path(setup_path: &Path) -> PathBuf {
    setup_path.with_file_name(FILE)
}

/// Whether `text`, written at `written`, asks the process `pid` to restart
/// at `now`: one of its lines is `pid`, and it is at most [`FRESH`] old (a
/// time ahead of `now` counts as fresh: clocks set back).
pub fn asks(text: &str, pid: u32, written: SystemTime, now: SystemTime) -> bool {
    let fresh = !matches!(now.duration_since(written), Ok(age) if age > FRESH);
    fresh
        && text
            .lines()
            .any(|line| line.trim().parse::<u32>().is_ok_and(|id| id == pid))
}

/// Whether the file at `path` asks the process `pid` to restart now; a
/// request for it is removed (it is answered), another is left as it is.
pub fn requested(path: &Path, pid: u32, now: SystemTime) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(written) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    let asked = asks(&text, pid, written, now);
    if asked {
        let _ = std::fs::remove_file(path);
    }
    asked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_request_for_this_process_asks() {
        let now = SystemTime::now();
        assert!(asks("4242\n", 4242, now, now));
        assert!(asks("17\n4242\n", 4242, now - FRESH, now), "one line each");
        assert!(asks(" 4242 ", 4242, now + FRESH, now), "clock set back");
    }

    #[test]
    fn another_process_or_a_stale_request_does_not() {
        let now = SystemTime::now();
        assert!(!asks("4243\n", 4242, now, now));
        assert!(!asks("", 4242, now, now));
        assert!(!asks("42420\nx4242\n", 4242, now, now));
        let stale = now - FRESH - Duration::from_secs(1);
        assert!(!asks("4242\n", 4242, stale, now));
    }

    #[test]
    fn the_file_sits_beside_the_setup() {
        let setup = Path::new("/somewhere/Signal/setup.toml");
        assert_eq!(
            request_path(setup),
            Path::new("/somewhere/Signal/restart-quit")
        );
    }

    #[test]
    fn a_request_is_answered_once_and_another_is_left() {
        let dir = std::env::temp_dir().join(format!("signal-restart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = request_path(&dir.join("setup.toml"));
        let now = SystemTime::now() + Duration::from_secs(1);
        assert!(!requested(&path, 7, now), "no file");

        std::fs::write(&path, "8\n").unwrap();
        assert!(!requested(&path, 7, now));
        assert!(path.exists(), "another process's request stays");

        std::fs::write(&path, "8\n7\n").unwrap();
        assert!(requested(&path, 7, now));
        assert!(!path.exists(), "answered: removed");
        assert!(!requested(&path, 7, now), "only once");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

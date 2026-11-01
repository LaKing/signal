//! Opening, reopening and reverting the project.

use std::path::{Path, PathBuf};

use signal_project::PROJECT_FILE;

/// The folder of `path`: its parent for a project's `project.toml`.
pub(super) fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if path.file_name().is_some_and(|n| n == PROJECT_FILE) && path.is_file() => {
            dir.to_owned()
        }
        _ => path.to_owned(),
    }
}

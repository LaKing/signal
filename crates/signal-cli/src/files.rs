//! Project files (`.signal`, M2.8-12, ADR 0025): `signal pack` and `signal
//! unpack`, and the project argument of `render`, `play` and `ui`, which
//! takes a `.signal` file wherever it takes a project folder by unpacking it
//! into a temporary folder for the run (`signal_project::archive`).

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;
use signal_project::archive::{self, ArchiveError, TempProject};

use crate::{Failure, print_stdout};

/// Arguments of `signal pack`.
#[derive(Args)]
pub(crate) struct PackArgs {
    /// The project folder (containing project.toml).
    folder: PathBuf,
    /// The .signal file to write (replaced atomically if it exists).
    file: PathBuf,
}

/// Arguments of `signal unpack`.
#[derive(Args)]
pub(crate) struct UnpackArgs {
    /// The .signal file.
    file: PathBuf,
    /// The folder to unpack into (made; it must not exist or be empty).
    folder: PathBuf,
}

/// An archive refused or not written: exit 1, its reason named.
pub(crate) fn archive_failure(file: &Path) -> impl FnOnce(ArchiveError) -> Failure + '_ {
    move |error| match error {
        error @ ArchiveError::Io { .. } => Failure::Io(error.to_string()),
        error => Failure::Archive(format!("{}: {error}", file.display())),
    }
}

/// `signal pack`.
pub(crate) fn pack(args: &PackArgs, json: bool) -> Result<(), Failure> {
    let bytes = archive::pack(&args.folder).map_err(archive_failure(&args.folder))?;
    archive::write_atomic(&args.file, &bytes).map_err(archive_failure(&args.file))?;
    if json {
        print_stdout(
            &json!({"file": args.file.display().to_string(), "bytes": bytes.len()}).to_string(),
        );
    } else {
        print_stdout(&format!(
            "packed {} into {} ({} bytes)",
            args.folder.display(),
            args.file.display(),
            bytes.len()
        ));
    }
    Ok(())
}

/// `signal unpack`.
pub(crate) fn unpack(args: &UnpackArgs, json: bool) -> Result<(), Failure> {
    let bytes = archive::read_archive(&args.file).map_err(archive_failure(&args.file))?;
    let unpacked = archive::unpack(&bytes, &args.folder).map_err(archive_failure(&args.file))?;
    if json {
        print_stdout(
            &json!({
                "folder": args.folder.display().to_string(),
                "files": unpacked.files,
                "bytes": unpacked.bytes,
            })
            .to_string(),
        );
    } else {
        print_stdout(&format!(
            "unpacked {} files of {} into {}",
            unpacked.files.len(),
            args.file.display(),
            args.folder.display()
        ));
    }
    Ok(())
}

/// A command's project: a folder, or a `.signal` file unpacked for the run.
pub(crate) struct ProjectArg {
    dir: PathBuf,
    shown: PathBuf,
    temporary: Option<TempProject>,
}

impl ProjectArg {
    /// The project folder the run reads.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// What diagnostics are located against: the folder, or the `.signal`
    /// file (`kick.signal/project.toml:3:1`).
    pub(crate) fn shown(&self) -> &Path {
        &self.shown
    }

    /// The `.signal` file, when the project is one.
    pub(crate) fn file(&self) -> Option<&Path> {
        self.temporary.as_ref().map(|_| self.shown.as_path())
    }

    /// The temporary folder, for a run that keeps it (`signal ui`).
    pub(crate) fn into_temporary(self) -> Option<TempProject> {
        self.temporary
    }
}

/// `path` as a command's project: a `*.signal` file is unpacked into a
/// temporary folder, removed when the run ends; anything else is a folder.
pub(crate) fn project_arg(path: &Path) -> Result<ProjectArg, Failure> {
    if !(archive::is_project_file(path) && path.is_file()) {
        return Ok(ProjectArg {
            dir: path.to_owned(),
            shown: path.to_owned(),
            temporary: None,
        });
    }
    let temporary = TempProject::unpack(path).map_err(archive_failure(path))?;
    Ok(ProjectArg {
        dir: temporary.dir().to_owned(),
        shown: path.to_owned(),
        temporary: Some(temporary),
    })
}

/// The note for a live run of a `.signal` file: its edits stay in the
/// temporary copy.
pub(crate) fn temporary_note(project: &ProjectArg) {
    if let Some(file) = project.file() {
        eprintln!(
            "signal: {} is played from a temporary copy; edits to it are not written back (use Signal.app, or `signal unpack`)",
            file.display()
        );
    }
}

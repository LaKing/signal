//! The project's single writer (ADR 0040, 0048): what a request that
//! writes the project's files is given to write them with. A writer takes
//! the project's writer lock only through its [`WriteCtx`].

use std::io;
use std::path::PathBuf;

use signal_project::transaction::ProjectLock;

use super::Host;

/// What a writer of the open project's files is given: the folder it
/// writes, whose writer lock it takes through [`WriteCtx::lock`].
pub(super) struct WriteCtx {
    dir: PathBuf,
}

impl WriteCtx {
    /// The project's writer lock, for one change (ADR 0040): taken at once
    /// when this thread holds it already (a request run holding it), else
    /// waited for as [`ProjectLock::acquire`] does.
    pub(super) fn lock(&self) -> io::Result<ProjectLock> {
        ProjectLock::acquire(&self.dir)
    }
}

impl Host {
    /// A writer's context for the project open now.
    pub(super) fn write_ctx(&self) -> WriteCtx {
        WriteCtx {
            dir: self.opening.dir.clone(),
        }
    }
}

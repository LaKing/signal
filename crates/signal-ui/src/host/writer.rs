//! The project's single writer (ADR 0040, 0048): what a request that
//! writes the project's files is given to write them with. A writer takes
//! the project's writer lock only through its [`WriteCtx`].

use std::io;
use std::path::PathBuf;

use signal_project::transaction::{Holding, ProjectLock};

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

    /// The project's writer lock held for the rest of a request, taken
    /// without waiting ([`ProjectLock::try_hold`]): `None` while another
    /// writer holds it.
    pub(super) fn try_hold(&self) -> io::Result<Option<Holding>> {
        ProjectLock::try_hold(&self.dir)
    }
}

impl Host {
    /// The open project's writer lock held for a request about to run
    /// ([`WriteCtx::try_hold`]).
    pub(super) fn try_hold(&self) -> io::Result<Option<Holding>> {
        ProjectLock::try_hold(&self.opening.dir)
    }

    /// A writer's context for the project open now.
    pub(super) fn write_ctx(&self) -> WriteCtx {
        WriteCtx {
            dir: self.opening.dir.clone(),
        }
    }
}

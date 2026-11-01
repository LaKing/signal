//! ADR 0053: what a project's panel may do. A client that said `hello`
//! with the project's panel token ([`Scope::Project`]) acts only on the
//! project it was handed out for: it may send the requests a panel and a
//! panel window need ([`PANEL_REQUESTS`]), never `project.open`, the
//! machine's Setup, keymap or device routing, or the app's power, and none
//! at all once another project is open (it is evicted then, and its token
//! admits nothing more). The session token ([`Scope::App`]: Signal's own
//! pages, `signal mcp`, the CLI) keeps every request.

use std::path::PathBuf;

use serde_json::json;

use crate::server::Scope;

use super::Host;
use super::failure::{Failure, fail};

/// The requests a project's panel may send: reading and streaming the
/// project, live edits and their commits, the project's own files (each
/// confined to the project by its own rules), snapshots, the transport,
/// Save of a `.signal` project, and the reads a panel window and the
/// panel's controls make (`app.get`, `keys.get`, `slot.projects`).
pub(super) const PANEL_REQUESTS: &[&str] = &[
    "describe",
    "graph",
    "modules",
    "patch",
    "schematic.move",
    "schematic.reset",
    "subscribe",
    "unsubscribe",
    "set",
    "ramp",
    "trigger",
    "note",
    "commit",
    "retry",
    "revert",
    "snapshot.list",
    "snapshot.save",
    "snapshot.restore",
    "snapshot.delete",
    "snapshot.undo",
    "snapshot.move",
    "snapshot.transition",
    "snapshot.exclude",
    "panel.check",
    "panel.read",
    "panel.write",
    "pattern.read",
    "pattern.write",
    "file.read",
    "file.write",
    "vendor",
    "slot.projects",
    "keys.get",
    "app.get",
    "transport.play",
    "transport.stop",
    "project.save",
    "stats",
    "ping",
    "hello",
];

impl Host {
    /// Which project a panel token is for: a `.signal` project's file (so
    /// Revert to Saved, which unpacks it into a new working folder, keeps
    /// its panel), `signal ui`'s project as given, else the open folder
    /// (empty while Signal.app has none).
    pub(super) fn scope_identity(&self) -> PathBuf {
        if let Some(document) = &self.opening.document {
            return document.file().to_owned();
        }
        if self.app.is_none() && !self.opening.source.as_os_str().is_empty() {
            return self.opening.source.clone();
        }
        self.opening.dir.clone()
    }

    /// The panel token follows the open project: another project open makes
    /// a new one, and every client of the old one is evicted, so a panel
    /// never hears of, nor acts on, a project other than its own.
    pub(super) fn sync_scope(&mut self) {
        let identity = self.scope_identity();
        if !self.server.panel.serve(&identity) {
            return;
        }
        let stale: Vec<u64> = self
            .clients
            .scoped()
            .filter(|(_, scope)| **scope != Scope::Project(identity.clone()))
            .map(|(client, _)| client)
            .collect();
        for client in stale {
            self.evict(client);
        }
    }

    /// Whether client `client` may send a message of type `kind`: refused
    /// (`not-allowed`) for a project's panel outside [`PANEL_REQUESTS`],
    /// and (`project-changed`) once its project is no longer the open one.
    pub(super) fn scope_refuses(&self, client: u64, kind: &str) -> Option<Failure> {
        // A panel's messages read before it was evicted or closed, or
        // waiting for the writer lock, never run once it is gone.
        if self.clients.panel_gone(client) {
            return Some(fail(
                "project-changed",
                "not done: this panel's connection was closed",
            ));
        }
        let Some(Scope::Project(identity)) = self.clients.scope(client) else {
            return None;
        };
        if *identity != self.scope_identity() {
            return Some(fail(
                "project-changed",
                "not done: this panel's project is no longer open",
            ));
        }
        (!PANEL_REQUESTS.contains(&kind)).then(|| {
            fail(
                "not-allowed",
                format!(
                    "a project's panel may not send `{kind}`: it acts only on its own project (ADR 0053)"
                ),
            )
        })
    }

    /// [`Host::scope_refuses`] answered to the client (with `id`): true when
    /// the message is refused and must not run.
    pub(super) fn refuse_out_of_scope(
        &mut self,
        client: u64,
        id: &serde_json::Value,
        kind: &str,
    ) -> bool {
        match self.scope_refuses(client, kind) {
            Some(failure) => {
                let error = json!({
                    "type": "error",
                    "id": id,
                    "code": failure.code,
                    "message": failure.message,
                });
                self.send_text(client, &error);
                true
            }
            None => false,
        }
    }
}

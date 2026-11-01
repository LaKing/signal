//! The project's single writer (ADR 0040, 0048): what a request that
//! writes the project's files is given to write them with. A writer takes
//! the project's writer lock only through its [`WriteCtx`]: this is the
//! one module of the host that calls `ProjectLock::acquire` and
//! `ProjectLock::try_hold` (`crates/signal-ui/clippy.toml`).
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};
use signal_project::transaction::{Holding, ProjectLock};

use super::failure::Failure;
use super::{Host, with};

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

/// What a client's message did, to be sent once it has run
/// ([`Host::apply_outcome`]): its reply (or none yet: a `project.save`
/// answered when written, a live edit answered by its ack), then the
/// pushes to every client, in this order.
pub(super) struct Outcome {
    reply: Option<Value>,
    pushes: Vec<Push>,
}

/// A push to every client after a message's reply.
enum Push {
    /// `describe` (`restarted`: by a Setup's Apply), and the graph.
    Describe { restarted: bool },
    /// The graph again, even unchanged (`force`).
    Graph { force: bool },
    /// A message as it is.
    Message(Value),
}

impl Outcome {
    /// The outcome of the message `kind` (`message`, with `id`) that gave
    /// `result`: the reply with its `id`, or the error with its detail;
    /// after a reply, the pushes of that kind.
    pub(super) fn of(
        kind: &str,
        message: &Value,
        id: Value,
        result: Result<Option<Value>, Failure>,
    ) -> Outcome {
        match result {
            Ok(Some(reply)) => {
                let mut pushes = Vec::new();
                let restarted =
                    kind == "setup.apply" && reply.get("restarted") == Some(&json!(true));
                // The plan or what plays changed: every client is told.
                let changed = matches!(
                    kind,
                    "transport.play"
                        | "transport.stop"
                        | "project.open"
                        | "project.revert"
                        | "app.power"
                );
                let reply = with(reply, "id", id);
                if restarted || changed {
                    pushes.push(Push::Describe { restarted });
                }
                // Every client sees the positions written.
                if kind.starts_with("schematic.") {
                    pushes.push(Push::Graph { force: true });
                }
                // Every client gets the written panel (ADR 0026 §5).
                if kind == "panel.write" {
                    pushes.push(Push::Message(json!({
                        "type": "panel.written",
                        "file": reply.get("file").cloned().unwrap_or(Value::Null),
                        "base": reply.get("base").cloned().unwrap_or(Value::Null),
                        "plate": reply.get("plate").cloned().unwrap_or(Value::Null),
                        "html": message.get("html").cloned().unwrap_or(Value::Null),
                    })));
                }
                // Every client learns of the written pattern (ADR 0028 §7).
                if kind == "pattern.write" {
                    pushes.push(Push::Message(json!({
                        "type": "pattern.written",
                        "name": reply.get("name").cloned().unwrap_or(Value::Null),
                        "base": reply.get("base").cloned().unwrap_or(Value::Null),
                        "text": message.get("text").cloned().unwrap_or(Value::Null),
                    })));
                }
                Outcome {
                    reply: Some(reply),
                    pushes,
                }
            }
            Ok(None) => Outcome {
                reply: None,
                pushes: Vec::new(),
            },
            Err(failure) => {
                let mut error = json!({
                    "type": "error",
                    "id": id,
                    "code": failure.code,
                    "message": failure.message,
                });
                if let (Some(Value::Object(detail)), Some(error)) =
                    (failure.detail, error.as_object_mut())
                {
                    for (key, value) in detail {
                        error.entry(key).or_insert(value);
                    }
                }
                Outcome {
                    reply: Some(error),
                    pushes: Vec::new(),
                }
            }
        }
    }
}

impl Host {
    /// Sends `outcome` (a message's reply to `client`, then its pushes to
    /// every client, in order). A writer's own pushes (a control write's
    /// `describe` and graph) were sent while it ran, before this reply.
    pub(super) fn apply_outcome(&mut self, client: u64, outcome: Outcome) {
        if let Some(reply) = &outcome.reply {
            self.send_text(client, reply);
        }
        for push in outcome.pushes {
            match push {
                Push::Describe { restarted } => {
                    let describe = self.describe(restarted);
                    self.broadcast(&describe);
                    self.push_graph(false);
                }
                Push::Graph { force } => self.push_graph(force),
                Push::Message(pushed) => self.broadcast(&pushed),
            }
        }
    }
}

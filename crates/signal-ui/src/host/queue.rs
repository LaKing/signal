//! The requests waiting for the project's writer lock (ADR 0048): how each
//! message is ordered against them, the project it names (ADR 0049), and
//! their run once the lock is free.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_project::transaction::{self, ProjectLock};

use crate::server::Permit;

use super::Host;
use super::failure::{Failure, fail};
use super::project_io::{SaveDone, WindowsEdit};

/// The messages that write the project's files under its writer lock (ADR
/// 0040): each runs holding it, or waits in the host's queue while another
/// writer holds it (ADR 0048), never on the control thread.
const WRITES_FILES: [&str; 16] = [
    "schematic.move",
    "schematic.reset",
    "patch",
    "commit",
    "retry",
    "snapshot.save",
    "snapshot.restore",
    "snapshot.delete",
    "snapshot.undo",
    "snapshot.move",
    "snapshot.transition",
    "snapshot.exclude",
    "panel.write",
    "pattern.write",
    "file.write",
    "vendor",
];

/// The live edits that change a parameter's value at once: kept behind a
/// queued request that can change the same value, in the client's order.
pub(super) const LIVE_EDITS: [&str; 5] = ["set", "ramp", "trigger", "note", "revert"];

/// Messages kept behind every queued request (they save, open or revert the
/// project, so what was asked before them is done first).
const ORDERED: [&str; 2] = ["project.open", "project.revert"];

/// Messages that save, revert or replace the open project: a stale
/// `generation` or `projectId` on one is refused as on any mutation (ADR
/// 0049), so a page of an earlier opening can neither discard the new
/// opening's edits nor save or switch it away.
const PROJECT_CHANGES: [&str; 3] = ["project.save", "project.revert", "project.open"];

/// What the host does with a message while requests wait for the lock.
enum Order {
    /// It writes the project's files: run holding the lock, or queued.
    Write,
    /// `project.save`: run at once unless it would have to wait (its
    /// corrections need the lock) or requests wait; queued then.
    Save,
    /// Queued behind any waiting request ([`ORDERED`]).
    Ordered,
    /// A live edit of `address`: queued only behind a request that can
    /// change that value.
    Live(String),
    /// Never queued (`ping`, `describe`, `stats`, subscriptions, reads).
    Free,
}

/// How many file requests may wait for the writer lock at once; another is
/// refused (`busy`).
const WAITING_MAX: usize = 64;

/// A file request waiting for the project's writer lock: the opening it is
/// of, when it was queued, and what it does. `generation` is the opening
/// open when it was queued, or `None` for a request that names no project
/// queued behind an open or revert: it means the project open when it runs
/// (second review of batch 2, item 5).
pub(super) struct Waiting {
    generation: Option<u64>,
    since: Instant,
    pub(super) job: Job,
}

pub(super) enum Job {
    /// A client's message, answered when it runs: one that writes files
    /// (`lock`: it runs holding the writer lock), or one kept behind them
    /// in the client's order ([`Host::order_of`]). `kind` and `address` say
    /// which values it can change ([`Host::queued_for`]).
    Message {
        client: u64,
        id: Value,
        text: String,
        kind: String,
        address: Option<String>,
        lock: bool,
        /// It carries a `generation` or `projectId` (ADR 0049): checked
        /// again when it runs.
        named: bool,
        /// A message over `MAX_MESSAGE` (a large `pattern.write`) keeps its
        /// share of the server's large-message budget while it waits, so
        /// the budget bounds what waits here too (second review, item 1).
        /// Held, never read: its drop gives the share back.
        #[allow(dead_code)]
        permit: Option<Permit>,
    },
    /// A Save or Save As of the open project ([`Host::save_document_then`],
    /// [`Host::save_as_then`]): run holding the lock, so its corrections
    /// are written first, after every file request queued before it.
    Save {
        save_as: Option<PathBuf>,
        done: SaveDone,
    },
    /// The window layout's write (app mode).
    Windows {
        dir: PathBuf,
        edit: WindowsEdit,
        done: SaveDone,
    },
}

impl Host {
    pub(super) fn handle(&mut self, client: u64, text: &str, permit: Option<Permit>) {
        let permit = std::cell::Cell::new(permit);
        let message: Value = match serde_json::from_str(text) {
            Ok(message) => message,
            Err(_) => {
                let error = json!({"type": "error", "code": "bad-message", "message": "not JSON"});
                return self.send_text(client, &error);
            }
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        // ADR 0048: a request that writes the
        // project's files runs holding the writer lock, taken without
        // waiting; when another writer holds it (or requests wait already,
        // to keep their order) it waits in the queue, not on this thread.
        if let Err(failure) = self.same_project(kind, &message) {
            let error = json!({
                "type": "error",
                "id": id,
                "code": failure.code,
                "message": failure.message,
            });
            return self.send_text(client, &error);
        }
        let address = message
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let queue = |lock: bool| Job::Message {
            client,
            id: id.clone(),
            text: text.to_owned(),
            kind: kind.to_owned(),
            address: address.clone(),
            lock,
            named: ["generation", "projectId"]
                .iter()
                .any(|key| message.get(*key).is_some_and(|v| !v.is_null())),
            permit: permit.take(),
        };
        match Host::order_of(kind, &message) {
            Order::Write => {
                if self.waiting.is_empty() {
                    match ProjectLock::try_hold(&self.dir) {
                        Ok(Some(holding)) => {
                            self.respond(client, id, kind, &message);
                            drop(holding);
                            return;
                        }
                        Ok(None) => {}
                        // The writer's own `acquire` says why, at once.
                        Err(_) => return self.respond(client, id, kind, &message),
                    }
                }
                self.wait_for_lock(queue(true));
            }
            Order::Save => {
                if self.waiting.is_empty() && !self.save_must_wait() {
                    return self.respond(client, id, kind, &message);
                }
                self.wait_for_lock(queue(true));
            }
            Order::Ordered if !self.waiting.is_empty() => self.wait_for_lock(queue(false)),
            Order::Live(address) if self.queued_for(&address) => {
                self.wait_for_lock(queue(false));
            }
            _ => self.respond(client, id, kind, &message),
        }
    }

    /// A mutation (a file write, a live edit, `project.save`, and the open
    /// project's `project.revert` or another `project.open`) names the
    /// project it means, or is taken to mean the open one (ADR 0049): a
    /// `generation` (the open project's, as `welcome`, `describe`, `graph`
    /// and `app` give it) must be the open project's generation, else a
    /// `projectId` (`graph.projectId`) must be its id; either one not
    /// matching is `project-changed`, and nothing is done. A message
    /// without either is taken to mean the open project (MCP, the CLI and
    /// pages that send none).
    fn same_project(&self, kind: &str, message: &Value) -> Result<(), Failure> {
        let mutation = WRITES_FILES.contains(&kind)
            || LIVE_EDITS.contains(&kind)
            || PROJECT_CHANGES.contains(&kind);
        if !mutation {
            return Ok(());
        }
        let changed = |what: &str| {
            Err(fail(
                "project-changed",
                format!(
                    "not done: the request names {what} of a project no longer open (another project, or a new opening of it, is open now)"
                ),
            ))
        };
        match message.get("generation") {
            None | Some(Value::Null) => {}
            Some(given) => {
                return match given.as_u64() {
                    Some(g) if g == self.generation => Ok(()),
                    Some(_) => changed("the generation"),
                    None => Err(fail("bad-value", "`generation` must be a whole number")),
                };
            }
        }
        match message.get("projectId") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(id)) if *id == self.project_id() => Ok(()),
            Some(Value::String(_)) => changed("the id"),
            Some(_) => Err(fail("bad-value", "`projectId` must be a string")),
        }
    }

    /// How the message `kind` is ordered against the requests waiting for
    /// the writer lock (ADR 0048): file writes wait for the lock; a Save,
    /// an open or a revert of the project waits behind every queued
    /// request; a live edit waits only behind a queued request that can
    /// change the same value; everything else (`ping`, `describe`, `stats`,
    /// subscriptions, reads) never waits.
    fn order_of(kind: &str, message: &Value) -> Order {
        if WRITES_FILES.contains(&kind) {
            return Order::Write;
        }
        if kind == "project.save" {
            return Order::Save;
        }
        if ORDERED.contains(&kind) {
            return Order::Ordered;
        }
        if LIVE_EDITS.contains(&kind) {
            let address = message.get("address").and_then(Value::as_str);
            return Order::Live(address.unwrap_or_default().to_owned());
        }
        Order::Free
    }

    /// Whether a queued request can change the value at `address`: a
    /// `commit`, `retry` or live edit of it, or a snapshot restore or undo,
    /// an open or a revert of the project (any value). A live edit of it
    /// then waits behind them, so the client's order holds for that value.
    fn queued_for(&self, address: &str) -> bool {
        self.waiting.iter().any(|w| match &w.job {
            Job::Message {
                kind, address: a, ..
            } => match kind.as_str() {
                // A batch `commit` (`values`, no `address`) can change any.
                "commit" if a.is_none() => true,
                "commit" | "retry" => a.as_deref() == Some(address),
                k if LIVE_EDITS.contains(&k) => a.as_deref() == Some(address),
                "snapshot.restore" | "snapshot.undo" => true,
                k => ORDERED.contains(&k),
            },
            _ => false,
        })
    }

    /// Whether a Save now would have to wait for the writer lock: its
    /// corrections are pending and another writer holds the lock.
    pub(super) fn save_must_wait(&self) -> bool {
        self.document.is_some()
            && !self.pending_corrections().is_empty()
            && matches!(ProjectLock::try_hold(&self.dir), Ok(None))
    }

    /// Queues `job` until the project's writer lock is free
    /// ([`Host::run_waiting`]); refused at once (`busy`) when
    /// [`WAITING_MAX`] requests wait already.
    pub(super) fn wait_for_lock(&mut self, job: Job) {
        if self.waiting.len() >= WAITING_MAX {
            let message = format!(
                "not done: {WAITING_MAX} requests already wait for another writer to release the project's lock; try again"
            );
            return self.answer_job(job, fail("busy", message));
        }
        // A request naming no project, queued behind an open or revert,
        // means the project open when it runs, not the one open now.
        let named = matches!(job, Job::Message { named: true, .. });
        let behind_an_open = self.waiting.iter().any(
            |w| matches!(&w.job, Job::Message { kind, .. } if ORDERED.contains(&kind.as_str())),
        );
        self.waiting.push_back(Waiting {
            generation: (named || !behind_an_open).then_some(self.generation),
            since: Instant::now(),
            job,
        });
    }

    /// Answers `job` with `failure`, unrun.
    fn answer_job(&mut self, job: Job, failure: Failure) {
        match job {
            Job::Message { client, id, .. } => {
                let error = json!({
                    "type": "error",
                    "id": id,
                    "code": failure.code,
                    "message": failure.message,
                });
                self.send_text(client, &error);
            }
            Job::Windows { done, .. } => done(Err(format!(
                "{} was not written: {}",
                crate::WINDOWS_FILE,
                failure.message
            ))),
            Job::Save { done, .. } => done(Err(format!("not saved: {}", failure.message))),
        }
    }

    /// Runs the requests waiting for the writer lock, in order, while it is
    /// free: each holding it, as [`Host::handle`] runs one. One that has
    /// waited the host's `lock_wait` runs without it, so its writer fails at
    /// once and says so (`write-failed`, as a writer that waited
    /// [`transaction::LOCK_WAIT`] did); one of a project no longer open
    /// (another was opened since it was queued) is refused unrun.
    pub(super) fn run_waiting(&mut self) {
        while let Some(front) = self.waiting.front() {
            if front.generation.is_some_and(|g| g != self.generation) {
                if let Some(stale) = self.waiting.pop_front() {
                    let failure = fail(
                        "project-changed",
                        "not done: another project was opened while it waited for the writer lock",
                    );
                    self.answer_job(stale.job, failure);
                }
                continue;
            }
            // A live edit or an open kept in order needs no lock.
            let lock = !matches!(front.job, Job::Message { lock: false, .. });
            let holding = match lock.then(|| ProjectLock::try_hold(&self.dir)) {
                None => None,
                Some(Ok(Some(holding))) => Some(holding),
                Some(Ok(None)) if front.since.elapsed() < self.lock_wait => break,
                Some(Ok(None) | Err(_)) => None,
            };
            let Some(waiting) = self.waiting.pop_front() else {
                break;
            };
            // Without the lock, the writer's `acquire` fails at once.
            let _no_wait = transaction::NoWait::enter(waiting.since.elapsed());
            match waiting.job {
                Job::Message {
                    client,
                    id,
                    text,
                    kind,
                    ..
                } => {
                    if let Ok(message) = serde_json::from_str::<Value>(&text) {
                        // The project it names, against the opening now.
                        match self.same_project(&kind, &message) {
                            Ok(()) => self.respond(client, id, &kind, &message),
                            Err(failure) => {
                                let job = Job::Message {
                                    client,
                                    id,
                                    text: String::new(),
                                    kind,
                                    address: None,
                                    lock: false,
                                    named: true,
                                    permit: None,
                                };
                                self.answer_job(job, failure);
                            }
                        }
                    }
                }
                Job::Windows { dir, edit, done } => done(self.write_windows_now(&dir, edit)),
                Job::Save { save_as, done } => match save_as {
                    Some(file) => self.save_as_now(&file, done),
                    None => self.save_document_now(done),
                },
            }
            drop(holding);
        }
    }

    /// Whether requests wait for the project's writer lock (the control
    /// loop then steps rather than sleeping in a wait).
    pub fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Test hooks: how long a request waits for the writer lock before it
    /// fails ([`transaction::LOCK_WAIT`] by default).
    #[cfg(feature = "test-hooks")]
    pub fn set_lock_wait(&mut self, wait: Duration) {
        self.lock_wait = wait;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use signal_core::Registry;

    use super::*;
    use crate::host::AppHostConfig;
    use crate::player::Backend;
    use crate::server::ServerOptions;

    /// A host with no project open (app mode, the null driver), for the
    /// request-ordering checks below; its folder and generation are set by
    /// the test.
    fn idle_host(name: &str) -> Host {
        let base =
            std::env::temp_dir().join(format!("signal-ui-host-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("project")).unwrap();
        let config = AppHostConfig {
            setup_path: base.join("setup.toml"),
            project: None,
            open_root: base.join("open"),
            registry: Registry::new(),
            backend: Backend::Null(Vec::new()),
            compute: Vec::new(),
            server: ServerOptions {
                port: 0,
                remote: false,
                ui_dir: base.join("ui"),
                project_ui: base.join("project").join("ui"),
                client_queue: 16,
            },
            plays: false,
        };
        let mut host = Host::new_app(config).unwrap();
        host.dir = base.join("project");
        host.generation = 7;
        host
    }

    fn order_name(order: Order) -> String {
        match order {
            Order::Write => "write".to_owned(),
            Order::Save => "save".to_owned(),
            Order::Ordered => "ordered".to_owned(),
            Order::Live(address) => format!("live {address}"),
            Order::Free => "free".to_owned(),
        }
    }

    const MUTATIONS: [&str; 24] = [
        "schematic.move",
        "schematic.reset",
        "patch",
        "commit",
        "retry",
        "snapshot.save",
        "snapshot.restore",
        "snapshot.delete",
        "snapshot.undo",
        "snapshot.move",
        "snapshot.transition",
        "snapshot.exclude",
        "panel.write",
        "pattern.write",
        "file.write",
        "vendor",
        "set",
        "ramp",
        "trigger",
        "note",
        "revert",
        "project.save",
        "project.revert",
        "project.open",
    ];

    /// ADR 0048: how each message kind is ordered against the requests
    /// waiting for the writer lock.
    #[test]
    fn each_kind_has_its_order() {
        let message = json!({"address": "/a/freq"});
        let order = |kind: &str, message: &Value| order_name(Host::order_of(kind, message));
        for kind in &MUTATIONS[..16] {
            assert_eq!(order(kind, &message), "write", "{kind}");
        }
        for kind in ["set", "ramp", "trigger", "note", "revert"] {
            assert_eq!(order(kind, &message), "live /a/freq", "{kind}");
            assert_eq!(order(kind, &json!({})), "live ", "{kind}");
        }
        assert_eq!(order("project.save", &message), "save");
        for kind in ["project.open", "project.revert"] {
            assert_eq!(order(kind, &message), "ordered", "{kind}");
        }
        for kind in [
            "ping",
            "describe",
            "stats",
            "subscribe",
            "unsubscribe",
            "file.read",
            "panel.read",
            "pattern.read",
            "snapshot.list",
            "setup.get",
            "app.get",
            "",
            "no.such",
        ] {
            assert_eq!(order(kind, &message), "free", "{kind}");
        }
    }

    fn code_of(result: Result<(), Failure>) -> &'static str {
        match result {
            Ok(()) => "ok",
            Err(failure) => failure.code,
        }
    }

    /// ADR 0049: a mutation naming another opening is refused; the
    /// generation decides before the id; nothing else is checked.
    #[test]
    fn a_mutation_names_the_open_project() {
        let host = idle_host("same-project");
        let id = host.project_id();
        for kind in MUTATIONS {
            let check = |message: Value| code_of(host.same_project(kind, &message));
            assert_eq!(check(json!({})), "ok", "{kind}");
            assert_eq!(check(json!({"generation": null, "projectId": null})), "ok");
            assert_eq!(check(json!({"generation": 7})), "ok", "{kind}");
            assert_eq!(check(json!({"generation": 8})), "project-changed");
            assert_eq!(check(json!({"generation": -1})), "bad-value");
            assert_eq!(check(json!({"generation": 7.5})), "bad-value");
            assert_eq!(check(json!({"generation": "7"})), "bad-value");
            assert_eq!(check(json!({"projectId": id})), "ok", "{kind}");
            assert_eq!(check(json!({"projectId": "other"})), "project-changed");
            assert_eq!(check(json!({"projectId": 7})), "bad-value");
            // The generation decides; the id is then not looked at.
            assert_eq!(check(json!({"generation": 7, "projectId": "x"})), "ok");
            assert_eq!(
                check(json!({"generation": 8, "projectId": id})),
                "project-changed"
            );
            assert_eq!(
                check(json!({"generation": null, "projectId": "x"})),
                "project-changed"
            );
        }
        for kind in ["ping", "describe", "subscribe", "file.read", "app.get", ""] {
            let message = json!({"generation": 8, "projectId": 3});
            assert_eq!(code_of(host.same_project(kind, &message)), "ok", "{kind}");
        }
        let changed = host.same_project("commit", &json!({"generation": 8}));
        assert_eq!(
            changed.err().map(|failure| failure.message),
            Some("not done: the request names the generation of a project no longer open (another project, or a new opening of it, is open now)".to_owned())
        );
        let changed = host.same_project("commit", &json!({"projectId": "x"}));
        assert_eq!(
            changed.err().map(|failure| failure.message),
            Some("not done: the request names the id of a project no longer open (another project, or a new opening of it, is open now)".to_owned())
        );
    }

    fn queued(kind: &str, address: Option<&str>) -> Waiting {
        Waiting {
            generation: Some(7),
            since: Instant::now(),
            job: Job::Message {
                client: 1,
                id: Value::Null,
                text: String::new(),
                kind: kind.to_owned(),
                address: address.map(str::to_owned),
                lock: true,
                named: false,
                permit: None,
            },
        }
    }

    /// ADR 0048: a live edit waits only behind a queued request that can
    /// change its value.
    #[test]
    fn a_live_edit_waits_behind_what_changes_its_value() {
        let mut host = idle_host("queued-for");
        assert!(!host.queued_for("/a/freq"));
        // The queued request's kind and address; whether a live edit of
        // `/a/freq`, and of `/b/freq`, waits behind it.
        let cases: [(&str, Option<&str>, bool, bool); 16] = [
            ("commit", None, true, true),
            ("commit", Some("/a/freq"), true, false),
            ("retry", Some("/a/freq"), true, false),
            ("retry", None, false, false),
            ("set", Some("/a/freq"), true, false),
            ("ramp", Some("/a/freq"), true, false),
            ("trigger", Some("/a/freq"), true, false),
            ("note", Some("/a/freq"), true, false),
            ("revert", Some("/a/freq"), true, false),
            ("snapshot.restore", None, true, true),
            ("snapshot.undo", None, true, true),
            ("project.open", None, true, true),
            ("project.revert", None, true, true),
            ("project.save", None, false, false),
            ("patch", Some("/a/freq"), false, false),
            ("snapshot.save", None, false, false),
        ];
        for (kind, address, a, b) in cases {
            host.waiting.clear();
            host.waiting.push_back(queued(kind, address));
            assert_eq!(host.queued_for("/a/freq"), a, "{kind} {address:?}");
            assert_eq!(host.queued_for("/b/freq"), b, "{kind} {address:?}");
        }
        // A queued Save changes no live value.
        host.waiting.clear();
        host.waiting.push_back(Waiting {
            generation: Some(7),
            since: Instant::now(),
            job: Job::Save {
                save_as: None,
                done: Box::new(|_| {}),
            },
        });
        assert!(!host.queued_for("/a/freq"));
        // Any one queued request that can change it is enough.
        host.waiting.push_back(queued("patch", None));
        host.waiting.push_back(queued("set", Some("/b/freq")));
        assert!(!host.queued_for("/a/freq"));
        assert!(host.queued_for("/b/freq"));
    }
}

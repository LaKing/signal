//! The requests waiting for the project's writer lock (ADR 0048): how each
//! message is ordered against them, the project it names (ADR 0049), and
//! their run once the lock is free.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_project::transaction;

use crate::server::Permit;

use super::Host;
use super::failure::{Failure, fail};
use super::project_io::{SaveDone, WindowsEdit};

/// What the host does with a message ([`class_of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Class {
    /// It changes the open project: it names the project it means, and a
    /// stale `generation` or `projectId` on it is refused (ADR 0049,
    /// [`Opening::admit`](super::opening::Opening::admit)).
    pub(super) mutation: bool,
    /// How it is ordered against the requests waiting for the lock.
    pub(super) order: Order,
}

/// What the host does with a message while requests wait for the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Order {
    /// It writes the project's files: run holding the lock, or queued.
    Write,
    /// `project.save`: run at once unless it would have to wait (its
    /// corrections need the lock) or requests wait; queued then.
    Save,
    /// Queued behind any waiting request.
    Ordered,
    /// A live edit of `address`: queued only behind a request that can
    /// change that value.
    Live(String),
    /// Never queued (`ping`, `describe`, `stats`, subscriptions, reads).
    Free,
}

/// The class of the message `kind` (ADR 0048, 0049); a live edit's
/// address is read from `message` (none is `""`). File writes wait for
/// the lock; a Save, an open or a revert of the project waits behind every
/// queued request; a live edit waits only behind a queued request that can
/// change the same value; everything else (`ping`, `describe`, `stats`,
/// subscriptions, reads) never waits and changes nothing.
pub(super) fn class_of(kind: &str, message: &Value) -> Class {
    let (mutation, order) = match kind {
        // The messages that write the project's files under its writer lock
        // (ADR 0040): each runs holding it, or waits in the host's queue
        // while another writer holds it (ADR 0048), never on the control
        // thread.
        "schematic.move"
        | "schematic.reset"
        | "patch"
        | "commit"
        | "retry"
        | "snapshot.save"
        | "snapshot.restore"
        | "snapshot.delete"
        | "snapshot.undo"
        | "snapshot.move"
        | "snapshot.transition"
        | "snapshot.exclude"
        | "panel.write"
        | "pattern.write"
        | "file.write"
        | "vendor" => (true, Order::Write),
        // Messages that save, revert or replace the open project: a stale
        // `generation` or `projectId` on one is refused as on any mutation
        // (ADR 0049), so a page of an earlier opening can neither discard
        // the new opening's edits nor save or switch it away.
        "project.save" => (true, Order::Save),
        // Kept behind every queued request (they save, open or revert the
        // project, so what was asked before them is done first).
        "project.open" | "project.revert" => (true, Order::Ordered),
        // The live edits that change a parameter's value at once: kept
        // behind a queued request that can change the same value, in the
        // client's order.
        "set" | "ramp" | "trigger" | "note" | "revert" => {
            let address = message.get("address").and_then(Value::as_str);
            (true, Order::Live(address.unwrap_or_default().to_owned()))
        }
        _ => (false, Order::Free),
    };
    Class { mutation, order }
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
    /// in the client's order ([`class_of`]). `kind` and `address` say
    /// which values it can change ([`LockQueue::queued_for`]).
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

/// The requests waiting for the project's writer lock (ADR 0048), and how
/// long one waits for it before it fails.
pub(super) struct LockQueue {
    /// ADR 0048: the file requests waiting for
    /// another writer to release the project's lock, in order.
    pub(super) waiting: VecDeque<Waiting>,
    /// How long a request waits for the lock before it fails.
    pub(super) lock_wait: Duration,
}

impl Default for LockQueue {
    fn default() -> LockQueue {
        LockQueue {
            waiting: VecDeque::new(),
            lock_wait: transaction::LOCK_WAIT,
        }
    }
}

impl LockQueue {
    /// Whether a queued request can change the value at `address`: a
    /// `commit`, `retry` or live edit of it, or a snapshot restore or undo,
    /// an open or a revert of the project (any value). A live edit of it
    /// then waits behind them, so the client's order holds for that value.
    pub(super) fn queued_for(&self, address: &str) -> bool {
        self.waiting.iter().any(|w| match &w.job {
            Job::Message {
                kind, address: a, ..
            } => match kind.as_str() {
                // A batch `commit` (`values`, no `address`) can change any.
                "commit" if a.is_none() => true,
                "commit" | "retry" => a.as_deref() == Some(address),
                k if matches!(class_of(k, &Value::Null).order, Order::Live(_)) => {
                    a.as_deref() == Some(address)
                }
                "snapshot.restore" | "snapshot.undo" => true,
                k => class_of(k, &Value::Null).order == Order::Ordered,
            },
            _ => false,
        })
    }
}

/// A request to the host, from any of its four entry paths: a client's
/// message ([`Host::handle`]), the window layout's write
/// ([`Host::write_windows_then`]), and the window's Save and Save As
/// ([`Host::save_document_then`], [`Host::save_as_then`]). Each goes
/// through [`Host::submit`]: run at once, or queued behind the writer lock.
pub(super) enum Request {
    /// A client's message, parsed.
    Message {
        client: u64,
        id: Value,
        text: String,
        kind: String,
        message: Value,
        permit: Option<Permit>,
    },
    /// The window layout's write (app mode).
    Windows {
        dir: PathBuf,
        edit: WindowsEdit,
        done: SaveDone,
    },
    /// The window's Save or Save As of the open project: one op, which
    /// waits behind the requests waiting for the writer lock or is refused
    /// while they wait (`when_waiting`).
    Save {
        save_as: Option<PathBuf>,
        when_waiting: WhenWaiting,
        done: SaveDone,
    },
}

/// What a Save does while requests wait for the writer lock (ADR 0048).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WhenWaiting {
    /// Refused, nothing saved ([`Host::save_document`]): a Save made now
    /// would leave out writes asked before it. Otherwise it is written
    /// before [`Host::submit`] returns, without the queue.
    Refuse,
    /// Queued behind them, as `project.save` is, and also while its
    /// corrections need the lock another writer holds
    /// ([`Host::save_document_then`], [`Host::save_as_then`]).
    Queue,
}

impl Request {
    /// How it is ordered against the requests waiting for the lock: a
    /// client's message by its class; the layout's write as a file write;
    /// the window's Save and Save As as `project.save`.
    fn order(&self) -> Order {
        match self {
            Request::Message { kind, message, .. } => class_of(kind, message).order,
            Request::Windows { .. } => Order::Write,
            Request::Save { .. } => Order::Save,
        }
    }

    /// The request as it waits in the queue (`lock`: a client's message
    /// runs holding the writer lock).
    fn into_job(self, lock: bool) -> Job {
        match self {
            Request::Message {
                client,
                id,
                text,
                kind,
                message,
                permit,
            } => Job::Message {
                client,
                id,
                text,
                kind,
                address: message
                    .get("address")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                lock,
                named: ["generation", "projectId"]
                    .iter()
                    .any(|key| message.get(*key).is_some_and(|v| !v.is_null())),
                permit,
            },
            Request::Windows { dir, edit, done } => Job::Windows { dir, edit, done },
            Request::Save { save_as, done, .. } => Job::Save { save_as, done },
        }
    }
}

impl Host {
    pub(super) fn handle(&mut self, client: u64, text: &str, permit: Option<Permit>) {
        let message: Value = match serde_json::from_str(text) {
            Ok(message) => message,
            Err(_) => {
                let error = json!({"type": "error", "code": "bad-message", "message": "not JSON"});
                return self.send_text(client, &error);
            }
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let kind = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        self.submit(Request::Message {
            client,
            id,
            text: text.to_owned(),
            kind,
            message,
            permit,
        });
    }

    /// Runs `request` now, or queues it behind the writer lock (ADR 0048):
    /// a request that writes the project's files runs holding the writer
    /// lock, taken without waiting; when another writer holds it (or
    /// requests wait already, to keep their order) it waits in the queue,
    /// not on this thread. A client's message naming another opening is
    /// refused first (ADR 0049).
    pub(super) fn submit(&mut self, request: Request) {
        if let Request::Save {
            when_waiting: WhenWaiting::Refuse,
            ..
        } = request
        {
            return self.save_or_refuse(request);
        }
        // The window's requests never wait for the lock on this thread.
        let _no_wait = matches!(request, Request::Windows { .. } | Request::Save { .. })
            .then(|| transaction::NoWait::enter(Duration::ZERO));
        if let Request::Message {
            client,
            id,
            kind,
            message,
            ..
        } = &request
            && let Err(failure) = self
                .opening
                .admit(class_of(kind, message).mutation, message)
        {
            let error = json!({
                "type": "error",
                "id": id,
                "code": failure.code,
                "message": failure.message,
            });
            return self.send_text(*client, &error);
        }
        match request.order() {
            Order::Write => {
                if self.queue.waiting.is_empty() {
                    match self.try_hold() {
                        Ok(Some(holding)) => {
                            self.execute(request);
                            drop(holding);
                            return;
                        }
                        Ok(None) => {}
                        // The writer's own `acquire` says why, at once.
                        Err(_) => return self.execute(request),
                    }
                }
                self.wait_for_lock(request.into_job(true));
            }
            Order::Save => {
                if self.queue.waiting.is_empty() && !self.save_must_wait() {
                    return self.execute(request);
                }
                self.wait_for_lock(request.into_job(true));
            }
            Order::Ordered if !self.queue.waiting.is_empty() => {
                self.wait_for_lock(request.into_job(false));
            }
            Order::Live(address) if self.queue.queued_for(&address) => {
                self.wait_for_lock(request.into_job(false));
            }
            _ => self.execute(request),
        }
    }

    /// A Save that is refused while requests wait for the writer lock
    /// ([`WhenWaiting::Refuse`]), else run now.
    fn save_or_refuse(&mut self, request: Request) {
        if self.opening.document.is_some() && !self.queue.waiting.is_empty() {
            if let Request::Save { done, .. } = request {
                self.save_document_refused(done);
            }
            return;
        }
        self.execute(request);
    }

    /// Runs `request` now: a client's message answered, the layout written,
    /// the Save or Save As started (or, one refused while requests wait,
    /// written).
    fn execute(&mut self, request: Request) {
        match request {
            Request::Message {
                client,
                id,
                kind,
                message,
                ..
            } => {
                let ctx = self.write_ctx();
                self.respond(&ctx, client, id, &kind, &message);
            }
            Request::Windows { dir, edit, done } => {
                let ctx = self.write_ctx();
                done(self.write_windows_now(&ctx, &dir, edit));
            }
            Request::Save {
                save_as,
                when_waiting,
                done,
            } => {
                let ctx = self.write_ctx();
                match (save_as, when_waiting) {
                    (Some(file), _) => self.save_as_now(&ctx, &file, done),
                    (None, WhenWaiting::Queue) => self.save_document_now(&ctx, done),
                    (None, WhenWaiting::Refuse) => self.save_document_here(&ctx, done),
                }
            }
        }
    }

    /// Whether a Save now would have to wait for the writer lock: its
    /// corrections are pending and another writer holds the lock.
    pub(super) fn save_must_wait(&self) -> bool {
        self.opening.document.is_some()
            && !self.pending_corrections().is_empty()
            && matches!(self.try_hold(), Ok(None))
    }

    /// Queues `job` until the project's writer lock is free
    /// ([`Host::run_waiting`]); refused at once (`busy`) when
    /// [`WAITING_MAX`] requests wait already.
    pub(super) fn wait_for_lock(&mut self, job: Job) {
        if self.queue.waiting.len() >= WAITING_MAX {
            let message = format!(
                "not done: {WAITING_MAX} requests already wait for another writer to release the project's lock; try again"
            );
            return self.answer_job(job, fail("busy", message));
        }
        // A request naming no project, queued behind an open or revert,
        // means the project open when it runs, not the one open now.
        let named = matches!(job, Job::Message { named: true, .. });
        let behind_an_open = self.queue.waiting.iter().any(|w| {
            matches!(&w.job, Job::Message { kind, .. } if class_of(kind, &Value::Null).order == Order::Ordered)
        });
        self.queue.waiting.push_back(Waiting {
            generation: (named || !behind_an_open).then_some(self.opening.generation),
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
        while let Some(front) = self.queue.waiting.front() {
            if front
                .generation
                .is_some_and(|g| g != self.opening.generation)
            {
                if let Some(stale) = self.queue.waiting.pop_front() {
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
            let holding = match lock.then(|| self.try_hold()) {
                None => None,
                Some(Ok(Some(holding))) => Some(holding),
                Some(Ok(None)) if front.since.elapsed() < self.queue.lock_wait => break,
                Some(Ok(None) | Err(_)) => None,
            };
            let Some(waiting) = self.queue.waiting.pop_front() else {
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
                        match self
                            .opening
                            .admit(class_of(&kind, &message).mutation, &message)
                        {
                            Ok(()) => {
                                let ctx = self.write_ctx();
                                self.respond(&ctx, client, id, &kind, &message);
                            }
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
                Job::Windows { dir, edit, done } => {
                    let ctx = self.write_ctx();
                    done(self.write_windows_now(&ctx, &dir, edit));
                }
                Job::Save { save_as, done } => {
                    let ctx = self.write_ctx();
                    match save_as {
                        Some(file) => self.save_as_now(&ctx, &file, done),
                        None => self.save_document_now(&ctx, done),
                    }
                }
            }
            drop(holding);
        }
    }

    /// Whether requests wait for the project's writer lock (the control
    /// loop then steps rather than sleeping in a wait).
    pub fn waiting(&self) -> bool {
        !self.queue.waiting.is_empty()
    }

    /// Test hooks: how long a request waits for the writer lock before it
    /// fails ([`transaction::LOCK_WAIT`] by default).
    #[cfg(feature = "test-hooks")]
    pub fn set_lock_wait(&mut self, wait: Duration) {
        self.queue.lock_wait = wait;
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
        host.opening.dir = base.join("project");
        host.opening.generation = 7;
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
        let order = |kind: &str, message: &Value| order_name(class_of(kind, message).order);
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
        let id = host.opening.project_id();
        for kind in MUTATIONS {
            let check = |message: Value| {
                code_of(
                    host.opening
                        .admit(class_of(kind, &message).mutation, &message),
                )
            };
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
            assert_eq!(
                code_of(
                    host.opening
                        .admit(class_of(kind, &message).mutation, &message)
                ),
                "ok",
                "{kind}"
            );
        }
        let changed = host.opening.admit(
            class_of("commit", &Value::Null).mutation,
            &json!({"generation": 8}),
        );
        assert_eq!(
            changed.err().map(|failure| failure.message),
            Some("not done: the request names the generation of a project no longer open (another project, or a new opening of it, is open now)".to_owned())
        );
        let changed = host.opening.admit(
            class_of("commit", &Value::Null).mutation,
            &json!({"projectId": "x"}),
        );
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
        assert!(!host.queue.queued_for("/a/freq"));
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
            host.queue.waiting.clear();
            host.queue.waiting.push_back(queued(kind, address));
            assert_eq!(host.queue.queued_for("/a/freq"), a, "{kind} {address:?}");
            assert_eq!(host.queue.queued_for("/b/freq"), b, "{kind} {address:?}");
        }
        // A queued Save changes no live value.
        host.queue.waiting.clear();
        host.queue.waiting.push_back(Waiting {
            generation: Some(7),
            since: Instant::now(),
            job: Job::Save {
                save_as: None,
                done: Box::new(|_| {}),
            },
        });
        assert!(!host.queue.queued_for("/a/freq"));
        // Any one queued request that can change it is enough.
        host.queue.waiting.push_back(queued("patch", None));
        host.queue.waiting.push_back(queued("set", Some("/b/freq")));
        assert!(!host.queue.queued_for("/a/freq"));
        assert!(host.queue.queued_for("/b/freq"));
    }
}

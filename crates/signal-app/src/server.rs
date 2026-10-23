//! The server in the app (ADR 0023 §2): the control thread, which owns the
//! UI protocol's host in app mode (`signal_ui::Host::new_app`), and so the
//! run, the file watch and the Setup, with Signal.app's rules
//! (`signal_ui::app`, shared with [`AppState`](crate::state::AppState)).
//! The window's WebView is one of its clients: it loads [`AppServer::url`],
//! whose fragment carries the session token. The control thread steps the
//! host about every millisecond, as `signal play` polls, so the runner's
//! queues are drained whether or not a page is open; switched off
//! (`app.power`), it waits blocked for the next message instead.
//!
//! The window asks the control thread too (M2.8-12): the open project and
//! whether it is edited (before a close prompt), Save, Save As and Open of
//! a chosen or dropped file; each request waits for its answer.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use signal_core::Registry;
use signal_ui::keys::Keys;
use signal_ui::{APP_PAGE, AppHostConfig, Edits, Host, ProjectStatus, ServerOptions};

use crate::setup::{Setup, Window};
use crate::state::{Backend, compute_lines};

/// Each client's queue of outgoing messages (as `signal ui`'s).
const CLIENT_QUEUE: usize = 256;

/// How long the control thread waits for a message while the app is
/// switched off: also how soon it then sees the window's requests.
const OFF_WAIT: Duration = Duration::from_millis(100);

/// What the window is told whenever the open project or its edited state
/// changes (its title).
pub type OnProject = Box<dyn Fn(Option<ProjectStatus>) + Send>;

/// What the window is told whenever the keymap changes (ADR 0034: the
/// person's `keys.toml` written or edited elsewhere): its menu's chords.
pub type OnKeys = Box<dyn Fn(&Keys) + Send>;

/// What the server in the app runs.
pub struct AppConfig {
    /// The Setup file.
    pub setup_path: PathBuf,
    /// The project folder or `.signal` file to open (else the Setup's last
    /// project).
    pub project: Option<PathBuf>,
    /// Where opened `.signal` files are unpacked
    /// (`signal_ui::document::open_root`).
    pub open_root: PathBuf,
    /// CoreAudio, or the null driver with a device list.
    pub backend: Backend,
    /// The UI library to serve (`signal_ui::library_dir`).
    pub ui_dir: PathBuf,
    /// Told on the control thread whenever the project shown changes.
    pub on_project: Option<OnProject>,
    /// Told on the control thread whenever the keymap changes.
    pub on_keys: Option<OnKeys>,
}

/// What the window asks the control thread.
enum Request {
    /// Exit: the window's size, and whether an edited project's changes are
    /// let go (else its working folder is kept).
    Exit(Option<Window>, bool),
    /// The open project, its edited state looked at now.
    Project(Sender<Option<ProjectStatus>>),
    /// Save.
    Save(Sender<Result<(), String>>),
    /// Save As this `.signal` file.
    SaveAs(PathBuf, Sender<Result<(), String>>),
    /// Open this folder or `.signal` file.
    Open(PathBuf, Edits, Sender<Result<(), String>>),
    /// Edit `ui/windows.toml` of this project folder (ADR 0032).
    WriteWindows(PathBuf, WindowsEdit, Sender<Result<(), String>>),
}

/// An edit of `ui/windows.toml`'s text (`signal_ui::Host::write_windows`).
pub type WindowsEdit = Box<dyn FnOnce(&str) -> Result<String, String> + Send>;

/// What the control thread hands back once its host runs.
struct Ready {
    url: String,
    token: String,
    #[cfg(feature = "test-hooks")]
    connector: signal_ui::LocalConnector,
}

/// The control thread and the host's servers.
pub struct AppServer {
    url: String,
    token: String,
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
    #[cfg(feature = "test-hooks")]
    connector: signal_ui::LocalConnector,
}

impl AppServer {
    /// Starts the control thread and its servers on 127.0.0.1; returns once
    /// they run (or why they did not start).
    pub fn start(config: AppConfig) -> Result<AppServer, String> {
        AppServer::spawn(config, false)
    }

    /// Test hooks: [`start`](AppServer::start) with servers that bind
    /// nothing; clients connect with [`connect_local`](AppServer::connect_local).
    #[cfg(feature = "test-hooks")]
    pub fn start_local(config: AppConfig) -> Result<AppServer, String> {
        AppServer::spawn(config, true)
    }

    /// Test hooks: a new client's end of an in-process connection.
    #[cfg(feature = "test-hooks")]
    pub fn connect_local(&self) -> signal_ui::LocalStream {
        self.connector.connect()
    }

    fn spawn(mut config: AppConfig, local: bool) -> Result<AppServer, String> {
        let (ready_tx, ready) = mpsc::channel::<Result<Ready, String>>();
        let (requests, received) = mpsc::channel::<Request>();
        let on_project = config.on_project.take();
        let on_keys = config.on_keys.take();
        let thread = thread::Builder::new()
            .name("signal-control".to_owned())
            .spawn(move || {
                // Built on this thread: the audio streams it opens stay here.
                let mut host = match host(config, local) {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(Ready {
                    url: host.page_url(APP_PAGE),
                    token: host.token().to_owned(),
                    #[cfg(feature = "test-hooks")]
                    connector: host.local_connector(),
                }));
                run(&mut host, &received, on_project, on_keys);
            })
            .map_err(|error| format!("the control thread did not start: {error}"))?;
        let ready = match ready.recv() {
            Ok(Ok(ready)) => ready,
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = thread.join();
                return Err("the control thread ended before its server ran".to_owned());
            }
        };
        Ok(AppServer {
            url: ready.url,
            token: ready.token,
            requests: Some(requests),
            thread: Some(thread),
            #[cfg(feature = "test-hooks")]
            connector: ready.connector,
        })
    }

    /// The window's page, with the WebSocket's port and the session token
    /// in its fragment.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The session token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Sends `request` (made with its answer's sender) and waits for the
    /// answer; `gone` if the control thread has ended.
    fn ask<T>(&self, request: impl FnOnce(Sender<T>) -> Request, gone: T) -> T {
        let (answer, answered) = mpsc::channel();
        match &self.requests {
            Some(requests) if requests.send(request(answer)).is_ok() => {
                answered.recv().unwrap_or(gone)
            }
            _ => gone,
        }
    }

    /// The open project, whether it is edited looked at now.
    pub fn project(&self) -> Option<ProjectStatus> {
        self.ask(Request::Project, None)
    }

    /// Save: a `.signal` project's working folder written back to its file.
    pub fn save(&self) -> Result<(), String> {
        self.ask(Request::Save, Err(GONE.to_owned()))
    }

    /// Save As the `.signal` file `file`.
    pub fn save_as(&self, file: &Path) -> Result<(), String> {
        let file = file.to_owned();
        self.ask(|a| Request::SaveAs(file, a), Err(GONE.to_owned()))
    }

    /// Opens the folder, `project.toml` or `.signal` file at `path`, with
    /// what to do with an edited project's changes.
    pub fn open(&self, path: &Path, edits: Edits) -> Result<(), String> {
        let path = path.to_owned();
        self.ask(|a| Request::Open(path, edits, a), Err(GONE.to_owned()))
    }

    /// The window layout of the project folder `dir` (ADR 0032): its
    /// `ui/windows.toml` edited by `edit`, written by the control thread,
    /// which takes the write as seen (nothing reloads); refused when `dir`
    /// is no longer the open project.
    pub fn write_windows(&self, dir: &Path, edit: WindowsEdit) -> Result<(), String> {
        let dir = dir.to_owned();
        self.ask(
            |a| Request::WriteWindows(dir, edit, a),
            Err(GONE.to_owned()),
        )
    }

    /// [`write_windows`](AppServer::write_windows) without waiting: the
    /// answer comes on the receiver, so the window's event loop never
    /// waits for the write (and its sync).
    pub fn write_windows_later(
        &self,
        dir: &Path,
        edit: WindowsEdit,
    ) -> mpsc::Receiver<Result<(), String>> {
        let (answer, answered) = mpsc::channel();
        let gone = answer.clone();
        let request = Request::WriteWindows(dir.to_owned(), edit, answer);
        if self
            .requests
            .as_ref()
            .is_none_or(|requests| requests.send(request).is_err())
        {
            let _ = gone.send(Err(GONE.to_owned()));
        }
        answered
    }

    /// Stops playing, records the window's size (if known) in the Setup,
    /// saves it, closes the project (an edited one's working folder is kept
    /// unless `discard`), stops the servers (their clients are closed), and
    /// waits for the control thread.
    pub fn exit(&mut self, window: Option<Window>, discard: bool) {
        if let Some(requests) = self.requests.take() {
            let _ = requests.send(Request::Exit(window, discard));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const GONE: &str = "the app's control thread has ended";

/// Without [`exit`](AppServer::exit), the control thread still stops,
/// saves the Setup and is waited for; an edited project's working folder is
/// kept.
impl Drop for AppServer {
    fn drop(&mut self) {
        self.exit(None, false);
    }
}

/// The host in app mode, with the standard modules and the Setup's compute
/// inventory.
fn host(config: AppConfig, local: bool) -> Result<Host, String> {
    let mut registry = Registry::new();
    if let Err(error) = signal_std::register(&mut registry) {
        eprintln!("signal-app: the standard modules did not register: {error}");
    }
    let (setup, _) = Setup::load(&config.setup_path);
    let config = AppHostConfig {
        setup_path: config.setup_path,
        project: config.project,
        open_root: config.open_root,
        registry,
        backend: config.backend,
        compute: compute_lines(&setup.gpu_devices),
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: config.ui_dir,
            project_ui: PathBuf::new(),
            client_queue: CLIENT_QUEUE,
        },
        // ADR 0033: no Play or Stop; the open project plays while the app
        // is switched on.
        plays: true,
    };
    #[cfg(feature = "test-hooks")]
    if local {
        return Host::new_app_local(config);
    }
    let _ = local;
    Host::new_app(config)
}

/// The control loop: steps the host and answers the window until it exits
/// (or goes away without saying so, which saves without the window's size
/// and keeps an edited project's working folder).
fn run(
    host: &mut Host,
    requests: &Receiver<Request>,
    on_project: Option<OnProject>,
    on_keys: Option<OnKeys>,
) {
    let mut shown = None;
    let mut keys = host.keys_version();
    loop {
        match requests.try_recv() {
            Ok(Request::Exit(window, discard)) => return exit(host, window, discard),
            Err(TryRecvError::Disconnected) => return exit(host, None, false),
            Ok(Request::Project(answer)) => {
                let _ = answer.send(host.project_status());
            }
            Ok(Request::Save(answer)) => {
                let _ = answer.send(host.save_document());
            }
            Ok(Request::SaveAs(file, answer)) => {
                let _ = answer.send(host.save_as(&file));
            }
            Ok(Request::Open(path, edits, answer)) => {
                let _ = answer.send(host.open_project(&path, edits));
            }
            Ok(Request::WriteWindows(dir, edit, answer)) => {
                let _ = answer.send(host.write_windows(&dir, edit));
            }
            Err(TryRecvError::Empty) => {}
        }
        for message in host.step() {
            eprintln!("signal-app: {message}");
        }
        if let Some(on_project) = &on_project {
            let now = host.project_shown();
            if now != shown {
                on_project(now.clone());
                shown = now;
            }
        }
        if let Some(on_keys) = &on_keys
            && host.keys_version() != keys
        {
            keys = host.keys_version();
            on_keys(host.keys());
        }
        if host.powered() {
            thread::sleep(Duration::from_millis(1));
        } else {
            // Switched off: nothing runs or streams, so the thread sleeps
            // in the wait for a client's message (or the window's request).
            host.wait(OFF_WAIT);
        }
    }
}

fn exit(host: &mut Host, window: Option<Window>, discard: bool) {
    for message in host.close(window, discard) {
        eprintln!("signal-app: {message}");
    }
}

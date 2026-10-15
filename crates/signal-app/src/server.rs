//! The server in the app (ADR 0023 §2): the control thread, which owns the
//! UI protocol's host in app mode (`signal_ui::Host::new_app`), and so the
//! run, the file watch and the Setup, with Signal.app's rules
//! (`signal_ui::app`, shared with [`AppState`](crate::state::AppState)).
//! The window's WebView is one of its clients: it loads [`AppServer::url`],
//! whose fragment carries the session token. The control thread steps the
//! host about every millisecond, as `signal play` polls, so the runner's
//! queues are drained whether or not a page is open; switched off
//! (`app.power`), it waits blocked for the next message instead.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use signal_core::Registry;
use signal_ui::{APP_PAGE, AppHostConfig, Host, ServerOptions};

use crate::setup::{Setup, Window};
use crate::state::{Backend, compute_lines};

/// Each client's queue of outgoing messages (as `signal ui`'s).
const CLIENT_QUEUE: usize = 256;

/// How long the control thread waits for a message while the app is
/// switched off: also how soon it then sees the window's exit.
const OFF_WAIT: Duration = Duration::from_millis(100);

/// What the server in the app runs.
pub struct AppConfig {
    /// The Setup file.
    pub setup_path: PathBuf,
    /// The project folder to open (else the Setup's last project).
    pub project: Option<PathBuf>,
    /// CoreAudio, or the null driver with a device list.
    pub backend: Backend,
    /// The UI library to serve (`signal_ui::library_dir`).
    pub ui_dir: PathBuf,
}

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
    exit: Option<Sender<Option<Window>>>,
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

    fn spawn(config: AppConfig, local: bool) -> Result<AppServer, String> {
        let (ready_tx, ready) = mpsc::channel::<Result<Ready, String>>();
        let (exit, requests) = mpsc::channel::<Option<Window>>();
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
                run(&mut host, &requests);
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
            exit: Some(exit),
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

    /// Stops playing, records the window's size (if known) in the Setup,
    /// saves it, stops the servers (their clients are closed), and waits
    /// for the control thread.
    pub fn exit(&mut self, window: Option<Window>) {
        if let Some(exit) = self.exit.take() {
            let _ = exit.send(window);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Without [`exit`](AppServer::exit), the control thread still stops,
/// saves the Setup and is waited for.
impl Drop for AppServer {
    fn drop(&mut self) {
        self.exit(None);
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
    };
    #[cfg(feature = "test-hooks")]
    if local {
        return Host::new_app_local(config);
    }
    let _ = local;
    Host::new_app(config)
}

/// The control loop: steps the host until the window exits (or goes away
/// without saying so, which saves without the window's size).
fn run(host: &mut Host, requests: &Receiver<Option<Window>>) {
    loop {
        let window = match requests.try_recv() {
            Ok(window) => Some(window),
            Err(TryRecvError::Disconnected) => Some(None),
            Err(TryRecvError::Empty) => None,
        };
        if let Some(window) = window {
            for message in host.close(window) {
                eprintln!("signal-app: {message}");
            }
            return;
        }
        for message in host.step() {
            eprintln!("signal-app: {message}");
        }
        if host.powered() {
            thread::sleep(Duration::from_millis(1));
        } else {
            // Switched off: nothing runs or streams, so the thread sleeps
            // in the wait for a client's message (or the window's exit).
            host.wait(OFF_WAIT);
        }
    }
}

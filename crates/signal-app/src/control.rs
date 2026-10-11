//! The control thread: owns the [`AppState`] (and so the `Controller`, the
//! file watch and the Setup), and talks to the GUI thread only through two
//! `rtrb` queues, commands in and [`View`] snapshots out (`docs/architecture.md`,
//! "Threads"). It polls about every millisecond, as `signal play` does, so the
//! runner's queues are drained whether or not the window is drawn.

use std::io;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::setup::Window;
use crate::state::{AppState, Command, View};

/// How often a snapshot is sent.
const PUBLISH: Duration = Duration::from_millis(16);

/// The GUI thread's side of the control thread.
pub struct Control {
    commands: Option<Producer<Command>>,
    views: Consumer<View>,
    thread: Option<JoinHandle<()>>,
}

impl Control {
    /// Starts the control thread. `make` builds the state on that thread
    /// (the audio streams it opens stay there); `repaint` asks the window to
    /// redraw after each snapshot.
    pub fn spawn(
        make: impl FnOnce() -> AppState + Send + 'static,
        repaint: impl Fn() + Send + 'static,
    ) -> io::Result<Control> {
        let (commands, mut incoming) = RingBuffer::<Command>::new(64);
        let (mut outgoing, views) = RingBuffer::<View>::new(4);
        let thread = thread::Builder::new()
            .name("signal-control".to_owned())
            .spawn(move || {
                let mut state = make();
                let mut published: Option<Instant> = None;
                let mut last: Option<View> = None;
                loop {
                    // Checked before draining, so a command pushed just
                    // before the window went away is still carried out.
                    let abandoned = incoming.is_abandoned();
                    while let Ok(command) = incoming.pop() {
                        let exit = matches!(command, Command::Exit(_));
                        state.apply(command);
                        if exit {
                            return;
                        }
                    }
                    if abandoned {
                        // The window is gone without saying so: save anyway.
                        state.apply(Command::Exit(None));
                        return;
                    }
                    state.tick();
                    if published.is_none_or(|at| at.elapsed() >= PUBLISH) {
                        published = Some(Instant::now());
                        // Only a changed view is sent and redrawn (meters
                        // change while playing; nothing does while idle).
                        let view = state.view();
                        if last.as_ref() != Some(&view) {
                            // A full queue: the window is behind; it gets
                            // the next change.
                            if outgoing.push(view.clone()).is_ok() {
                                last = Some(view);
                            }
                            repaint();
                        }
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })?;
        Ok(Control {
            commands: Some(commands),
            views,
            thread: Some(thread),
        })
    }

    /// Sends `command`; hands it back when the queue is full.
    pub fn send(&mut self, command: Command) -> Result<(), Command> {
        match self.commands.as_mut() {
            Some(commands) => commands.push(command).map_err(|PushError::Full(c)| c),
            None => Err(command),
        }
    }

    /// The newest snapshot since the last call, if any.
    pub fn latest(&mut self) -> Option<View> {
        let mut latest = None;
        while let Ok(view) = self.views.pop() {
            latest = Some(view);
        }
        latest
    }

    /// Asks the control thread to stop playing, record `window` and save the
    /// Setup, and waits until it has. If the queue stays full, the queue is
    /// dropped instead, which the control thread takes as an exit without
    /// the window's size.
    pub fn exit(&mut self, window: Option<Window>) {
        let mut command = Command::Exit(window);
        let deadline = Instant::now() + Duration::from_secs(1);
        while let Err(back) = self.send(command) {
            if Instant::now() >= deadline {
                break;
            }
            command = back;
            thread::sleep(Duration::from_millis(1));
        }
        self.join();
    }

    /// Drops the command queue, which the control thread takes as an exit,
    /// and waits for it.
    fn join(&mut self) {
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Without [`exit`](Control::exit) (the window dropped without closing),
/// the control thread still stops, saves the Setup and is waited for.
impl Drop for Control {
    fn drop(&mut self) {
        self.join();
    }
}

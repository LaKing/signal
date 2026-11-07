//! The body of the device's audio callback: runs the [`Runner`] into a
//! scratch buffer of the sink's channels, maps them to the device's, feeds
//! the secondary devices' adapters (ADR 0052), measures the callback, and
//! hands the Runner back on request.

use std::sync::Arc;
use std::time::Instant;

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use signal_output::Runner;

use crate::map::ChannelMap;
use crate::secondary::SecondaryFeed;
use crate::stats::CallbackStats;

/// What the audio callback holds: the Runner, a scratch buffer, the channel
/// map and the statistics. [`fill`](OutputCallback::fill) allocates, frees,
/// locks and blocks nowhere, and never drops the Runner: on a stop request it
/// moves it into a one-slot return queue (after its fade-out, with
/// [`with_fade_out`](OutputCallback::with_fade_out)), and keeps it to retry
/// if the push fails.
pub struct OutputCallback {
    runner: Option<Runner>,
    scratch: Vec<f32>,
    map: ChannelMap,
    sample_rate: u32,
    stats: Arc<CallbackStats>,
    stop: Consumer<()>,
    back: Producer<Runner>,
    stopping: bool,
    /// The fade-out before a stop, in frames (ADR 0036 §4); 0 hands the
    /// Runner back at once.
    fade: u32,
    /// A stop was requested and the Runner is fading out.
    fading: bool,
    /// The secondary devices' feeds (ADR 0052), by slot.
    feeds: Vec<Option<SecondaryFeed>>,
    /// Feeds to put into a slot (`None`: empty it), from [`FeedControl`].
    feed_in: Option<Consumer<(usize, Option<SecondaryFeed>)>>,
    /// Feeds taken out, for the control thread to drop.
    feed_back: Option<Producer<SecondaryFeed>>,
}

/// The control thread's side of an [`OutputCallback`]'s feeds: puts a
/// secondary device's feed into a slot or empties it, and takes the
/// replaced feeds back to drop them off the audio thread.
pub struct FeedControl {
    commands: Producer<(usize, Option<SecondaryFeed>)>,
    back: Consumer<SecondaryFeed>,
    slots: usize,
}

impl FeedControl {
    /// Puts `feed` into `slot` (`None`: empties it); the callback applies
    /// it at its next start. False when the queue is full or the slot does
    /// not exist (the feed is dropped here, on the control thread).
    pub fn set(&mut self, slot: usize, feed: Option<SecondaryFeed>) -> bool {
        self.collect();
        slot < self.slots && self.commands.push((slot, feed)).is_ok()
    }

    /// Drops the feeds the callback took out (control thread).
    pub fn collect(&mut self) {
        while self.back.pop().is_ok() {}
    }

    /// The slots.
    pub fn slots(&self) -> usize {
        self.slots
    }
}

/// The control thread's side of an [`OutputCallback`]: asks it to stop and
/// takes the Runner back.
pub struct CallbackControl {
    stop: Producer<()>,
    back: Consumer<Runner>,
}

impl OutputCallback {
    /// A callback running `runner` (a sink of `map.sink_channels()`) for
    /// device callbacks of up to `max_frames` frames at `sample_rate`. Larger
    /// device buffers are run in pieces. Built on the control thread.
    pub fn new(
        runner: Runner,
        map: ChannelMap,
        max_frames: usize,
        sample_rate: u32,
        stats: Arc<CallbackStats>,
    ) -> (OutputCallback, CallbackControl) {
        let (stop_tx, stop_rx) = RingBuffer::new(1);
        let (back_tx, back_rx) = RingBuffer::new(1);
        let callback = OutputCallback {
            runner: Some(runner),
            scratch: vec![0.0; max_frames.max(1) * map.sink_channels()],
            map,
            sample_rate,
            stats,
            stop: stop_rx,
            back: back_tx,
            stopping: false,
            fade: 0,
            fading: false,
            feeds: Vec::new(),
            feed_in: None,
            feed_back: None,
        };
        let control = CallbackControl {
            stop: stop_tx,
            back: back_rx,
        };
        (callback, control)
    }

    /// With `slots` empty slots for secondary devices' feeds (ADR 0052),
    /// filled and emptied through the returned [`FeedControl`]. Control
    /// thread: allocates the slots and queues.
    pub fn with_feeds(mut self, slots: usize) -> (OutputCallback, FeedControl) {
        let queue = (2 * slots).max(1);
        let (commands, feed_in) = RingBuffer::new(queue);
        let (feed_back, back) = RingBuffer::new(queue);
        self.feeds = (0..slots).map(|_| None).collect();
        self.feed_in = Some(feed_in);
        self.feed_back = Some(feed_back);
        let control = FeedControl {
            commands,
            back,
            slots,
        };
        (self, control)
    }

    /// One device callback: `data` holds frames of the device's channels,
    /// interleaved. Runs the Runner over them (silence once stopping or
    /// stopped), and records the callback's duration in the statistics.
    pub fn fill(&mut self, data: &mut [f32]) {
        self.fill_at(data, None);
    }

    /// [`fill`](OutputCallback::fill) at the callback's host time `time_ns`
    /// (nanoseconds, the clock all streams share), if known: each piece of
    /// the run's frame also goes to the secondary devices' feeds, stamped
    /// with its own time.
    pub fn fill_at(&mut self, data: &mut [f32], time_ns: Option<u64>) {
        let start = Instant::now();
        self.apply_feeds();
        if self.stop.pop().is_ok() && !self.fading {
            self.fading = true;
            if let Some(runner) = self.runner.as_mut() {
                runner.fade_out(self.fade);
            }
        }
        self.hand_back();
        let device = self.map.device_channels();
        let sink = self.map.sink_channels();
        let frames = data.len().checked_div(device).unwrap_or(0);
        match self.runner.as_mut() {
            Some(runner) if !self.stopping && sink > 0 && device > 0 => {
                let piece = (self.scratch.len() / sink).max(1);
                let frame_ns = 1e9 / f64::from(self.sample_rate.max(1));
                let mut offset = 0u64;
                for out in data.chunks_mut(piece * device) {
                    let n = out.len() / device;
                    if let Some(src) = self.scratch.get_mut(..n * sink) {
                        // The piece's place in the callback: the inputs
                        // time their frames from its start.
                        runner.process_at(src, sink, offset as usize);
                        self.map.scatter(src, out);
                        let time =
                            time_ns.map(|t| t.saturating_add((offset as f64 * frame_ns) as u64));
                        let ceiling = runner.ceiling();
                        for feed in self.feeds.iter_mut().flatten() {
                            feed.push(src, sink, time, ceiling);
                        }
                    }
                    offset += n as u64;
                    if let Some(rest) = out.get_mut(n * device..) {
                        rest.fill(0.0);
                    }
                }
            }
            _ => data.fill(0.0),
        }
        // A fade that ended in this callback hands the Runner back now.
        self.hand_back();
        self.stats.record(frames, start.elapsed(), self.sample_rate);
    }

    /// With a fade-out before each stop of `frames` frames (ADR 0036 §4): a
    /// stop request ramps the Runner's output out
    /// ([`Runner::fade_out`]), the Runner runs on until it has faded out,
    /// and only then goes back. 0 (the default) hands it back on the stop
    /// request's callback.
    pub fn with_fade_out(mut self, frames: u32) -> OutputCallback {
        self.fade = frames;
        self
    }

    /// Applies the feeds the control thread sent: a slot's old feed goes
    /// back to it, never dropped here (a command waits while the return
    /// queue is full).
    fn apply_feeds(&mut self) {
        let (Some(commands), Some(back)) = (self.feed_in.as_mut(), self.feed_back.as_mut()) else {
            return;
        };
        while let Ok(slot) = commands.peek().map(|(slot, _)| *slot) {
            let occupied = self.feeds.get(slot).is_some_and(Option::is_some);
            if occupied && back.is_full() {
                break;
            }
            let Ok((slot, feed)) = commands.pop() else {
                break;
            };
            let Some(place) = self.feeds.get_mut(slot) else {
                // Never sent ([`FeedControl::set`] checks); kept from the
                // audio thread's drop all the same.
                if let Some(feed) = feed {
                    let _ = back.push(feed);
                }
                continue;
            };
            if let Some(old) = std::mem::replace(place, feed) {
                let _ = back.push(old);
            }
        }
    }

    /// Once a stop was requested and the Runner has faded out (or there is
    /// nothing to fade), moves it into the return queue; keeps it to retry
    /// if the push fails. Never drops it.
    fn hand_back(&mut self) {
        let silent = self.map.sink_channels() == 0 || self.map.device_channels() == 0;
        if self.fading && self.runner.as_ref().is_none_or(|r| silent || r.faded_out()) {
            self.stopping = true;
        }
        if self.stopping
            && let Some(runner) = self.runner.take()
            && let Err(PushError::Full(runner)) = self.back.push(runner)
        {
            // Never dropped here: kept, and returned on a later callback.
            self.runner = Some(runner);
        }
    }
}

impl CallbackControl {
    /// Asks the callback to stop running the Runner and hand it back.
    pub fn request_stop(&mut self) {
        let _ = self.stop.push(());
    }

    /// The Runner, once the callback has handed it back.
    pub fn take_runner(&mut self) -> Option<Runner> {
        self.back.pop().ok()
    }
}

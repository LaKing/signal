//! The body of the device's audio callback: runs the [`Runner`] into a
//! scratch buffer of the sink's channels, maps them to the device's, measures
//! the callback, and hands the Runner back on request.

use std::sync::Arc;
use std::time::Instant;

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use signal_output::Runner;

use crate::map::ChannelMap;
use crate::stats::CallbackStats;

/// What the audio callback holds: the Runner, a scratch buffer, the channel
/// map and the statistics. [`fill`](OutputCallback::fill) allocates, frees,
/// locks and blocks nowhere, and never drops the Runner: on a stop request it
/// moves it into a one-slot return queue, and keeps it to retry if the push
/// fails.
pub struct OutputCallback {
    runner: Option<Runner>,
    scratch: Vec<f32>,
    map: ChannelMap,
    sample_rate: u32,
    stats: Arc<CallbackStats>,
    stop: Consumer<()>,
    back: Producer<Runner>,
    stopping: bool,
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
        };
        let control = CallbackControl {
            stop: stop_tx,
            back: back_rx,
        };
        (callback, control)
    }

    /// One device callback: `data` holds frames of the device's channels,
    /// interleaved. Runs the Runner over them (silence once stopping or
    /// stopped), and records the callback's duration in the statistics.
    pub fn fill(&mut self, data: &mut [f32]) {
        let start = Instant::now();
        if self.stop.pop().is_ok() {
            self.stopping = true;
        }
        if self.stopping
            && let Some(runner) = self.runner.take()
            && let Err(PushError::Full(runner)) = self.back.push(runner)
        {
            // Never dropped here: kept, and returned on a later callback.
            self.runner = Some(runner);
        }
        let device = self.map.device_channels();
        let sink = self.map.sink_channels();
        let frames = data.len().checked_div(device).unwrap_or(0);
        match self.runner.as_mut() {
            Some(runner) if !self.stopping && sink > 0 && device > 0 => {
                let piece = (self.scratch.len() / sink).max(1);
                for out in data.chunks_mut(piece * device) {
                    let n = out.len() / device;
                    if let Some(src) = self.scratch.get_mut(..n * sink) {
                        runner.process(src, sink);
                        self.map.scatter(src, out);
                    }
                    if let Some(rest) = out.get_mut(n * device..) {
                        rest.fill(0.0);
                    }
                }
            }
            _ => data.fill(0.0),
        }
        self.stats.record(frames, start.elapsed(), self.sample_rate);
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

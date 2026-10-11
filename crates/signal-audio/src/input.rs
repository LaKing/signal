//! The body of an input device's callback, for metering only: per callback
//! and channel, the peak and the sum of squares, sent as [`Meter`]s to the
//! control thread. Input never reaches the graph (that is M4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::Frame;
use signal_engine::Meter;

/// What the input callback holds. [`capture`](InputCallback::capture)
/// allocates, frees, locks and blocks nowhere: the levels are sized here, and
/// a meter that does not fit in the queue is counted and dropped.
pub struct InputCallback {
    channels: usize,
    levels: Vec<(f32, f64)>,
    next_frame: u64,
    meters: Producer<Meter>,
    overflows: Arc<AtomicU64>,
}

/// The control thread's side of an [`InputCallback`]: its meters, and how
/// many were dropped.
pub struct InputMeters {
    channels: usize,
    meters: Consumer<Meter>,
    overflows: Arc<AtomicU64>,
}

impl InputCallback {
    /// A callback for input of `channels` channels, with room for
    /// `capacity` meters between drains. Built on the control thread.
    pub fn new(channels: usize, capacity: usize) -> (InputCallback, InputMeters) {
        let (producer, consumer) = RingBuffer::new(capacity.max(1));
        let overflows = Arc::new(AtomicU64::new(0));
        let callback = InputCallback {
            channels,
            levels: vec![(0.0, 0.0); channels],
            next_frame: 0,
            meters: producer,
            overflows: Arc::clone(&overflows),
        };
        let meters = InputMeters {
            channels,
            meters: consumer,
            overflows,
        };
        (callback, meters)
    }

    /// One device callback: `data` holds frames of the input's channels,
    /// interleaved. Sends one meter per channel (raw samples: an input at or
    /// above full scale reads 1.0 or more).
    pub fn capture(&mut self, data: &[f32]) {
        if self.channels == 0 {
            return;
        }
        let frames = (data.len() / self.channels) as u64;
        if frames == 0 {
            return;
        }
        self.levels.fill((0.0, 0.0));
        for frame in data.chunks_exact(self.channels) {
            for (level, &x) in self.levels.iter_mut().zip(frame) {
                let x = if x.is_finite() { x } else { 0.0 };
                level.0 = level.0.max(x.abs());
                level.1 += f64::from(x) * f64::from(x);
            }
        }
        for (channel, &(peak, sum_squares)) in self.levels.iter().enumerate() {
            let meter = Meter {
                first_frame: Frame(self.next_frame),
                frames,
                channel,
                peak,
                sum_squares,
            };
            if self.meters.push(meter).is_err() {
                self.overflows.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.next_frame = self.next_frame.saturating_add(frames);
    }
}

impl InputMeters {
    /// The input's channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Hands every meter sent so far to `sink`, oldest first.
    pub fn drain(&mut self, mut sink: impl FnMut(&Meter)) {
        while let Ok(meter) = self.meters.pop() {
            sink(&meter);
        }
    }

    /// Meters dropped because the queue was full.
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::InputCallback;

    #[test]
    fn each_channel_is_metered_and_a_full_queue_is_counted() {
        let (mut callback, mut meters) = InputCallback::new(2, 2);
        callback.capture(&[0.5, -0.25, -1.5, 0.0, 0.0, f32::NAN]);
        let mut got = Vec::new();
        meters.drain(|m| got.push(*m));
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].channel, got[0].frames, got[0].first_frame.0),
            (0, 3, 0)
        );
        assert_eq!(got[0].peak, 1.5);
        assert_eq!(got[0].sum_squares, 0.25 + 2.25);
        assert_eq!((got[1].peak, got[1].sum_squares), (0.25, 0.0625));
        // Two more callbacks into a queue of two: the second's are dropped.
        callback.capture(&[0.1, 0.1]);
        callback.capture(&[0.1, 0.1]);
        assert_eq!(meters.overflows(), 2);
        got.clear();
        meters.drain(|m| got.push(*m));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].first_frame.0, 3);
    }
}

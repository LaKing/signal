//! Meter ballistics, computed on the control side from the per-block peak
//! and sum of squares the audio thread sends ([`Meter`], M2-3): the peak in
//! dBFS with a 1.5 s hold, the RMS over 300 ms, and a clip indicator latched
//! until cleared. Time is counted in frames, so the ballistics are the same
//! however the blocks arrive.

use std::collections::VecDeque;

use signal_engine::Meter;

/// How long the held peak stays, in seconds.
pub const PEAK_HOLD_SECONDS: f64 = 1.5;

/// The RMS window, in seconds.
pub const RMS_WINDOW_SECONDS: f64 = 0.3;

/// `20 log10(x)`; −∞ for silence.
pub fn dbfs(x: f64) -> f64 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// What a meter shows, in dBFS (−∞ for silence).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The latest block's peak.
    pub peak: f64,
    /// The highest peak of the last 1.5 s.
    pub held: f64,
    /// The RMS of the last 300 ms.
    pub rms: f64,
    /// Whether a block reached full scale since the latch was last cleared.
    pub clip: bool,
}

/// One channel's ballistics.
#[derive(Clone, Debug)]
pub struct ChannelMeter {
    hold_frames: u64,
    window_frames: u64,
    peak: f32,
    held: f32,
    /// The end frame of the block that set `held`.
    held_at: u64,
    blocks: VecDeque<(u64, f64)>,
    frames: u64,
    sum_squares: f64,
    clip: bool,
}

impl ChannelMeter {
    /// A meter for blocks at `sample_rate`.
    pub fn new(sample_rate: u32) -> ChannelMeter {
        let frames = |seconds: f64| (seconds * f64::from(sample_rate)).round() as u64;
        ChannelMeter {
            hold_frames: frames(PEAK_HOLD_SECONDS),
            window_frames: frames(RMS_WINDOW_SECONDS).max(1),
            peak: 0.0,
            held: 0.0,
            held_at: 0,
            blocks: VecDeque::new(),
            frames: 0,
            sum_squares: 0.0,
            clip: false,
        }
    }

    /// Takes one block. A peak of 1.0 or more latches the clip indicator (a
    /// clamped output sample reads exactly 1.0).
    pub fn push(&mut self, meter: &Meter) {
        let end = meter.first_frame.0.saturating_add(meter.frames);
        self.peak = meter.peak;
        if meter.peak >= self.held || end.saturating_sub(self.held_at) > self.hold_frames {
            self.held = meter.peak;
            self.held_at = end;
        }
        if meter.peak >= 1.0 {
            self.clip = true;
        }
        self.blocks.push_back((meter.frames, meter.sum_squares));
        self.frames += meter.frames;
        self.sum_squares += meter.sum_squares;
        // Keep the newest blocks that cover the window.
        while let Some(&(frames, sum)) = self.blocks.front() {
            if self.frames - frames < self.window_frames {
                break;
            }
            self.blocks.pop_front();
            self.frames -= frames;
            self.sum_squares -= sum;
        }
    }

    /// Clears the clip indicator.
    pub fn clear_clip(&mut self) {
        self.clip = false;
    }

    /// What it shows now.
    pub fn reading(&self) -> Reading {
        let rms = if self.frames > 0 {
            (self.sum_squares.max(0.0) / self.frames as f64).sqrt()
        } else {
            0.0
        };
        Reading {
            peak: dbfs(f64::from(self.peak)),
            held: dbfs(f64::from(self.held)),
            rms: dbfs(rms),
            clip: self.clip,
        }
    }
}

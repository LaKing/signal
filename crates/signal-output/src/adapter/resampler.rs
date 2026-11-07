//! The converter (ADR 0052 §4): source frames kept in a history and read at
//! any fractional position through the [`SincTable`].
//!
//! Positions are 32.32 fixed point in source frames (`u64`: frame index in
//! the high 32 bits, fraction in the low 32), wrapping with the 32-bit frame
//! index every 2³² frames (about 25 hours at 48 kHz); every comparison is a
//! wrapping difference, so a run may last any time.
//!
//! The history is planar and mirrored: each channel holds `capacity`
//! frames twice, frame `n` at `n mod capacity` and again `capacity` later,
//! so the `taps` frames under the filter are always one contiguous slice.

use super::sinc::{FilterSpec, SincTable};

/// One whole source frame in 32.32 fixed point.
pub const ONE: u64 = 1 << 32;

/// A position's frame index (the high 32 bits).
pub fn frame_of(position: u64) -> u32 {
    (position >> 32) as u32
}

/// `a − b` for 32-bit wrapping frame indices, as a signed distance.
pub fn frames_between(a: u32, b: u32) -> i64 {
    i64::from(a.wrapping_sub(b) as i32)
}

/// The converter: a history of source frames and the filter that reads it.
#[derive(Debug)]
pub struct Resampler {
    table: SincTable,
    channels: usize,
    capacity: usize,
    mask: u32,
    /// `channels` planes of `2 × capacity` samples.
    history: Vec<f32>,
    /// The index of the next frame [`Resampler::write`] stores.
    written: u32,
    /// One row of interpolated coefficients.
    row: Vec<f32>,
}

impl Resampler {
    /// A converter for `channels` channels whose history holds at least
    /// `history_frames` frames (rounded up to a power of two, and at least
    /// four filter lengths). Control thread: allocates.
    pub fn new(spec: FilterSpec, channels: usize, history_frames: usize) -> Resampler {
        let table = SincTable::new(spec);
        let taps = table.taps();
        let capacity = history_frames.max(4 * taps).next_power_of_two();
        let channels = channels.max(1);
        Resampler {
            row: vec![0.0; taps],
            table,
            channels,
            capacity,
            mask: (capacity - 1) as u32,
            history: vec![0.0; channels * 2 * capacity],
            written: 0,
        }
    }

    /// The filter.
    pub fn spec(&self) -> &FilterSpec {
        self.table.spec()
    }

    /// The taps.
    pub fn taps(&self) -> usize {
        self.table.taps()
    }

    /// Frames the history holds.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The index of the next frame to be written: every frame before it,
    /// back to `capacity` frames, is in the history.
    pub fn written(&self) -> u32 {
        self.written
    }

    /// Restarts the history at frame `at`, every stored frame silent.
    /// Bounded: `channels × 2 × capacity` stores.
    pub fn reset(&mut self, at: u32) {
        self.history.fill(0.0);
        self.written = at;
    }

    /// Appends interleaved frames (a partial last frame is ignored).
    /// Real-time safe.
    pub fn write(&mut self, interleaved: &[f32]) {
        let whole = interleaved.len() / self.channels * self.channels;
        self.write_split(interleaved.get(..whole).unwrap_or_default(), &[]);
    }

    /// Appends interleaved samples that may arrive in two slices split
    /// anywhere (a ring's two halves); `channels` must divide their total.
    /// One channel at a time, so each plane is written in order: frame by
    /// frame, every frame touches all the planes, which at 64 channels
    /// costs about ten times as much. Real-time safe.
    pub fn write_split(&mut self, first: &[f32], second: &[f32]) {
        let channels = self.channels;
        let capacity = self.capacity;
        let frames = (first.len() + second.len()) / channels;
        let start = self.written;
        let mask = self.mask;
        for (channel, plane) in self.history.chunks_exact_mut(2 * capacity).enumerate() {
            let (low, high) = plane.split_at_mut(capacity);
            for n in 0..frames {
                let i = n * channels + channel;
                let sample = match first.get(i) {
                    Some(&x) => x,
                    None => second.get(i - first.len()).copied().unwrap_or(0.0),
                };
                let at = (start.wrapping_add(n as u32) & mask) as usize;
                if let Some(slot) = low.get_mut(at) {
                    *slot = sample;
                }
                if let Some(slot) = high.get_mut(at) {
                    *slot = sample;
                }
            }
        }
        self.written = start.wrapping_add(frames as u32);
    }

    /// The first frame index after the last one a read at `position` needs
    /// (`position`'s frame plus half the taps, plus 1).
    pub fn needs_until(&self, position: u64) -> u32 {
        frame_of(position)
            .wrapping_add(self.table.spec().half() as u32)
            .wrapping_add(1)
    }

    /// How many reads, starting at `position` and `step` apart, frames
    /// before index `available` serve: the largest `m` with every read up
    /// to `position + (m − 1)·step` needing only frames before `available`.
    pub fn reads_before(&self, position: u64, step: u64, available: u32) -> u64 {
        let limit = available.wrapping_sub(self.table.spec().half() as u32);
        let room = frames_between(limit, frame_of(position)).clamp(-(1 << 30), 1 << 30);
        let room_fixed = (room << 32) - i64::from(position as u32);
        if room_fixed <= 0 || step == 0 {
            return 0;
        }
        (room_fixed as u64).div_ceil(step)
    }

    /// Writes into `out` the mix of the stored frames from index `from`
    /// (`out.len()` of them, at most the capacity): the sum of at most
    /// `most` channels, evenly spaced, so its cost does not grow with the
    /// channel count. A splice's alignment reference. Real-time safe:
    /// `out.len() × min(channels, most)` adds.
    pub fn mix_into(&self, from: u32, most: usize, out: &mut [f32]) {
        out.fill(0.0);
        let len = out.len().min(self.capacity);
        let start = (from & self.mask) as usize;
        let stride = self.channels.div_ceil(most.max(1));
        for plane in self.history.chunks_exact(2 * self.capacity).step_by(stride) {
            let Some(x) = plane.get(start..start + len) else {
                continue;
            };
            for (o, &v) in out.iter_mut().zip(x) {
                *o += v;
            }
        }
    }

    /// Adds `gain` times the interpolated value of every channel at
    /// `position` into `frame` (`channels` samples). The frames under the
    /// filter must be in the history ([`Resampler::needs_until`] ≤
    /// [`Resampler::written`]); others read whatever the history holds.
    /// Real-time safe: `taps` multiply-adds for the coefficients and
    /// `taps × channels` for the channels.
    pub fn read_add(&mut self, position: u64, gain: f32, frame: &mut [f32]) {
        self.table.coefficients(position as u32, &mut self.row);
        let taps = self.row.len();
        let first = frame_of(position).wrapping_sub(self.table.spec().half() as u32 - 1);
        let start = (first & self.mask) as usize;
        for (plane, out) in self
            .history
            .chunks_exact(2 * self.capacity)
            .zip(frame.iter_mut())
        {
            let Some(window) = plane.get(start..start + taps) else {
                continue;
            };
            let mut sum = 0.0f32;
            for (&x, &c) in window.iter().zip(&self.row) {
                sum += x * c;
            }
            *out += gain * sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resampler(channels: usize) -> Resampler {
        Resampler::new(FilterSpec::for_rates(48_000, 48_000), channels, 1024)
    }

    /// At whole positions with a fraction of 0 the filter's centre tap
    /// dominates: a constant reads back as itself (rows sum to 1).
    #[test]
    fn constant_in_constant_out() {
        let mut r = resampler(2);
        let frames: Vec<f32> = (0..400).flat_map(|_| [0.5f32, -0.25]).collect();
        r.write(&frames);
        for position in [200 * ONE, 200 * ONE + (ONE / 3), 250 * ONE + 12345] {
            let mut out = [0.0f32; 2];
            r.read_add(position, 1.0, &mut out);
            assert!((out[0] - 0.5).abs() < 1e-5 && (out[1] + 0.25).abs() < 1e-5);
        }
    }

    /// The split write equals the plain one, wherever the split falls.
    #[test]
    fn split_write_is_plain_write() {
        let samples: Vec<f32> = (0..300).map(|n| n as f32).collect();
        let mut a = resampler(3);
        a.write(&samples);
        let mut b = resampler(3);
        b.write_split(&samples[..127], &samples[127..]);
        assert_eq!(a.history, b.history);
        assert_eq!(a.written(), b.written());
    }

    /// `reads_before` counts exactly the reads whose frames are present.
    #[test]
    fn reads_before_counts_reads() {
        let r = resampler(1);
        let step = ONE + ONE / 1000;
        let start = 10 * ONE + 77;
        for available in [0u32, 10, 59, 60, 100, 500] {
            let m = r.reads_before(start, step, available);
            for j in 0..m {
                let need = r.needs_until(start + j * step);
                assert!(frames_between(need, available) <= 0, "{available}: {j}");
            }
            let next = r.needs_until(start + m * step);
            assert!(frames_between(next, available) > 0, "{available}: {m}");
        }
    }

    /// Positions wrap with the 32-bit frame index.
    #[test]
    fn wrapping_distances() {
        assert_eq!(frames_between(5, u32::MAX - 4), 10);
        assert_eq!(frames_between(u32::MAX - 4, 5), -10);
    }
}

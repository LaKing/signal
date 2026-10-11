//! Executor-applied fades: of `audio` outputs on suspend and resume (ADR 0016
//! §1a) and of every signal output after a fault.
//!
//! A gain is an integer ratio divided in f64 first, then multiplied once by the
//! value; the final frame of a fade is assigned (exactly +0.0, or the live value)
//! rather than computed, so no fade ends near its target.

/// The fault guard's fade length in frames, at any sample rate.
pub(crate) const FAULT_FADE_FRAMES: u64 = 64;

/// The suspension fade length *F*: 5 ms at `sample_rate` frames per second,
/// rounded half up, and at least one frame.
pub(crate) fn fade_frames(sample_rate: u32) -> u64 {
    ((u64::from(sample_rate) * 5 + 500) / 1000).max(1)
}

/// Frame `j` (0 = the first) of a fade-out of `len` frames from `held`:
/// `held · ((len − 1 − j) / len)`, and exactly +0.0 from `j = len − 1` on.
pub(crate) fn fade_out(held: f64, j: u64, len: u64) -> f64 {
    let remaining = len.saturating_sub(j.saturating_add(1));
    if remaining == 0 {
        0.0
    } else {
        held * (remaining as f64 / len as f64)
    }
}

/// Frame `j` (0 = the first) of a fade-in of `len` frames to `live`:
/// `live · ((j + 1) / len)`, and exactly `live` from `j = len − 1` on.
pub(crate) fn fade_in(live: f64, j: u64, len: u64) -> f64 {
    let done = j.saturating_add(1);
    if done >= len {
        live
    } else {
        live * (done as f64 / len as f64)
    }
}

/// The suspension fade of an instance's `audio` outputs. A suspend always starts
/// a new fade-out and a resume a new fade-in from 0, abandoning any fade in
/// progress (§1a: "fade in from 0 to their live value over F frames").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Fade {
    /// Outputs pass unchanged (running) or are 0 (fully faded out).
    #[default]
    None,
    /// Suspended: the next frame is frame `j` of the fade-out.
    Out(u64),
    /// Resumed: the next frame is frame `j` of the fade-in.
    In(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_length_is_5_ms_rounded_half_up() {
        for (rate, frames) in [
            (44_100, 221), // 220.5
            (48_000, 240),
            (96_000, 480),
            (192_000, 960),
            (22_050, 110), // 110.25
            (100, 1),      // 0.5
            (1, 1),        // at least one frame
        ] {
            assert_eq!(fade_frames(rate), frames, "{rate} Hz");
        }
    }

    #[test]
    fn fades_end_exactly_on_their_targets() {
        assert_eq!(fade_out(-0.5, 238, 240), -0.5 * (1.0 / 240.0));
        // Exactly +0.0, not -0.0, even from a negative held value.
        assert_eq!(fade_out(-0.5, 239, 240).to_bits(), 0.0f64.to_bits());
        assert_eq!(fade_out(-0.5, 1000, 240).to_bits(), 0.0f64.to_bits());
        let live = 0.1 + 0.2; // not a round number
        assert_eq!(fade_in(live, 239, 240).to_bits(), live.to_bits());
        assert_eq!(fade_in(live, 0, 1).to_bits(), live.to_bits());
        assert_eq!(fade_out(live, 0, 1).to_bits(), 0.0f64.to_bits());
    }

    /// The plan's worked trace at F = 240 (48 kHz): a one-frame suspension, then
    /// a full fade-in from 0. Division first, then one multiply.
    #[test]
    fn one_frame_suspension_trace_at_f_240() {
        let f = fade_frames(48_000);
        assert_eq!(fade_out(0.75, 0, f), 0.746875);
        assert_eq!(fade_in(0.75, 0, f), 0.003125);
        assert_eq!(fade_in(0.75, 1, f), 0.00625);
        assert_eq!(fade_in(0.75, 2, f), 0.009375000000000001);
        assert_eq!(fade_in(0.75, 238, f), 0.746875);
        assert_eq!(fade_in(0.75, 239, f), 0.75);
        // An interrupted fade-in held at j = 99 (gain 100/240) is what a new
        // fade-out starts from.
        assert_eq!(fade_in(0.75, 99, f), 0.3125);
        assert_eq!(fade_out(0.75, 99, f), 0.4375);
    }
}

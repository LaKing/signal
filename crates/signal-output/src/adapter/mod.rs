//! The clock adapter (ADR 0052 §2–4, phase 1: offline, no device): carries
//! a stream of frames from one clock to another. It has **no caller yet**:
//! phase 2 (M4.1b) connects it to secondary devices' streams. It stays as
//! M4.1b's first phase on the roadmap (the operator's decision against
//! hard rule 8, 2026-11-01), tested offline by its own tests.
//!
//! - **Ring.** [`AdapterInput::push`] puts frames into a lock-free `rtrb`
//!   ring at the source's rate; a full ring drops and counts them, never
//!   blocks. Each push also leaves a **mark**: the frames pushed so far and
//!   the push's host time, if the caller has one.
//! - **Converter.** [`AdapterOutput::pull`] makes exactly the frames its
//!   caller must deliver, reading the ring through a polyphase
//!   Kaiser-windowed sinc ([`sinc`], [`Resampler`]) at a 32.32 fixed-point
//!   position that advances by nominal × (1 + correction) source frames per
//!   output frame. It runs on the side whose frame count demand fixes: the
//!   secondary device's callback for an output, the clock callback for an
//!   input (ADR 0052 §2).
//! - **Drift loop.** Once per pull, the fill (ring plus the converter's
//!   unread history, in source frames) is compared with the target F* and a
//!   PI loop ([`DriftLoop`]) sets the correction. With host times on both
//!   sides the fill's producer staircase is linearised from the last mark
//!   (below); without them the raw fill is used, as ADR 0052 §2 wrote it.
//! - **Splices.** Startup waits for F* in the ring, then fades in over
//!   10 ms from where the signal starts. A pull that would leave fewer
//!   than J of frames beyond it jumps back into the history by F* − fill
//!   (a repeat); a fill beyond 3 × F* jumps forward to F* (a skip); both
//!   are counted in `slips`. Each jump is aligned: up to 5 ms further back
//!   (or shorter ahead), to where the material best correlates with what
//!   the old head still plays (WSOLA's rule, phase 1's addition), and
//!   crossfaded over 5 ms or the frames the old head has left (at least
//!   J): gains summing to 1 when the material correlates, equal power
//!   otherwise. A producer that stopped (empty ring, or no push for
//!   [`STOP_PERIODS`] periods by the host times) gets the same aligned
//!   jump back, far enough for a 5 ms fade-out from the history, then
//!   silence until the ring holds F* again.
//!
//! **The linearised fill** (phase 1's change to ADR 0052 §2, measured in
//! `docs/benchmarks.md` "Clock adapter"). The raw fill sampled at the
//! consumer's callbacks carries the producer's staircase: up to one
//! producer period, depending on where the two callbacks fall. With
//! jittered callbacks that is noise the low-pass cannot remove inside the
//! track loop's band; with steady ones whose periods beat, a slow sawtooth
//! the loop follows. With the host time of each push (its callback's time,
//! cpal's `StreamInstant`) and of the pull, the fill is estimated as
//! `pushed at the last mark + rate × (pull time − mark time) − push / 2 −
//! read position`: the producer as if it delivered continuously, centred on
//! its staircase. A mark further than [`MARK_REACH`] periods from the pull,
//! or a pull or push without a time, falls back to the raw fill.
//!
//! **Real time** (ADR 0052 §4): every buffer (ring, marks, history,
//! tables, crossfade gains) is allocated by [`clock_adapter`] on the
//! control thread; [`AdapterInput::push`] and [`AdapterOutput::pull`]
//! allocate, lock and block nowhere, and their work is bounded:
//! `push` copies at most the ring's free space; `pull` makes at most
//! `max_pull` frames per piece (a longer buffer runs in pieces), each frame
//! from at most two read heads (one, but two during a crossfade), each head
//! `taps` multiply-adds for its coefficients plus `taps × channels` for the
//! channels; a splice's alignment a mix of at most [`ALIGN_CHANNELS`]
//! searched within [`ALIGN_COARSE_BUDGET`]; a restart clears the history once
//! (`channels × 2 × capacity` stores).
//! [`AdapterOutput::worst_case_multiply_adds`] gives the bound.

mod drift;
mod resampler;
pub mod sinc;

pub use drift::{DriftLoop, LoopMode, LoopParams, PPM};
pub use resampler::{ONE, Resampler, frame_of, frames_between};
pub use sinc::{FilterSpec, MAX_TAPS, PHASES, SincTable};

use std::f64::consts::FRAC_PI_2;

use crate::fade::{Fade, fade_frames};

/// The splice crossfade: 5 ms (ADR 0052 §3).
pub const SPLICE_SECONDS: f64 = 0.005;

/// The jitter margin J to start with: 1 ms (ADR 0052 §3).
pub const JITTER_SECONDS: f64 = 0.001;

/// Each slip widens J by this, up to [`MAX_JITTER_SECONDS`] (ADR 0052,
/// answered question 1).
pub const JITTER_STEP_SECONDS: f64 = 0.001;

/// The widest J: 3 ms.
pub const MAX_JITTER_SECONDS: f64 = 0.003;

/// A fill above this many targets is an overrun (ADR 0052 §3).
pub const OVERRUN_TARGETS: f64 = 3.0;

/// The converter keeps at least this much history (ADR 0052 §3).
pub const HISTORY_FRAMES: usize = 1024;

/// A mark is used for the linearised fill while its time is within this
/// many of the longer period of the pull's.
pub const MARK_REACH: f64 = 8.0;

/// An aligned jump whose material correlates at least this much is
/// crossfaded with gains summing to 1 (equal power otherwise).
pub const MATCHED: f64 = 0.5;

/// A producer silent this many source periods (plus J) has stopped.
pub const STOP_PERIODS: f64 = 2.0;

/// The most channels an adapter carries.
pub const MAX_CHANNELS: usize = 64;

/// A pull whose host time is more than this many destination periods past
/// the frames the previous pulls delivered is a clock that jumped (ADR
/// 0052 §6): a resync.
pub const JUMP_PERIODS: f64 = 2.0;

/// A tracking loop whose filtered error stays beyond half the target this
/// long resyncs (ADR 0052 §6).
pub const FAR_SECONDS: f64 = 0.5;

/// A correction held at its clamp this long resyncs (ADR 0052 §6: the
/// clamp means the clock is beyond the loop's reach), and again each time
/// it has been held this long since.
pub const CLAMP_RESYNC_SECONDS: f64 = 1.0;

/// At the clamp this long in all since the loop last locked, the device is
/// **clock suspect** (ADR 0052 §6): faded out until it locks again.
pub const SUSPECT_SECONDS: f64 = 10.0;

/// This many slips within [`SUSPECT_SECONDS`] also make the device clock
/// suspect (phase 2's addition to ADR 0052 §6): each splice moves the
/// loop's error back to F*, so a clock beyond the loop's reach touches the
/// clamp only now and then, slipping every few seconds instead. A device
/// within reach never slips once locked, nor while it locks.
pub const SUSPECT_SLIPS: usize = 3;

/// The most channels a splice's alignment mixes into its reference: the
/// search's cost does not grow with the channel count.
pub const ALIGN_CHANNELS: usize = 8;

/// The multiply-adds a splice's coarse search may spend: it tries every
/// `s`-th lag on every `s`-th frame, `s` the least that keeps it within,
/// then every lag within `s` of the best on every frame. So the search is
/// bounded whatever the rate (its lags and frames grow with it).
pub const ALIGN_COARSE_BUDGET: usize = 8_192;

/// What a clock adapter carries and between which clocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdapterConfig {
    /// Interleaved channels per frame, 1 to [`MAX_CHANNELS`].
    pub channels: usize,
    /// The pushing side's nominal rate, frames per second.
    pub source_rate: u32,
    /// The pulling side's nominal rate.
    pub dest_rate: u32,
    /// The pushing side's period (frames per push, the largest expected).
    pub source_period: usize,
    /// The pulling side's period.
    pub dest_period: usize,
    /// The most frames one pull piece makes; longer pulls run in pieces
    /// (the device's largest buffer, checked when the route is made).
    pub max_pull: usize,
    /// The jitter margin J to start with, seconds.
    pub jitter_seconds: f64,
    /// The drift loop's constants.
    pub loop_params: LoopParams,
}

impl AdapterConfig {
    /// A configuration with ADR 0052's defaults: J = 1 ms, the default
    /// loop, pieces up to four destination periods.
    pub fn new(
        channels: usize,
        source_rate: u32,
        dest_rate: u32,
        source_period: usize,
        dest_period: usize,
    ) -> AdapterConfig {
        AdapterConfig {
            channels,
            source_rate,
            dest_rate,
            source_period,
            dest_period,
            max_pull: 4 * dest_period.max(1),
            jitter_seconds: JITTER_SECONDS,
            loop_params: LoopParams::default(),
        }
    }

    /// The nominal step: source frames per output frame.
    pub fn nominal_step(&self) -> f64 {
        f64::from(self.source_rate) / f64::from(self.dest_rate)
    }

    /// The target fill F* in source frames for a jitter margin of `jitter`
    /// seconds: source period + destination period (in source frames) +
    /// J + the converter's half length (ADR 0052 §3).
    pub fn target_for(&self, jitter: f64) -> f64 {
        let spec = FilterSpec::for_rates(self.source_rate, self.dest_rate);
        self.source_period as f64
            + self.dest_period as f64 * self.nominal_step()
            + jitter * f64::from(self.source_rate)
            + spec.half() as f64
    }
}

/// Why an adapter could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterError {
    /// Channels outside 1 to [`MAX_CHANNELS`].
    Channels,
    /// A rate, period or piece of 0, or a ratio beyond 8.
    Shape,
    /// A jitter margin or loop constant negative or not finite.
    Loop,
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdapterError::Channels => {
                write!(f, "a clock adapter carries 1 to {MAX_CHANNELS} channels")
            }
            AdapterError::Loop => write!(
                f,
                "a clock adapter's jitter margin and loop constants must be finite and non-negative"
            ),
            AdapterError::Shape => write!(
                f,
                "a clock adapter needs nonzero rates and periods, at a ratio of at most 8"
            ),
        }
    }
}

impl std::error::Error for AdapterError {}

/// A push's record for the linearised fill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mark {
    /// Frames pushed so far, this push included (wrapping).
    pushed: u32,
    /// This push's frames.
    frames: u32,
    /// The push's host time, nanoseconds; `None` without one.
    time_ns: Option<u64>,
}

/// The pushing end of a clock adapter: the engine side of an output, the
/// device side of an input.
#[derive(Debug)]
pub struct AdapterInput {
    ring: rtrb::Producer<f32>,
    marks: rtrb::Producer<Mark>,
    channels: usize,
    pushed: u32,
    dropped_frames: u64,
    dropped_marks: u64,
}

impl AdapterInput {
    /// Pushes interleaved frames (a partial last frame is ignored), stamped
    /// with the push's host time in nanoseconds if there is one. What does
    /// not fit is dropped and counted; returns the frames taken.
    /// Real-time safe: copies, never blocks.
    pub fn push(&mut self, interleaved: &[f32], time_ns: Option<u64>) -> usize {
        let channels = self.channels;
        let frames = interleaved.len() / channels;
        let room = self.ring.slots() / channels;
        let taken = frames.min(room);
        let samples = interleaved.get(..taken * channels).unwrap_or(&[]);
        if self.ring.push_entire_slice(samples).is_err() {
            self.dropped_frames += frames as u64;
            return 0;
        }
        self.dropped_frames += (frames - taken) as u64;
        self.pushed = self.pushed.wrapping_add(taken as u32);
        let mark = Mark {
            pushed: self.pushed,
            frames: taken as u32,
            time_ns,
        };
        if taken > 0 && self.marks.push(mark).is_err() {
            self.dropped_marks += 1;
        }
        taken
    }

    /// Frames dropped because the ring was full.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Marks dropped because their queue was full (the fill falls back to
    /// the older mark).
    pub fn dropped_marks(&self) -> u64 {
        self.dropped_marks
    }
}

/// What the pulling end is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterState {
    /// Silence until the ring holds F*.
    Filling,
    /// Converting, the loop in acquire.
    Locking,
    /// Converting, the loop in track.
    Locked,
    /// Converting but faded out: the correction sat at its clamp for
    /// [`SUSPECT_SECONDS`] since the loop last locked (ADR 0052 §6). It
    /// fades in again when the loop locks.
    Suspect,
}

/// The pulling end's counters and loop state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdapterStatus {
    /// Filling, locking or locked.
    pub state: AdapterState,
    /// The correction in use, ppm.
    pub correction_ppm: f64,
    /// The low-passed fill error, source frames.
    pub fill_error: f64,
    /// The last fill estimate, source frames.
    pub fill: f64,
    /// The target F*, source frames.
    pub target: f64,
    /// The jitter margin J in use, seconds.
    pub jitter_seconds: f64,
    /// Repeats and skips (ADR 0052 §3's `slips`).
    pub slips: u64,
    /// Of those, repeats (underruns).
    pub repeats: u64,
    /// Of those, skips (overruns).
    pub skips: u64,
    /// Fades to silence on an empty ring.
    pub stops: u64,
    /// Starts after a fill (the first included).
    pub starts: u64,
    /// Pulls that used the linearised fill.
    pub linearised: u64,
    /// Pulls that used the raw fill (while converting).
    pub raw: u64,
    /// Resyncs (ADR 0052 §6): a splice to F* and the loop back in acquire
    /// after a clock jump, a tracking error beyond F*/2 or a correction at
    /// its clamp. Not counted in `slips`.
    pub resyncs: u64,
    /// Times the adapter became clock suspect.
    pub suspects: u64,
}

/// A crossfade in progress: the old head fading out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Crossfade {
    /// The old head's position.
    old: u64,
    /// Frames done.
    at: usize,
    /// Frames in all.
    len: usize,
    /// Reads the old head still makes forward; after them it reads
    /// backwards (reflected about its last read), so it fades over the
    /// whole 5 ms however few frames were left ahead of it.
    forward: usize,
    /// The heads' material correlates: gains summing to 1, not their
    /// squares.
    matched: bool,
}

/// The pulling end of a clock adapter, where the converter runs.
#[derive(Debug)]
pub struct AdapterOutput {
    config: AdapterConfig,
    ring: rtrb::Consumer<f32>,
    marks: rtrb::Consumer<Mark>,
    last_mark: Option<Mark>,
    converter: Resampler,
    drift: DriftLoop,
    state: AdapterState,
    /// The read position, 32.32 source frames.
    position: u64,
    /// The step in use, 32.32.
    step: u64,
    nominal_step: f64,
    jitter: f64,
    target: f64,
    fill: f64,
    crossfade: Option<Crossfade>,
    /// cos(π/2 · x) over the crossfade, `splice_frames + 1` values.
    fall: Vec<f32>,
    splice_frames: usize,
    /// A splice's alignment: the old head's mix (`splice_frames`) and the
    /// candidates' (`2 × splice_frames + 1`).
    align_a: Vec<f32>,
    align_b: Vec<f32>,
    /// The furthest a repeat may jump back: the history the heads of a
    /// piece leave.
    most_back: f64,
    fade: Fade,
    /// Output frames of silence before the fade-in starts.
    fade_wait: usize,
    /// Fading out after the producer stopped.
    stopping: bool,
    /// One frame's accumulator.
    frame: Vec<f32>,
    /// Seconds per destination frame.
    frame_seconds: f64,
    /// The longer period, seconds (the mark reach's unit).
    period_seconds: f64,
    slips: u64,
    repeats: u64,
    skips: u64,
    stops: u64,
    starts: u64,
    linearised: u64,
    raw: u64,
    /// The previous pull's host time and frames (the clock-jump check).
    last_pull: Option<(u64, usize)>,
    /// Seconds the tracking error has been beyond F*/2.
    far_for: f64,
    /// Seconds the correction has been at its clamp without a break.
    clamped_for: f64,
    /// Seconds at the clamp in all since the loop last locked.
    clamped_total: f64,
    /// Faded out as clock suspect.
    suspect: bool,
    /// Seconds of pulls so far (the slips' clock).
    clock: f64,
    /// The latest slips' times on that clock, newest last.
    slip_times: [f64; SUSPECT_SLIPS],
    resyncs: u64,
    suspects: u64,
}

/// Makes a clock adapter's two ends (control thread: allocates the ring,
/// the marks, the converter's table and history and the crossfade gains).
pub fn clock_adapter(config: AdapterConfig) -> Result<(AdapterInput, AdapterOutput), AdapterError> {
    if config.channels == 0 || config.channels > MAX_CHANNELS {
        return Err(AdapterError::Channels);
    }
    let step = config.nominal_step();
    if config.source_rate == 0
        || config.dest_rate == 0
        || config.source_period == 0
        || config.dest_period == 0
        || config.max_pull == 0
        || !(1.0 / 8.0..=8.0).contains(&step)
    {
        return Err(AdapterError::Shape);
    }
    // Every constant the callbacks clamp with must be finite and
    // non-negative (`f64::clamp` panics on NaN or reversed bounds).
    let p = &config.loop_params;
    let constants = [
        config.jitter_seconds,
        p.tau,
        p.acquire_tau,
        p.zeta,
        p.acquire_hz,
        p.track_hz,
        p.lock_frames,
        p.lock_seconds,
        p.lock_rate,
        p.clamp,
        p.acquire_slew,
        p.track_slew,
    ];
    if constants.iter().any(|c| !c.is_finite() || *c < 0.0) {
        return Err(AdapterError::Loop);
    }
    let channels = config.channels;
    let spec = FilterSpec::for_rates(config.source_rate, config.dest_rate);
    let jitter = config.jitter_seconds.clamp(0.0, MAX_JITTER_SECONDS);
    let widest = config.target_for(MAX_JITTER_SECONDS.max(jitter));
    let ring_frames = ((4.0 * widest).ceil() as usize).next_power_of_two();
    let splice_frames = ((SPLICE_SECONDS * f64::from(config.dest_rate)).round() as usize).max(1);
    // Most a pull piece's heads reach: the piece and a crossfade, at the
    // clamped step, plus a skip over the whole ring.
    let most_step = step * (1.0 + config.loop_params.clamp) + 1.0;
    let reach = ((config.max_pull + splice_frames) as f64 * most_step).ceil() as usize;
    let history = (ring_frames + reach + 2 * spec.taps).max(HISTORY_FRAMES + reach + 2 * spec.taps);
    let converter = Resampler::new(spec, channels, history);
    let most_back = converter.capacity().saturating_sub(reach + 2 * spec.taps) as f64;
    let (ring_in, ring_out) = rtrb::RingBuffer::new(ring_frames * channels);
    let marks = (ring_frames / config.source_period.max(1)).clamp(16, 4096);
    let (marks_in, marks_out) = rtrb::RingBuffer::new(marks);
    let fall = (0..=splice_frames)
        .map(|n| (FRAC_PI_2 * n as f64 / splice_frames as f64).cos() as f32)
        .collect();
    let target = config.target_for(jitter);
    let period_seconds = (config.source_period as f64 / f64::from(config.source_rate))
        .max(config.dest_period as f64 / f64::from(config.dest_rate));
    let input = AdapterInput {
        ring: ring_in,
        marks: marks_in,
        channels,
        pushed: 0,
        dropped_frames: 0,
        dropped_marks: 0,
    };
    let output = AdapterOutput {
        config,
        ring: ring_out,
        marks: marks_out,
        last_mark: None,
        converter,
        drift: DriftLoop::new(config.loop_params, f64::from(config.source_rate)),
        state: AdapterState::Filling,
        position: 0,
        step: to_fixed(step),
        nominal_step: step,
        jitter,
        target,
        fill: 0.0,
        crossfade: None,
        fall,
        splice_frames,
        align_a: vec![0.0; splice_frames],
        align_b: vec![0.0; 2 * splice_frames + 1],
        most_back,
        fade: Fade::default(),
        fade_wait: 0,
        stopping: false,
        frame: vec![0.0; channels],
        frame_seconds: 1.0 / f64::from(config.dest_rate),
        period_seconds,
        slips: 0,
        repeats: 0,
        skips: 0,
        stops: 0,
        starts: 0,
        linearised: 0,
        raw: 0,
        last_pull: None,
        far_for: 0.0,
        clamped_for: 0.0,
        clamped_total: 0.0,
        suspect: false,
        clock: 0.0,
        slip_times: [f64::NEG_INFINITY; SUSPECT_SLIPS],
        resyncs: 0,
        suspects: 0,
    };
    Ok((input, output))
}

/// The coarse search's stride for `len` frames and `search + 1` lags: the
/// least `s` with `(search / s + 1) × ⌈len / s⌉` within
/// [`ALIGN_COARSE_BUDGET`].
fn coarse_step(len: usize, search: usize) -> usize {
    let mut s = 1;
    while (search / s + 1) * len.div_ceil(s) > ALIGN_COARSE_BUDGET {
        s += 1;
    }
    s
}

/// The normalised correlation of `x` and `y` on every `stride`-th frame:
/// Σ x·y / √(Σx²·Σy²), 0 when either is silent.
fn normalised(x: &[f32], y: &[f32], stride: usize) -> f64 {
    let (mut xy, mut xx, mut yy) = (0.0f32, 0.0f32, 0.0f32);
    for (&p, &q) in x.iter().step_by(stride).zip(y.iter().step_by(stride)) {
        xy += p * q;
        xx += p * p;
        yy += q * q;
    }
    let norm = f64::from(xx) * f64::from(yy);
    if norm > 0.0 {
        f64::from(xy) / norm.sqrt()
    } else {
        0.0
    }
}

/// A step in 32.32 fixed point.
fn to_fixed(step: f64) -> u64 {
    (step * ONE as f64).round().max(1.0) as u64
}

impl AdapterOutput {
    /// Fills `out` (interleaved, `channels` per frame) with exactly its
    /// frames, converted from the ring; `time_ns` is this callback's host
    /// time, if there is one. Real-time safe (module docs).
    pub fn pull(&mut self, out: &mut [f32], time_ns: Option<u64>) {
        let channels = self.config.channels;
        out.fill(0.0);
        let piece = self.config.max_pull * channels;
        let mut offset_frames = 0u64;
        for chunk in out.chunks_mut(piece) {
            let time = time_ns.map(|t| {
                t.saturating_add((offset_frames as f64 * self.frame_seconds * 1e9) as u64)
            });
            let frames = chunk.len() / channels;
            if let Some(whole) = chunk.get_mut(..frames * channels) {
                self.pull_piece(whole, frames, time);
            }
            offset_frames += frames as u64;
        }
    }

    /// The configuration.
    pub fn config(&self) -> &AdapterConfig {
        &self.config
    }

    /// The converter's filter.
    pub fn filter(&self) -> &FilterSpec {
        self.converter.spec()
    }

    /// The latency this adapter adds, in source frames, from a frame's time
    /// on the source clock (where it falls in its push's period) to its
    /// read: F* − source period / 2. A frame waits F* on average from its
    /// push to its read, and is pushed on average half a period after its
    /// own time. F* includes the converter's half length, its group delay;
    /// the device's own latency comes on top (ADR 0052 §3). Verified by the
    /// marker delays of `tests/adapter_clocks.rs`.
    pub fn latency_frames(&self) -> f64 {
        self.target - self.config.source_period as f64 / 2.0
    }

    /// The most multiply-adds one pull piece can take: two heads (a
    /// crossfade) over `max_pull` frames, each `taps × (1 + channels)`,
    /// plus a splice's alignment ([`align_multiply_adds`](AdapterOutput::align_multiply_adds)).
    pub fn worst_case_multiply_adds(&self) -> usize {
        let channels = self.config.channels;
        2 * self.config.max_pull * self.converter.taps() * (1 + channels)
            + self.align_multiply_adds()
    }

    /// The most a splice's alignment takes: the mixes (`3 × splice` frames
    /// of at most [`ALIGN_CHANNELS`]), the coarse search
    /// ([`ALIGN_COARSE_BUDGET`], counted as three per product) and the
    /// fine one (`2s − 1` lags of `3 × splice`).
    pub fn align_multiply_adds(&self) -> usize {
        let splice = self.splice_frames;
        let s = coarse_step(splice, splice);
        3 * splice * self.config.channels.min(ALIGN_CHANNELS)
            + 3 * ALIGN_COARSE_BUDGET
            + 3 * (2 * s - 1) * splice
            + 3 * splice
    }

    /// The counters and the loop's state.
    pub fn status(&self) -> AdapterStatus {
        AdapterStatus {
            state: self.state,
            correction_ppm: self.drift.correction() / PPM,
            fill_error: self.drift.filtered_error(),
            fill: self.fill,
            target: self.target,
            jitter_seconds: self.jitter,
            slips: self.slips,
            repeats: self.repeats,
            skips: self.skips,
            stops: self.stops,
            starts: self.starts,
            linearised: self.linearised,
            raw: self.raw,
            resyncs: self.resyncs,
            suspects: self.suspects,
        }
    }

    /// The source frame the read position is at, with its fraction
    /// (tests: where an output frame came from).
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Takes every waiting mark, keeping the latest.
    fn drain_marks(&mut self) {
        while let Ok(mark) = self.marks.pop() {
            self.last_mark = Some(mark);
        }
    }

    /// Frames waiting in the ring.
    fn ring_frames(&self) -> usize {
        self.ring.slots() / self.config.channels
    }

    /// The index one past the last frame available (history and ring).
    fn available(&self) -> u32 {
        self.converter
            .written()
            .wrapping_add(self.ring_frames() as u32)
    }

    /// Frames from the read position to the end of what is available.
    fn raw_fill(&self) -> f64 {
        let whole = frames_between(self.available(), frame_of(self.position));
        whole as f64 - (self.position as u32) as f64 / ONE as f64
    }

    /// The linearised fill at `time_ns`, if a mark and the time allow it.
    fn linear_fill(&self, time_ns: Option<u64>) -> Option<f64> {
        let mark = self.last_mark?;
        let now = i128::from(time_ns?);
        let then = i128::from(mark.time_ns?);
        let since = (now - then) as f64 * 1e-9;
        if since.abs() > MARK_REACH * self.period_seconds {
            return None;
        }
        let pushed = frames_between(mark.pushed, frame_of(self.position)) as f64
            - (self.position as u32) as f64 / ONE as f64;
        Some(pushed + f64::from(self.config.source_rate) * since - f64::from(mark.frames) / 2.0)
    }

    /// Moves ring frames into the history up to index `until` (or as many
    /// as there are).
    fn take_until(&mut self, until: u32) {
        let wanted = frames_between(until, self.converter.written());
        if wanted <= 0 {
            return;
        }
        let frames = (wanted as usize).min(self.ring_frames());
        let samples = frames * self.config.channels;
        if let Ok(chunk) = self.ring.read_chunk(samples) {
            let (first, second) = chunk.as_slices();
            self.converter.write_split(first, second);
            chunk.commit_all();
        }
    }

    /// Discards up to `frames` ring frames; returns how many went.
    fn discard(&mut self, frames: usize) -> usize {
        let frames = frames.min(self.ring_frames());
        match self.ring.read_chunk(frames * self.config.channels) {
            Ok(chunk) => {
                chunk.commit_all();
                frames
            }
            Err(_) => 0,
        }
    }

    /// Starts converting once the ring holds F*: drops what is beyond F*,
    /// clears the history, puts the read position at the first ring frame,
    /// the loop in acquire at the nominal ratio, and fades in. With a
    /// linearised fill, the read position then moves so that fill is F*
    /// (the raw fill at one instant is up to a period off the mean), as far
    /// as this pull of `frames` still has its frames.
    fn try_start(&mut self, frames: usize, time_ns: Option<u64>) -> bool {
        let need = self.target.ceil() as usize;
        let have = self.ring_frames();
        if have < need {
            return false;
        }
        // The dropped frames keep their indices: the position counts every
        // frame pushed, as the marks do.
        let dropped = self.discard(have - need);
        let first = self.converter.written().wrapping_add(dropped as u32);
        self.converter.reset(first);
        self.position = u64::from(first) << 32;
        if let Some(fill) = self.linear_fill(time_ns) {
            let half = self.converter.spec().half() as f64;
            let most = need as f64 - frames as f64 * self.nominal_step - half - 2.0;
            // Back into the cleared history (silence) as far as two
            // targets; the history holds more than four.
            let ahead = (fill - self.target).clamp(-2.0 * self.target, most.max(0.0));
            let fixed = (ahead.abs() * ONE as f64).round() as u64;
            self.position = if ahead >= 0.0 {
                self.position.wrapping_add(fixed)
            } else {
                self.position.wrapping_sub(fixed)
            };
        }
        self.step = to_fixed(self.nominal_step);
        let fill = self.linear_fill(time_ns).unwrap_or_else(|| self.raw_fill());
        self.drift.restart(fill - self.target, false);
        self.crossfade = None;
        // The fade-in starts where the signal does: a read position moved
        // back into the cleared history reads silence until it reaches the
        // first frame (the filter's ringing ahead of it stays faded out).
        let before = frames_between(first, frame_of(self.position)) as f64;
        self.fade_wait = (before / self.nominal_step).floor().max(0.0) as usize;
        if self.fade_wait > 0 || self.suspect {
            self.fade.start_out(0);
        } else {
            self.fade.start_in(fade_frames(self.config.dest_rate));
        }
        self.state = AdapterState::Locking;
        self.starts += 1;
        true
    }

    /// A crossfade from an old head at `old`: 5 ms, or as long as the
    /// available frames serve the old head (a splice starts while at least
    /// J of them are left beyond the pull, so at least that). If not even
    /// `old` is served, the head first moves back to the last position that
    /// is (at most half the taps plus 2 frames), and past its last served
    /// read it reads backwards. `matched`: the heads' material correlates
    /// (an aligned repeat), so the gains sum to 1 rather than their
    /// squares.
    fn crossfade_from(&self, old: u64, matched: bool) -> Crossfade {
        let available = self.available();
        let mut old = old;
        let mut serves = self.converter.reads_before(old, self.step, available);
        let mut tries = self.converter.spec().half() + 2;
        while serves == 0 && tries > 0 {
            old = old.wrapping_sub(ONE);
            serves = self.converter.reads_before(old, self.step, available);
            tries -= 1;
        }
        let len = (serves as usize).clamp(1, self.splice_frames);
        Crossfade {
            old,
            at: 0,
            len,
            forward: len,
            matched,
        }
    }

    /// A jump back, aligned: from `back` frames to up to 5 ms further, the
    /// one whose frames best match (normalised correlation) the frames the
    /// old head still plays over the crossfade, so a short crossfade joins
    /// like material (WSOLA's rule). Returns the jump and its correlation.
    /// Bounded by [`best_lag`](AdapterOutput::best_lag), `len` at most
    /// 5 ms.
    fn align_back(&mut self, old: u64, back: f64, serves: u64) -> (f64, f64) {
        let len = (serves as usize).min(self.splice_frames);
        let room = (self.most_back - back).max(0.0) as usize;
        let search = self.splice_frames.min(room);
        if len < 2 || search == 0 {
            return (back, 0.0);
        }
        let a = frame_of(old);
        // The old head's frames over the crossfade, into the history.
        self.take_until(
            a.wrapping_add(len as u32)
                .wrapping_add(self.converter.spec().half() as u32 + 1),
        );
        let len = len.min(frames_between(self.converter.written(), a).max(0) as usize);
        let base = back as u32;
        let best = self.best_lag(a, a.wrapping_sub(base), search, len);
        (back + best.0 as f64, best.1)
    }

    /// A skip's jump, aligned the same way: from `ahead` frames to up to
    /// 5 ms shorter (never below 0). Returns the jump and its correlation.
    fn align_ahead(&mut self, old: u64, ahead: f64, serves: u64) -> (f64, f64) {
        let len = (serves as usize).min(self.splice_frames);
        let search = self.splice_frames.min(ahead as usize);
        if len < 2 || search == 0 {
            return (ahead, 0.0);
        }
        let a = frame_of(old);
        let jump = ahead as u32;
        self.take_until(
            a.wrapping_add(jump)
                .wrapping_add(len as u32)
                .wrapping_add(self.converter.spec().half() as u32 + 1),
        );
        let len =
            len.min(frames_between(self.converter.written(), a.wrapping_add(jump)).max(0) as usize);
        let best = self.best_lag(a, a.wrapping_add(jump), search, len);
        (ahead - best.0 as f64, best.1)
    }

    /// Of the candidates `top − d` for `d` in `0..=search`, the one whose
    /// `len` frames best match (normalised correlation) the `len` frames
    /// from `a`, on a mix of at most [`ALIGN_CHANNELS`] channels, coarse
    /// then fine ([`ALIGN_COARSE_BUDGET`]): `(d, correlation)`. Bounded by
    /// [`align_multiply_adds`](AdapterOutput::align_multiply_adds), whatever
    /// the channels and the rate.
    fn best_lag(&mut self, a: u32, top: u32, search: usize, len: usize) -> (usize, f64) {
        let len = len.min(self.align_a.len());
        let search = search.min(self.align_b.len().saturating_sub(len + 1));
        if len == 0 {
            return (0, 0.0);
        }
        let (Some(x), Some(y)) = (
            self.align_a.get_mut(..len),
            self.align_b.get_mut(..search + len),
        ) else {
            return (0, 0.0);
        };
        self.converter.mix_into(a, ALIGN_CHANNELS, x);
        self.converter
            .mix_into(top.wrapping_sub(search as u32), ALIGN_CHANNELS, y);
        let (x, y) = (&*x, &*y);
        // Candidate d is y from search − d.
        let score = |d: usize, stride: usize| {
            y.get(search - d..search - d + len)
                .map_or(0.0, |y| normalised(x, y, stride))
        };
        let s = coarse_step(len, search);
        let mut best = (0usize, f64::MIN);
        for d in (0..=search).step_by(s) {
            let c = score(d, s);
            if c > best.1 {
                best = (d, c);
            }
        }
        let (lo, hi) = (best.0.saturating_sub(s - 1), (best.0 + s - 1).min(search));
        let mut fine = (best.0, f64::MIN);
        for d in lo..=hi {
            let c = score(d, 1);
            if c > fine.1 {
                fine = (d, c);
            }
        }
        fine
    }

    /// Whether the producer has missed [`STOP_PERIODS`] source periods
    /// (plus J) since its last push, by the host times: it stopped, so a
    /// pull short of frames fades out rather than repeating.
    fn producer_overdue(&self, time_ns: Option<u64>) -> bool {
        let (Some(now), Some(mark)) = (time_ns, self.last_mark) else {
            return false;
        };
        let Some(then) = mark.time_ns else {
            return false;
        };
        let since = (i128::from(now) - i128::from(then)) as f64 * 1e-9;
        let period = self.config.source_period as f64 / f64::from(self.config.source_rate);
        since > STOP_PERIODS * period + self.jitter
    }

    /// Widens J by a step (up to 3 ms) after a slip; F* follows.
    fn widen(&mut self) {
        let wider = (self.jitter + JITTER_STEP_SECONDS).min(MAX_JITTER_SECONDS);
        if wider > self.jitter {
            self.jitter = wider;
            let target = self.config.target_for(wider);
            self.drift.shift(self.target - target);
            self.target = target;
        }
    }

    /// A jump back into the history by `back` frames (aligned), with a
    /// crossfade from the old head. Returns the jump.
    fn jump_back(&mut self, back: f64, serves: u64) -> f64 {
        let old = self.position;
        let (back, score) = self.align_back(old, back, serves);
        self.position = self.position.wrapping_sub((back as u64) << 32);
        self.crossfade = Some(self.crossfade_from(old, score >= MATCHED));
        back
    }

    fn pull_piece(&mut self, out: &mut [f32], frames: usize, time_ns: Option<u64>) {
        self.drain_marks();
        if self.state == AdapterState::Filling && !self.try_start(frames, time_ns) {
            return;
        }
        let dt = frames as f64 * self.frame_seconds;
        let fill = match self.linear_fill(time_ns) {
            Some(fill) => {
                self.linearised += 1;
                fill
            }
            None => {
                self.raw += 1;
                self.raw_fill()
            }
        };
        self.fill = fill;
        self.clock += dt;
        let correction = self.drift.update(fill - self.target, dt);
        self.step = to_fixed(self.nominal_step * (1.0 + correction));
        let resync = self.resync_due(time_ns, dt, correction);
        self.last_pull = time_ns.map(|t| (t, frames));

        let step = self.step;
        let step_frames = step as f64 / ONE as f64;
        let available = self.available();
        let serves = self.converter.reads_before(self.position, step, available);
        // A splice starts while J's worth of frames is still left beyond
        // this pull, so the old head has them for its crossfade; in the
        // steady state the margin is half a source period more than J.
        let margin = (self.jitter * f64::from(self.config.dest_rate)).ceil() as u64;
        let lacking = (frames as u64 + margin).saturating_sub(serves);
        let short = lacking as f64 * step_frames + 1.0;
        // A crossfade under way finishes first, unless this pull would run
        // out of frames.
        let busy = self.crossfade.is_some() && serves >= frames as u64;
        if self.stopping || busy {
            // Fading out, or a crossfade under way: no splice.
        } else if resync && lacking == 0 {
            // A resync: to F*, the loop back in acquire, its integral (the
            // drift) kept.
            self.splice_to_target(frames, serves, step_frames);
            self.drift.restart(self.fill - self.target, true);
            self.far_for = 0.0;
            self.clamped_for = 0.0;
            self.resyncs += 1;
        } else if lacking > 0 && (self.ring_frames() == 0 || self.producer_overdue(time_ns)) {
            // The producer stopped: back into the history far enough for
            // the 5 ms fade-out, then silence and the startup fill.
            let fade = (self.splice_frames as u64).saturating_sub(serves) as f64 * step_frames;
            let back = short.max(fade + 1.0).min(self.most_back).ceil();
            self.jump_back(back, serves);
            self.fade.start_out(self.splice_frames as u32);
            self.stopping = true;
            self.stops += 1;
        } else if lacking > 0 {
            // A repeat: back into the history to F* (the loop's fill), and
            // at least as far as this pull lacks.
            self.widen();
            let back = (self.target - self.fill)
                .max(short)
                .min(self.most_back)
                .ceil();
            let back = self.jump_back(back, serves);
            self.drift.shift(back);
            self.slips += 1;
            self.slipped_in_track();
            self.repeats += 1;
        } else if self.raw_fill() > OVERRUN_TARGETS * self.target {
            // A skip: forward to F*, as far as the fill leaves this pull
            // its frames.
            let raw = self.raw_fill();
            let half = self.converter.spec().half() as f64;
            let most = raw - frames as f64 * step_frames - half - 2.0;
            let ahead = (raw - self.target).min(most).floor().max(0.0);
            let old = self.position;
            let (ahead, score) = self.align_ahead(old, ahead, serves);
            self.position = self.position.wrapping_add((ahead as u64) << 32);
            self.crossfade = Some(self.crossfade_from(old, score >= MATCHED));
            self.drift.shift(-ahead);
            self.slips += 1;
            self.slipped_in_track();
            self.skips += 1;
        }

        // Bring in the frames every head needs for this piece.
        let mut until = self.converter.written();
        let mut furthest = |position: u64, reads: usize| {
            if reads > 0 {
                let last = position.wrapping_add(step.wrapping_mul(reads as u64 - 1));
                let need = self.converter.needs_until(last);
                if frames_between(need, until) > 0 {
                    until = need;
                }
            }
        };
        furthest(self.position, frames);
        if let Some(x) = self.crossfade {
            furthest(x.old, x.forward.min(frames));
        }
        self.take_until(until);
        let written = self.converter.written();

        let channels = self.config.channels;
        for frame_out in out.chunks_exact_mut(channels) {
            self.frame.iter_mut().for_each(|s| *s = 0.0);
            let mut main_gain = 1.0f32;
            if let Some(x) = self.crossfade.as_mut() {
                // Gains at the end of each frame's step: the last is fall
                // 0, rise 1.
                let done_at =
                    ((x.at + 1) * self.splice_frames / x.len.max(1)).min(self.splice_frames);
                let mut fall = self.fall.get(done_at).copied().unwrap_or(0.0);
                let mut rise = self
                    .fall
                    .get(self.splice_frames - done_at)
                    .copied()
                    .unwrap_or(1.0);
                if x.matched {
                    // Like material adds in amplitude: gains summing to 1.
                    fall *= fall;
                    rise *= rise;
                }
                if frames_between(written, self.converter.needs_until(x.old)) >= 0 {
                    self.converter.read_add(x.old, fall, &mut self.frame);
                }
                if x.forward > 1 {
                    x.forward -= 1;
                    x.old = x.old.wrapping_add(step);
                } else {
                    // Past the last served read: reflect about it.
                    x.forward = 0;
                    x.old = x.old.wrapping_sub(step);
                }
                x.at += 1;
                main_gain = rise;
                if x.at >= x.len {
                    self.crossfade = None;
                }
            }
            if frames_between(written, self.converter.needs_until(self.position)) >= 0 {
                self.converter
                    .read_add(self.position, main_gain, &mut self.frame);
            }
            self.position = self.position.wrapping_add(step);
            for (o, &s) in frame_out.iter_mut().zip(&self.frame) {
                *o = s;
            }
        }
        if self.fade_wait > 0 {
            let wait = self.fade_wait.min(frames);
            self.fade_wait -= wait;
            let (silent, rest) = out.split_at_mut((wait * channels).min(out.len()));
            silent.fill(0.0);
            if self.fade_wait == 0 {
                self.fade.start_in(fade_frames(self.config.dest_rate));
            }
            self.fade.apply(rest, channels);
        } else {
            self.fade.apply(out, channels);
        }
        if self.stopping && self.fade.is_silent() {
            // Faded out: silence until the ring holds F* again.
            self.stopping = false;
            self.crossfade = None;
            self.fade = Fade::default();
            self.state = AdapterState::Filling;
        } else if self.state != AdapterState::Filling {
            let locked = self.drift.mode() == LoopMode::Track;
            if locked {
                self.clamped_total = 0.0;
            }
            if self.suspect && locked && !self.stopping {
                // Locked again: back in.
                self.suspect = false;
                self.fade.start_in(fade_frames(self.config.dest_rate));
            } else if !self.suspect
                && (self.clamped_total >= SUSPECT_SECONDS
                    || self.clock - self.slip_times[0] <= SUSPECT_SECONDS)
            {
                self.suspect = true;
                self.suspects += 1;
                if !self.stopping {
                    self.fade.start_out(self.splice_frames as u32);
                }
            }
            self.state = if self.suspect {
                AdapterState::Suspect
            } else if locked {
                AdapterState::Locked
            } else {
                AdapterState::Locking
            };
        }
    }

    /// A slip while tracking: the loop is not keeping up with the clock
    /// (none occurs in the steady state, ADR 0052 §3), so back to acquire,
    /// whose faster slew reaches a clock beyond the track loop's: a resync.
    fn slipped_in_track(&mut self) {
        self.slip_times.rotate_left(1);
        if let Some(last) = self.slip_times.last_mut() {
            *last = self.clock;
        }
        if self.drift.mode() == LoopMode::Track {
            self.drift.restart(self.drift.filtered_error(), true);
            self.resyncs += 1;
        }
    }

    /// Whether this pull resyncs (ADR 0052 §6): the clock jumped, the
    /// tracking error stayed beyond F*/2, or the correction sat at its
    /// clamp. Updates the timers.
    fn resync_due(&mut self, time_ns: Option<u64>, dt: f64, correction: f64) -> bool {
        let clamp = self.drift.params().clamp;
        if clamp > 0.0 && correction.abs() >= clamp * (1.0 - 1e-9) {
            self.clamped_for += dt;
            self.clamped_total += dt;
        } else {
            self.clamped_for = 0.0;
        }
        let tracking = self.drift.mode() == LoopMode::Track;
        if tracking && self.drift.filtered_error().abs() > self.target / 2.0 {
            self.far_for += dt;
        } else {
            self.far_for = 0.0;
        }
        let period = self.config.dest_period as f64 * self.frame_seconds;
        let jumped = match (self.last_pull, time_ns) {
            (Some((then, delivered)), Some(now)) => {
                let expected = then as f64 * 1e-9 + delivered as f64 * self.frame_seconds;
                now as f64 * 1e-9 - expected > JUMP_PERIODS * period
            }
            _ => false,
        };
        jumped || self.far_for >= FAR_SECONDS || self.clamped_for >= CLAMP_RESYNC_SECONDS
    }

    /// A resync's splice: forward or back to F*, aligned and crossfaded as
    /// a skip or a repeat, as far as this pull of `frames` keeps its
    /// frames and the history reaches.
    fn splice_to_target(&mut self, frames: usize, serves: u64, step_frames: f64) {
        let raw = self.raw_fill();
        let delta = self.fill - self.target;
        if delta >= 1.0 {
            let half = self.converter.spec().half() as f64;
            let most = raw - frames as f64 * step_frames - half - 2.0;
            let ahead = delta.min(most).floor().max(0.0);
            if ahead >= 1.0 {
                let old = self.position;
                let (ahead, score) = self.align_ahead(old, ahead, serves);
                self.position = self.position.wrapping_add((ahead as u64) << 32);
                self.crossfade = Some(self.crossfade_from(old, score >= MATCHED));
                self.fill -= ahead;
            }
        } else if delta <= -1.0 {
            let back = (-delta).min(self.most_back).ceil();
            let back = self.jump_back(back, serves);
            self.fill += back;
        }
    }
}

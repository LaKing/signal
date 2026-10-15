//! `lab.gpu_partials`: a bank of [`PARTIALS`] sine partials computed per
//! block on the GPU (ADR 0019 §3), with its CPU reference ([`reference`]).
//!
//! **Ports:** `out` (signal out, audio, `[−1, 1]`). No inputs.
//!
//! **Parameters**, read at the first frame of each block of the module's own:
//! - `freq` (Hz, `[0, 20 000]`, default 55, add): the fundamental *f*;
//! - `stretch` (`[0, 1]`, default 0, add): inharmonicity *s*;
//! - `tilt` (`[0, 4]`, default 1, add): spectral slope τ;
//! - `gain` (`[0, 1]`, default 0.25, multiply): *g*.
//!
//! **Formula.** Blocks are [`BLOCK_FRAMES`] frames, counted from the
//! instance's first frame. For block *m* with parameters (*f*, *s*, τ, *g*)
//! at its first frame and partial *i* = 0 … 1023:
//!
//! ```text
//! f_i   = f·(i + 1)·(1 + s·i)
//! inc_i = fract(f_i / sr)                    cycles per frame (mod 1)
//! a_i   = g·(i + 1)^−τ / Σ_j (j + 1)^−τ     if f_i < sr/2, else 0
//! y_m[t] = Σ_i a_i·sin(2π·(φ_i[m] + inc_i·t))     t = 0 … BLOCK_FRAMES − 1
//! φ_i[m + 1] = fract(φ_i[m] + inc_i·BLOCK_FRAMES),   φ_i[0] = 0
//! out[m·B + t] = y_{m − L}[t] for m ≥ L, and 0 for m < L
//! ```
//!
//! with *L* = [`LATENCY_BLOCKS`] = 2 and *B* = [`BLOCK_FRAMES`] = 256. The
//! output is exactly *L* blocks late: it depends only on parameters of
//! earlier blocks, so the type is state-output, and it may close a cycle.
//!
//! **GPU and CPU.** [`reference`] computes `y_m` in f64 on the CPU. The GPU
//! kernel computes it in f32: one invocation per frame sums the partials in
//! a fixed order (32 groups of 32), so its result is the same bit for bit on
//! the same machine and build (reproducibility `same_machine`), but not
//! across GPUs. **Tolerance:** |GPU − CPU| ≤ [`TOLERANCE`] = 1e-4 relative
//! to the bank's full scale Σ|a_i| (≤ *g*), per sample. Operator measurement
//! (2026-10-12, Apple M5 Max, Metal): worst 4.5e-5 at 48 kHz and 1.9e-5 at
//! 96 kHz, the same on every run; the first estimate, 1e-5, was made without a
//! GPU. The excess over the f32 kernel's simulated error comes from the GPU's
//! `sin`, which is not correctly rounded. The phase is passed
//! split (a 15-bit `inc_hi`, whose product with *t* < 256 is exact in f32,
//! and the rest) so that f32 keeps it to about 1e-7 cycles.
//!
//! **Time.** The phases (1 024 multiply-adds per block), the upload of the
//! block's partials and every per-frame call are bounded and allocation
//! free. The GPU work runs on its device's `signal-gpu-<id>` thread; the
//! module never waits for it. The engine waits for a due result offline only
//! (`due`).
//!
//! **Device (M2.7-2).** The instance runs on the device its host lends it
//! (`PrepareContext::lent`, a [`Lease`] placed by `signal_gpu::GpuHost`);
//! without one it has no channel and every due result is lost. When its
//! device is lost and the pool replaces it, the next block's `due` takes the
//! new channel ([`PlacedChannel::refresh`]): the results still in flight on
//! the old device are lost (cause `device_lost`), the next ones come back.

use libm::{exp, floor, log, sin};
use signal_core::{
    BlockLatency, Combine, Direction, Due, Feedthrough, Inputs, LossCause, ModuleDescriptor,
    Outputs, ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext, Range,
    RateDivisor, Reproducibility, StateOutputPrimitive,
};
use signal_gpu::{Channel, Failure, Job, KernelSpec, Lease, Outcome, PlacedChannel};

/// The partials in the bank.
pub const PARTIALS: usize = 1024;
/// The module's block, in frames: part of the result, fixed for the type.
pub const BLOCK_FRAMES: usize = 256;
/// *L*: the output in block *m* is the result for block *m* − *L*. Two, so
/// that in real time a block's job has a whole block (2.67 ms at 96 kHz) to
/// come back; with one it would be due on the frame after its submission.
pub const LATENCY_BLOCKS: u32 = 2;
/// The GPU's documented tolerance, relative to the bank's full scale Σ|a_i|:
/// about twice the worst error measured on an Apple M5 Max (4.5e-5).
pub const TOLERANCE: f64 = 1e-4;
/// Jobs per instance: *L* in flight, the one shown, and spares for jobs a
/// reset or a late result left in flight.
pub const JOBS: usize = 2 * LATENCY_BLOCKS as usize + 2;
/// `f32` words per partial in a job's input: phase, inc_hi, inc_lo, amplitude.
const WORDS: usize = 4;
/// `inc_hi` keeps this many fractional bits.
const INC_HI_SCALE: f64 = 32_768.0;

/// The GPU kernel: one invocation per frame of the block.
pub const KERNEL: KernelSpec = KernelSpec {
    wgsl: include_str!("gpu_partials.wgsl"),
    entry: "partials",
    input_words: PARTIALS * WORDS,
    output_words: BLOCK_FRAMES,
    workgroups: ((BLOCK_FRAMES / 64) as u32, 1, 1),
};

const fn parameter(
    name: &'static str,
    kind: PortKind,
    unit: Option<&'static str>,
    (min, max, default): (f64, f64, f64),
    combine: Combine,
) -> ParameterDescriptor {
    ParameterDescriptor {
        name,
        kind,
        rate: RateDivisor::ONE,
        unit,
        range: Range { min, max },
        default,
        combine,
    }
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "lab.gpu_partials",
    doc: "A bank of 1024 sine partials computed per 256-frame block on the GPU, 2 blocks late (lab, ADR 0019 §3).",
    ports: &[PortDescriptor {
        name: "out",
        direction: Direction::Out,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Audio,
        unit: None,
        range: Some(Range {
            min: -1.0,
            max: 1.0,
        }),
    }],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            (0.0, 20_000.0, 55.0),
            Combine::Add,
        ),
        parameter(
            "stretch",
            PortKind::Control,
            None,
            (0.0, 1.0, 0.0),
            Combine::Add,
        ),
        parameter(
            "tilt",
            PortKind::Control,
            None,
            (0.0, 4.0, 1.0),
            Combine::Add,
        ),
        parameter(
            "gain",
            PortKind::Gain,
            None,
            (0.0, 1.0, 0.25),
            Combine::Multiply,
        ),
    ],
    feedthrough: Feedthrough::StateOutput,
    sink: None,
    latency: Some(BlockLatency {
        latency_blocks: LATENCY_BLOCKS,
        block_frames: BLOCK_FRAMES as u32,
    }),
    reproducibility: Reproducibility::SameMachine,
};

/// A block's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// The fundamental, in Hz.
    pub freq: f64,
    /// The inharmonicity.
    pub stretch: f64,
    /// The spectral slope.
    pub tilt: f64,
    /// The gain.
    pub gain: f64,
}

impl Default for Params {
    /// The descriptor's defaults.
    fn default() -> Params {
        Params {
            freq: 55.0,
            stretch: 0.0,
            tilt: 1.0,
            gain: 0.25,
        }
    }
}

/// One block's partials: what the GPU job and [`reference`] compute from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// a_i.
    pub amp: Vec<f64>,
    /// φ_i at the block's first frame, in cycles, in [0, 1).
    pub phase: Vec<f64>,
    /// inc_i, in cycles per frame, in [0, 1).
    pub inc: Vec<f64>,
}

impl Block {
    /// Σ|a_i|: the bank's full scale, which [`TOLERANCE`] is relative to.
    pub fn full_scale(&self) -> f64 {
        self.amp.iter().map(|a| a.abs()).sum()
    }
}

/// The bank's state: the partials' phases, block to block. Shared by the
/// module, its tests and `signal bench`.
#[derive(Clone, Debug, Default)]
pub struct Bank {
    sample_rate: f64,
    /// ln(i + 1).
    ln: Vec<f64>,
    /// The tilt `weights` belong to (NaN: none yet).
    tilt: f64,
    /// (i + 1)^−τ / Σ_j (j + 1)^−τ.
    weights: Vec<f64>,
    /// φ_i of the next block.
    phases: Vec<f64>,
    block: Block,
}

impl Bank {
    /// A bank at `sample_rate`, every phase 0. Allocates.
    pub fn new(sample_rate: f64) -> Bank {
        Bank {
            sample_rate,
            ln: (0..PARTIALS).map(|i| log((i + 1) as f64)).collect(),
            tilt: f64::NAN,
            weights: vec![0.0; PARTIALS],
            phases: vec![0.0; PARTIALS],
            block: Block {
                amp: vec![0.0; PARTIALS],
                phase: vec![0.0; PARTIALS],
                inc: vec![0.0; PARTIALS],
            },
        }
    }

    /// Back to every phase 0. Real-time safe.
    pub fn reset(&mut self) {
        self.phases.fill(0.0);
    }

    /// The next block's partials for `p`; the phases then advance by a
    /// block. Real-time safe: bounded, no allocation.
    pub fn next_block(&mut self, p: Params) -> &Block {
        if p.tilt.to_bits() != self.tilt.to_bits() {
            for (w, ln) in self.weights.iter_mut().zip(&self.ln) {
                *w = exp(-p.tilt * ln);
            }
            let total: f64 = self.weights.iter().sum();
            for w in &mut self.weights {
                *w /= total;
            }
            self.tilt = p.tilt;
        }
        let nyquist = self.sample_rate / 2.0;
        let b = &mut self.block;
        let partials = b.amp.iter_mut().zip(&mut b.phase).zip(&mut b.inc);
        let state = self.phases.iter_mut().zip(&self.weights);
        for (i, (((amp, phase), inc), (next, weight))) in partials.zip(state).enumerate() {
            let n = i as f64;
            let f = p.freq * (n + 1.0) * (1.0 + p.stretch * n);
            *inc = fract(f / self.sample_rate);
            *amp = if f < nyquist { p.gain * weight } else { 0.0 };
            *phase = *next;
            *next = fract(*next + *inc * BLOCK_FRAMES as f64);
        }
        &self.block
    }
}

/// The CPU reference: `y[t]` of `block` in f64, for `t` = 0 … `out.len()` − 1.
pub fn reference(block: &Block, out: &mut [f64]) {
    for (t, y) in out.iter_mut().enumerate() {
        let t = t as f64;
        let partials = block.amp.iter().zip(&block.phase).zip(&block.inc);
        *y = partials
            .map(|((a, phase), inc)| a * sin(std::f64::consts::TAU * fract(phase + inc * t)))
            .sum();
    }
}

/// Writes `block` as the GPU job's input: per partial φ, inc_hi, inc_lo, a.
pub fn pack(block: &Block, input: &mut [f32]) {
    let partials = block.amp.iter().zip(&block.phase).zip(&block.inc);
    for (words, ((a, phase), inc)) in input.as_chunks_mut::<WORDS>().0.iter_mut().zip(partials) {
        let hi = floor(inc * INC_HI_SCALE) / INC_HI_SCALE;
        *words = [*phase as f32, hi as f32, (inc - hi) as f32, *a as f32];
    }
}

/// `x − ⌊x⌋`.
fn fract(x: f64) -> f64 {
    x - floor(x)
}

/// Returns `job` to `channel`'s pool. One the channel refuses (never one of
/// this module's own) is kept in `refused` and dropped with it, on the
/// control thread; with no room left it is leaked rather than freed here.
/// Real-time safe.
fn give_back(channel: &mut Channel, refused: &mut Vec<Job>, job: Job) {
    if let Err(job) = channel.recycle(job) {
        if refused.len() < refused.capacity() {
            refused.push(job);
        } else {
            std::mem::forget(job);
        }
    }
}

/// The cause the engine logs for a job that ended other than `Ok`.
fn cause_of(outcome: Outcome) -> Option<LossCause> {
    Some(match outcome {
        Outcome::Ok => return None,
        Outcome::Failed(Failure::Validation) => LossCause::Validation,
        Outcome::Failed(Failure::OutOfMemory) => LossCause::OutOfMemory,
        Outcome::Failed(Failure::Internal) => LossCause::Internal,
        Outcome::Failed(Failure::Mapping) => LossCause::Mapping,
        Outcome::Failed(Failure::Uncaptured) => LossCause::Uncaptured,
        Outcome::Failed(Failure::Stopped) => LossCause::Stopped,
        Outcome::DeviceLost => LossCause::DeviceLost,
    })
}

/// `lab.gpu_partials` (see the module documentation).
#[derive(Debug, Default)]
pub struct GpuPartials {
    bank: Bank,
    /// `None` without a device: every due result is then lost.
    channel: Option<PlacedChannel>,
    /// The parameters at the first frame of the block being absorbed.
    params: Params,
    /// The frame within the current block, for `emit` and `absorb` alike.
    frame: usize,
    /// The current block since `prepare` or `reset`.
    block: u64,
    /// The sequence number of block 0: raised past every earlier one by
    /// `reset`, so results still in flight are recognised as stale.
    first_seq: u64,
    /// Bit `seq % 64` set: that block's job could not be submitted.
    unsent: u64,
    /// Bit `seq % 64` set: that block's job was in flight on a device that
    /// was lost and replaced (M2.7-2); cleared, as `unsent`, by the next
    /// submission with the same bit.
    lost_at_rebind: u64,
    /// The job shown in the current block (`None`: silence).
    current: Option<Job>,
    /// Whether the current block's due result was dealt with (`due`, `miss`).
    resolved: bool,
    /// A recorded result an exact replay showed in the current block in place
    /// of the GPU's (`inject_result`, M2.7-4): [`BLOCK_FRAMES`] words,
    /// allocated in `prepare`.
    injected: Vec<f32>,
    /// Whether the current block shows `injected`.
    showing_injected: bool,
    /// Why the due result `due` last reported lost will never come: the
    /// job's failure (M2.7-5).
    cause: Option<LossCause>,
    /// Jobs the channel refused to take back (never one of this module's):
    /// kept, not dropped in real time; capacity [`JOBS`], from `prepare`.
    refused: Vec<Job>,
}

impl GpuPartials {
    /// The sequence number due in the current block, if one is.
    fn due_seq(&self) -> Option<u64> {
        let back = u64::from(LATENCY_BLOCKS);
        (self.block >= back).then(|| self.first_seq + self.block - back)
    }

    /// The bits (`seq % 64`) of every block submitted and not yet due.
    fn outstanding(&self) -> u64 {
        let from = self.due_seq().unwrap_or(self.first_seq);
        let to = self.first_seq + self.block;
        (from..to).fold(0u64, |bits, seq| bits | 1 << (seq % 64))
    }

    /// Why a block that was never sent, or is no longer coming back, is
    /// lost: its device was lost (now, or while it was in flight).
    fn unsent_cause(&self, bit: u64) -> Option<LossCause> {
        let device_lost = self.lost_at_rebind & bit != 0
            || self.channel.as_ref().is_some_and(|c| c.service_lost());
        device_lost.then_some(LossCause::DeviceLost)
    }

    /// Shows nothing in the current block; the shown job returns to the pool.
    fn silence(&mut self) {
        self.showing_injected = false;
        if let (Some(job), Some(channel)) = (self.current.take(), self.channel.as_mut()) {
            give_back(channel, &mut self.refused, job);
        }
    }
}

impl StateOutputPrimitive for GpuPartials {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.bank = Bank::new(ctx.sample_rate);
        // A new channel on the lent device (the old one, if any, is dropped
        // here, on the control thread).
        self.channel = ctx
            .lent
            .as_ref()
            .and_then(|lent| lent.downcast_ref::<Lease>())
            .and_then(|lease| lease.channel(KERNEL, JOBS).ok());
        self.current = None;
        self.refused = Vec::with_capacity(JOBS);
        self.injected = vec![0.0; BLOCK_FRAMES];
        self.first_seq = 0;
        self.reset();
    }

    fn reset(&mut self) {
        self.silence();
        self.bank.reset();
        self.params = Params::default();
        self.first_seq += self.block + 1;
        self.block = 0;
        self.frame = 0;
        self.unsent = 0;
        self.lost_at_rebind = 0;
        self.resolved = false;
        self.cause = None;
    }

    fn due(&mut self) -> Due {
        self.cause = None;
        if self.frame != 0 || self.resolved {
            return Due::Ready;
        }
        // M2.7-2: a replacement for a lost device is taken at a block's
        // start; the shown job goes back to its own (old) channel first, and
        // every block still in flight there is lost.
        if let Some(channel) = self.channel.as_mut() {
            let (current, refused) = (&mut self.current, &mut self.refused);
            let swapped = channel.refresh(|old| {
                if let Some(job) = current.take() {
                    give_back(old, refused, job);
                }
            });
            if swapped {
                self.showing_injected = false;
                let outstanding = self.outstanding();
                self.lost_at_rebind |= outstanding;
                self.unsent |= outstanding;
            }
        }
        let Some(due) = self.due_seq() else {
            self.silence();
            self.resolved = true;
            return Due::Ready;
        };
        let bit = 1 << (due % 64);
        let cause = self.unsent_cause(bit);
        let Some(channel) = self.channel.as_mut() else {
            return Due::Lost;
        };
        // Stale results (from before a reset, or given up on) go back to the
        // pool first, on every path: a pool emptied by late results refills
        // as soon as they arrive, so submissions resume.
        while channel.peek_done().is_some_and(|seq| seq < due) {
            if let Some(job) = channel.take_done() {
                give_back(channel, &mut self.refused, job);
            }
        }
        if self.unsent & bit != 0 {
            self.cause = cause;
            return Due::Lost;
        }
        match channel.peek_done() {
            Some(seq) if seq == due => {
                let Some(job) = channel.take_done() else {
                    return Due::Pending;
                };
                if job.failed() {
                    self.cause = cause_of(job.outcome());
                    give_back(channel, &mut self.refused, job);
                    return Due::Lost;
                }
                if let Some(shown) = self.current.replace(job) {
                    give_back(channel, &mut self.refused, shown);
                }
                self.showing_injected = false;
                self.resolved = true;
                Due::Ready
            }
            // A stale result published since the loop above: the next `due`
            // returns it to the pool (an offline wait asks again; in real
            // time a pending result is a miss, as a lost one; its cause is
            // the device's loss when the device is lost, auditor-2-015).
            // While the lost service still holds the channel's jobs (it
            // drains them), a miss is the device's loss too (auditor-1-008).
            Some(seq) if seq < due => {
                self.cause = cause;
                Due::Pending
            }
            Some(_) => {
                self.cause = cause;
                Due::Lost
            }
            None if channel.is_lost() => {
                self.cause = Some(if channel.service_lost() {
                    LossCause::DeviceLost
                } else {
                    LossCause::Stopped
                });
                Due::Lost
            }
            None => {
                self.cause = cause;
                Due::Pending
            }
        }
    }

    fn miss(&mut self) {
        self.silence();
        self.resolved = true;
        self.cause = None;
    }

    fn lost_cause(&self) -> Option<LossCause> {
        self.cause
    }

    fn result_words(&self) -> usize {
        BLOCK_FRAMES
    }

    fn delivered_result(&self) -> &[f32] {
        // The result shown: the GPU's, or in an exact replay the injected
        // one, which the engine checks against its recorded hash (M2.7-8).
        match &self.current {
            _ if self.resolved && self.showing_injected => &self.injected,
            Some(job) if self.resolved => job.output(),
            _ => &[],
        }
    }

    fn inject_result(&mut self, words: &[f32]) -> bool {
        if words.len() != self.injected.len() {
            return false;
        }
        // The GPU's own job for this block, if it comes, is stale from now
        // on and returns to the pool with the next block's `due`.
        self.silence();
        self.injected.copy_from_slice(words);
        self.showing_injected = true;
        self.resolved = true;
        true
    }

    fn settle(&mut self) -> Due {
        // Only the absorb that ends a block submits.
        if self.frame + 1 < BLOCK_FRAMES {
            return Due::Ready;
        }
        let Some(channel) = self.channel.as_mut() else {
            return Due::Lost;
        };
        // Every result before the next block's due one was dealt with:
        // finished stale jobs go back to the pool, as the next `due` would
        // return them.
        let back = u64::from(LATENCY_BLOCKS);
        let next_due = (self.block + 1 >= back).then(|| self.first_seq + self.block + 1 - back);
        while channel
            .peek_done()
            .is_some_and(|seq| next_due.is_some_and(|due| seq < due))
        {
            if let Some(job) = channel.take_done() {
                give_back(channel, &mut self.refused, job);
            }
        }
        if channel.free_jobs() > 0 {
            Due::Ready
        } else if channel.is_lost() {
            Due::Lost
        } else {
            Due::Pending
        }
    }

    fn emit(&self, outputs: &mut Outputs<'_>) {
        let value = match &self.current {
            _ if self.showing_injected && self.resolved => {
                self.injected.get(self.frame).copied().unwrap_or(0.0)
            }
            Some(job) if self.resolved => job.output().get(self.frame).copied().unwrap_or(0.0),
            _ => 0.0,
        };
        outputs.signal(0).fill(f64::from(value));
    }

    fn absorb(&mut self, inputs: &Inputs<'_>) {
        if self.frame == 0 {
            let at =
                |j: usize, default: f64| inputs.parameter(j).first().copied().unwrap_or(default);
            let d = Params::default();
            self.params = Params {
                freq: at(0, d.freq),
                stretch: at(1, d.stretch),
                tilt: at(2, d.tilt),
                gain: at(3, d.gain),
            };
        }
        self.frame += 1;
        if self.frame < BLOCK_FRAMES {
            return;
        }
        let seq = self.first_seq + self.block;
        let block = self.bank.next_block(self.params);
        let sent = match self.channel.as_mut() {
            Some(channel) => channel.submit(seq, |input| pack(block, input)),
            None => false,
        };
        let bit = 1 << (seq % 64);
        self.unsent = if sent {
            self.unsent & !bit
        } else {
            self.unsent | bit
        };
        self.lost_at_rebind &= !bit;
        self.frame = 0;
        self.block += 1;
        self.resolved = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's arithmetic (`gpu_partials.wgsl`) in f32 on the CPU, with
    /// libm's `sinf` for the GPU's `sin`.
    fn kernel_in_f32(input: &[f32], out: &mut [f32]) {
        let partials = input.as_chunks::<WORDS>().0;
        for (t, y) in out.iter_mut().enumerate() {
            let tf = t as f32;
            let mut total = 0.0f32;
            for group in partials.chunks(32) {
                let mut sum = 0.0f32;
                for &[phase, hi, lo, a] in group {
                    let x = phase + (hi * tf).fract() + lo * tf;
                    let cycles = x - x.floor();
                    let centred = cycles - (cycles + 0.5).floor();
                    sum += a * libm::sinf(std::f32::consts::TAU * centred);
                }
                total += sum;
            }
            *y = total;
        }
    }

    /// The packing (φ, a 15-bit inc_hi, inc_lo, a) keeps f32 arithmetic
    /// within the documented tolerance of the f64 reference, over many
    /// blocks of advancing phases and every kind of parameter. Measurement
    /// of the design only: the GPU's own `sin` is checked on a GPU.
    #[test]
    fn f32_kernel_arithmetic_is_within_the_tolerance() {
        let mut worst: f64 = 0.0;
        for sample_rate in [44_100.0, 96_000.0, 192_000.0] {
            let mut bank = Bank::new(sample_rate);
            for m in 0..64 {
                let x = m as f64;
                let p = Params {
                    freq: [20.0, 55.0, 261.6, 1_000.0, 7_000.0][m % 5] * (1.0 + 0.003 * x),
                    stretch: [0.0, 0.0002, 0.01, 1.0][m % 4],
                    tilt: (0.29 * x) % 4.0,
                    gain: 1.0,
                };
                let block = bank.next_block(p);
                let mut input = vec![0.0f32; PARTIALS * WORDS];
                pack(block, &mut input);
                let mut gpu = vec![0.0f32; BLOCK_FRAMES];
                kernel_in_f32(&input, &mut gpu);
                let mut cpu = vec![0.0; BLOCK_FRAMES];
                reference(block, &mut cpu);
                let scale = block.full_scale();
                for (g, c) in gpu.iter().zip(&cpu) {
                    worst = worst.max((f64::from(*g) - c).abs() / scale);
                }
            }
        }
        eprintln!("f32 kernel arithmetic: worst |f32 − f64| / Σ|a| = {worst:.3e}");
        assert!(worst <= TOLERANCE / 4.0, "{worst}");
    }

    /// One block as the offline engine runs it: wait for a pending result,
    /// `miss` a lost one. Whether its due result was delivered, and the
    /// cause the engine would log for one that was lost.
    fn offline_block(module: &mut GpuPartials) -> (bool, Option<LossCause>) {
        let values = {
            let p = Params::default();
            [p.freq, p.stretch, p.tilt, p.gain]
        };
        let mut delivered = true;
        let mut cause = None;
        for t in 0..BLOCK_FRAMES {
            let started = std::time::Instant::now();
            loop {
                match module.due() {
                    Due::Ready => break,
                    Due::Pending => {
                        assert!(started.elapsed().as_secs() < 10, "a due result never came");
                        std::thread::yield_now();
                    }
                    Due::Lost => {
                        delivered &= t != 0;
                        cause = module.lost_cause();
                        module.miss();
                        break;
                    }
                }
            }
            let mut out = [0.0];
            module.emit(&mut Outputs::new(1, &mut out, &mut []));
            module.absorb(&Inputs::new(1, &[], &values, &[], &[]));
        }
        (delivered && module.block > u64::from(LATENCY_BLOCKS), cause)
    }

    /// M2.7-2 (auditor-1-003, auditor-2-011): when its device is replaced,
    /// the shown job goes back to its own (old) channel, so nothing is
    /// refused or leaked; the blocks in flight are lost with the cause
    /// `device_lost`, their marks are cleared as `unsent` is, by the next
    /// block sent with the same mark (`seq % 64`), and
    /// the bank delivers again on the new channel.
    #[test]
    fn a_replaced_device_takes_back_every_job() {
        use signal_gpu::DevicePool;
        use std::sync::Arc;

        let pool = Arc::new(DevicePool::noop(1).unwrap());
        let mut module = GpuPartials::default();
        module.prepare(&PrepareContext {
            sample_rate: 96_000.0,
            max_frames: 256,
            rng: signal_core::RngStream::derive(1, &"/bank".parse().unwrap()),
            lent: Some(Arc::new(DevicePool::lease(&pool, 0))),
            generation: 0,
        });
        for _ in 0..4 {
            offline_block(&mut module);
        }
        assert!(offline_block(&mut module).0, "delivering before the loss");
        assert!(module.current.is_some(), "a job is shown");
        pool.lose_device(0);
        let started = std::time::Instant::now();
        while pool.status()[0].generation < 2 {
            assert!(started.elapsed().as_secs() < 10, "not replaced");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut causes = Vec::new();
        let mut delivered = 0;
        // Past 64 blocks: every mark (`seq % 64`) is sent again.
        for _ in 0..70 {
            match offline_block(&mut module) {
                (true, _) => delivered += 1,
                (false, cause) => causes.push(cause),
            }
            assert!(
                module.refused.is_empty(),
                "every job went to its own channel"
            );
        }
        assert!(!causes.is_empty(), "the blocks in flight were lost");
        assert!(
            causes.iter().all(|c| *c == Some(LossCause::DeviceLost)),
            "{causes:?}"
        );
        assert!(delivered > 2 * JOBS, "delivering again: {delivered}");
        assert_eq!(module.lost_at_rebind, 0, "cleared by later submissions");
        assert_eq!(module.unsent, 0);
    }

    /// Partials at or above Nyquist are silent; Σ|a_i| is the gain.
    #[test]
    fn partials_above_nyquist_are_silent() {
        let mut bank = Bank::new(48_000.0);
        let block = bank.next_block(Params {
            freq: 1_000.0,
            stretch: 0.0,
            tilt: 0.0,
            gain: 0.5,
        });
        let audible = block.amp.iter().filter(|&&a| a != 0.0).count();
        assert_eq!(audible, 23, "1 kHz to 23 kHz below 24 kHz");
        let all = bank.next_block(Params {
            freq: 1.0,
            stretch: 0.0,
            tilt: 0.0,
            gain: 0.5,
        });
        assert!((all.full_scale() - 0.5).abs() < 1e-12);
    }
}

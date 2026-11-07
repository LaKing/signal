//! The offline WAV render: the engine's offline driver
//! ([`run_offline`]) with the audio adapter as its output. It records the
//! audio sink's inputs times its `gain` and writes `audio.wav`, `log.jsonl`
//! and `manifest.json`. Not real-time code: it allocates and writes files.

use std::fs;
use std::path::Path;

use signal_core::{Fnv1a64, Frame, InstanceAddress, fnv1a64};
use signal_engine::{
    ComputeDevice, Engine, EngineError, Manifest, OfflineError, Placement, Provenance, SameMachine,
    SinkOutput, run_offline,
};

use signal_graph::ExecutionPlan;

use crate::audio::{AudioSink, Conversion, RenderError, to_device};
use crate::inputs::RunInput;
use crate::vectors::{VectorTap, VectorTaps, VectorTrack};

/// The result of [`render`]: the audio and the log, in memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    /// The sample rate in frames per second.
    pub sample_rate: u32,
    /// The master seed.
    pub seed: u64,
    /// The number of frames rendered.
    pub frames: u64,
    /// The audio sink's signal inputs: one channel per input, in port order.
    pub channels: u16,
    /// The samples, interleaved by frame.
    pub samples: Vec<f64>,
    /// The bytes of `log.jsonl`.
    pub log: Vec<u8>,
    /// The instances of any of its plans that are reproducible on the same
    /// machine only (ADR 0019 §3), by address; empty for a bit-exact render.
    pub same_machine: Vec<SameMachine>,
    /// How `audio.wav` and its hash store the samples (M2.7-4): as `f32`
    /// casts, or as the device received them in an exact replay.
    pub conversion: Conversion,
    /// An ideal render of a session (`signal replay --ideal`, M2.7-4): every
    /// latent result was awaited. The manifest says so.
    pub ideal: bool,
    /// An exact replay that `--trust-provenance` let run although the
    /// recording's build or machine differ (M2.7-8). The manifest says so.
    pub trust_provenance: bool,
    /// The compute devices and the placement (M2.7-2), for the manifest;
    /// the program that placed its instances sets them, empty otherwise.
    pub compute_devices: Vec<ComputeDevice>,
    /// See [`compute_devices`](Render::compute_devices).
    pub placement: Vec<Placement>,
}

/// Renders `frames` frames of a fresh `engine`.
///
/// The plan must contain exactly one instance whose descriptor declares
/// `sink = Audio`; each frame records that instance's signal inputs (as summed
/// by the executor, whether or not the sink itself is suspended or muted). If
/// the sink's type declares a parameter named `gain`, each recorded input is
/// multiplied by that parameter's effective value (`docs/semantics.md` §8). Each
/// frame's log records go to `log.jsonl` in canonical order: a swap, then edits,
/// then events and discards by target port address, then faults.
///
/// It runs blocks of the engine's block capacity; the result does not depend
/// on the block size (`docs/semantics.md` §13, with §14's exception).
pub fn render(engine: &mut Engine, frames: u64) -> Result<Render, RenderError> {
    let block = engine.config().block_capacity;
    render_blocks(engine, frames, block, |_| Ok(None))
}

/// [`render`], calling `before` before each frame runs, so that it can commit
/// a swap or push input effective from [`Engine::next_frame`] (ADR 0006). Each
/// frame's log is drained after it is recorded, as a swap requires.
///
/// The audio sink and its `gain` are looked up again after every swap (the
/// sink's type may change, and with it whether it declares `gain`). A
/// swapped-in plan must keep exactly one audio sink at the same address with
/// as many signal inputs; otherwise the render stops with [`RenderError::SinkChanged`]. An error from
/// `before` stops it with [`RenderError::Edit`].
pub fn render_with(
    engine: &mut Engine,
    frames: u64,
    mut before: impl FnMut(&mut Engine) -> Result<(), EngineError>,
) -> Result<Render, RenderError> {
    render_blocks(engine, frames, 1, |engine| {
        before(engine)?;
        Ok(Some(Frame(engine.next_frame().0.saturating_add(1))))
    })
}

/// [`render`] in blocks of at most `block` frames (and the engine's block
/// capacity), calling `before` before the first block and again before the
/// block starting at the frame it returns, so that it can commit a swap or
/// push input there ([`run_offline`]).
///
/// Errors as [`render_with`].
pub fn render_blocks(
    engine: &mut Engine,
    frames: u64,
    block: usize,
    before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
) -> Result<Render, RenderError> {
    render_blocks_tapped(engine, frames, block, before, &[], &[]).map(|(render, _)| render)
}

/// [`render_blocks`], also taking snapshots of the vector outputs `taps`
/// (M2.8-7): one [`VectorTrack`] per tap, its snapshots at the frames that
/// are multiples of its decimation, as the real-time vector adapter takes
/// them (`crate::vectors`). The audio and the log are those of
/// [`render_blocks`]: the taps only read. `plans[v]` is the plan the engine
/// runs at version `v` (`Engine::version`: the first, then one per swap
/// `before` commits): a tap is refused, from the swap on, in a plan where its
/// decimation is not a multiple of its port's rate divisor, or for which no
/// plan is given ([`VectorTrack::refused`]).
pub fn render_blocks_tapped(
    engine: &mut Engine,
    frames: u64,
    block: usize,
    before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
    taps: &[VectorTap],
    plans: &[ExecutionPlan],
) -> Result<(Render, Vec<VectorTrack>), RenderError> {
    render_inner(engine, frames, block, before, taps, plans, None, Vec::new())
        .map(|(render, tracks, _)| (render, tracks))
}

/// One more play side of a render (ADR 0038 §9): a device slot's sink other
/// than the main one, its inputs times its `gain`, written as
/// `devices/<slot>.wav` beside `audio.wav`.
#[derive(Clone, Debug, PartialEq)]
pub struct SideRender {
    /// The sink.
    pub address: InstanceAddress,
    /// Its channels.
    pub channels: u16,
    /// The samples, interleaved by frame.
    pub samples: Vec<f64>,
}

impl SideRender {
    /// Writes the side as a 32-bit float WAV at `path` (each sample rounded
    /// to `f32`, as `audio.wav` under [`Conversion::Plain`]).
    pub fn write(&self, path: &Path, sample_rate: u32) -> Result<(), RenderError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut wav = hound::WavWriter::create(path, spec)?;
        for &sample in &self.samples {
            wav.write_sample(sample as f32)?;
        }
        wav.finalize()?;
        Ok(())
    }

    /// FNV-1a 64 of the samples as its WAV stores them.
    pub fn audio_fnv1a64(&self) -> u64 {
        let mut hasher = Fnv1a64::new();
        for &sample in &self.samples {
            hasher.update(&(sample as f32).to_le_bytes());
        }
        hasher.finish()
    }
}

/// [`render`] (`sides: None`) or [`render_sides`], `inputs` feeding the
/// sources (`signal render --input`, ADR 0038 §9); a source none serves
/// reads 0.
pub fn render_fed(
    engine: &mut Engine,
    frames: u64,
    sides: Option<(&InstanceAddress, &[InstanceAddress])>,
    inputs: Vec<RunInput>,
) -> Result<(Render, Vec<SideRender>), RenderError> {
    let block = engine.config().block_capacity;
    render_inner(engine, frames, block, |_| Ok(None), &[], &[], sides, inputs)
        .map(|(render, _, sides)| (render, sides))
}

/// [`render`] of a project with several play sides (ADR 0038 §9): `main`
/// is the sink `audio.wav` records (the [`Render`]), and each of `others`
/// is recorded beside it, in that order. The engine runs as for
/// [`render`]; the sides only read, so `main`'s samples are exactly
/// [`render`]'s for the same plan.
pub fn render_sides(
    engine: &mut Engine,
    frames: u64,
    main: &InstanceAddress,
    others: &[InstanceAddress],
) -> Result<(Render, Vec<SideRender>), RenderError> {
    let block = engine.config().block_capacity;
    render_inner(
        engine,
        frames,
        block,
        |_| Ok(None),
        &[],
        &[],
        Some((main, others)),
        Vec::new(),
    )
    .map(|(render, _, sides)| (render, sides))
}

/// [`render_blocks_tapped`] of a plan of several play sides (ADR 0038):
/// `audio.wav` records the sink at `main` (a session's replay records the
/// main side, as the live run's session did); the others are not recorded.
/// `None` is [`render_blocks_tapped`] exactly. `inputs` feed the sources
/// (a replay's recorded inputs, ADR 0038 §9); a source none serves reads 0.
#[allow(clippy::too_many_arguments)]
pub fn render_blocks_tapped_main(
    engine: &mut Engine,
    frames: u64,
    block: usize,
    before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
    taps: &[VectorTap],
    plans: &[ExecutionPlan],
    main: Option<&InstanceAddress>,
    inputs: Vec<RunInput>,
) -> Result<(Render, Vec<VectorTrack>), RenderError> {
    let sides = main.map(|m| (m, &[][..]));
    render_inner(engine, frames, block, before, taps, plans, sides, inputs)
        .map(|(render, tracks, _)| (render, tracks))
}

/// The main sink and the other play sides of a routed render.
type Sides<'a> = Option<(&'a InstanceAddress, &'a [InstanceAddress])>;

/// The renders' one body: the plan's one audio sink, or `sides` (the main
/// sink and the others).
#[allow(clippy::too_many_arguments)]
fn render_inner(
    engine: &mut Engine,
    frames: u64,
    block: usize,
    before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
    taps: &[VectorTap],
    plans: &[ExecutionPlan],
    sides: Sides<'_>,
    inputs: Vec<RunInput>,
) -> Result<(Render, Vec<VectorTrack>, Vec<SideRender>), RenderError> {
    if engine.next_frame() != Frame::ZERO {
        return Err(RenderError::AlreadyStarted);
    }
    let most = usize::from(u16::MAX);
    let (sink, others) = match sides {
        None => (AudioSink::select(engine, most)?, Vec::new()),
        Some((main, others)) => (
            AudioSink::at(engine, main, most)?,
            others
                .iter()
                .map(|a| AudioSink::at(engine, a, most).map(|s| (s, Vec::new())))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let routed = sides.is_some();
    let channels = u16::try_from(sink.width).unwrap_or(u16::MAX);
    let mut file = File {
        sink,
        others,
        routed,
        started: false,
        samples: Vec::new(),
        changed: None,
        taps: VectorTaps::new(taps, plans),
        at: 0,
        inputs,
    };
    let run = match run_offline(engine, frames, block, before, &mut file) {
        Ok(run) => run,
        Err(OfflineError::AlreadyStarted) => return Err(RenderError::AlreadyStarted),
        Err(OfflineError::Unbound) => {
            return Err(file.changed.unwrap_or(RenderError::NoAudioSink));
        }
        Err(OfflineError::Edit(error)) => return Err(RenderError::Edit(error)),
        Err(OfflineError::EngineFailed(failure)) => {
            return Err(RenderError::EngineFailed(failure));
        }
        Err(OfflineError::Json(error)) => return Err(RenderError::Json(error)),
    };
    let render = Render {
        sample_rate: run.sample_rate,
        seed: run.seed,
        frames: run.frames,
        channels,
        samples: file.samples,
        log: run.log,
        same_machine: run.same_machine,
        conversion: Conversion::Plain,
        ideal: false,
        trust_provenance: false,
        compute_devices: Vec::new(),
        placement: Vec::new(),
    };
    let sides = file
        .others
        .into_iter()
        .map(|(sink, samples)| SideRender {
            channels: u16::try_from(sink.width).unwrap_or(u16::MAX),
            address: sink.address,
            samples,
        })
        .collect();
    Ok((render, file.taps.tracks, sides))
}

/// The render's output: the audio sink's inputs times its `gain`, frame by
/// frame, interleaved.
struct File {
    sink: AudioSink,
    /// ADR 0038 §9: the other play sides, each with its samples.
    others: Vec<(AudioSink, Vec<f64>)>,
    /// The sinks were named: each must stay at its address across swaps.
    routed: bool,
    started: bool,
    samples: Vec<f64>,
    /// Why a swap lost the sink.
    changed: Option<RenderError>,
    /// Vector outputs to snapshot (M2.8-7).
    taps: VectorTaps,
    /// Frames rendered so far.
    at: u64,
    /// ADR 0038 §9: the sources' recorded inputs (a replay's), bound by
    /// address at each swap.
    inputs: Vec<RunInput>,
}

impl SinkOutput for File {
    fn bind(&mut self, engine: &Engine) -> bool {
        self.taps.bind(engine);
        for input in &mut self.inputs {
            input.bind(engine);
        }
        if !self.started {
            // Chosen by `render_blocks` from this plan.
            self.started = true;
            return true;
        }
        let kept = if self.routed {
            self.sink.same_at(engine).and_then(|()| {
                self.others
                    .iter_mut()
                    .try_for_each(|(sink, _)| sink.same_at(engine))
            })
        } else {
            self.sink.same(engine)
        };
        match kept {
            Ok(()) => true,
            Err(error) => {
                self.changed = Some(error);
                false
            }
        }
    }

    fn sources(&mut self, engine: &mut Engine, frames: usize) {
        for input in &mut self.inputs {
            input.fill(engine, frames, 0);
        }
    }

    fn block(&mut self, engine: &Engine, frames: usize) {
        self.taps.block(engine, self.at, frames);
        self.at += frames as u64;
        for (sink, samples) in &mut self.others {
            let gain = sink.gain_index();
            for f in 0..frames {
                for channel in 0..sink.width {
                    let x = engine.sink_input_at(sink.id, channel, f).unwrap_or(0.0);
                    samples.push(match gain.and_then(|j| engine.parameter_at_frame(j, f)) {
                        Some(gain) => gain * x,
                        None => x,
                    });
                }
            }
        }
        let gain = self.sink.gain_index();
        for f in 0..frames {
            for channel in 0..self.sink.width {
                let x = engine
                    .sink_input_at(self.sink.id, channel, f)
                    .unwrap_or(0.0);
                self.samples
                    .push(match gain.and_then(|j| engine.parameter_at_frame(j, f)) {
                        Some(gain) => gain * x,
                        None => x,
                    });
            }
        }
    }
}

impl Render {
    /// FNV-1a 64 of `log.jsonl`.
    pub fn log_fnv1a64(&self) -> u64 {
        fnv1a64(&self.log)
    }

    /// FNV-1a 64 of the samples as `audio.wav` stores them: each converted to
    /// f32 by the render's [`conversion`](Render::conversion), little-endian,
    /// interleaved.
    pub fn audio_fnv1a64(&self) -> u64 {
        let mut hasher = Fnv1a64::new();
        for &sample in &self.samples {
            hasher.update(&self.conversion.sample(sample).to_le_bytes());
        }
        hasher.finish()
    }

    /// The samples its [`conversion`](Render::conversion) clipped, counted as
    /// the real-time runner counts them ([`Runner::clipped`](crate::Runner::clipped)):
    /// under [`Conversion::Device`], every clamped or non-finite sample; none
    /// under [`Conversion::Plain`].
    pub fn clipped(&self) -> u64 {
        let mut clipped = 0;
        if self.conversion == Conversion::Device {
            for &sample in &self.samples {
                to_device(sample, &mut clipped);
            }
        }
        clipped
    }

    /// The manifest of this render: `provenance` plus what the engine knows.
    pub fn manifest(&self, provenance: Provenance) -> Manifest {
        Manifest {
            provenance,
            seed: self.seed,
            sample_rate: self.sample_rate,
            frames: self.frames,
            debug_assertions: cfg!(debug_assertions),
            log_fnv1a64: format!("{:016x}", self.log_fnv1a64()),
            audio_fnv1a64: format!("{:016x}", self.audio_fnv1a64()),
            same_machine: self.same_machine.clone(),
            ideal: self.ideal,
            device_conversion: self.conversion == Conversion::Device,
            trust_provenance: self.trust_provenance,
            compute_devices: self.compute_devices.clone(),
            placement: self.placement.clone(),
        }
    }

    /// Writes `audio.wav` (IEEE float 32-bit at the sample rate), `log.jsonl`
    /// and `manifest.json` into `dir`, creating it if needed, and returns the
    /// manifest.
    pub fn write(&self, dir: &Path, provenance: Provenance) -> Result<Manifest, RenderError> {
        fs::create_dir_all(dir)?;
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut wav = hound::WavWriter::create(dir.join("audio.wav"), spec)?;
        for &sample in &self.samples {
            wav.write_sample(self.conversion.sample(sample))?;
        }
        wav.finalize()?;
        fs::write(dir.join("log.jsonl"), &self.log)?;
        let manifest = self.manifest(provenance);
        let mut json = serde_json::to_vec_pretty(&manifest)?;
        json.push(b'\n');
        fs::write(dir.join("manifest.json"), json)?;
        Ok(manifest)
    }
}

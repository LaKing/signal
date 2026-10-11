//! The offline driver, the reference implementation (ADR 0012): renders N frames,
//! records the audio sink's inputs and the event log, and writes `audio.wav`,
//! `log.jsonl` and `manifest.json`. Not real-time code: it allocates and writes
//! files; [`Engine::run_frame`] does neither.

use std::fs;
use std::path::Path;

use signal_core::{Fnv1a64, Frame, InstanceAddress, PortAddress, fnv1a64};

use crate::engine::Engine;
use crate::error::{EngineError, RenderError};
use crate::log::InstanceId;
use crate::manifest::{Manifest, Provenance};

/// The name of the sink parameter the driver applies to every recorded channel.
const GAIN: &str = "gain";

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
pub fn render(engine: &mut Engine, frames: u64) -> Result<Render, RenderError> {
    render_with(engine, frames, |_| Ok(()))
}

/// [`render`], calling `before` before each frame runs, so that it can commit
/// a swap or push input effective from [`Engine::next_frame`] (ADR 0006). Each
/// frame's log is drained after it is recorded, as a swap requires.
///
/// The audio sink is looked up again after every swap. A swapped-in plan must
/// keep exactly one audio sink at the same address with as many signal inputs;
/// otherwise the render stops with [`RenderError::SinkChanged`]. An error from
/// `before` stops it with [`RenderError::Edit`].
pub fn render_with(
    engine: &mut Engine,
    frames: u64,
    mut before: impl FnMut(&mut Engine) -> Result<(), EngineError>,
) -> Result<Render, RenderError> {
    if engine.next_frame() != Frame::ZERO {
        return Err(RenderError::AlreadyStarted);
    }
    let sinks = engine.audio_sinks();
    let (mut sink, address) = match sinks.as_slice() {
        [sink] => match engine.instance_address(*sink) {
            Some(address) => (*sink, address.clone()),
            None => return Err(RenderError::NoAudioSink),
        },
        [] => return Err(RenderError::NoAudioSink),
        several => {
            let instances = several
                .iter()
                .filter_map(|&s| engine.instance_address(s).cloned())
                .collect();
            return Err(RenderError::SeveralAudioSinks { instances });
        }
    };
    let width = engine.instance_inputs(sink).len();
    let channels = match u16::try_from(width) {
        Ok(channels) if channels > 0 => channels,
        _ => {
            return Err(RenderError::AudioChannels {
                instance: address,
                channels: width,
            });
        }
    };

    // The sink's `gain` parameter, if its type declares one.
    let gain = PortAddress::new(&address, GAIN)
        .ok()
        .filter(|port| engine.parameter(port).is_some());

    let mut samples = Vec::new();
    let mut log = Vec::new();
    let mut records = Vec::new();
    let mut version = engine.version();
    for _ in 0..frames {
        before(engine).map_err(RenderError::Edit)?;
        if engine.version() != version {
            version = engine.version();
            sink = same_sink(engine, &address, width)?;
        }
        engine.run_frame();
        let inputs = engine.instance_inputs(sink);
        match gain.as_ref().and_then(|port| engine.parameter(port)) {
            Some(gain) => samples.extend(inputs.iter().map(|x| gain.effective * x)),
            None => samples.extend_from_slice(inputs),
        }
        records.clear();
        records.extend_from_slice(engine.frame_log());
        records.sort_by_key(|record| record.order());
        for record in &records {
            if let Some(line) = engine.log_line(record) {
                serde_json::to_writer(&mut log, &line)?;
                log.push(b'\n');
            }
        }
        engine.drain_log(&mut |_| {});
    }

    let config = engine.config();
    Ok(Render {
        sample_rate: config.sample_rate,
        seed: config.seed,
        frames,
        channels,
        samples,
        log,
    })
}

/// The audio sink after a swap: it must be the only one, at `address`, with
/// `width` signal inputs.
fn same_sink(
    engine: &Engine,
    address: &InstanceAddress,
    width: usize,
) -> Result<InstanceId, RenderError> {
    let sinks = engine.audio_sinks();
    let after: Vec<InstanceAddress> = sinks
        .iter()
        .filter_map(|&s| engine.instance_address(s).cloned())
        .collect();
    match sinks.as_slice() {
        [sink]
            if engine.instance_address(*sink) == Some(address)
                && engine.instance_inputs(*sink).len() == width =>
        {
            Ok(*sink)
        }
        _ => Err(RenderError::SinkChanged {
            before: address.clone(),
            after,
            channels: width,
        }),
    }
}

impl Render {
    /// FNV-1a 64 of `log.jsonl`.
    pub fn log_fnv1a64(&self) -> u64 {
        fnv1a64(&self.log)
    }

    /// FNV-1a 64 of the samples as `audio.wav` stores them: each converted to
    /// f32 (round to nearest, ties to even), little-endian, interleaved.
    pub fn audio_fnv1a64(&self) -> u64 {
        let mut hasher = Fnv1a64::new();
        for &sample in &self.samples {
            hasher.update(&(sample as f32).to_le_bytes());
        }
        hasher.finish()
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
            wav.write_sample(sample as f32)?;
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

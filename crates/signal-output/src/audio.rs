//! The audio adapter's policy (ADR 0022): which sink is the audio sink, its
//! `gain`, and the conversion to the device's `f32`. Shared by the offline
//! render and the real-time runner.

use std::fmt;

use signal_core::{Frame, InstanceAddress, PortAddress, SinkKind};
use signal_engine::{Engine, EngineError, EngineFailure, InstanceId};

/// The name of the sink parameter applied to every recorded channel.
const GAIN: &str = "gain";

/// One audio-sink channel over one callback, after the f32 conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    /// The callback's first frame.
    pub first_frame: Frame,
    /// The callback's number of frames.
    pub frames: u64,
    /// The sink channel (its signal input, in port order).
    pub channel: usize,
    /// The largest magnitude.
    pub peak: f32,
    /// The sum of the squared samples.
    pub sum_squares: f64,
}

/// A sample as a device receives it: finite samples clamped to [−1, 1] and
/// anything else 0, then rounded to `f32`. The real-time runner's
/// conversion, and an exact replay's (M2.7-4).
pub fn device_sample(x: f64) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0) as f32
    } else {
        0.0
    }
}

/// A sample for the device ([`device_sample`]); a clamped or non-finite one
/// counts as clipped.
pub(crate) fn to_device(x: f64, clipped: &mut u64) -> f32 {
    let sample = device_sample(x);
    if !x.is_finite() || x.clamp(-1.0, 1.0) != x {
        *clipped = clipped.saturating_add(1);
    }
    sample
}

/// How a render's samples become the `f32` of `audio.wav` (M2.7-4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conversion {
    /// Each sample rounded to `f32` (round to nearest, ties to even): every
    /// offline render.
    #[default]
    Plain,
    /// As the real-time runner hands samples to the device
    /// ([`device_sample`]): the replay of a session that records this
    /// conversion, or (recorded before it) its boundary outcomes, so that it
    /// equals the live recording bit for bit.
    Device,
}

impl Conversion {
    /// `sample` as `audio.wav` stores it.
    pub fn sample(self, sample: f64) -> f32 {
        match self {
            Conversion::Plain => sample as f32,
            Conversion::Device => device_sample(sample),
        }
    }

    /// Its name in a session's `conversion` (`docs/session-format.md` §2).
    pub fn name(self) -> &'static str {
        match self {
            Conversion::Plain => "plain",
            Conversion::Device => "device",
        }
    }

    /// The conversion named `name`, if any.
    pub fn from_name(name: &str) -> Option<Conversion> {
        [Conversion::Plain, Conversion::Device]
            .into_iter()
            .find(|c| c.name() == name)
    }
}

/// The audio sink a run plays: its instance, address, width and `gain`.
#[derive(Clone, Debug)]
pub(crate) struct AudioSink {
    pub(crate) id: InstanceId,
    pub(crate) address: InstanceAddress,
    pub(crate) width: usize,
    /// `<sink>.gain`, built once at selection (it allocates). The address
    /// outlives a swap; whether it names a parameter does not.
    gain_port: Option<PortAddress>,
    /// The gain's parameter index in the running plan, if the sink's type
    /// there declares it: rebound with the id on every plan, since a swap
    /// may change the sink's type at the same address and width.
    gain: Option<usize>,
}

/// The running plan's audio sinks.
fn audio_sinks(engine: &Engine) -> impl Iterator<Item = signal_engine::Sink<'_>> {
    engine.sinks().filter(|sink| sink.kind == SinkKind::Audio)
}

impl AudioSink {
    /// The plan's one audio sink with `1..=most` signal inputs.
    pub(crate) fn select(engine: &Engine, most: usize) -> Result<AudioSink, RenderError> {
        let sinks: Vec<_> = audio_sinks(engine).collect();
        let sink = match sinks.as_slice() {
            [sink] => *sink,
            [] => return Err(RenderError::NoAudioSink),
            several => {
                let instances = several.iter().map(|s| s.address.clone()).collect();
                return Err(RenderError::SeveralAudioSinks { instances });
            }
        };
        if sink.inputs == 0 || sink.inputs > most {
            return Err(RenderError::AudioChannels {
                instance: sink.address.clone(),
                channels: sink.inputs,
            });
        }
        let mut selected = AudioSink {
            id: sink.id,
            address: sink.address.clone(),
            width: sink.inputs,
            gain_port: PortAddress::new(sink.address, GAIN).ok(),
            gain: None,
        };
        selected.rebind(engine);
        Ok(selected)
    }

    /// Resolves the gain in the running plan. Allocates nothing.
    fn rebind(&mut self, engine: &Engine) {
        self.gain = self
            .gain_port
            .as_ref()
            .and_then(|port| engine.parameter_index(port));
    }

    /// After a swap offline: the plan must keep the sink, the only one, at
    /// its address, as wide; otherwise the error.
    pub(crate) fn same(&mut self, engine: &Engine) -> Result<(), RenderError> {
        let sinks: Vec<_> = audio_sinks(engine).collect();
        match sinks.as_slice() {
            [sink] if *sink.address == self.address && sink.inputs == self.width => {
                self.id = sink.id;
                self.rebind(engine);
                Ok(())
            }
            _ => Err(RenderError::SinkChanged {
                before: self.address.clone(),
                after: sinks.iter().map(|s| s.address.clone()).collect(),
                channels: self.width,
            }),
        }
    }

    /// After a swap in real time (the Controller refused a plan without the
    /// run's sinks, so its address and width hold): the plan's first audio
    /// sink and its gain. Allocates nothing.
    pub(crate) fn follow(&mut self, engine: &Engine) {
        if let Some(sink) = audio_sinks(engine).next() {
            self.id = sink.id;
        }
        self.rebind(engine);
    }

    /// The gain's parameter index in the running plan.
    pub(crate) fn gain_index(&self) -> Option<usize> {
        self.gain
    }
}

/// Why an offline render failed, or the audio adapter refused a plan.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    /// The engine has already run frames; a render starts at frame 0.
    AlreadyStarted,
    /// The plan has no audio sink instance.
    NoAudioSink,
    /// The plan has more than one audio sink instance (M1 supports one).
    SeveralAudioSinks {
        /// The sinks, in address order.
        instances: Vec<InstanceAddress>,
    },
    /// The audio sink has no signal inputs, or more than a WAV file can hold.
    AudioChannels {
        /// The sink.
        instance: InstanceAddress,
        /// Its number of signal inputs.
        channels: usize,
    },
    /// A swap during [`render_with`](crate::render_with) did not keep the
    /// audio sink: exactly one, at the same address, as wide.
    SinkChanged {
        /// The sink the render started with.
        before: InstanceAddress,
        /// The audio sinks of the swapped-in plan.
        after: Vec<InstanceAddress>,
        /// The number of signal inputs the render records.
        channels: usize,
    },
    /// The caller's step before a frame of [`render_with`](crate::render_with)
    /// failed: a swap or an edit was refused.
    Edit(EngineError),
    /// The engine failed and stopped (`docs/semantics.md` §14): nothing of
    /// the render is kept.
    EngineFailed(EngineFailure),
    /// Writing an artifact failed.
    Io(std::io::Error),
    /// Writing `audio.wav` failed.
    Wav(hound::Error),
    /// Serializing a log line or the manifest failed.
    Json(serde_json::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::AlreadyStarted => f.write_str("a render must start at frame 0"),
            RenderError::NoAudioSink => f.write_str("the plan has no audio sink"),
            RenderError::SeveralAudioSinks { instances } => {
                f.write_str("the plan has several audio sinks:")?;
                for instance in instances {
                    write!(f, " {instance}")?;
                }
                Ok(())
            }
            RenderError::AudioChannels { instance, channels } => write!(
                f,
                "{instance}: an audio sink needs 1 to 65535 signal inputs, it has {channels}"
            ),
            RenderError::SinkChanged {
                before,
                after,
                channels,
            } => {
                write!(
                    f,
                    "a swap must keep the audio sink {before} with {channels} inputs; the new plan's audio sinks:"
                )?;
                for instance in after {
                    write!(f, " {instance}")?;
                }
                Ok(())
            }
            RenderError::Edit(error) => write!(f, "applying an edit: {error}"),
            RenderError::EngineFailed(failure) => write!(f, "{failure}"),
            RenderError::Io(error) => write!(f, "writing render artifacts: {error}"),
            RenderError::Wav(error) => write!(f, "writing audio.wav: {error}"),
            RenderError::Json(error) => write!(f, "serializing render artifacts: {error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Io(error) => Some(error),
            RenderError::Wav(error) => Some(error),
            RenderError::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RenderError {
    fn from(error: std::io::Error) -> RenderError {
        RenderError::Io(error)
    }
}

impl From<hound::Error> for RenderError {
    fn from(error: hound::Error) -> RenderError {
        RenderError::Wav(error)
    }
}

impl From<serde_json::Error> for RenderError {
    fn from(error: serde_json::Error) -> RenderError {
        RenderError::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Out-of-range samples are clamped, non-finite ones written as 0; all
    /// are counted.
    #[test]
    fn non_finite_samples_are_written_as_zero_and_counted() {
        let mut clipped = 0;
        assert_eq!(to_device(0.5, &mut clipped), 0.5);
        assert_eq!(to_device(1.5, &mut clipped), 1.0);
        assert_eq!(to_device(-1.5, &mut clipped), -1.0);
        assert_eq!(to_device(f64::INFINITY, &mut clipped), 0.0);
        assert_eq!(to_device(f64::NEG_INFINITY, &mut clipped), 0.0);
        assert_eq!(to_device(f64::NAN, &mut clipped), 0.0);
        assert_eq!(clipped, 5);
    }
}

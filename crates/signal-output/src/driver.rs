//! Signal's driver trait (ADR 0011) and the null driver, which runs a
//! [`Runner`] without hardware: callback sizes drawn from a seeded stream,
//! synchronously or on its own thread, output captured.

use std::fmt;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::{Producer, RingBuffer};
use signal_core::SplitMix64;

use crate::runner::Runner;

/// The most frames the null driver asks for in one callback.
pub const MAX_CALLBACK_FRAMES: usize = 4096;

/// A source of audio callbacks: CoreAudio (`signal-audio`), or the
/// [`NullDriver`]. The [`Runner`] is moved into the driver by
/// [`start`](Driver::start) and handed back by [`stop`](Driver::stop), so
/// that it is dropped on the control thread.
///
/// The callback contract: each callback calls [`Runner::process`] once with
/// `out.len() == frames × channels()`, 1 ≤ `frames` ≤
/// [`max_frames`](Driver::max_frames); `process` never blocks, and nothing
/// else touches the Runner while the driver runs.
pub trait Driver {
    /// The frames per second the driver runs at.
    fn sample_rate(&self) -> u32;
    /// The device channels of each callback.
    fn channels(&self) -> usize;
    /// The most frames of one callback.
    fn max_frames(&self) -> usize;
    /// Takes the Runner and starts calling it.
    fn start(&mut self, runner: Runner) -> Result<(), DriverError>;
    /// Stops calling the Runner and hands it back (`None` if never started).
    fn stop(&mut self) -> Result<Option<Runner>, DriverError>;
}

/// Why a driver could not start or stop.
#[derive(Debug)]
pub enum DriverError {
    /// It already holds a Runner.
    Running,
    /// It holds no Runner, or is running it on its own thread.
    NotStarted,
    /// Its thread could not be started.
    Spawn(std::io::Error),
    /// Its thread panicked; the Runner is lost.
    Panicked,
    /// The audio device failed: its stream could not be built or started, or
    /// it stopped calling back before handing the Runner back.
    Device(String),
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverError::Running => f.write_str("the driver is already running"),
            DriverError::NotStarted => f.write_str("the driver holds no runner to call here"),
            DriverError::Spawn(error) => write!(f, "the driver thread did not start: {error}"),
            DriverError::Panicked => f.write_str("the driver thread panicked"),
            DriverError::Device(message) => write!(f, "the audio device failed: {message}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// The primes [`CallbackSizes`] draws from.
const PRIMES: [usize; 8] = [3, 7, 31, 127, 509, 1021, 2039, 4093];

/// Seeded callback sizes from 1 to [`MAX_CALLBACK_FRAMES`]: 1, then a prime,
/// then a power of two (2 to 4096), then a mix of powers of two (1 to 4096),
/// primes and uniform draws.
#[derive(Clone, Debug)]
pub struct CallbackSizes {
    rng: SplitMix64,
    drawn: u64,
}

impl CallbackSizes {
    /// The sizes of `seed`.
    pub fn new(seed: u64) -> CallbackSizes {
        CallbackSizes {
            rng: SplitMix64::new(seed),
            drawn: 0,
        }
    }

    /// The next size.
    pub fn next_size(&mut self) -> usize {
        let r = self.rng.next_u64();
        let drawn = self.drawn;
        self.drawn = drawn.saturating_add(1);
        // 2^k for k from `from` to 12.
        let power = |r: u64, from: u64| 1usize << (from + r % (13 - from));
        let prime = |r: u64| {
            let k = (r % PRIMES.len() as u64) as usize;
            PRIMES.get(k).copied().unwrap_or(1)
        };
        match drawn {
            0 => 1,
            1 => prime(r),
            2 => power(r, 1),
            _ => match r % 4 {
                0 => power(r >> 2, 0),
                1 => prime(r >> 2),
                _ => 1 + ((r >> 2) % MAX_CALLBACK_FRAMES as u64) as usize,
            },
        }
    }
}

impl Iterator for CallbackSizes {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        Some(self.next_size())
    }
}

/// What a threaded null driver's thread hands back.
type Finished = (Runner, Vec<f32>, CallbackSizes);

/// A [`Driver`] without hardware. Synchronous by default: after
/// [`start`](Driver::start), [`run`](NullDriver::run) calls the Runner on the
/// caller's thread. [`threaded`](NullDriver::threaded) makes `start` spawn a
/// thread that calls it until [`stop`](Driver::stop), told through a
/// one-slot `rtrb` queue checked between callbacks; the Runner and the output
/// come back through the thread's join.
pub struct NullDriver {
    sample_rate: u32,
    channels: usize,
    sizes: CallbackSizes,
    threaded: bool,
    pace: bool,
    runner: Option<Runner>,
    buffer: Vec<f32>,
    output: Vec<f32>,
    thread: Option<(Producer<()>, JoinHandle<Finished>)>,
}

impl NullDriver {
    /// A synchronous null driver with `channels` device channels and callback
    /// sizes from `seed`.
    pub fn new(sample_rate: u32, channels: usize, seed: u64) -> NullDriver {
        NullDriver {
            sample_rate,
            channels,
            sizes: CallbackSizes::new(seed),
            threaded: false,
            pace: false,
            runner: None,
            buffer: vec![0.0; MAX_CALLBACK_FRAMES * channels],
            output: Vec::new(),
            thread: None,
        }
    }

    /// Runs the Runner on its own thread from `start` to `stop`. With `pace`,
    /// the thread sleeps for each callback's duration after it, as a device
    /// would wait; without, it runs as fast as it can.
    pub fn threaded(mut self, pace: bool) -> NullDriver {
        self.threaded = true;
        self.pace = pace;
        self
    }

    /// Synchronous mode: runs callbacks until `frames` frames have run (the
    /// last one shortened to fit), calling `between` before each one and
    /// appending the output to [`output`](NullDriver::output).
    pub fn run(&mut self, frames: u64, between: impl FnMut()) -> Result<(), DriverError> {
        let mut output = std::mem::take(&mut self.output);
        let result = self.run_with(frames, between, |callback| {
            output.extend_from_slice(callback);
        });
        self.output = output;
        result
    }

    /// [`run`](NullDriver::run), handing each callback's output to `capture`
    /// instead of keeping it.
    pub fn run_with(
        &mut self,
        frames: u64,
        mut between: impl FnMut(),
        mut capture: impl FnMut(&[f32]),
    ) -> Result<(), DriverError> {
        let Some(runner) = self.runner.as_mut() else {
            return Err(DriverError::NotStarted);
        };
        let mut left = frames;
        while left > 0 {
            let n = (self.sizes.next_size() as u64).min(left) as usize;
            between();
            let Some(out) = self.buffer.get_mut(..n * self.channels) else {
                break;
            };
            runner.process(out, self.channels);
            capture(out);
            left -= n as u64;
        }
        Ok(())
    }

    /// The captured output, interleaved.
    pub fn output(&self) -> &[f32] {
        &self.output
    }

    /// Takes the captured output.
    pub fn take_output(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.output)
    }
}

impl Driver for NullDriver {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn max_frames(&self) -> usize {
        MAX_CALLBACK_FRAMES
    }

    fn start(&mut self, runner: Runner) -> Result<(), DriverError> {
        if self.runner.is_some() || self.thread.is_some() {
            return Err(DriverError::Running);
        }
        if !self.threaded {
            self.runner = Some(runner);
            return Ok(());
        }
        let (stop, mut stopped) = RingBuffer::<()>::new(1);
        let (channels, sample_rate, pace) = (self.channels, self.sample_rate, self.pace);
        let mut sizes = self.sizes.clone();
        let mut buffer = std::mem::take(&mut self.buffer);
        let mut output = std::mem::take(&mut self.output);
        let thread = thread::Builder::new()
            .name("signal-null-driver".to_owned())
            .spawn(move || {
                let mut runner = runner;
                // Stops on the token, or when the driver is gone without one.
                while stopped.pop().is_err() && !stopped.is_abandoned() {
                    let n = sizes.next_size();
                    if let Some(out) = buffer.get_mut(..n * channels) {
                        runner.process(out, channels);
                        // Between callbacks, never inside `process`.
                        output.extend_from_slice(out);
                    }
                    if pace {
                        thread::sleep(Duration::from_secs_f64(
                            n as f64 / f64::from(sample_rate.max(1)),
                        ));
                    }
                }
                (runner, output, sizes)
            })
            .map_err(DriverError::Spawn)?;
        self.thread = Some((stop, thread));
        Ok(())
    }

    fn stop(&mut self) -> Result<Option<Runner>, DriverError> {
        let Some((mut stop, thread)) = self.thread.take() else {
            return Ok(self.runner.take());
        };
        let _ = stop.push(());
        let (runner, output, sizes) = thread.join().map_err(|_| DriverError::Panicked)?;
        self.output = output;
        self.sizes = sizes;
        self.buffer = vec![0.0; MAX_CALLBACK_FRAMES * self.channels];
        Ok(Some(runner))
    }
}

/// Dropping the driver stops and joins its thread, so the Runner it held is
/// dropped here, on the dropping thread, and nothing keeps running.
impl Drop for NullDriver {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

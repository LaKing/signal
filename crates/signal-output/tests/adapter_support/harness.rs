//! The fake-clock harness (ADR 0052 verification 1): virtual time, an engine
//! producer pushing a source period every `source_period / source_rate`
//! seconds and a fake device pulling a destination period at its own clock,
//! `dest_rate × (1 + ε)`, ε in ppm (constant or a ramp). Each callback runs
//! at its ideal time plus seeded jitter (uniform, ± a fraction of its
//! period), which decides the order of pushes and pulls; each carries its
//! ideal time as its host time (a device's callback time stamp is its
//! clock's, not the thread's). Deterministic: the same scenario gives the
//! same run, bit for bit.
//!
//! The producer pushes silence with a unit impulse (a **marker**) every
//! `marker_seconds`; the harness finds each marker's peak in the output
//! (parabolic interpolation) and records its delay from the marker's source
//! time, in destination frames.

use signal_output::adapter::{AdapterConfig, AdapterState, LoopParams, clock_adapter};

use super::Rng;

/// A run's setting.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Source (engine) rate.
    pub source_rate: u32,
    /// Destination (device) rate, nominal.
    pub dest_rate: u32,
    /// Source period.
    pub source_period: usize,
    /// Destination period.
    pub dest_period: usize,
    /// The device's clock error at the start, ppm (positive: faster).
    pub epsilon_ppm: f64,
    /// The device's clock error at the end of `ramp_seconds` (a linear
    /// ramp from `epsilon_ppm`), if any.
    pub ramp_to_ppm: Option<f64>,
    /// The ramp's duration.
    pub ramp_seconds: f64,
    /// The device's jitter, ± this fraction of its period: a callback runs
    /// between its time stamp and 2 × this many periods after it,
    /// uniformly (a callback never runs before the time its stamp gives,
    /// as CoreAudio's I/O cycle never does), unless `stamp_centre`.
    pub jitter: f64,
    /// The pessimistic variant: the stamp is the centre of the jitter, so
    /// callbacks may run up to `jitter` periods before their stamp.
    pub stamp_centre: bool,
    /// Noise on every host time stamp, ± this many microseconds (uniform):
    /// a host clock's reading of a callback's time is not exact.
    pub stamp_noise_us: f64,
    /// The producer's jitter, ± this fraction of its period.
    pub source_jitter: f64,
    /// The jitter's seed.
    pub seed: u64,
    /// Simulated seconds.
    pub seconds: f64,
    /// Host times on pushes and pulls (the linearised fill); without, the
    /// raw fill.
    pub timestamps: bool,
    /// A marker impulse this often, seconds; `None`: none.
    pub marker_seconds: Option<f64>,
    /// The loop's constants.
    pub params: LoopParams,
    /// Record per-pull samples from this simulated time on.
    pub record_from: f64,
}

impl Scenario {
    /// A scenario at the given rates and periods: ε = 0, jitter ±0.5 device
    /// period, seed 1, 60 s, host times on, no markers, the default loop.
    pub fn new(
        source_rate: u32,
        dest_rate: u32,
        source_period: usize,
        dest_period: usize,
    ) -> Scenario {
        Scenario {
            source_rate,
            dest_rate,
            source_period,
            dest_period,
            epsilon_ppm: 0.0,
            ramp_to_ppm: None,
            ramp_seconds: 0.0,
            jitter: 0.5,
            source_jitter: 0.0,
            stamp_centre: false,
            stamp_noise_us: 0.0,
            seed: 1,
            seconds: 60.0,
            timestamps: true,
            marker_seconds: None,
            params: LoopParams::default(),
            record_from: 0.0,
        }
    }

    /// The device's clock error at `t`, a fraction.
    fn epsilon(&self, t: f64) -> f64 {
        let ppm = match self.ramp_to_ppm {
            Some(end) if self.ramp_seconds > 0.0 => {
                self.epsilon_ppm + (end - self.epsilon_ppm) * (t / self.ramp_seconds).min(1.0)
            }
            Some(end) => end,
            None => self.epsilon_ppm,
        };
        ppm * 1e-6
    }
}

/// One pull's record.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// The pull's ideal time, seconds.
    pub t: f64,
    /// The low-passed fill error, frames.
    pub error: f64,
    /// The correction, ppm.
    pub correction_ppm: f64,
    /// The correction that cancels the device's clock error then, ppm
    /// (1/(1 + ε) − 1: a faster device needs a smaller step).
    pub epsilon_ppm: f64,
    /// Locked (track).
    pub locked: bool,
}

/// What a run gave.
#[derive(Clone, Debug)]
pub struct Run {
    /// When conversion started, seconds.
    pub started_at: Option<f64>,
    /// When the loop entered track, seconds.
    pub track_at: Option<f64>,
    /// The last time before track the error was outside the lock window
    /// (|e| > 2): lock was reached then and held, seconds.
    pub lock_at: Option<f64>,
    /// Per pull, from `record_from`.
    pub samples: Vec<Sample>,
    /// Marker delays, destination frames, in order.
    pub marker_delays: Vec<f64>,
    /// The final status.
    pub status: signal_output::adapter::AdapterStatus,
    /// Frames the producer dropped (full ring).
    pub dropped_frames: u64,
    /// Pulls.
    pub pulls: u64,
    /// The adapter's latency, source frames.
    pub latency_frames: f64,
}

impl Run {
    /// Seconds from the start of conversion to lock.
    pub fn lock_seconds(&self) -> Option<f64> {
        Some(self.lock_at? - self.started_at?)
    }

    /// Track statistics from `from` seconds on: (mean |e|, rms of the
    /// correction minus the true ε, ppm; peak |correction − ε|, ppm).
    pub fn track_stats(&self, from: f64) -> (f64, f64, f64) {
        let window: Vec<&Sample> = self.samples.iter().filter(|s| s.t >= from).collect();
        let n = window.len().max(1) as f64;
        let mean_e = window.iter().map(|s| s.error.abs()).sum::<f64>() / n;
        let rms = (window
            .iter()
            .map(|s| (s.correction_ppm - s.epsilon_ppm).powi(2))
            .sum::<f64>()
            / n)
            .sqrt();
        let peak = window
            .iter()
            .map(|s| (s.correction_ppm - s.epsilon_ppm).abs())
            .fold(0.0, f64::max);
        (mean_e, rms, peak)
    }

    /// The rms of the correction about its own mean from `from` on, ppm
    /// (the ratio's jitter, whatever its offset).
    pub fn correction_jitter(&self, from: f64) -> f64 {
        let window: Vec<f64> = self
            .samples
            .iter()
            .filter(|s| s.t >= from)
            .map(|s| s.correction_ppm - s.epsilon_ppm)
            .collect();
        let n = window.len().max(1) as f64;
        let mean = window.iter().sum::<f64>() / n;
        (window.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / n).sqrt()
    }

    /// The spread (max − min) of the marker delays after the first `skip`.
    pub fn marker_spread(&self, skip: usize) -> f64 {
        let delays = self.marker_delays.iter().skip(skip);
        let max = delays.clone().fold(f64::MIN, |a, &b| a.max(b));
        let min = delays.fold(f64::MAX, |a, &b| a.min(b));
        max - min
    }
}

/// Runs a scenario.
pub fn run(s: &Scenario) -> Run {
    let config = AdapterConfig {
        loop_params: s.params,
        ..AdapterConfig::new(
            1,
            s.source_rate,
            s.dest_rate,
            s.source_period,
            s.dest_period,
        )
    };
    let (mut input, mut output) = clock_adapter(config).unwrap();
    let latency_frames = output.latency_frames();
    let mut rng = Rng::new(s.seed);
    let source_dt = s.source_period as f64 / f64::from(s.source_rate);
    let marker_every = s
        .marker_seconds
        .map(|m| (m * f64::from(s.source_rate)).round() as u64);

    let mut push_buffer = vec![0.0f32; s.source_period];
    let mut pull_buffer = vec![0.0f32; s.dest_period];
    let mut pushes = 0u64;
    let mut pushed_frames = 0u64;
    // The device's ideal callback times.
    let mut device_t = 0.0f64;
    // A callback's run time after its stamp, in periods.
    let offset = |rng: &mut Rng, jitter: f64| {
        let u = rng.symmetric() * jitter;
        if s.stamp_centre { u } else { u + jitter }
    };
    let mut device_jitter = offset(&mut rng, s.jitter);
    let mut source_jitter = offset(&mut rng, s.source_jitter);

    let mut run = Run {
        started_at: None,
        track_at: None,
        lock_at: None,
        samples: Vec::new(),
        marker_delays: Vec::new(),
        status: output.status(),
        dropped_frames: 0,
        pulls: 0,
        latency_frames,
    };
    // The marker finder: the last two output samples and their times.
    let mut prev = [(0.0f32, 0.0f64); 2];
    let mut markers_found = 0u64;
    let mut last_outside: Option<f64> = None;

    loop {
        let source_t = pushes as f64 * source_dt;
        let device_period_s =
            s.dest_period as f64 / (f64::from(s.dest_rate) * (1.0 + s.epsilon(device_t)));
        let source_exec = source_t + source_jitter * source_dt;
        let device_exec = device_t + device_jitter * device_period_s;
        if source_t.min(device_t) > s.seconds {
            break;
        }
        if source_exec <= device_exec {
            push_buffer.fill(0.0);
            if let Some(every) = marker_every {
                for (i, x) in push_buffer.iter_mut().enumerate() {
                    let n = pushed_frames + i as u64;
                    if n > 0 && n.is_multiple_of(every) {
                        *x = 1.0;
                    }
                }
            }
            let noise = rng.symmetric() * s.stamp_noise_us * 1e-6;
            let time = s
                .timestamps
                .then(|| ((source_t + noise) * 1e9).round() as u64);
            input.push(&push_buffer, time);
            pushes += 1;
            pushed_frames += s.source_period as u64;
            source_jitter = offset(&mut rng, s.source_jitter);
        } else {
            let noise = rng.symmetric() * s.stamp_noise_us * 1e-6;
            let time = s
                .timestamps
                .then(|| ((device_t + noise) * 1e9).round() as u64);
            output.pull(&mut pull_buffer, time);
            run.pulls += 1;
            let status = output.status();
            if run.started_at.is_none() && status.state != AdapterState::Filling {
                run.started_at = Some(device_t);
            }
            let locked = status.state == AdapterState::Locked;
            if run.track_at.is_none() {
                if locked {
                    run.track_at = Some(device_t);
                    run.lock_at = last_outside.or(run.started_at);
                } else if status.state == AdapterState::Locking
                    && status.fill_error.abs() > s.params.lock_frames
                {
                    last_outside = Some(device_t);
                }
            }
            if device_t >= s.record_from {
                run.samples.push(Sample {
                    t: device_t,
                    error: status.fill_error,
                    correction_ppm: status.correction_ppm,
                    epsilon_ppm: (1.0 / (1.0 + s.epsilon(device_t)) - 1.0) * 1e6,
                    locked,
                });
            }
            if let Some(every) = marker_every {
                let frame_s = device_period_s / s.dest_period as f64;
                for (j, &y) in pull_buffer.iter().enumerate() {
                    let t = device_t + j as f64 * frame_s;
                    let (p2, p1) = (prev[0], prev[1]);
                    if p1.0 > 0.3 && p1.0 >= p2.0 && p1.0 > y {
                        let (a, b, c) = (f64::from(p2.0), f64::from(p1.0), f64::from(y));
                        let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
                        let peak_t = p1.1 + delta * frame_s;
                        markers_found += 1;
                        // Markers are found in order; skip any lost during
                        // a stop by matching the nearest source time.
                        let source_s = (peak_t - latency_frames / f64::from(s.source_rate))
                            * f64::from(s.source_rate);
                        let k = (source_s / every as f64).round().max(1.0);
                        let marker_t = k * every as f64 / f64::from(s.source_rate);
                        run.marker_delays
                            .push((peak_t - marker_t) * f64::from(s.dest_rate));
                    }
                    prev = [p1, (y, t)];
                }
            }
            device_t += device_period_s;
            device_jitter = offset(&mut rng, s.jitter);
        }
    }
    let _ = markers_found;
    run.status = output.status();
    run.dropped_frames = input.dropped_frames();
    run
}

//! Real-time wake policies (M2.7-1, `docs/realtime-rules.md` "Wake
//! policies"): how idle workers wait between jobs and how they are woken, the
//! time-constraint computation they are given, and the wake-up trace. Timing
//! only: no policy changes a sample or a log record (ADR 0019 §2).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// How idle workers wait and are woken in real time. The default is
/// [`RtPolicy::Park50`], the behaviour before M2.7-1, until the operator
/// decides from device measurements (`docs/benchmarks.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RtPolicy {
    /// Spin 50 µs after the last job or wake, then park. A parallel segment
    /// wakes one parked worker; each worker that joins wakes up to two more.
    #[default]
    Park50,
    /// As `Park50`, but an idle worker spins until 0.9 × its computation
    /// budget has passed since it last resumed (at most 3 ms), then parks.
    BudgetSpin,
    /// As `Park50`, but workers block on a semaphore of their own (a Mach
    /// semaphore through the driver's [`Waiter`]) and the audio thread
    /// signals every parked worker at the start of each callback that can
    /// run in parallel.
    Semaphore,
    /// As `Park50`, but at the start of each callback that can run in
    /// parallel the audio thread wakes two parked workers, and each worker
    /// that resumes wakes two more, once per callback.
    Fanout,
}

impl RtPolicy {
    /// Every policy, in documentation order.
    pub const ALL: [RtPolicy; 4] = [
        RtPolicy::Park50,
        RtPolicy::BudgetSpin,
        RtPolicy::Semaphore,
        RtPolicy::Fanout,
    ];

    /// Its name on the command line and in the Setup.
    pub fn name(self) -> &'static str {
        match self {
            RtPolicy::Park50 => "park50",
            RtPolicy::BudgetSpin => "budget-spin",
            RtPolicy::Semaphore => "semaphore",
            RtPolicy::Fanout => "fanout",
        }
    }

    /// The policy named `name`.
    pub fn from_name(name: &str) -> Option<RtPolicy> {
        RtPolicy::ALL.into_iter().find(|p| p.name() == name)
    }
}

impl fmt::Display for RtPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The time-constraint computation of a real-time thread, as a fraction of
/// the device period, in parts per million (an integer, so that the
/// configurations holding it stay `Eq`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Computation(u32);

impl Computation {
    /// A quarter of the period: the computation before M2.7-1.
    pub const DEFAULT: Computation = Computation(250_000);

    /// `fraction` of the period, from more than 0 to 1.
    pub fn from_fraction(fraction: f64) -> Option<Computation> {
        (fraction.is_finite() && fraction > 0.0 && fraction <= 1.0)
            .then(|| Computation(((fraction * 1e6).round() as u32).max(1)))
    }

    /// The fraction of the period.
    pub fn fraction(self) -> f64 {
        f64::from(self.0) / 1e6
    }

    /// Parts per million of the period.
    pub fn ppm(self) -> u32 {
        self.0
    }

    /// The computation of a period of `period_ns`, in ns.
    pub fn of_ns(self, period_ns: u64) -> u64 {
        u64::try_from(u128::from(period_ns) * u128::from(self.0) / 1_000_000).unwrap_or(u64::MAX)
    }
}

impl Default for Computation {
    fn default() -> Computation {
        Computation::DEFAULT
    }
}

/// What a real-time run is given beside its engine configuration: the wake
/// policy, the computation budget (`--rt-computation`, a separate knob) and
/// whether the wake-up trace is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RtOptions {
    /// How idle workers wait and are woken.
    pub policy: RtPolicy,
    /// The time-constraint computation of the workers, and `budget-spin`'s
    /// budget.
    pub computation: Computation,
    /// Keep the wake-up trace ([`WakeStats`]); a few clock reads per task.
    pub trace: bool,
}

/// How a worker blocks and is woken under the `semaphore` policy: one
/// semaphore per worker. `signal` must be kept if it comes before the
/// `wait` (a counting semaphore), so that no wake is lost. Neither may
/// allocate or lock; `signal` must not block (it runs on the audio thread).
pub trait Waiter: Send + Sync {
    /// Blocks worker `worker` until it is signalled (or spuriously).
    fn wait(&self, worker: usize);
    /// Wakes worker `worker`, or lets its next `wait` return at once.
    fn signal(&self, worker: usize);
}

/// Makes the [`Waiter`] of `workers` workers (the driver's: Mach semaphores
/// in `signal-audio`), or says why it cannot.
pub type MakeWaiter = fn(usize) -> Result<Arc<dyn Waiter>, String>;

/// Octaves of the trace's histograms (1 ns to about 18 minutes).
const OCTAVES: usize = 40;
/// Buckets per octave.
const PER_OCTAVE: usize = 8;

/// A histogram of durations in ns: eight buckets per octave, and the exact
/// maximum. Atomics only: recording neither allocates nor locks.
pub(crate) struct Histogram {
    buckets: Box<[AtomicU64]>,
    max: AtomicU64,
}

impl Histogram {
    pub(crate) fn new() -> Histogram {
        Histogram {
            buckets: (0..OCTAVES * PER_OCTAVE)
                .map(|_| AtomicU64::new(0))
                .collect(),
            max: AtomicU64::new(0),
        }
    }

    fn bucket(ns: u64) -> usize {
        if ns < PER_OCTAVE as u64 {
            return ns as usize;
        }
        let octave = 63 - ns.leading_zeros() as usize;
        let sub = ((ns >> (octave - 3)) & 7) as usize;
        (octave - 2) * PER_OCTAVE + sub
    }

    /// The largest value of bucket `b`.
    fn upper(b: usize) -> u64 {
        if b < PER_OCTAVE {
            return b as u64;
        }
        let octave = b / PER_OCTAVE + 2;
        let sub = (b % PER_OCTAVE) as u64;
        ((8 + sub + 1) << (octave - 3)) - 1
    }

    pub(crate) fn record(&self, ns: u64) {
        let b = Histogram::bucket(ns).min(self.buckets.len() - 1);
        if let Some(bucket) = self.buckets.get(b) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
        self.max.fetch_max(ns, Ordering::Relaxed);
    }

    /// Adds this histogram's counts into `total` (control-thread code).
    pub(crate) fn add_to(&self, total: &Histogram) {
        for (mine, theirs) in self.buckets.iter().zip(total.buckets.iter()) {
            theirs.fetch_add(mine.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        total
            .max
            .fetch_max(self.max.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// The count, median, p99 (nearest rank, a bucket's largest value, at
    /// most the maximum) and maximum (control-thread code).
    pub(crate) fn spread(&self) -> Spread {
        let counts: Vec<u64> = self
            .buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect();
        let count: u64 = counts.iter().sum();
        let max = self.max.load(Ordering::Relaxed);
        let rank = |p: f64| {
            let k = ((p * count as f64).ceil() as u64).max(1);
            let mut seen = 0;
            for (b, &n) in counts.iter().enumerate() {
                seen += n;
                if seen >= k {
                    return Histogram::upper(b).min(max);
                }
            }
            max
        };
        if count == 0 {
            return Spread::default();
        }
        Spread {
            count,
            median_ns: rank(0.5),
            p99_ns: rank(0.99),
            max_ns: max,
        }
    }
}

/// A distribution of durations: how many, median, p99 and maximum, in ns.
/// The median and p99 are a histogram bucket's largest value (eight buckets
/// per octave, so within 12.5 %); the maximum is exact. Empty is all 0 with
/// `count` 0: a missing observation is never a 0 in a distribution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spread {
    /// Observations.
    pub count: u64,
    /// Their median.
    pub median_ns: u64,
    /// Their 99th percentile.
    pub p99_ns: u64,
    /// The largest.
    pub max_ns: u64,
}

/// The wake-up trace of a real-time run (`RtOptions::trace`), per callback
/// (`docs/realtime-rules.md` "Wake-up trace"). Statistics only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WakeStats {
    /// Callbacks traced.
    pub callbacks: u64,
    /// Callbacks in which no worker joined a job: they add nothing to
    /// `first_join` and `last_join`.
    pub callbacks_without_join: u64,
    /// Wakes of parked workers sent by the audio thread or a worker (the
    /// driver's requests and the shutdown are not counted).
    pub wakes_signalled: u64,
    /// Those wakes whose worker has resumed: `wake` holds their latency.
    /// `wakes_signalled − wakes_observed` were still pending, or (rarely, a
    /// wake interleaved with a driver's request) went unobserved.
    pub wakes_observed: u64,
    /// From a wake to its worker running again, all workers.
    pub wake: Spread,
    /// The same per worker, in worker order.
    pub per_worker: Vec<Spread>,
    /// From the callback's start to the first worker joining a job.
    pub first_join: Spread,
    /// From the callback's start to the last worker joining a job.
    pub last_join: Spread,
    /// The longest task body of each callback that ran a block, on the
    /// workers or on the calling thread alone.
    pub slowest_task: Spread,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for policy in RtPolicy::ALL {
            assert_eq!(RtPolicy::from_name(policy.name()), Some(policy));
        }
        assert_eq!(RtPolicy::from_name("spin"), None);
        assert_eq!(RtPolicy::default(), RtPolicy::Park50);
    }

    #[test]
    fn computation_is_a_fraction_of_the_period() {
        assert_eq!(Computation::DEFAULT.fraction(), 0.25);
        assert_eq!(
            Computation::from_fraction(0.75).map(|c| c.ppm()),
            Some(750_000)
        );
        assert_eq!(
            Computation::from_fraction(1.0).map(|c| c.ppm()),
            Some(1_000_000)
        );
        for bad in [0.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(Computation::from_fraction(bad), None, "{bad}");
        }
        assert_eq!(Computation::DEFAULT.of_ns(1_333_333), 333_333);
    }

    #[test]
    fn histogram_buckets_cover_every_value_in_order() {
        let mut last = 0;
        for ns in (0..100_000).chain([1 << 30, u64::MAX >> 30]) {
            let b = Histogram::bucket(ns);
            assert!(b >= last || ns >= 1 << 30, "{ns}");
            assert!(ns <= Histogram::upper(b), "{ns} above bucket {b}");
            assert!(
                b == 0 || ns > Histogram::upper(b - 1),
                "{ns} below bucket {b}"
            );
            last = b;
        }
    }

    #[test]
    fn spread_reads_percentiles_within_a_bucket() {
        let h = Histogram::new();
        assert_eq!(h.spread(), Spread::default());
        for ns in 1..=1000 {
            h.record(ns * 1000);
        }
        let s = h.spread();
        assert_eq!((s.count, s.max_ns), (1000, 1_000_000));
        assert!((500_000..=500_000 * 9 / 8).contains(&s.median_ns), "{s:?}");
        assert!((990_000..=1_000_000).contains(&s.p99_ns), "{s:?}");
    }
}

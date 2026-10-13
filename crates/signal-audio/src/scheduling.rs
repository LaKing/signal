//! How the engine's worker threads are scheduled in real time (ADR 0019 §2,
//! §4): the time-constraint policy for a device period, the path each worker
//! took, and the match of a cpal device to its CoreAudio object. Plain code,
//! tested without a device; the system calls are in [`rt_thread`](crate::rt_thread).
//! Statistics only: none of it reaches the Runner, the log or the audio.

use std::fmt;

use signal_engine::Computation;

use crate::rt_thread;

/// How one worker thread is scheduled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerPath {
    /// The time-constraint policy is set and the thread joined the output
    /// device's I/O workgroup.
    Workgroup,
    /// The time-constraint policy is set; the thread is not in the workgroup,
    /// for the reason given (none available, or the join failed).
    TimeConstraint(String),
    /// Ordinary scheduling, for the reason given.
    None(String),
}

impl WorkerPath {
    /// `"workgroup"`, `"time-constraint"` or `"none"`.
    pub fn name(&self) -> &'static str {
        match self {
            WorkerPath::Workgroup => "workgroup",
            WorkerPath::TimeConstraint(_) => "time-constraint",
            WorkerPath::None(_) => "none",
        }
    }

    /// Why it is not the workgroup.
    pub fn reason(&self) -> Option<&str> {
        match self {
            WorkerPath::Workgroup => None,
            WorkerPath::TimeConstraint(reason) | WorkerPath::None(reason) => Some(reason),
        }
    }
}

impl fmt::Display for WorkerPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason() {
            Some(reason) => write!(f, "{} ({reason})", self.name()),
            None => f.write_str(self.name()),
        }
    }
}

/// The Mach time-constraint policy of a worker thread, in absolute-time
/// ticks: `period` and `constraint` are the device's period, `computation` a
/// quarter of it (the V12 budget of a callback), within the kernel's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeConstraint {
    /// The period, in ticks.
    pub period: u32,
    /// The computation the thread needs per period, in ticks.
    pub computation: u32,
    /// The time within which it must be done, in ticks.
    pub constraint: u32,
    /// Whether the computation may be interrupted.
    pub preemptible: bool,
}

impl TimeConstraint {
    /// The shortest computation asked for, in ns.
    pub const MIN_COMPUTATION_NS: u64 = 50_000;
    /// The longest computation the kernel accepts, in ns.
    pub const MAX_COMPUTATION_NS: u64 = 50_000_000;

    /// The policy for a device period of `frames` at `rate` frames per
    /// second, with the Mach timebase `(numer, denom)` (one tick is
    /// numer / denom ns).
    pub fn from_period(frames: u32, rate: u32, timebase: (u32, u32)) -> TimeConstraint {
        TimeConstraint::for_period(frames, rate, timebase, Computation::DEFAULT)
    }

    /// [`from_period`](TimeConstraint::from_period) with the computation
    /// `computation` of the period (`--rt-computation`, M2.7-1), within the
    /// kernel's limits. The SDK (`mach/thread_policy.h`) says the kernel
    /// raises a computation below half the constraint to half of it, so any
    /// fraction up to 0.5 asks for the same policy (`docs/realtime-rules.md`
    /// "The policy").
    pub fn for_period(
        frames: u32,
        rate: u32,
        (numer, denom): (u32, u32),
        computation: Computation,
    ) -> TimeConstraint {
        let (numer, denom) = (u128::from(numer.max(1)), u128::from(denom.max(1)));
        let ticks = |ns_num: u128, ns_den: u128| -> u128 {
            // ns_num / ns_den ns, rounded to the nearest tick.
            let num = ns_num * denom;
            let den = ns_den * numer;
            (num + den / 2) / den
        };
        let period = ticks(u128::from(frames) * 1_000_000_000, u128::from(rate.max(1)));
        let min = ticks(u128::from(Self::MIN_COMPUTATION_NS), 1);
        let max = ticks(u128::from(Self::MAX_COMPUTATION_NS), 1);
        let computation =
            (period * u128::from(computation.ppm()) / 1_000_000).clamp(min, max.max(min));
        let constraint = period.max(computation);
        let narrow = |t: u128| u32::try_from(t).unwrap_or(u32::MAX);
        TimeConstraint {
            period: narrow(period),
            computation: narrow(computation),
            constraint: narrow(constraint),
            preemptible: true,
        }
    }
}

/// How a real-time run's worker threads are scheduled: what
/// `signal play` and the app report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheduling {
    /// The threads a block runs on, the audio thread included.
    pub threads: usize,
    /// Each worker's path, in worker order.
    pub workers: Vec<WorkerPath>,
    /// Why the output device's workgroup was not available (`None` when it
    /// was read).
    pub no_workgroup: Option<String>,
    /// The Mach ports of the workers that entered the time-constraint
    /// policy, for [`Demotions`].
    pub ports: Vec<u32>,
}

impl Scheduling {
    /// `workers` ordinary workers, for `reason` (the null driver, no device).
    pub fn ordinary(threads: usize, workers: usize, reason: &str) -> Scheduling {
        Scheduling {
            threads,
            workers: vec![WorkerPath::None(reason.to_owned()); workers],
            no_workgroup: Some(reason.to_owned()),
            ports: Vec::new(),
        }
    }

    /// `"workgroup"`, `"time-constraint"` or `"none"` when every worker took
    /// that path (`"none"` without workers), else `"mixed"`.
    pub fn path(&self) -> &'static str {
        let mut names = self.workers.iter().map(WorkerPath::name);
        match names.next() {
            None => "none",
            Some(first) if names.all(|n| n == first) => first,
            Some(_) => "mixed",
        }
    }

    /// The path for people: `workgroup`, `time-constraint (reason)`,
    /// `none (no worker threads)`, `mixed (6 workgroup, 1 none)`.
    pub fn summary(&self) -> String {
        let Some(first) = self.workers.first() else {
            return "none (no worker threads)".to_owned();
        };
        if self.workers.iter().all(|p| p == first) {
            return first.to_string();
        }
        let path = self.path();
        if path != "mixed" {
            return path.to_owned();
        }
        let counts: Vec<String> = ["workgroup", "time-constraint", "none"]
            .into_iter()
            .filter_map(|name| {
                let n = self.workers.iter().filter(|p| p.name() == name).count();
                (n > 0).then(|| format!("{n} {name}"))
            })
            .collect();
        format!("mixed ({})", counts.join(", "))
    }
}

/// Sampled demotions of real-time workers (M2.7-1). macOS exposes no count
/// of throttling or demotion events to a process: the kernel's real-time
/// fail-safe is only a kdebug trace code (`MACH_FAILSAFE`, SDK
/// `sys/kdebug.h`), readable through `ktrace` as root. So the control thread
/// samples instead: each read of a worker that entered the time-constraint
/// policy that finds [`POLICY_TIMESHARE`](rt_thread::POLICY_TIMESHARE) in
/// effect counts. A demotion between two samples is not seen; a read that
/// fails (the thread has ended) is `unsampled`, never a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Demotions {
    /// Successful reads.
    pub samples: u64,
    /// Successful reads that found the standard (timeshare) policy.
    pub sampled_timeshare: u64,
    /// Reads that failed.
    pub unsampled: u64,
}

impl Demotions {
    /// Reads the policy in effect for each of `ports` once.
    pub fn sample(&mut self, ports: &[u32]) {
        for &port in ports {
            self.count(rt_thread::policy_in_effect(port));
        }
    }

    /// Counts one read.
    pub fn count(&mut self, read: Option<i32>) {
        match read {
            Some(policy) => {
                self.samples += 1;
                if policy == rt_thread::POLICY_TIMESHARE {
                    self.sampled_timeshare += 1;
                }
            }
            None => self.unsampled += 1,
        }
    }
}

/// Why a cpal device was not matched to a CoreAudio object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceMatch {
    /// The system's device list changed while it was read.
    Changed,
    /// The device's UID is not in the list.
    Absent,
}

impl fmt::Display for DeviceMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeviceMatch::Changed => "the device list changed while matching",
            DeviceMatch::Absent => "the device is not in the system's device list",
        })
    }
}

/// The CoreAudio object of the device whose id is `id`. `before` and `after`
/// are the system's device list (`kAudioHardwarePropertyDevices`) read just
/// before and after cpal listed `ids`: cpal lists the same property, one
/// device per object, in order, unfiltered (`Host::devices`), and an entry
/// is `None` where cpal could not read the device's id. The lists must agree
/// in length; only the matched device's position counts.
pub fn match_device(
    before: &[u32],
    ids: &[Option<String>],
    after: &[u32],
    id: &str,
) -> Result<u32, DeviceMatch> {
    if before != after || ids.len() != before.len() {
        return Err(DeviceMatch::Changed);
    }
    ids.iter()
        .zip(before)
        .find(|(candidate, _)| candidate.as_deref() == Some(id))
        .map(|(_, &object)| object)
        .ok_or(DeviceMatch::Absent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apple Silicon's timebase: one tick is 125 / 3 ns (24 MHz).
    const APPLE_SILICON: (u32, u32) = (125, 3);

    #[test]
    fn a_128_frame_period_at_96_khz_is_32000_ticks_with_a_quarter_computed() {
        let policy = TimeConstraint::from_period(128, 96_000, APPLE_SILICON);
        assert_eq!(
            policy,
            TimeConstraint {
                period: 32_000,
                computation: 8_000,
                constraint: 32_000,
                preemptible: true,
            }
        );
    }

    #[test]
    fn a_timebase_of_one_ns_per_tick_counts_nanoseconds() {
        let policy = TimeConstraint::from_period(480, 48_000, (1, 1));
        assert_eq!(policy.period, 10_000_000);
        assert_eq!(policy.computation, 2_500_000);
        assert_eq!(policy.constraint, 10_000_000);
    }

    #[test]
    fn the_computation_is_clamped_to_the_kernel_limits() {
        // 1 frame at 96 kHz: 10.4 µs; the computation is raised to 50 µs and
        // the constraint with it.
        let short = TimeConstraint::from_period(1, 96_000, (1, 1));
        assert_eq!(short.period, 10_417);
        assert_eq!(short.computation, 50_000);
        assert_eq!(short.constraint, 50_000);
        // 4 s: the computation is lowered to 50 ms.
        let long = TimeConstraint::from_period(192_000, 48_000, (1, 1));
        assert_eq!(long.period, 4_000_000_000);
        assert_eq!(long.computation, 50_000_000);
        assert_eq!(long.constraint, 4_000_000_000);
        // A period beyond u32 ticks saturates instead of wrapping.
        let huge = TimeConstraint::from_period(u32::MAX, 1, (1, 1));
        assert_eq!(huge.period, u32::MAX);
    }

    #[test]
    fn a_zero_rate_or_timebase_does_not_divide_by_zero() {
        let policy = TimeConstraint::from_period(128, 0, (0, 0));
        assert!(policy.period > 0 && policy.constraint >= policy.computation);
    }

    /// C4 (M2.7-1): `--rt-computation` sets the computation alone; period
    /// and constraint stay the device period.
    #[test]
    fn the_computation_is_the_fraction_asked_for() {
        for (fraction, ticks) in [(0.25, 8_000), (0.5, 16_000), (0.75, 24_000), (1.0, 32_000)] {
            let computation = Computation::from_fraction(fraction).unwrap();
            let policy = TimeConstraint::for_period(128, 96_000, APPLE_SILICON, computation);
            assert_eq!(policy.computation, ticks, "{fraction}");
            assert_eq!((policy.period, policy.constraint), (32_000, 32_000));
        }
        assert_eq!(
            TimeConstraint::for_period(128, 96_000, APPLE_SILICON, Computation::DEFAULT),
            TimeConstraint::from_period(128, 96_000, APPLE_SILICON)
        );
    }

    /// C6 (M2.7-1): a failed read is unsampled, never a sample.
    #[test]
    fn demotions_count_only_successful_reads() {
        let mut demotions = Demotions::default();
        demotions.count(Some(2));
        demotions.count(Some(rt_thread::POLICY_TIMESHARE));
        demotions.count(None);
        assert_eq!(
            demotions,
            Demotions {
                samples: 2,
                sampled_timeshare: 1,
                unsampled: 1
            }
        );
        // A port name no thread holds cannot be read.
        demotions.sample(&[u32::MAX]);
        assert_eq!((demotions.samples, demotions.unsampled), (2, 2));
        // The calling thread can be read.
        demotions.sample(&[rt_thread::current_port()]);
        assert_eq!(demotions.samples, 3);
    }

    fn scheduling(workers: Vec<WorkerPath>) -> Scheduling {
        Scheduling {
            threads: workers.len() + 1,
            workers,
            no_workgroup: None,
            ports: Vec::new(),
        }
    }

    #[test]
    fn the_path_names_what_every_worker_took() {
        let wg = WorkerPath::Workgroup;
        let tc = WorkerPath::TimeConstraint("no workgroup".to_owned());
        let none = WorkerPath::None("thread_policy_set failed (kern_return 5)".to_owned());

        let all = scheduling(vec![wg.clone(); 7]);
        assert_eq!(
            (all.path(), all.summary().as_str()),
            ("workgroup", "workgroup")
        );

        let fallback = scheduling(vec![tc.clone(); 3]);
        assert_eq!(fallback.path(), "time-constraint");
        assert_eq!(fallback.summary(), "time-constraint (no workgroup)");

        let refused = scheduling(vec![none.clone(); 2]);
        assert_eq!(refused.path(), "none");
        assert_eq!(
            refused.summary(),
            "none (thread_policy_set failed (kern_return 5))"
        );

        let mixed = scheduling(vec![wg.clone(), wg, tc, none]);
        assert_eq!(mixed.path(), "mixed");
        assert_eq!(
            mixed.summary(),
            "mixed (2 workgroup, 1 time-constraint, 1 none)"
        );

        // The same path for different reasons: the path alone.
        let reasons = scheduling(vec![
            WorkerPath::TimeConstraint("a".to_owned()),
            WorkerPath::TimeConstraint("b".to_owned()),
        ]);
        assert_eq!(reasons.summary(), "time-constraint");
    }

    #[test]
    fn no_workers_and_ordinary_workers_are_none() {
        let empty = Scheduling::ordinary(1, 0, "null driver");
        assert_eq!(empty.path(), "none");
        assert_eq!(empty.summary(), "none (no worker threads)");

        let null = Scheduling::ordinary(4, 3, "null driver");
        assert_eq!(null.workers.len(), 3);
        assert_eq!(null.path(), "none");
        assert_eq!(null.summary(), "none (null driver)");
        assert_eq!(null.no_workgroup.as_deref(), Some("null driver"));
        assert_eq!(
            WorkerPath::None("pending".to_owned()).to_string(),
            "none (pending)"
        );
    }

    #[test]
    fn a_device_is_matched_by_its_position_in_an_unchanged_list() {
        let ids = vec![
            Some("coreaudio:A".to_owned()),
            None,
            Some("coreaudio:C".to_owned()),
        ];
        let list = [41, 42, 43];
        assert_eq!(match_device(&list, &ids, &list, "coreaudio:C"), Ok(43));
        assert_eq!(match_device(&list, &ids, &list, "coreaudio:A"), Ok(41));
        // A device whose id cpal could not read keeps its position and
        // matches nothing.
        assert_eq!(
            match_device(&list, &ids, &list, "coreaudio:B"),
            Err(DeviceMatch::Absent)
        );
    }

    #[test]
    fn a_list_that_changed_is_not_matched() {
        let ids = vec![
            Some("coreaudio:A".to_owned()),
            Some("coreaudio:B".to_owned()),
        ];
        assert_eq!(
            match_device(&[1, 2], &ids, &[1, 3], "coreaudio:A"),
            Err(DeviceMatch::Changed)
        );
        assert_eq!(
            match_device(&[1, 2, 3], &ids, &[1, 2, 3], "coreaudio:A"),
            Err(DeviceMatch::Changed)
        );
        assert_eq!(
            DeviceMatch::Changed.to_string(),
            "the device list changed while matching"
        );
    }
}

//! The machine's performance cores, read once per process (ADR 0019 §2: the
//! default thread count is the performance cores, the calling thread and
//! one worker per other core). Control-thread code: it runs `sysctl`.

use std::process::Command;
use std::sync::OnceLock;

/// Where [`performance_cores`] got its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreSource {
    /// `sysctl`: the physical cores of every performance level whose
    /// `hw.perflevelN.name` is not `Efficiency`.
    Sysctl,
    /// `sysctl` did not answer (a sandbox, another OS):
    /// [`std::thread::available_parallelism`].
    Fallback,
}

impl CoreSource {
    /// `"sysctl"` or `"fallback"`.
    pub fn name(self) -> &'static str {
        match self {
            CoreSource::Sysctl => "sysctl",
            CoreSource::Fallback => "fallback",
        }
    }
}

/// The performance cores of this machine and where the number came from,
/// read on the first call and kept.
pub fn performance_cores() -> (usize, CoreSource) {
    static CORES: OnceLock<(usize, CoreSource)> = OnceLock::new();
    *CORES.get_or_init(|| match from_sysctl() {
        Some(cores) if cores > 0 => (cores, CoreSource::Sysctl),
        _ => (
            std::thread::available_parallelism().map_or(1, |n| n.get()),
            CoreSource::Fallback,
        ),
    })
}

fn sysctl(key: &str) -> Option<String> {
    let output = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn from_sysctl() -> Option<usize> {
    let levels: usize = sysctl("hw.nperflevels")?.parse().ok()?;
    let mut cores = 0;
    for level in 0..levels {
        let name = sysctl(&format!("hw.perflevel{level}.name"))?;
        if name.eq_ignore_ascii_case("efficiency") {
            continue;
        }
        let count: usize = sysctl(&format!("hw.perflevel{level}.physicalcpu"))?
            .parse()
            .ok()?;
        cores += count;
    }
    Some(cores)
}

/// One performance level of the CPU (M2.7-2, `signal devices --compute`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfLevel {
    /// `N` of `hw.perflevelN` (0 is the fastest).
    pub index: usize,
    /// Its name (`Performance`, `Efficiency`).
    pub name: String,
    /// Its physical cores.
    pub physical: usize,
    /// Its logical cores.
    pub logical: usize,
}

/// The machine's performance levels, read now, and where they came from:
/// `sysctl`, or where it does not answer one level, `all`, of
/// [`std::thread::available_parallelism`] cores. Control-thread code.
pub fn cpu_levels() -> (Vec<PerfLevel>, CoreSource) {
    let read = || -> Option<Vec<PerfLevel>> {
        let levels: usize = sysctl("hw.nperflevels")?.parse().ok()?;
        (0..levels)
            .map(|index| {
                let key = |k: &str| sysctl(&format!("hw.perflevel{index}.{k}"));
                Some(PerfLevel {
                    index,
                    name: key("name")?,
                    physical: key("physicalcpu")?.parse().ok()?,
                    logical: key("logicalcpu")?.parse().ok()?,
                })
            })
            .collect()
    };
    match read() {
        Some(levels) if !levels.is_empty() => (levels, CoreSource::Sysctl),
        _ => {
            let n = std::thread::available_parallelism().map_or(1, |n| n.get());
            let all = PerfLevel {
                index: 0,
                name: "all".to_owned(),
                physical: n,
                logical: n,
            };
            (vec![all], CoreSource::Fallback)
        }
    }
}

/// The CPU part of the compute inventory for people, one line per level
/// (`signal devices --compute` and the app's interface setup).
pub fn cpu_lines(levels: &[PerfLevel], source: CoreSource) -> Vec<String> {
    let mut lines = vec![format!("CPU ({}):", source.name())];
    for level in levels {
        lines.push(format!(
            "  level {} {}: {} cores ({} logical)",
            level.index, level.name, level.physical, level.logical
        ));
    }
    lines
}

/// The threads a real-time run uses when its `threads` is 0 (auto): two
/// thirds of the performance cores, rounded down, at least 1. The rest are
/// left to the control thread, the GUI and the system, whose preemption of a
/// joined worker delays the whole block (`docs/realtime-rules.md` "Worker
/// threads", operator measurement 2026-10-12). Offline, auto is every
/// performance core.
pub fn realtime_threads() -> usize {
    (performance_cores().0 * 2 / 3).max(1)
}

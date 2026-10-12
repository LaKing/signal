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

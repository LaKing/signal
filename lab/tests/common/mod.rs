//! Shared by the `lab.gpu_partials` tests: the registry, a plan that plays
//! the bank, and the devices (AC10 of M2.6-4: without a usable adapter the
//! GPU tests say so and pass vacuously). M2.7-2: every test has its own
//! device pool; there is no process-wide service.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use signal_core::{Lent, ModuleType, Registry};
use signal_gpu::{DevicePool, GpuHost, Injected, Selection};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, InstanceDef, build_plan};

/// `std.*` and `lab.*`, with no device lent.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    signal_lab::register(&mut registry).unwrap();
    registry
}

/// `lab.gpu_partials` as registered.
pub fn module_type() -> ModuleType {
    *registry().get("lab.gpu_partials").unwrap()
}

/// A pool and the registry that places GPU-backed instances on it.
pub struct Gpu {
    pub pool: Arc<DevicePool>,
}

impl Gpu {
    fn of(pool: DevicePool) -> Gpu {
        Gpu {
            pool: Arc::new(pool),
        }
    }

    /// [`registry`], its GPU-backed instances placed on this pool.
    pub fn registry(&self) -> Registry {
        self.registry_and_host().0
    }

    /// [`registry`](Gpu::registry) and the host that places its instances.
    pub fn registry_and_host(&self) -> (Registry, Arc<GpuHost>) {
        let mut registry = registry();
        let host = Arc::new(GpuHost::new(
            Arc::clone(&self.pool),
            &signal_lab::GPU_TYPES,
            BTreeMap::new(),
        ));
        registry.set_host(Arc::clone(&host) as Arc<dyn signal_core::Host>);
        (registry, host)
    }

    /// Device 0, as a host lends it.
    pub fn lent(&self) -> Lent {
        Arc::new(DevicePool::lease(&self.pool, 0))
    }

    /// Whether device 0 is the noop device, which computes nothing.
    pub fn is_noop(&self) -> bool {
        self.pool
            .devices()
            .first()
            .is_none_or(|d| d.backend == "Noop")
    }

    pub fn set_completion_delay(&self, delay: Duration) {
        self.pool
            .with_service(0, |s| s.set_completion_delay(delay))
            .unwrap();
    }

    pub fn fail_next_job(&self, injected: Injected) {
        self.pool
            .with_service(0, |s| s.fail_next_job(injected))
            .unwrap();
    }

    pub fn lose_device(&self) {
        self.pool.lose_device(0);
    }

    pub fn jobs_run(&self) -> u64 {
        self.pool.with_service(0, |s| s.jobs_run()).unwrap_or(0)
    }
}

/// The machine's GPUs, or `None` after printing why there is none: for the
/// tests of computed values.
pub fn gpu() -> Option<Gpu> {
    let pool = DevicePool::open(&Selection::All);
    match pool.devices().first() {
        Some(device) => {
            eprintln!("GPU: {} ({})", device.name, device.backend);
            Some(Gpu::of(pool))
        }
        None => {
            eprintln!(
                "skipped: no usable GPU adapter ({:?}); operator steps: docs/benchmarks.md, \"GPU partial bank (M2.6-4)\"",
                signal_gpu::inventory().error
            );
            None
        }
    }
}

/// The GPU, or else one noop device (which moves jobs but computes
/// nothing): for the tests of the job path.
pub fn device() -> Gpu {
    let gpu = gpu().unwrap_or_else(|| Gpu::of(DevicePool::noop(1).unwrap()));
    eprintln!("device: {}", gpu.pool.devices()[0].id);
    gpu
}

/// `devices` noop devices.
pub fn noop(devices: usize) -> Gpu {
    Gpu::of(DevicePool::noop(devices).unwrap())
}

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
        scope: None,
    }
}

fn connection(from: &str, to: &str, scale: Option<f64>) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    }
}

/// `bank` (a `lab.gpu_partials`, its `freq` modulated by a 3 Hz LFO, so
/// its parameters change from block to block) into both channels of `out`.
pub fn plan(registry: &Registry) -> ExecutionPlan {
    let root = CompositeDef {
        instances: vec![
            instance(
                "bank",
                "lab.gpu_partials",
                &[
                    ("freq", 110.0),
                    ("stretch", 0.0005),
                    ("tilt", 1.2),
                    ("gain", 0.3),
                ],
            ),
            instance("wobble", "std.lfo", &[("freq_hz", 3.0)]),
            instance("out", "std.audio_out", &[]),
        ],
        connections: vec![
            connection("wobble.out", "bank.freq", Some(20.0)),
            connection("bank.out", "out.l", None),
            connection("bank.out", "out.r", None),
        ],
        ..CompositeDef::default()
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

/// `banks` banks (`bank0`, `bank1`, …, each at its own frequency) mixed
/// into both channels of `out` (M2.7-2: one per device and more).
pub fn banks(registry: &Registry, banks: usize) -> ExecutionPlan {
    let mut instances: Vec<InstanceDef> = (0..banks)
        .map(|i| {
            instance(
                &format!("bank{i}"),
                "lab.gpu_partials",
                &[("freq", 55.0 * (i + 1) as f64), ("gain", 0.1)],
            )
        })
        .collect();
    instances.push(instance("out", "std.audio_out", &[]));
    let mut connections = Vec::new();
    for i in 0..banks {
        connections.push(connection(&format!("bank{i}.out"), "out.l", None));
        connections.push(connection(&format!("bank{i}.out"), "out.r", None));
    }
    let root = CompositeDef {
        instances,
        connections,
        ..CompositeDef::default()
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

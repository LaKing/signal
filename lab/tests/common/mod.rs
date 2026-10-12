//! Shared by the `lab.gpu_partials` tests: the registry, a plan that plays
//! the bank, and the GPU gate (AC10 of M2.6-4: without a usable adapter the
//! GPU tests say so and pass vacuously).

#![allow(dead_code)]

use signal_core::{ModuleType, Registry};
use signal_gpu::GpuService;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, InstanceDef, build_plan};

/// `std.*` and `lab.*`.
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

/// A real GPU, or `None` after printing why there is none: for the tests
/// of computed values.
pub fn gpu() -> Option<&'static GpuService> {
    match GpuService::shared() {
        Some(service) if !service.is_noop() => {
            eprintln!("GPU: {}", service.adapter());
            Some(service)
        }
        _ => {
            eprintln!(
                "skipped: no usable GPU adapter ({:?}); operator steps: docs/benchmarks.md, \"GPU partial bank (M2.6-4)\"",
                GpuService::shared_error()
            );
            None
        }
    }
}

/// The GPU, or else the noop device (which moves jobs but computes
/// nothing) as the process's service: for the tests of the job path.
pub fn device() -> &'static GpuService {
    let service = GpuService::shared_or_noop().unwrap();
    eprintln!("device: {}", service.adapter());
    service
}

/// `bank` (a `lab.gpu_partials`, its `freq` modulated by a 3 Hz LFO, so
/// its parameters change from block to block) into both channels of `out`.
pub fn plan(registry: &Registry) -> ExecutionPlan {
    let instance = |name: &str, module: &str, parameters: &[(&str, f64)]| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
    };
    let connection = |from: &str, to: &str, scale: Option<f64>| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    };
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

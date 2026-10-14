//! M2.7-5: the device service's correctness, on private noop services
//! (wgpu validates kernels and dispatches before any backend sees them, so
//! the noop device refuses what a GPU would; it computes nothing). The
//! regressions of the external audit of 986cc16 (findings 3, 4 and 5).

signal_contract::install_counting_allocator!();

use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_contract::alloc_counter::Guard;
use signal_gpu::{
    Channel, DevicePool, Failure, GpuError, GpuService, Injected, JOIN_MARGIN, Job, KernelSpec,
    Lease, MAX_IN_FLIGHT, Outcome, SHUTDOWN_TIMEOUT, Selection, ServiceState,
};

/// The audit's copy kernel: `output[i] = input[i]` for the first two words.
const COPY: &str = "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < 2u) { output[id.x] = input[id.x]; }
}";

const SPEC: KernelSpec = KernelSpec {
    wgsl: COPY,
    entry: "main",
    input_words: 2,
    output_words: 2,
    workgroups: (1, 1, 1),
};

fn noop() -> GpuService {
    GpuService::start_noop().unwrap()
}

fn wait(channel: &mut Channel) -> Job {
    let start = Instant::now();
    while channel.peek_done().is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "no result");
        std::thread::yield_now();
    }
    channel.take_done().unwrap()
}

/// Finding 3: the kernel cache kept the first channel's whole spec, so a
/// second channel of the same shader ran with the first one's workgroups. A
/// second channel whose own dispatch cannot run must be refused, or its job
/// must fail; on 26d4ae08 it ran the cached (1, 1, 1) and reported success.
#[test]
fn a_cached_kernel_does_not_decide_another_channels_dispatch() {
    let service = noop();
    let _first = service.channel(SPEC, 1).unwrap();
    let second = KernelSpec {
        workgroups: (65_536, 1, 1),
        ..SPEC
    };
    match service.channel(second, 1) {
        Err(e) => eprintln!("refused at preparation: {e}"),
        Ok(mut channel) => {
            assert!(channel.submit(0, |input| input.copy_from_slice(&[3.0, 7.0])));
            let job = wait(&mut channel);
            assert!(
                job.failed(),
                "ran the first channel's dispatch as a success"
            );
        }
    }
}

/// M2.7-8: a service names its adapter as its backend describes it, what a
/// session records of a device it used. On the noop device the backend is
/// `Noop`; a real adapter's fields are the operator's to check (C15).
#[test]
fn a_service_names_its_adapter() {
    let service = noop();
    let identity = service.identity();
    assert_eq!(identity.backend, "Noop", "{identity:?}");
    assert!(!identity.name.is_empty(), "{identity:?}");
    assert!(service.adapter().contains(&identity.name));
}

/// Finding 4: an invalid dispatch raised validation errors, yet the job came
/// back as zeros with `failed = false`. It is refused, or its job fails.
#[test]
fn an_invalid_dispatch_is_never_a_success() {
    let service = noop();
    let spec = KernelSpec {
        workgroups: (65_536, 1, 1),
        ..SPEC
    };
    match service.channel(spec, 1) {
        Err(e) => eprintln!("refused at preparation: {e}"),
        Ok(mut channel) => {
            assert!(channel.submit(0, |input| input.copy_from_slice(&[3.0, 7.0])));
            let job = wait(&mut channel);
            assert!(job.failed(), "an invalid dispatch reported success");
        }
    }
}

/// Finding 5: returning another channel's job to a full pool freed it on
/// the calling thread. Recycling a foreign job neither allocates nor frees.
#[test]
fn recycling_a_foreign_job_frees_nothing_on_the_caller() {
    let service = noop();
    let mut a = service.channel(SPEC, 1).unwrap();
    let mut b = service.channel(SPEC, 1).unwrap();
    assert!(b.submit(0, |input| input.fill(1.0)));
    let foreign = wait(&mut b);
    let guard = Guard::start();
    let back = a.recycle(foreign);
    let counts = guard.counts();
    drop(guard);
    assert!(counts.is_zero(), "{counts:?}");
    drop(back);
}

/// SC4: the refused job is handed back whole, still b's, and b takes it.
#[test]
fn a_foreign_job_is_handed_back_to_its_owner() {
    let service = noop();
    let mut a = service.channel(SPEC, 1).unwrap();
    let mut b = service.channel(SPEC, 1).unwrap();
    assert!(b.submit(5, |input| input.fill(1.0)));
    let foreign = wait(&mut b);
    let back = a.recycle(foreign).unwrap_err();
    assert_eq!((back.owner(), back.seq()), (b.id(), 5));
    b.recycle(back).unwrap();
    assert_eq!(b.free_jobs(), 1);
    assert_eq!(a.free_jobs(), 1);
}

/// NN2: the module's side of a channel never touches the heap, whether a
/// job is its own or another channel's.
#[test]
fn the_job_path_does_not_allocate() {
    let service = noop();
    let mut a = service.channel(SPEC, 2).unwrap();
    let mut b = service.channel(SPEC, 1).unwrap();
    assert!(b.submit(0, |_| {}));
    let foreign = wait(&mut b);
    let guard = Guard::start();
    assert!(a.submit(1, |input| input.fill(2.0)));
    let start = Instant::now();
    while a.peek_done().is_none() && start.elapsed() < Duration::from_secs(10) {
        std::hint::spin_loop();
    }
    let own = a.take_done().unwrap();
    let own_back = a.recycle(own);
    let foreign_back = a.recycle(foreign);
    let counts = guard.counts();
    drop(guard);
    assert!(counts.is_zero(), "{counts:?}");
    assert!(own_back.is_ok());
    drop(foreign_back);
}

/// SC1: two channels of one shader with different workgroups share one
/// compiled pipeline, and each is dispatched with its own workgroups.
#[test]
fn each_channel_dispatches_its_own_workgroups_on_one_pipeline() {
    let service = noop();
    let mut a = service.channel(SPEC, 1).unwrap();
    let wide = KernelSpec {
        workgroups: (2, 1, 1),
        ..SPEC
    };
    let mut b = service.channel(wide, 1).unwrap();
    assert_eq!(service.kernels_cached(), 1);
    for channel in [&mut a, &mut b] {
        assert!(channel.submit(0, |_| {}));
        assert_eq!(wait(channel).outcome(), Outcome::Ok);
    }
    assert_eq!(a.last_dispatch(), Some((1, 1, 1)));
    assert_eq!(b.last_dispatch(), Some((2, 1, 1)));
}

/// SC1: the cache is keyed by the shader's text, not where it lives, and
/// by its entry point.
#[test]
fn the_cache_is_keyed_by_content_and_entry_point() {
    let service = noop();
    let _a = service.channel(SPEC, 1).unwrap();
    let copy: &'static str = String::from(COPY).leak();
    let _b = service
        .channel(KernelSpec { wgsl: copy, ..SPEC }, 1)
        .unwrap();
    assert_eq!(service.kernels_cached(), 1);
    const TWO: &str = "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(1) fn one(@builtin(global_invocation_id) id: vec3<u32>) { output[0] = input[0]; }
@compute @workgroup_size(1) fn two(@builtin(global_invocation_id) id: vec3<u32>) { output[1] = input[1]; }";
    let _c = service
        .channel(
            KernelSpec {
                wgsl: TWO,
                entry: "one",
                ..SPEC
            },
            1,
        )
        .unwrap();
    let _d = service
        .channel(
            KernelSpec {
                wgsl: TWO,
                entry: "two",
                ..SPEC
            },
            1,
        )
        .unwrap();
    assert_eq!(service.kernels_cached(), 3);
}

/// SC2: a channel that cannot run is refused when it is made: workgroups
/// and buffers beyond the device's limits, a shader that needs a resource
/// the channel does not supply or with another access, and buffers smaller
/// than the shader needs. One correctly sized channel of that same shader
/// is accepted on the same pipeline.
#[test]
fn a_channel_that_cannot_run_is_refused_at_preparation() {
    let service = noop();
    let refused = |spec: KernelSpec, what: &str| match service.channel(spec, 1) {
        Err(e) => eprintln!("{what}: {e}"),
        Ok(_) => panic!("{what}: accepted"),
    };
    refused(
        KernelSpec {
            workgroups: (0, 1, 1),
            ..SPEC
        },
        "zero workgroups",
    );
    refused(
        KernelSpec {
            workgroups: (1, 1, 65_536),
            ..SPEC
        },
        "workgroups over the limit",
    );
    refused(
        KernelSpec {
            input_words: 1 << 40,
            ..SPEC
        },
        "input over the limit",
    );
    for (what, wgsl) in [
        (
            "a second bind group",
            "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(1) @binding(0) var<storage, read> extra: array<f32>;
@compute @workgroup_size(1) fn main() { output[0] = input[0] + extra[0]; }",
        ),
        (
            "a third binding",
            "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<storage, read> extra: array<f32>;
@compute @workgroup_size(1) fn main() { output[0] = input[0] + extra[0]; }",
        ),
        (
            "an input written to",
            "@group(0) @binding(0) var<storage, read_write> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(1) fn main() { input[0] = 1.0; output[0] = input[0]; }",
        ),
        (
            "a uniform output",
            "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<uniform> output: vec4<f32>;
@compute @workgroup_size(1) fn main() { let x = output.x + input[0]; }",
        ),
    ] {
        let spec = KernelSpec {
            wgsl: String::from(wgsl).leak(),
            ..SPEC
        };
        assert!(
            matches!(service.channel(spec, 1), Err(GpuError::Kernel(_))),
            "{what}: accepted"
        );
    }
    // A fixed-size input of 4 words: 1 word is too small, 4 are right.
    const U4: &str = "@group(0) @binding(0) var<storage, read> input: array<f32, 4>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(1) fn main() { output[0] = input[3]; }";
    let small = KernelSpec {
        wgsl: U4,
        input_words: 1,
        ..SPEC
    };
    match service.channel(small, 1) {
        Err(GpuError::Invalid(e)) => eprintln!("undersized input: {e}"),
        other => panic!("undersized input: {other:?}"),
    }
    let cached = service.kernels_cached();
    let mut right = service
        .channel(
            KernelSpec {
                input_words: 4,
                ..small
            },
            1,
        )
        .unwrap();
    assert_eq!(
        service.kernels_cached(),
        cached,
        "one pipeline for both sizes"
    );
    assert!(right.submit(0, |_| {}));
    assert_eq!(wait(&mut right).outcome(), Outcome::Ok);
}

/// SC3: a job's outcome is its own errors': a validation error, a mapping
/// error, and after each the same channel runs `Ok` again (its staging
/// buffer recovers from a refused map, twice), with no error left to any
/// other job as uncaptured.
#[test]
fn failures_are_explicit_and_a_channel_recovers_from_them() {
    let service = noop();
    let mut channel = service.channel(SPEC, 1).unwrap();
    let before = service.faults().uncaptured_errors;
    let mut run = |inject: Option<Injected>| {
        if let Some(inject) = inject {
            service.fail_next_job(inject);
        }
        assert!(channel.submit(0, |input| input.fill(3.0)));
        let job = wait(&mut channel);
        let outcome = job.outcome();
        assert!(job.failed() || outcome == Outcome::Ok);
        if outcome != Outcome::Ok {
            assert!(job.output().iter().all(|&x| x == 0.0));
        }
        channel.recycle(job).unwrap();
        outcome
    };
    assert_eq!(
        run(Some(Injected::Dispatch)),
        Outcome::Failed(Failure::Validation)
    );
    assert_eq!(run(None), Outcome::Ok);
    assert_eq!(run(Some(Injected::Map)), Outcome::Failed(Failure::Mapping));
    assert_eq!(
        run(None),
        Outcome::Ok,
        "the lane recovers from a refused map"
    );
    assert_eq!(run(Some(Injected::Map)), Outcome::Failed(Failure::Mapping));
    assert_eq!(run(None), Outcome::Ok, "and again");
    let faults = service.faults();
    assert_eq!(faults.uncaptured_errors, before);
    assert_eq!((faults.validation, faults.mapping), (1, 2));
    assert!(faults.last_error.is_some());
}

/// AC2: two slow channels and a fast one in flight at once: every fast job
/// completes while the slow ones are still on the device.
#[test]
fn a_slow_channel_does_not_hold_back_a_fast_one() {
    let service = noop();
    let mut slow: Vec<Channel> = (0..2).map(|_| service.channel(SPEC, 1).unwrap()).collect();
    let mut fast = service.channel(SPEC, 1).unwrap();
    for channel in &mut slow {
        channel.set_execution_delay(Duration::from_millis(500));
        assert!(channel.submit(0, |_| {}));
    }
    let start = Instant::now();
    for seq in 0..10 {
        assert!(fast.submit(seq, |_| {}));
        let job = wait(&mut fast);
        assert_eq!((job.seq(), job.outcome()), (seq, Outcome::Ok));
        fast.recycle(job).unwrap();
        assert!(
            slow.iter().all(|c| c.peek_done().is_none()),
            "a slow job finished before fast job {seq}"
        );
    }
    eprintln!("10 fast jobs in {:?}", start.elapsed());
    for channel in &mut slow {
        assert_eq!(wait(channel).outcome(), Outcome::Ok);
    }
}

/// SC5: at most `MAX_IN_FLIGHT` jobs are on the device at once; the others
/// wait their turn, and every job completes.
#[test]
fn jobs_in_flight_are_bounded() {
    let service = noop();
    let mut channels: Vec<Channel> = (0..MAX_IN_FLIGHT + 4)
        .map(|_| service.channel(SPEC, 1).unwrap())
        .collect();
    for channel in &mut channels {
        channel.set_execution_delay(Duration::from_millis(300));
        assert!(channel.submit(0, |_| {}));
    }
    for channel in &mut channels {
        assert_eq!(wait(channel).outcome(), Outcome::Ok);
    }
    let peak = service.peak_in_flight();
    assert!((1..=MAX_IN_FLIGHT).contains(&peak), "{peak}");
}

/// AC3: a lost device: the jobs in flight and every later job end
/// `DeviceLost`, the service says `Lost`, and shutdown completes within
/// its bound.
#[test]
fn a_lost_device_loses_every_job_and_shuts_down_in_time() {
    let mut service = noop();
    let mut held: Vec<Channel> = (0..3).map(|_| service.channel(SPEC, 2).unwrap()).collect();
    for channel in &mut held {
        channel.set_execution_delay(Duration::from_secs(60));
        assert!(channel.submit(0, |_| {}));
    }
    std::thread::sleep(Duration::from_millis(50));
    service.lose_device();
    assert_eq!(service.state(), ServiceState::Lost);
    for channel in &mut held {
        let job = wait(channel);
        assert_eq!(job.outcome(), Outcome::DeviceLost);
        channel.recycle(job).unwrap();
        assert!(channel.submit(1, |_| {}));
        assert_eq!(wait(channel).outcome(), Outcome::DeviceLost);
    }
    assert!(matches!(service.channel(SPEC, 1), Err(GpuError::Stopped)));
    let report = service.shutdown();
    eprintln!("{report}");
    assert_eq!(report.final_state, ServiceState::Lost);
    assert!(!report.timed_out);
    assert!(report.elapsed < SHUTDOWN_TIMEOUT + JOIN_MARGIN);
}

/// AC3 with SC6: a loss followed at once by shutdown. Jobs submitted after
/// the loss and still queued when draining starts end `DeviceLost`, not
/// `Failed(Stopped)`: loss takes precedence over shutdown.
#[test]
fn after_a_loss_shutdown_returns_every_job_as_lost() {
    let mut service = noop();
    let mut channel = service.channel(SPEC, 4).unwrap();
    service.lose_device();
    for seq in 0..4 {
        assert!(channel.submit(seq, |_| {}));
    }
    let report = service.shutdown();
    eprintln!("{report}");
    assert_eq!(report.final_state, ServiceState::Lost);
    assert!(!report.timed_out);
    assert!(report.elapsed < SHUTDOWN_TIMEOUT + JOIN_MARGIN, "{report}");
    for seq in 0..4 {
        let job = wait(&mut channel);
        assert_eq!((job.seq(), job.outcome()), (seq, Outcome::DeviceLost));
    }
}

/// SC6: running → draining → stopped. An idle service stops at once; a job
/// on the device past the bound is abandoned and reported, never waited
/// for; jobs not yet started come back `Failed(Stopped)`.
#[test]
fn shutdown_is_bounded_and_reported() {
    let mut idle = noop();
    assert_eq!(idle.state(), ServiceState::Running);
    let report = idle.shutdown();
    assert_eq!(report.final_state, ServiceState::Stopped);
    assert!(!report.timed_out && report.abandoned_jobs == 0, "{report}");
    assert!(matches!(idle.channel(SPEC, 1), Err(GpuError::Stopped)));

    let mut busy = noop();
    let mut stuck = busy.channel(SPEC, 2).unwrap();
    stuck.set_execution_delay(Duration::from_secs(60));
    assert!(stuck.submit(0, |_| {}));
    // Wait until it is on the device, then queue one behind it.
    let start = Instant::now();
    while busy.jobs_run() == 0 {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::yield_now();
    }
    assert!(stuck.submit(1, |_| {}));
    let report = busy.shutdown();
    eprintln!("{report}");
    assert_eq!(report.final_state, ServiceState::Stopped);
    assert!(report.elapsed < SHUTDOWN_TIMEOUT + JOIN_MARGIN, "{report}");
    assert!(report.abandoned_jobs >= 1, "{report}");
    assert!(!report.timed_out);
    // The queued job came back as not run; the one on the device did not.
    let queued = wait(&mut stuck);
    assert_eq!(
        (queued.seq(), queued.outcome()),
        (1, Outcome::Failed(Failure::Stopped))
    );
    assert!(stuck.is_lost());
}

/// The machine's first GPU, or `None` after saying why (the noop device
/// computes nothing): the audit's adapter reproductions, run by the operator.
fn gpu() -> Option<Lease> {
    let pool = Arc::new(DevicePool::open(&Selection::All));
    match pool.devices().first() {
        Some(device) => {
            eprintln!("GPU: {} ({})", device.name, device.backend);
            Some(DevicePool::lease(&pool, 0))
        }
        None => {
            eprintln!(
                "skipped: no usable GPU adapter ({:?}); the operator runs this on the M5 Max",
                signal_gpu::inventory().error
            );
            None
        }
    }
}

/// Finding 3 on a GPU (the audit's `gpu_cache`): two channels of one shader
/// with different workgroups each get their own result.
#[test]
fn adapter_two_channels_of_one_shader_each_get_their_own_result() {
    let Some(service) = gpu() else {
        return;
    };
    let _first = service.channel(SPEC, 1).unwrap();
    let mut second = service
        .channel(
            KernelSpec {
                workgroups: (2, 1, 1),
                ..SPEC
            },
            1,
        )
        .unwrap();
    assert!(second.submit(0, |input| input.copy_from_slice(&[3.0, 7.0])));
    let job = wait(&mut second);
    assert_eq!(job.outcome(), Outcome::Ok);
    assert_eq!(job.output(), &[3.0, 7.0]);
}

/// Finding 4 on a GPU (the audit's `gpu_validation`): an invalid dispatch is
/// refused, or its job fails; never zeros reported as a success.
#[test]
fn adapter_an_invalid_dispatch_is_never_a_success() {
    let Some(service) = gpu() else {
        return;
    };
    let spec = KernelSpec {
        workgroups: (65_536, 1, 1),
        ..SPEC
    };
    if let Ok(mut channel) = service.channel(spec, 1) {
        assert!(channel.submit(0, |input| input.copy_from_slice(&[3.0, 7.0])));
        assert!(wait(&mut channel).failed());
    }
}

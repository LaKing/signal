//! M2.6-3 (ADR 0019 §2, §4) without a device: the kick's worker threads put
//! into the Mach time-constraint policy through `rt_thread::enter` (no
//! workgroup offline) play bit for bit what the offline render gives, every
//! worker reports a path, and `exit` takes them out again (twice is a no-op,
//! as on a thread that never entered). The workgroup join itself needs a
//! device: the operator's check.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use signal_audio::{TimeConstraint, WorkerPath, rt_thread};
use signal_core::Registry;
use signal_engine::{
    Driver, Engine, EngineConfig, NullDriver, RealtimeConfig, Schedule, realtime, render,
};
use signal_project::Project;

const WAIT: Duration = Duration::from_secs(10);

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

#[test]
fn time_constraint_workers_play_the_offline_render() {
    let registry = registry();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    let frames = 16_384;
    let mut engine =
        Engine::new(&plan, &registry, EngineConfig::new(rate, seed), Vec::new()).unwrap();
    let rendered = render(&mut engine, frames).unwrap();
    let reference: Vec<f32> = rendered
        .samples
        .iter()
        .map(|&x| x.clamp(-1.0, 1.0) as f32)
        .collect();

    let mut config = RealtimeConfig::with_buffer(rate, seed, 128);
    config.engine.threads = 4;
    config.engine.schedule = Schedule::FORCED;
    let (runner, _controller) = realtime(&plan, &registry, config).unwrap();
    let workers = runner.workers();
    assert_eq!(workers.count(), 3);

    let policy = TimeConstraint::from_period(128, rate, rt_thread::timebase());
    let paths = Arc::new(Mutex::new(vec![None; 3]));
    let slots = Arc::clone(&paths);
    let entered = workers.post(move |w| {
        let path = rt_thread::enter(&policy, Err("offline: no device"));
        slots.lock().unwrap()[w] = Some(path);
    });
    assert!(workers.wait(entered, WAIT).complete());
    for path in paths.lock().unwrap().iter() {
        // The sandbox may refuse the policy: then the path says so.
        match path.as_ref().expect("every worker reports a path") {
            WorkerPath::TimeConstraint(reason) => assert_eq!(reason, "offline: no device"),
            WorkerPath::None(reason) => assert!(reason.contains("thread_policy_set"), "{reason}"),
            WorkerPath::Workgroup => panic!("no workgroup offline"),
        }
    }

    let channels = usize::from(rendered.channels);
    let mut driver = NullDriver::new(rate, channels, 3);
    driver.start(runner).unwrap();
    driver.run(frames, || {}).unwrap();
    let runner = driver.stop().unwrap().unwrap();
    let output = driver.take_output();
    assert_eq!(output.len(), reference.len());
    assert!(
        output
            .iter()
            .zip(&reference)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "real-time workers change no sample"
    );
    assert!(runner.schedule_stats().worker_tasks > 0);

    // Out again, twice: the second finds nothing to undo.
    for _ in 0..2 {
        let left = workers.post(|_| rt_thread::exit());
        assert!(workers.wait(left, WAIT).complete());
    }
    let exclusivity = signal_engine::exclusivity();
    assert!(exclusivity.checked && exclusivity.claims > 0);
    assert_eq!(exclusivity.violations, 0);
}

#[test]
fn exit_on_a_thread_that_never_entered_does_nothing() {
    rt_thread::exit();
    rt_thread::exit();
}

#[test]
fn the_timebase_is_read() {
    let (numer, denom) = rt_thread::timebase();
    assert!(numer > 0 && denom > 0);
}

//! M2.6-3 (ADR 0019 §2, §4) without a device: the kick's worker threads put
//! into the Mach time-constraint policy through `rt_thread::enter` (no
//! workgroup offline) play bit for bit what the offline render gives, every
//! worker reports a path, and `exit` takes them out again (twice is a no-op,
//! as on a thread that never entered). The workgroup join itself needs a
//! device: the operator's check.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use signal_audio::rt_thread::MachSemaphores;
use signal_audio::{TimeConstraint, WorkerPath, rt_thread};
use signal_core::{Registry, SplitMix64};
use signal_engine::{
    Driver, Engine, EngineConfig, HookPoint, NullDriver, RealtimeConfig, RtOptions, RtPolicy,
    Schedule, Waiter, realtime, realtime_with, render, render_blocks,
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

/// A signal sent before the wait is kept (Mach semaphores count): no wake
/// is lost to a worker that has not yet blocked.
#[test]
fn a_mach_semaphore_keeps_a_signal_sent_before_the_wait() {
    let semaphores = Arc::new(MachSemaphores::new(2).unwrap());
    assert_eq!(semaphores.len(), 2);
    semaphores.signal(1);
    semaphores.signal(1);
    let (done, waited) = mpsc::channel();
    let waiter = Arc::clone(&semaphores);
    thread::spawn(move || {
        waiter.wait(1);
        waiter.wait(1);
        let _ = done.send(());
    });
    assert!(waited.recv_timeout(WAIT).is_ok(), "both signals were kept");
}

/// Signals and returns from waits of the `semaphore` policy's Mach
/// semaphores, counted.
/// One test's counts: signals sent and waits returned. Each test has its own,
/// since tests run concurrently.
struct Counts {
    signals: AtomicU64,
    returns: AtomicU64,
}

static STOP_COUNTS: Counts = Counts {
    signals: AtomicU64::new(0),
    returns: AtomicU64::new(0),
};
static STRESS_COUNTS: Counts = Counts {
    signals: AtomicU64::new(0),
    returns: AtomicU64::new(0),
};

struct Counted(MachSemaphores, &'static Counts);

impl Waiter for Counted {
    fn wait(&self, worker: usize) {
        self.0.wait(worker);
        self.1.returns.fetch_add(1, Ordering::SeqCst);
    }

    fn signal(&self, worker: usize) {
        self.1.signals.fetch_add(1, Ordering::SeqCst);
        self.0.signal(worker);
    }
}

fn counted(workers: usize) -> Result<Arc<dyn Waiter>, String> {
    MachSemaphores::new(workers).map(|s| Arc::new(Counted(s, &STOP_COUNTS)) as Arc<dyn Waiter>)
}

fn counted_stress(workers: usize) -> Result<Arc<dyn Waiter>, String> {
    MachSemaphores::new(workers).map(|s| Arc::new(Counted(s, &STRESS_COUNTS)) as Arc<dyn Waiter>)
}

/// Polls until every signal has been consumed by a wait (a counting
/// semaphore: once the workers block again, nothing is pending).
fn all_signals_consumed(counts: &Counts) -> bool {
    let equal = || counts.signals.load(Ordering::SeqCst) == counts.returns.load(Ordering::SeqCst);
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if equal() {
            thread::sleep(Duration::from_millis(20));
            if equal() {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

/// M2.7-1 C12 (auditor-2): under the `semaphore` policy with Mach semaphores
/// every wake path reaches workers blocked on them, and none is lost.
/// Posted requests (`rt_thread::enter` and `exit`) are served while every
/// worker waits; the kick plays through the threaded null driver bit for bit
/// like the offline render; then in the driver's order (stop, `exit`
/// posted, the engine dropped) everything ends; and every signal sent was
/// consumed by a wait, requests and the shutdown included.
#[test]
fn semaphore_workers_are_reached_by_every_wake_and_stop_cleanly() {
    let registry = registry();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    let frames = 16_384;
    let mut engine =
        Engine::new(&plan, &registry, EngineConfig::new(rate, seed), Vec::new()).unwrap();
    let rendered = render(&mut engine, frames).unwrap();

    let mut config = RealtimeConfig::with_buffer(rate, seed, 128);
    config.engine.threads = 4;
    config.engine.schedule = Schedule::FORCED;
    config.rt = RtOptions {
        policy: RtPolicy::Semaphore,
        trace: true,
        ..RtOptions::default()
    };
    let (runner, mut controller) = realtime_with(&plan, &registry, config, Some(counted)).unwrap();
    assert_eq!(runner.schedule_stats().waiter, "semaphore");
    assert_eq!(runner.waiter_error(), None);
    let workers = runner.workers();

    // Idle since their start: every worker waits on its semaphore.
    thread::sleep(Duration::from_millis(100));
    let policy = TimeConstraint::from_period(128, rate, rt_thread::timebase());
    let entered = workers.post(move |_| {
        rt_thread::enter(&policy, Err("offline: no device"));
    });
    assert!(workers.wait(entered, WAIT).complete(), "enter reached them");
    assert!(all_signals_consumed(&STOP_COUNTS));

    // Paced like a device, so the workers wait between callbacks, until the
    // frames compared have run (a debug build may run slower than real time).
    let mut driver = NullDriver::new(rate, usize::from(rendered.channels), 3).threaded(true);
    driver.start(runner).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while controller.status().next_frame.0 < frames && Instant::now() < deadline {
        controller.poll(&mut ());
        thread::sleep(Duration::from_millis(5));
    }
    let runner = driver.stop().unwrap().unwrap();
    let output = driver.take_output();
    let n = (frames as usize) * usize::from(rendered.channels);
    assert!(
        output.len() >= n,
        "the driver played {} samples",
        output.len()
    );
    let reference: Vec<f32> = rendered
        .samples
        .iter()
        .map(|&x| x.clamp(-1.0, 1.0) as f32)
        .collect();
    assert!(
        output[..n]
            .iter()
            .zip(&reference)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the semaphore policy changes no sample"
    );
    assert!(runner.schedule_stats().worker_tasks > 0);
    let wake = runner.wake_stats().unwrap();
    assert!(wake.wakes_signalled > 0, "{wake:?}");

    // The driver's stop order: the runner back, `exit` posted, then dropped.
    thread::sleep(Duration::from_millis(100));
    let left = workers.post(|_| rt_thread::exit());
    assert!(workers.wait(left, WAIT).complete(), "exit reached them");
    assert!(all_signals_consumed(&STOP_COUNTS));
    // Past their 50 µs spin: every worker waits when the shutdown comes.
    thread::sleep(Duration::from_millis(100));
    let (done, dropped) = mpsc::channel();
    thread::spawn(move || {
        drop(runner);
        let _ = done.send(());
    });
    assert!(
        dropped.recv_timeout(WAIT).is_ok(),
        "the engine did not stop"
    );
    assert!(
        all_signals_consumed(&STOP_COUNTS),
        "a signal was not consumed"
    );
    assert_eq!(signal_engine::exclusivity().violations, 0);
}

/// The random stream of the stress hook (any order of draws: only timing).
static DELAY_STATE: AtomicU64 = AtomicU64::new(11);

fn draw(range: u64) -> u64 {
    let state = DELAY_STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    SplitMix64::new(state).next_u64() % range
}

/// Spins 0–50 µs before every task.
fn random_delay(point: HookPoint) {
    if let HookPoint::Task { .. } = point {
        let until = Instant::now() + Duration::from_micros(draw(51));
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    }
}

/// M2.7-1 C12 (auditor-1-05): the stress test on Mach semaphores. 1 000
/// blocks of the kick on 8 threads (every segment on the workers), each a
/// callback of the `semaphore` policy after a random gap of 0–255 µs, with a
/// random 0–50 µs delay before every task, give the offline one-thread render
/// bit for bit; nothing deadlocks; and the engine stops with every signal
/// consumed.
#[test]
fn semaphore_workers_match_one_thread_under_random_delays_and_gaps() {
    let registry = registry();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    const BLOCK: usize = 128;
    let frames = 1_000 * BLOCK as u64;
    let mut one = Engine::new(&plan, &registry, EngineConfig::new(rate, seed), Vec::new()).unwrap();
    let reference = render_blocks(&mut one, frames, BLOCK, |_| Ok(None)).unwrap();

    let config = EngineConfig {
        threads: 8,
        block_capacity: BLOCK,
        schedule: Schedule {
            policy: RtPolicy::Semaphore,
            trace: true,
            ..Schedule::FORCED
        },
        ..EngineConfig::new(rate, seed)
    };
    let mut engine =
        Engine::with_waiter(&plan, &registry, config, Vec::new(), Some(counted_stress)).unwrap();
    assert_eq!(engine.schedule_stats().waiter, "semaphore");
    engine.set_task_hook(Some(random_delay));
    let stressed = render_blocks(&mut engine, frames, BLOCK, |engine| {
        engine.end_callback();
        thread::sleep(Duration::from_micros(draw(256)));
        engine.begin_callback(BLOCK);
        Ok(None)
    })
    .unwrap();
    engine.end_callback();
    assert!(
        stressed.samples == reference.samples,
        "the audio differs on semaphore workers"
    );
    assert!(stressed.log == reference.log, "the log differs");
    let stats = engine.schedule_stats();
    assert_eq!(stats.parallel_segments, 1_000);
    assert!(
        stats.worker_tasks > 0 && stats.worker_panics == 0,
        "{stats:?}"
    );
    let wake = engine.wake_stats().unwrap();
    assert!(wake.wakes_signalled > 0, "{wake:?}");
    assert!(
        all_signals_consumed(&STRESS_COUNTS),
        "a signal was not consumed"
    );
    thread::sleep(Duration::from_millis(100));
    let (done, dropped) = mpsc::channel();
    thread::spawn(move || {
        drop(engine);
        let _ = done.send(());
    });
    assert!(
        dropped.recv_timeout(WAIT).is_ok(),
        "the engine did not stop"
    );
    assert!(
        all_signals_consumed(&STRESS_COUNTS),
        "a signal was not consumed"
    );
    assert_eq!(signal_engine::exclusivity().violations, 0);
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

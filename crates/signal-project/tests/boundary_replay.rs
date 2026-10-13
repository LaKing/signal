//! M2.7-4 (audit finding 2): a session records the outcome of every latent
//! module's due block, and `replay` reproduces the performance exactly —
//! recorded misses are applied, recorded data shown, the audio stored as the
//! device received it — while `replay_with(ideal)` awaits every result and
//! says so. `test.latent` stands in for a slow device without a GPU: its
//! even blocks' results are in at the first `due`, its odd blocks' only at
//! the third, so in real time (the null driver) it delivers the even blocks
//! and misses the odd ones.

use std::fs;
use std::path::PathBuf;

use signal_core::{
    BlockLatency, Direction, Due, Feedthrough, Inputs, LossCause, ModuleDescriptor, ModuleType,
    Outputs, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
    Reproducibility, StateOutputPrimitive, fnv1a64,
};
use signal_engine::{Driver, NullDriver, RealtimeConfig, RunnerStatus, device_sample, realtime};
use signal_project::session::{
    BoundaryKind, BoundaryReplay, LiveRecorder, ReplayError, ReplayOptions, Session,
    SessionRecorder, replay, replay_with,
};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;
const BLOCK: usize = 4;
/// Callbacks of 256 frames the live runs play.
const CALLBACKS: usize = 16;

const fn port(name: &'static str, direction: Direction) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Audio,
        unit: None,
        range: None,
    }
}

const PORTS: &[PortDescriptor] = &[port("in", Direction::In), port("out", Direction::Out)];

const fn latent(type_name: &'static str) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "out in block m = scale · Σ in over block m − 1; slow on odd blocks",
        ports: PORTS,
        parameters: &[],
        feedthrough: Feedthrough::StateOutput,
        sink: None,
        latency: Some(BlockLatency {
            latency_blocks: 1,
            block_frames: BLOCK as u32,
        }),
        reproducibility: Reproducibility::SameMachine,
    }
}

/// `test.latent`: the block's sum ÷ 4, inside [−1, 1].
const QUIET: ModuleDescriptor = latent("test.latent");
/// `test.latent_loud`: the block's sum × 4, which clips.
const LOUD: ModuleDescriptor = latent("test.latent_loud");

/// `LOUD` picks the type; `OTHER` computes another result for the same type,
/// like another GPU.
#[derive(Default)]
struct Latent<const LOUD_: bool, const OTHER: bool> {
    frame: usize,
    block: u64,
    sum: f64,
    pending: f32,
    shown: [f32; 1],
    polls: u32,
    resolved: bool,
}

impl<const LOUD_: bool, const OTHER: bool> StateOutputPrimitive for Latent<LOUD_, OTHER> {
    fn describe() -> ModuleDescriptor {
        if LOUD_ { LOUD } else { QUIET }
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = Latent::default();
    }
    fn reset(&mut self) {
        *self = Latent::default();
    }
    fn due(&mut self) -> Due {
        if self.frame != 0 || self.resolved {
            return Due::Ready;
        }
        if self.block == 0 {
            self.resolved = true;
            return Due::Ready;
        }
        self.polls += 1;
        let needed = if self.block.is_multiple_of(2) { 1 } else { 3 };
        if self.polls < needed {
            return Due::Pending;
        }
        self.shown = [self.pending];
        self.resolved = true;
        Due::Ready
    }
    fn miss(&mut self) {
        self.shown = [0.0];
        self.resolved = true;
    }
    fn result_words(&self) -> usize {
        1
    }
    fn delivered_result(&self) -> &[f32] {
        &self.shown
    }
    fn inject_result(&mut self, words: &[f32]) -> bool {
        let [word] = words else {
            return false;
        };
        self.shown = [*word];
        self.resolved = true;
        true
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(f64::from(self.shown[0]));
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.sum += inputs.signal(0).first().copied().unwrap_or(0.0);
        self.frame += 1;
        if self.frame == BLOCK {
            let scale = if LOUD_ { 4.0 } else { 0.25 };
            let other = if OTHER { 0.125 } else { 0.0 };
            self.pending = (self.sum * scale + other) as f32;
            self.sum = 0.0;
            self.frame = 0;
            self.block += 1;
            self.resolved = false;
            self.polls = 0;
        }
    }
}

fn latent_type<const LOUD_: bool, const OTHER: bool>() -> ModuleType {
    ModuleType {
        descriptor: Latent::<LOUD_, OTHER>::describe(),
        create: || Primitive::StateOutput(Box::new(Latent::<LOUD_, OTHER>::default())),
    }
}

/// `std.*`, `test.latent` and `test.latent_loud`; with `other`, both compute
/// another result (another device).
fn registry_with(other: bool) -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let types = if other {
        [latent_type::<false, true>(), latent_type::<true, true>()]
    } else {
        [latent_type::<false, false>(), latent_type::<true, false>()]
    };
    for module_type in types {
        registry.register(module_type).unwrap();
    }
    registry
}

fn registry() -> Registry {
    registry_with(false)
}

/// A tone into `module` into both channels of the sink; no latent module
/// with `module` = `None`.
fn project(module: Option<&str>) -> Project {
    let body = match module {
        Some(module) => format!(
            "lat = {{ module = \"{module}\" }}\n\n[wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"lat.in\" }},\n  {{ from = \"lat.out\", to = \"out.l\" }},\n  {{ from = \"lat.out\", to = \"out.r\" }},\n]\n"
        ),
        None => "\n[wiring]\nconnections = [\n  { from = \"tone.out\", to = \"out.l\" },\n  { from = \"tone.out\", to = \"out.r\" },\n]\n".to_owned(),
    };
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"boundaries\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\nout = {{ module = \"std.audio_out\" }}\n{body}"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// How a live run is set up and ended.
#[derive(Clone, Copy)]
struct Run {
    data: bool,
    log: Option<usize>,
    data_queue: usize,
    /// Poll after the last callback and after the driver stops.
    final_poll: bool,
}

const PLAIN: Run = Run {
    data: false,
    log: None,
    data_queue: 0,
    final_poll: true,
};

/// A null-driver run of `project`, recorded: what the device received (what
/// `signal play --record-audio` writes), the stopped runner's status and the
/// recorder after `finish`.
fn live(
    project: &Project,
    registry: &Registry,
    run: Run,
) -> (Vec<f32>, RunnerStatus, SessionRecorder) {
    let mut plan = project.plan(registry).unwrap().plan;
    plan.version = 0;
    let mut config = RealtimeConfig::new(RATE, project.seed);
    if let Some(log) = run.log {
        config.log = log;
    }
    if run.data {
        config.engine.record_boundary_data = true;
        config.boundary_data = run.data_queue;
    }
    let (runner, mut controller) = realtime(&plan, registry, config).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
    let recorder = SessionRecorder::new(project, registry, RATE, project.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    if run.data {
        live = live.with_boundary_data();
    }
    let mut driver = NullDriver::new(RATE, 2, 3);
    driver.start(runner).unwrap();
    for callback in 0..CALLBACKS {
        driver.run(256, || {}).unwrap();
        if run.final_poll || callback + 1 < CALLBACKS {
            controller.poll(&mut live);
        }
    }
    let runner = driver.stop().unwrap().unwrap();
    if run.final_poll {
        controller.poll(&mut live);
    }
    live.finish(runner.next_frame()).unwrap();
    (
        driver.output().to_vec(),
        runner.status(),
        live.into_recorder(),
    )
}

fn written(recorder: &SessionRecorder, name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("boundary_replay")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    recorder.write(&dir).unwrap();
    dir
}

/// The samples of a replay as `audio.wav` stores them.
fn stored(replayed: &signal_project::session::Replay) -> Vec<f32> {
    let render = &replayed.render;
    render
        .samples
        .iter()
        .map(|&x| render.conversion.sample(x))
        .collect()
}

fn ideal(frames: Option<u64>) -> ReplayOptions {
    ReplayOptions {
        frames,
        threads: 1,
        ideal: true,
    }
}

/// The audit's reproduction (finding 2), with base APIs only: a live run that
/// missed results played silence for those blocks, and the replay of its
/// session must equal what was played bit for bit. On the base revision the
/// replay awaited every result and differed.
#[test]
fn live_misses_replay_exactly() {
    let registry = registry();
    let project = project(Some("test.latent"));
    let (played, status, recorder) = live(&project, &registry, PLAIN);
    assert!(status.late_results > 0, "{status:?}");
    let dir = written(&recorder, "exact");
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    let device = |x: f64| {
        if x.is_finite() {
            x.clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    };
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| device(x)).collect();
    let differing = samples
        .iter()
        .zip(&played)
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();
    assert_eq!(samples.len(), played.len());
    assert_eq!(
        differing, 0,
        "replay differs from what was played at {differing} samples"
    );
}

/// C1 and the exact replay's record: every due block is delivered (even
/// blocks) or missed (odd blocks), the replay's log has the live run's
/// `late_result` faults, and it is the same at any thread count.
#[test]
fn every_due_block_is_recorded_and_replays_on_any_thread_count() {
    let registry = registry();
    let project = project(Some("test.latent"));
    let (played, status, recorder) = live(&project, &registry, PLAIN);
    let boundaries = recorder.boundaries().unwrap();
    assert!(boundaries.complete, "{:?}", boundaries.reason);
    let blocks = (CALLBACKS * 256 / BLOCK) as u64 - 1;
    assert_eq!(boundaries.delivered + boundaries.missed, blocks);
    assert_eq!(
        (boundaries.delivered, boundaries.missed),
        (status.delivered_results, status.late_results)
    );
    for record in &boundaries.records {
        let m = record.frame.0 / BLOCK as u64;
        assert_eq!(record.frame.0 % BLOCK as u64, 0);
        let missed = matches!(record.outcome, BoundaryKind::Missed(_));
        assert_eq!(missed, m % 2 == 1, "block {m}");
    }
    let dir = written(&recorder, "threads");
    assert!(dir.join("boundaries.jsonl").exists());
    let session = Session::load(&dir, &registry).unwrap();
    let one = replay(&session, &registry, None, 1).unwrap();
    let four = replay(&session, &registry, None, 4).unwrap();
    assert_eq!(one.boundaries, BoundaryReplay::Exact);
    assert_eq!(stored(&one), played);
    assert_eq!(stored(&four), played);
    assert_eq!(one.render.log, four.render.log);
    let log = String::from_utf8_lossy(&one.render.log);
    assert_eq!(
        log.matches("late_result").count() as u64,
        status.late_results
    );
}

/// The device conversion reaches the artifact: a live run that clipped
/// replays to the same `audio.wav` samples and audio hash, and the manifest
/// says so; non-finite samples are 0 as the device gets them.
#[test]
fn clipped_live_output_replays_exactly() {
    let registry = registry();
    let project = project(Some("test.latent_loud"));
    let (played, status, recorder) = live(&project, &registry, PLAIN);
    assert!(status.clipped > 0, "{status:?}");
    let dir = written(&recorder, "clipped");
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert!(replayed.render.samples.iter().any(|x| x.abs() > 1.0));

    let out = dir.join("artifacts");
    let manifest = replayed.render.write(&out, Default::default()).unwrap();
    let wav = fs::read(out.join("audio.wav")).unwrap();
    assert_eq!(wav_samples(&wav), played);
    let bytes: Vec<u8> = played.iter().flat_map(|x| x.to_le_bytes()).collect();
    assert_eq!(manifest.audio_fnv1a64, format!("{:016x}", fnv1a64(&bytes)));
    let json = serde_json::to_value(&manifest).unwrap();
    assert_eq!(json["device_conversion"], true);
    assert!(json.get("ideal").is_none(), "{json}");

    assert_eq!(device_sample(f64::NAN), 0.0);
    assert_eq!(device_sample(f64::INFINITY), 0.0);
    assert_eq!(device_sample(f64::NEG_INFINITY), 0.0);
    assert_eq!(device_sample(-2.5), -1.0);
}

/// The 32-bit float samples of a WAV file's `data` chunk.
fn wav_samples(wav: &[u8]) -> Vec<f32> {
    let at = wav.windows(4).position(|w| w == b"data").unwrap();
    let len = u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
    wav[at + 8..at + 8 + len]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect()
}

/// C5 / AC1: `--ideal` awaits every result: no `late_result` faults, other
/// audio, and the manifest says `ideal` (and not `device_conversion`).
#[test]
fn ideal_differs_and_is_labelled() {
    let registry = registry();
    let project = project(Some("test.latent"));
    let (played, _, recorder) = live(&project, &registry, PLAIN);
    let dir = written(&recorder, "ideal");
    let session = Session::load(&dir, &registry).unwrap();
    let exact = replay(&session, &registry, None, 1).unwrap();
    let ideal = replay_with(&session, &registry, ideal(None)).unwrap();
    assert_eq!(ideal.boundaries, BoundaryReplay::Ideal);
    assert!(ideal.render.ideal);
    assert_ne!(stored(&ideal), played);
    assert_ne!(ideal.render.log_fnv1a64(), exact.render.log_fnv1a64());
    assert!(!String::from_utf8_lossy(&ideal.render.log).contains("late_result"));
    let json = serde_json::to_value(ideal.render.manifest(Default::default())).unwrap();
    assert_eq!(json["ideal"], true);
    assert!(json.get("device_conversion").is_none(), "{json}");
}

/// C2: with `--record-boundary-data` the delivered results are in the
/// session, and replay shows them: on another "device" (a registry whose
/// `test.latent` computes something else) it still equals what was played.
/// Without the data, that device's replay does not.
#[test]
fn recorded_data_replaces_the_device() {
    let registry = registry();
    let other = registry_with(true);
    let project = project(Some("test.latent"));
    let data = Run {
        data: true,
        data_queue: 4096,
        ..PLAIN
    };
    let (played, _, recorder) = live(&project, &registry, data);
    let boundaries = recorder.boundaries().unwrap();
    assert!(
        boundaries.complete && boundaries.data,
        "{:?}",
        boundaries.reason
    );
    let dir = written(&recorder, "data");
    assert!(dir.join("boundary-data.f32").exists());
    let session = Session::load(&dir, &other).unwrap();
    let replayed = replay(&session, &other, None, 1).unwrap();
    assert_eq!(stored(&replayed), played);

    let (played, _, recorder) = live(&project, &registry, PLAIN);
    let dir = written(&recorder, "no-data");
    let session = Session::load(&dir, &other).unwrap();
    let replayed = replay(&session, &other, None, 1).unwrap();
    assert_ne!(stored(&replayed), played);
}

/// C3 / C6: a record lost to a full queue is detected, the session says it
/// is incomplete, and exact replay fails with a clear error; `--ideal`
/// still renders, labelled. Three ways to lose one: the log queue, the
/// boundary-data queue, and a final runner status that never arrived.
#[test]
fn a_lost_record_fails_exact_replay() {
    let registry = registry();
    let project = project(Some("test.latent"));
    let cases = [
        (
            "log",
            Run {
                log: Some(2),
                ..PLAIN
            },
        ),
        (
            "data",
            Run {
                data: true,
                data_queue: 1,
                ..PLAIN
            },
        ),
        (
            "status",
            Run {
                final_poll: false,
                ..PLAIN
            },
        ),
    ];
    for (name, run) in cases {
        let (_, _, recorder) = live(&project, &registry, run);
        let boundaries = recorder.boundaries().unwrap();
        assert!(!boundaries.complete, "{name}");
        let dir = written(&recorder, &format!("lost-{name}"));
        let text = fs::read_to_string(dir.join("session.toml")).unwrap();
        assert!(text.contains("complete = false"), "{name}: {text}");
        let session = Session::load(&dir, &registry).unwrap();
        let error = match replay(&session, &registry, None, 1) {
            Err(error @ ReplayError::IncompleteBoundaries { .. }) => error.to_string(),
            Err(other) => panic!("{name}: {other}"),
            Ok(_) => panic!("{name}: an incomplete session replayed exactly"),
        };
        assert!(
            error.contains("incomplete") && error.contains("--ideal"),
            "{name}: {error}"
        );
        let ideal = replay_with(&session, &registry, ideal(None)).unwrap();
        assert!(ideal.render.ideal, "{name}");
    }
}

/// AC2: a session without boundary records (written before them) still
/// replays: as an ideal render, which it says. A project without latent
/// modules writes and replays as before: no boundary files, no new keys.
#[test]
fn old_sessions_still_replay() {
    let registry = registry();
    let (_, _, recorder) = live(&project(Some("test.latent")), &registry, PLAIN);
    let dir = written(&recorder, "old");
    fs::remove_file(dir.join("boundaries.jsonl")).unwrap();
    let text = fs::read_to_string(dir.join("session.toml")).unwrap();
    fs::write(
        dir.join("session.toml"),
        without_table(&text, "[boundaries]"),
    )
    .unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    assert!(session.boundaries.is_none());
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(replayed.boundaries, BoundaryReplay::NotRecorded);
    assert!(replayed.render.ideal);

    let (played, _, recorder) = live(&project(None), &registry, PLAIN);
    let dir = written(&recorder, "not-latent");
    assert!(!dir.join("boundaries.jsonl").exists());
    let text = fs::read_to_string(dir.join("session.toml")).unwrap();
    assert!(!text.contains("[boundaries]"), "{text}");
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(replayed.boundaries, BoundaryReplay::NotLatent);
    let plain: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(plain, played);
    let json = serde_json::to_value(replayed.render.manifest(Default::default())).unwrap();
    assert!(json.get("ideal").is_none() && json.get("device_conversion").is_none());
}

/// A malformed `words` (larger than any data, or larger than the data
/// present) is a validation error at its line, never a panic or a huge
/// allocation (auditor-1's probe).
#[test]
fn absurd_words_are_rejected_without_panicking() {
    let registry = registry();
    let data = Run {
        data: true,
        data_queue: 4096,
        ..PLAIN
    };
    let (_, _, recorder) = live(&project(Some("test.latent")), &registry, data);
    for (name, words) in [
        ("huge", "9223372036854775808"),
        ("max", "18446744073709551615"),
        ("beyond-data", "2"),
    ] {
        let dir = written(&recorder, &format!("words-{name}"));
        let path = dir.join("boundaries.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let changed = text.replacen("\"words\":1", &format!("\"words\":{words}"), 1);
        assert_ne!(changed, text);
        fs::write(&path, changed).unwrap();
        let error = match Session::load(&dir, &registry) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("{name}: a session with words {words} loaded"),
        };
        assert!(error.contains("boundaries.jsonl:"), "{name}: {error}");
    }
}

/// M2.7-5: a missed line may carry the cause of a lost result. It is
/// written, read back, and logged by the exact replay on that block's
/// `late_result` line; the other lines keep no cause. An unknown cause, or
/// a cause on a delivered line, is an error at its line.
#[test]
fn a_recorded_cause_round_trips_and_replays_and_a_bad_one_is_refused() {
    let registry = registry();
    let (_, _, mut recorder) = live(&project(Some("test.latent")), &registry, PLAIN);
    let mut boundaries = recorder.boundaries().unwrap().clone();
    let first = boundaries
        .records
        .iter_mut()
        .find(|r| matches!(r.outcome, BoundaryKind::Missed(None)))
        .unwrap();
    first.outcome = BoundaryKind::Missed(Some(LossCause::DeviceLost));
    let frame = first.frame.0;
    recorder.set_boundaries(boundaries.clone());
    let dir = written(&recorder, "cause");
    let text = fs::read_to_string(dir.join("boundaries.jsonl")).unwrap();
    assert_eq!(
        text.matches(",\"cause\":\"device_lost\"}").count(),
        1,
        "{text}"
    );
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(
        session.boundaries.as_ref().map(|b| &b.records),
        Some(&boundaries.records)
    );
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(replayed.boundaries, BoundaryReplay::Exact);
    let log = String::from_utf8(replayed.render.log.clone()).unwrap();
    let late: Vec<&str> = log.lines().filter(|l| l.contains("late_result")).collect();
    assert_eq!(late.len() as u64, boundaries.missed);
    for line in late {
        let has = line.contains(",\"cause\":\"device_lost\"}");
        assert_eq!(has, line.contains(&format!("\"frame\":{frame},")), "{line}");
    }

    for (name, from, to) in [
        (
            "unknown",
            "\"outcome\":\"missed\"}",
            "\"outcome\":\"missed\",\"cause\":\"gremlins\"}",
        ),
        (
            "on-delivered",
            "\"outcome\":\"delivered\"}",
            "\"outcome\":\"delivered\",\"cause\":\"mapping\"}",
        ),
    ] {
        let dir = written(&recorder, &format!("cause-{name}"));
        let path = dir.join("boundaries.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let changed = text.replacen(from, to, 1);
        assert_ne!(changed, text, "{name}");
        fs::write(&path, changed).unwrap();
        let error = match Session::load(&dir, &registry) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("{name}: loaded"),
        };
        assert!(
            error.contains("boundaries.jsonl:") && error.contains("cause"),
            "{name}: {error}"
        );
    }
}

/// An exact replay refuses frames past the recorded end, which were never
/// played; `--ideal` renders them.
#[test]
fn exact_replay_refuses_frames_past_the_recorded_end() {
    let registry = registry();
    let (_, _, recorder) = live(&project(Some("test.latent")), &registry, PLAIN);
    let dir = written(&recorder, "past-end");
    let session = Session::load(&dir, &registry).unwrap();
    let end = session.frames.unwrap();
    assert!(replay(&session, &registry, Some(end), 1).is_ok());
    match replay(&session, &registry, Some(end + 256), 1) {
        Err(error @ ReplayError::BeyondRecordedEnd { .. }) => {
            let message = error.to_string();
            assert!(message.contains("--ideal"), "{message}");
        }
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("an exact replay rendered past the recorded end"),
    }
    let ideal = replay_with(&session, &registry, ideal(Some(end + 256))).unwrap();
    assert_eq!(ideal.render.frames, end + 256);
}

/// `text` without the TOML table `header` (up to the next table).
fn without_table(text: &str, header: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if line.starts_with('[') {
            skipping = line == header;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

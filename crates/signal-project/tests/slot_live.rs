//! ADR 0037 test 5: a slot's Load, Unload and Replace as live edits
//! (`signal_project::live::send_edit`), on the null driver's real-time run
//! in seeded callback sizes, on 1 and 4 threads. A Load is one swap whose
//! newcomer fades in; an Unload suspends, then swaps at *s* + *F*; a Replace
//! is an unload, then a load a frame later: the old content reaches +0.0 at
//! *s* + *F* − 1, the new one fades in from a fresh state although both are
//! `/deck/osc`, a `std.sine_osc`, and the host's own sound is bit-continuous.
//! Each session replays to what was played (the Replace's middle version is
//! the slot unloaded, as `slot.unload` writes it). An interface edit that
//! changes no match is no edit at all. ADR 0045 test 5 (phase 1): a rewire
//! that retypes an auto port is one ordinary swap (a converter, sources
//! changed, survivors bit-continuous), one into a shape conflict is refused,
//! and a Replace that retypes an empty slot's auto output is still an
//! unload, then a load. The callbacks' allocations are
//! counted in `signal-output`'s `chained_swaps.rs`, which drives the same two
//! swaps.

use std::path::PathBuf;

use signal_core::Registry;
use signal_engine::{RealtimeConfig, Stamp};
use signal_output::{Controller, Driver, NullDriver, realtime};
use signal_project::live::{Applied, send_edit};
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;
/// *F*: 5 ms at 48 kHz.
const F: usize = 240;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn voice(name: &str, freq: f64) -> SourceFile {
    SourceFile {
        path: format!("composites/{name}.toml"),
        text: format!(
            "schema_version = 1\n\n[composite]\nname = \"{name}\"\n\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\n\n[exports]\noutputs = {{ out = \"osc.out\" }}\n"
        ),
    }
}

/// The host: its `tone` on the left; the slot `deck` (one audio output
/// `out`, and `extra` declared inputs nothing wires) loading `load` (or
/// empty) on the right; two voices to load, `voice_a` (220 Hz) and
/// `voice_b` (440 Hz), each an `osc` at `/deck/osc`.
fn host(load: Option<&str>, extra: &str) -> Project {
    let load = load.map_or(String::new(), |l| format!(", load = \"{l}\""));
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"slots\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         # The slot.\n\
         deck = {{ slot = {{ outputs = {{ out = \"audio\" }}{extra} }}{load} }}\n\
         out = {{ module = \"std.audio_out\" }}\n\n[wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.l\" }},\n  {{ from = \"deck.out\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text,
        },
        voice("voice_a", 220.0),
        voice("voice_b", 440.0),
    ])
    .unwrap()
}

struct Run {
    driver: NullDriver,
    controller: Controller,
    live: LiveRecorder,
    /// Commands may be late (a test that stamps them so).
    late: bool,
}

impl Run {
    fn start(project: &Project, registry: &Registry, threads: usize, seed: u64) -> Run {
        let mut plan = project.plan(registry).unwrap().plan;
        plan.version = 0;
        // Commands are stamped two 4096-frame buffers ahead: no callback
        // size makes one late.
        let mut config = RealtimeConfig::with_buffer(RATE, project.seed, 4096);
        config.engine.threads = threads;
        let (runner, controller) = realtime(&plan, registry, config).unwrap();
        let provenance = provenance::collect(&provenance::source_dir(), &project.files);
        let recorder =
            SessionRecorder::new(project, registry, RATE, project.seed, provenance).unwrap();
        let mut driver = NullDriver::new(RATE, 2, seed);
        driver.start(runner).unwrap();
        Run {
            driver,
            controller,
            live: LiveRecorder::new(recorder, registry.clone()),
            late: false,
        }
    }

    /// Plays at least `frames` more frames, polling after every callback.
    fn play(&mut self, frames: u64) {
        let (controller, live) = (&mut self.controller, &mut self.live);
        self.driver.run(frames, || controller.poll(live)).unwrap();
        self.controller.poll(&mut self.live);
    }

    /// Stops: what was played (left, right), the recorded edits, and the
    /// session's replay as f32.
    fn finish(mut self, registry: &Registry, name: &str) -> Played {
        while self.controller.in_flight() > 0 {
            self.play(1024);
        }
        self.play(1024);
        let played = self.driver.take_output();
        let runner = self.driver.stop().unwrap().unwrap();
        self.controller.poll(&mut self.live);
        self.live.finish(runner.next_frame()).unwrap();
        assert_eq!(self.live.refused(), &[] as &[String]);
        if !self.late {
            assert_eq!(runner.status().late, 0, "nothing late");
        }
        let recorder = self.live.into_recorder();
        let edits = recorder.edits();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("slot-live/{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        recorder.write(&dir).unwrap();
        let session = Session::load(&dir, registry).unwrap();
        let replayed = replay(&session, registry, None, 1).unwrap();
        let replayed: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
        assert_eq!(replayed, played, "{name}: replay equals what was played");
        Played {
            left: played.chunks(2).map(|c| c[0]).collect(),
            right: played.chunks(2).map(|c| c[1]).collect(),
            edits,
        }
    }
}

struct Played {
    left: Vec<f32>,
    right: Vec<f32>,
    edits: Vec<session::Edit>,
}

impl Played {
    /// Each edit's kind and frame.
    fn edits(&self) -> Vec<(&'static str, usize)> {
        self.edits
            .iter()
            .map(|e| {
                let kind = match e.command {
                    session::Command::Swap { .. } => "swap",
                    session::Command::Edit { .. } => "edit",
                    session::Command::Event { .. } => "event",
                };
                (kind, e.frame.0 as usize)
            })
            .collect()
    }
}

/// A fresh `std.sine_osc` at `freq` from its first frame, as f64 (what the
/// slot's content plays before the sink's conversion).
fn fresh(freq: f64, frames: u64) -> Vec<f64> {
    let registry = registry();
    let text = format!(
        "schema_version = 1\n[project]\nname = \"fresh\"\nsample_rate = {RATE}\nseed = 5\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\nout = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [ {{ from = \"osc.out\", to = \"out.r\" }} ]\n"
    );
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let mut engine = signal_engine::Engine::new(
        &plan,
        &registry,
        signal_engine::EngineConfig::new(RATE, 5),
        Vec::new(),
    )
    .unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    rendered.samples.chunks(2).map(|f| f[1]).collect()
}

/// Frame `j` of a fade-in of `F` frames to `live`, as the sink plays it.
fn faded_in(live: f64, j: usize) -> f32 {
    let done = j + 1;
    if done >= F {
        live as f32
    } else {
        (live * (done as f64 / F as f64)) as f32
    }
}

/// The host's tone alone, as played on the left.
fn tone(frames: u64) -> Vec<f32> {
    fresh(330.0, frames).iter().map(|&x| x as f32).collect()
}

#[test]
fn a_replace_unloads_then_loads_with_no_state_carried() {
    let registry = registry();
    let a = fresh(220.0, 60_000);
    let b = fresh(440.0, 60_000);
    let tone = tone(60_000);
    for (threads, seed) in [(1, 11), (4, 12)] {
        let old = host(Some("voice_a"), "");
        let mut run = Run::start(&old, &registry, threads, seed);
        run.play(3_000);
        let new = host(Some("voice_b"), "");
        let (_, applied) =
            send_edit(&mut run.controller, &mut run.live, &old, new, &registry).unwrap();
        assert_eq!(
            applied,
            Applied {
                sets: 0,
                swap: Some(1),
                removed: 1,
                replaced: 1,
                load_swap: Some(2),
                unfinished: false,
            }
        );
        assert_eq!(
            applied.to_string(),
            "replaced 1 slot content: unloaded by a swap to version 1, loaded by a swap to version 2 (0 sets, 1 removed)"
        );
        run.play(30_000);
        let played = run.finish(&registry, &format!("replace-{threads}"));
        let edits = played.edits();
        assert_eq!(
            edits.iter().map(|e| e.0).collect::<Vec<_>>(),
            ["edit", "swap", "swap"]
        );
        let s = edits[0].1;
        assert_eq!(edits[1].1, s + F, "the unload after the fade");
        assert_eq!(edits[2].1, s + F + 1, "the load a frame later");
        // The host's own sound is bit-continuous.
        assert_eq!(played.left, tone[..played.left.len()]);
        // The old voice until s, faded out to +0.0 at s + F − 1; the slot
        // empty at s + F.
        assert_eq!(played.right[s - 1], a[s - 1] as f32);
        assert!(
            played.right[s + F - 1..=s + F]
                .iter()
                .all(|x| x.to_bits() == 0),
            "+0.0 at s + F − 1 and s + F"
        );
        // The new voice from its own first frame, fading in: a fresh
        // oscillator, not the old one's phase at 440 Hz.
        for (j, &live) in b.iter().enumerate().take(2 * F) {
            assert_eq!(played.right[s + F + 1 + j], faded_in(live, j), "j {j}");
        }
    }
}

#[test]
fn a_load_fades_in_and_an_unload_fades_out() {
    let registry = registry();
    let a = fresh(220.0, 60_000);
    let tone = tone(60_000);
    for (threads, seed) in [(1, 21), (4, 22)] {
        // Load: one swap, the newcomer fading in.
        let empty = host(None, "");
        let mut run = Run::start(&empty, &registry, threads, seed);
        run.play(2_000);
        let loaded = host(Some("voice_a"), "");
        let (now, applied) = send_edit(
            &mut run.controller,
            &mut run.live,
            &empty,
            loaded,
            &registry,
        )
        .unwrap();
        assert_eq!((applied.swap, applied.load_swap), (Some(1), None));
        assert_eq!((applied.removed, applied.replaced), (0, 0));
        run.play(12_000);

        // Unload: suspended, then swapped out after the fade.
        let (_, applied) = send_edit(
            &mut run.controller,
            &mut run.live,
            &now,
            host(None, ""),
            &registry,
        )
        .unwrap();
        assert_eq!((applied.swap, applied.removed), (Some(2), 1));
        run.play(12_000);
        let played = run.finish(&registry, &format!("load-unload-{threads}"));
        let edits = played.edits();
        assert_eq!(
            edits.iter().map(|e| e.0).collect::<Vec<_>>(),
            ["swap", "edit", "swap"]
        );
        let load = edits[0].1;
        assert!(played.right[..load].iter().all(|&x| x == 0.0));
        for (j, &live) in a.iter().enumerate().take(2 * F) {
            assert_eq!(played.right[load + j], faded_in(live, j), "j {j}");
        }
        let s = edits[1].1;
        assert_eq!(edits[2].1, s + F);
        assert!(played.right[s] != 0.0);
        assert!(
            played.right[s + F - 1..].iter().all(|&x| x.to_bits() == 0),
            "+0.0 from s + F − 1 on"
        );
        assert_eq!(played.left, tone[..played.left.len()]);
    }
}

/// A Replace's two swaps stamped for one frame already played (a lead too
/// short for the runner): both are late, and they commit on two
/// consecutive frames, never one (one swap per frame); the session records
/// them where they applied and replays to what was played. (No suspend is
/// sent: a suspend as late as its swap lands on the swap's frame, after the
/// swap removed its instance, and is discarded, so the removal has no fade;
/// `send_edit` stamps the swap F after the suspend from a fresh status, so
/// only a control thread stalled for more than the latency gets there.)
#[test]
fn late_swaps_of_a_replace_commit_on_consecutive_frames_and_replay() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let mut run = Run::start(&old, &registry, 1, 41);
    run.late = true;
    run.play(5_000);
    let mid = host(None, "");
    let new = host(Some("voice_b"), "");
    let unload = run.live.swap_plan(mid).unwrap();
    let load = run.live.swap_plan(new).unwrap();
    let ready = run.controller.prepare_swap(&unload).unwrap();
    let after = run.controller.prepare_swap_after(&ready, &load).unwrap();
    let past = signal_core::Frame(100);
    run.controller.send_swap(ready, Stamp::At(past)).unwrap();
    run.controller.send_swap(after, Stamp::At(past)).unwrap();
    run.play(4_000);
    let played = run.finish(&registry, "late");
    let edits = played.edits();
    assert_eq!(
        edits.iter().map(|e| e.0).collect::<Vec<_>>(),
        ["swap", "swap"]
    );
    assert!(
        edits[0].1 >= 5_000,
        "late: applied on the next frame to run"
    );
    assert_eq!(edits[1].1, edits[0].1 + 1, "the load on the next frame");
}

/// An own copy of the content under another name (`voice_a_inside`, what
/// the editor's "edit the slot's insides" writes) plays alike: pointing
/// `load` at it is no Replace and sends nothing, so making the copy is
/// inaudible; a copy that differs (another frequency) is a Replace.
#[test]
fn loading_an_identical_copy_is_no_replace() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let with_copy = |freq: f64| {
        let mut files = host(Some("voice_a"), "").files;
        files[0].text = files[0]
            .text
            .replace("load = \"voice_a\"", "load = \"voice_a_inside\"");
        files.push(voice("voice_a_inside", freq));
        Project::from_sources(files).unwrap()
    };
    let mut run = Run::start(&old, &registry, 1, 51);
    run.play(2_000);
    let (now, applied) = send_edit(
        &mut run.controller,
        &mut run.live,
        &old,
        with_copy(220.0),
        &registry,
    )
    .unwrap();
    assert_eq!(applied, Applied::default(), "{applied}");
    run.play(2_000);
    let (_, applied) = send_edit(
        &mut run.controller,
        &mut run.live,
        &now,
        {
            // Another copy that differs: a Replace.
            let mut files = now.files.clone();
            files.push(voice("voice_a_other", 330.0));
            files[0].text = files[0]
                .text
                .replace("load = \"voice_a_inside\"", "load = \"voice_a_other\"");
            Project::from_sources(files).unwrap()
        },
        &registry,
    )
    .unwrap();
    assert_eq!(
        (applied.replaced, applied.load_swap),
        (1, Some(2)),
        "{applied}"
    );
    let played = run.finish(&registry, "own-copy");
    assert_eq!(
        played.edits().iter().map(|e| e.0).collect::<Vec<_>>(),
        ["edit", "swap", "swap"],
        "only the real Replace swaps"
    );
}

/// An interface edit that changes no match (an input nothing wires) plans
/// byte-identically: no swap, nothing sent.
#[test]
fn an_interface_edit_that_changes_no_match_is_no_edit() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let new = host(Some("voice_a"), ", inputs = { cue = \"gate\" }");
    let mut run = Run::start(&old, &registry, 1, 31);
    run.play(1_000);
    let (_, applied) = send_edit(&mut run.controller, &mut run.live, &old, new, &registry).unwrap();
    assert_eq!(applied, Applied::default());
    assert_eq!(applied.to_string(), "no change to apply");
    let played = run.finish(&registry, "interface");
    assert!(played.edits.is_empty());
}

/// `project` with `slot.inside` applied to slot `slot` (ADR 0037, editable
/// insides) as the host applies it: the level's file and the new inside.
fn own_inside(project: &Project, slot: &str, registry: &Registry) -> Project {
    use signal_project::patch;
    let files = project.files.clone();
    let module = |m: &str| -> Option<Vec<String>> {
        registry.get(m).map(|t| {
            t.descriptor
                .parameters
                .iter()
                .map(|p| p.name.to_owned())
                .collect()
        })
    };
    let templates = signal_project::interface_templates(&files);
    let content = |load: &str| {
        signal_project::content_names(Some(project), std::path::Path::new("."), load, registry)
    };
    let file = |path: &str| {
        files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let source = |load: &str| signal_project::inside::source(&files, load, registry);
    let ctx = patch::Context {
        level: "",
        module: &module,
        templates: &templates,
        content: &content,
        file: &file,
        source: &source,
    };
    let op: patch::Op =
        serde_json::from_value(serde_json::json!({"op": "slot.inside", "slot": slot})).unwrap();
    let patched = patch::apply(&files[0].text, None, &[op], &ctx).unwrap();
    let mut edited = files.clone();
    edited[0].text = patched.text;
    for change in patched.files {
        edited.retain(|f| f.path != change.path);
        if let Some(text) = change.after {
            edited.push(SourceFile {
                path: change.path,
                text,
            });
        }
    }
    Project::from_sources(edited).unwrap()
}

/// ADR 0037, editable insides, with phase 2's rule: a slot's own copy of a
/// composite made by `slot.inside` plans alike, so it is no Replace and
/// nothing is sent; an edit inside the own inside is an ordinary live edit
/// (no Replace); a primitive wrapped into its own inside plans otherwise
/// (one level down), so it is a proper Replace.
#[test]
fn an_own_inside_is_no_replace_and_its_edits_are_ordinary() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let mut run = Run::start(&old, &registry, 1, 61);
    run.play(2_000);
    let copied = own_inside(&old, "deck", &registry);
    assert!(
        copied
            .files
            .iter()
            .any(|f| f.path == "composites/deck_inside.toml")
    );
    let (now, applied) =
        send_edit(&mut run.controller, &mut run.live, &old, copied, &registry).unwrap();
    assert_eq!(applied, Applied::default(), "the copy: {applied}");
    run.play(2_000);
    // Inside it: the oscillator retuned, a parameter edit of the content.
    let mut files = now.files.clone();
    let inside = files
        .iter_mut()
        .find(|f| f.path == "composites/deck_inside.toml")
        .unwrap();
    inside.text = inside.text.replace("freq = 220", "freq = 330");
    let retuned = Project::from_sources(files).unwrap();
    let (_, applied) =
        send_edit(&mut run.controller, &mut run.live, &now, retuned, &registry).unwrap();
    assert_eq!((applied.replaced, applied.sets), (0, 1), "{applied}");
    let played = run.finish(&registry, "own-inside");
    assert_eq!(
        played.edits().iter().map(|e| e.0).collect::<Vec<_>>(),
        ["event"],
        "the copy sends nothing; the edit inside is one parameter set, no swap"
    );

    // A primitive wrapped: its plan moves one level down, a Replace.
    let wrapped_host = |load: &str| {
        let mut files = host(None, "").files;
        files[0].text = files[0].text.replace(
            "deck = { slot = { outputs = { out = \"audio\" } } }",
            &format!(
                "deck = {{ slot = {{ outputs = {{ out = \"audio\" }} }}, load = \"{load}\" }}"
            ),
        );
        Project::from_sources(files).unwrap()
    };
    let old = wrapped_host("std.sine_osc");
    let mut run = Run::start(&old, &registry, 1, 62);
    run.play(2_000);
    let wrapped = own_inside(&old, "deck", &registry);
    let (_, applied) =
        send_edit(&mut run.controller, &mut run.live, &old, wrapped, &registry).unwrap();
    assert_eq!(
        (applied.replaced, applied.load_swap.is_some()),
        (1, true),
        "{applied}"
    );
    run.finish(&registry, "own-inside-wrap");
}

// ADR 0045 test 5 (phase 1, without `retyped`): an inferred type never
// reaches the plan, so it sends nothing of its own; what is sent is the
// edit that caused it.

/// `test.watch`: its `seen` is 1 on a call's first frame when its `in` had
/// its sources changed by a swap (`Inputs::sources_changed`), else 0.
const WATCH: signal_core::ModuleDescriptor = signal_core::ModuleDescriptor {
    type_name: "test.watch",
    doc: "fixture: shows when a swap changed its input's sources",
    ports: &[
        signal_core::PortDescriptor {
            name: "in",
            direction: signal_core::Direction::In,
            shape: signal_core::PortShape::Signal,
            rate: signal_core::RateDivisor::ONE,
            kind: signal_core::PortKind::Control,
            unit: None,
            range: None,
        },
        signal_core::PortDescriptor {
            name: "seen",
            direction: signal_core::Direction::Out,
            shape: signal_core::PortShape::Signal,
            rate: signal_core::RateDivisor::ONE,
            kind: signal_core::PortKind::Audio,
            unit: None,
            range: None,
        },
    ],
    parameters: &[],
    feedthrough: signal_core::Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: None,
};

#[derive(Default)]
struct Watch;

impl signal_core::DirectPrimitive for Watch {
    fn describe() -> signal_core::ModuleDescriptor {
        WATCH
    }
    fn prepare(&mut self, _: &signal_core::PrepareContext) {}
    fn reset(&mut self) {}
    fn process(
        &mut self,
        inputs: &signal_core::Inputs<'_>,
        outputs: &mut signal_core::Outputs<'_>,
    ) {
        let seen = inputs.sources_changed(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = if n == 0 && seen { 1.0 } else { 0.0 };
        }
    }
}

/// The std library, `test.watch`, and `test.slow` (`std.const` at rate
/// divisor 64).
fn auto_registry() -> Registry {
    let mut registry = registry();
    registry
        .register(signal_core::ModuleType {
            descriptor: WATCH,
            create: || signal_core::Primitive::Direct(Box::new(Watch)),
        })
        .unwrap();
    let constant = *registry.get("std.const").unwrap();
    let mut descriptor = constant.descriptor;
    let slow = signal_core::RateDivisor::new(64).unwrap();
    descriptor.type_name = "test.slow";
    descriptor.ports = Box::leak(
        descriptor
            .ports
            .iter()
            .map(|p| signal_core::PortDescriptor { rate: slow, ..*p })
            .collect(),
    );
    descriptor.parameters = Box::leak(
        descriptor
            .parameters
            .iter()
            .map(|p| signal_core::ParameterDescriptor { rate: slow, ..*p })
            .collect(),
    );
    registry
        .register(signal_core::ModuleType {
            descriptor,
            create: constant.create,
        })
        .unwrap();
    registry
}

/// A host whose `deck` (auto `in` and `seen`) loads `test.watch`, fed by
/// `source` (`c`, a `std.const`; `s`, a `test.slow`; `e`, a `std.edge`'s
/// `rise`), its `seen` on the right, the tone on the left.
fn watched(source: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"auto\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         c = {{ module = \"std.const\", value = 0.25 }}\n\
         s = {{ module = \"test.slow\", value = 0.25 }}\n\
         e = {{ module = \"std.edge\" }}\n\
         deck = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ seen = {{}} }} }}, load = \"test.watch\" }}\n\
         out = {{ module = \"std.audio_out\" }}\n\n[wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.l\" }},\n  {{ from = \"{source}\", to = \"deck.in\" }},\n  {{ from = \"deck.seen\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// A rewire that changes an auto input's inferred rate (a `std.const` at 1
/// to a `test.slow` at 64) is one ordinary swap, which brings the hold
/// converter; the content survives and sees its input's sources changed
/// at the swap's frame; the host's tone is bit-continuous.
#[test]
fn a_rewire_that_retypes_an_auto_input_is_one_ordinary_swap() {
    let registry = auto_registry();
    let tone = tone(60_000);
    for (threads, seed) in [(1, 71), (4, 72)] {
        let old = watched("c.out");
        let new = watched("s.out");
        let planned = new.plan(&registry).unwrap();
        let deck = &planned.slots[0];
        assert_eq!(
            deck.interface.port("in").unwrap().rate,
            64,
            "auto, from /s.out"
        );
        assert!(
            planned
                .plan
                .instances
                .iter()
                .any(|i| i.address.as_str() == "/s/out_d1"),
            "the hold converter"
        );
        let mut run = Run::start(&old, &registry, threads, seed);
        run.play(3_000);
        let (_, applied) =
            send_edit(&mut run.controller, &mut run.live, &old, new, &registry).unwrap();
        assert_eq!(
            (
                applied.swap,
                applied.replaced,
                applied.load_swap,
                applied.removed
            ),
            (Some(1), 0, None, 0),
            "{applied}"
        );
        run.play(6_000);
        let played = run.finish(&registry, &format!("auto-rewire-{threads}"));
        let edits = played.edits();
        assert_eq!(edits.iter().map(|e| e.0).collect::<Vec<_>>(), ["swap"]);
        let s = edits[0].1;
        assert_eq!(played.left, tone[..played.left.len()], "bit-continuous");
        assert_eq!(played.right[s], 1.0, "sources changed at the swap");
        assert!(
            played
                .right
                .iter()
                .enumerate()
                .all(|(n, &x)| n == s || x == 0.0),
            "and nowhere else"
        );
    }
}

/// A rewire that makes a shape conflict (events into the content's signal
/// input, through the auto port) is refused, and nothing is sent.
#[test]
fn a_rewire_into_a_shape_conflict_is_refused() {
    let registry = auto_registry();
    let old = watched("c.out");
    let mut run = Run::start(&old, &registry, 1, 73);
    run.play(1_000);
    let refused = send_edit(
        &mut run.controller,
        &mut run.live,
        &old,
        watched("e.rise"),
        &registry,
    )
    .err()
    .expect("refused");
    assert!(
        refused.to_string().contains(
            "/deck.in (auto, from /e.rise) is events; it reaches test.watch's in, a signal"
        ),
        "{refused}"
    );
    let played = run.finish(&registry, "auto-conflict");
    assert!(played.edits.is_empty());
}

/// A composite `name` whose events input `in` reaches `e.<port>` of a
/// `module` (`std.counter`'s `trigger`, kind trigger; `std.latch`'s `in`,
/// kind generic) and whose `out` is a sine at `freq` (through a `std.gain`
/// `g`).
fn deck_content(name: &str, module: &str, port: &str, freq: f64) -> SourceFile {
    SourceFile {
        path: format!("composites/{name}.toml"),
        text: format!(
            "schema_version = 1\n\n[composite]\nname = \"{name}\"\n\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\ng = {{ module = \"std.gain\" }}\ne = {{ module = \"{module}\" }}\n\n[wiring]\nconnections = [ {{ from = \"osc.out\", to = \"g.in\" }} ]\n\n[exports]\ninputs = {{ in = \"e.{port}\" }}\noutputs = {{ out = \"g.out\" }}\n"
        ),
    }
}

/// The host of a Replace: `deck` (auto `in`) loads `load`; the empty slot
/// `spare`'s auto `out` feeds `deck.in` and `seq.reset` (events). Loaded,
/// `spare.out` is typed by `deck.in` (the content's `in`, its first
/// consumer by `str::cmp`, stage 4); in the Replace's middle version
/// (`deck` empty) `deck.in` is open, so no known end, and `spare.out` is
/// typed by `seq.reset`. Were an open end known as the default (a
/// signal), the middle version would fail the agreement check at
/// `seq.reset`.
fn retyping_host(load: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"retype\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         spare = {{ slot = {{ outputs = {{ out = {{}} }} }} }}\n\
         deck = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = \"audio\" }} }}, load = \"{load}\" }}\n\
         seq = {{ module = \"std.counter\" }}\n\
         out = {{ module = \"std.audio_out\" }}\n\n[wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.l\" }},\n  {{ from = \"spare.out\", to = \"deck.in\" }},\n  {{ from = \"spare.out\", to = \"seq.reset\" }},\n  {{ from = \"deck.out\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text,
        },
        deck_content("trigger_in", "std.counter", "trigger", 220.0),
        deck_content("generic_in", "std.latch", "in", 440.0),
    ])
    .unwrap()
}

/// A Replace that retypes another slot's empty auto output is still an
/// unload, then a load; the retype adds no swap. Its middle version (the
/// slot empty) leaves `deck.in` open, which is no known end, so it is not
/// refused by the agreement check (ADR 0045 test 3).
#[test]
fn a_replace_that_retypes_an_empty_auto_output_is_still_unload_then_load() {
    let registry = registry();
    let tone = tone(60_000);
    let spare = |project: &Project| {
        let planned = project.plan(&registry).unwrap_or_else(|e| panic!("{e:#?}"));
        let port = planned.slots.iter().find(|s| s.slot == "/spare").unwrap();
        let out = port.interface.port("out").unwrap().clone();
        (out.kind, out.auto.unwrap().from.port)
    };
    let old = retyping_host("trigger_in");
    let new = retyping_host("generic_in");
    use signal_core::PortKind;
    let from = |p: &str| Some(p.to_owned());
    assert_eq!(spare(&old), (PortKind::Trigger, from("/deck.in")));
    assert_eq!(spare(&new), (PortKind::Generic, from("/deck.in")));
    let mut emptied = retyping_host("trigger_in").files;
    emptied[0].text = emptied[0].text.replace(", load = \"trigger_in\"", "");
    let middle = Project::from_sources(emptied).unwrap();
    assert_eq!(spare(&middle), (PortKind::Trigger, from("/seq.reset")));
    for (threads, seed) in [(1, 81), (4, 82)] {
        let mut run = Run::start(&old, &registry, threads, seed);
        run.play(3_000);
        let (_, applied) = send_edit(
            &mut run.controller,
            &mut run.live,
            &old,
            retyping_host("generic_in"),
            &registry,
        )
        .unwrap();
        assert_eq!(
            (applied.replaced, applied.swap, applied.load_swap),
            (1, Some(1), Some(2)),
            "{applied}"
        );
        run.play(30_000);
        let played = run.finish(&registry, &format!("auto-replace-{threads}"));
        let edits = played.edits();
        // The content's three instances suspended, then the two swaps.
        assert_eq!(
            edits.iter().map(|e| e.0).collect::<Vec<_>>(),
            ["edit", "edit", "edit", "swap", "swap"]
        );
        let s = edits[0].1;
        assert_eq!(
            (edits[1].1, edits[2].1, edits[3].1, edits[4].1),
            (s, s, s + F, s + F + 1)
        );
        assert_eq!(played.left, tone[..played.left.len()]);
    }
}

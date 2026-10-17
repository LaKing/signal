//! M2.11-1 through the engine (ADR 0028 §4–6): the transport's `tick`
//! clock, `std.note_seq` and `std.step_seq8` playing pattern files, a live
//! pattern edit, and its session. Every project here is a fixture written by
//! the test; no committed project is touched.
//!
//! - Notes and steps fire on the tick formula's frames at two tempos and
//!   across a tempo change; loops wrap, a note crossing the loop end ends in
//!   the next loop, swing delays the even steps, accents scale the level.
//! - A pattern edit is a swap at an exact frame that keeps the sequencer's
//!   position, bit-identical for k = 1, 64, 256 on 1, 2, 4 and 8 threads, and
//!   a live run's session of it replays to what was played.
//! - At 30 Hz (21 or 22 ticks in a frame) every tick is counted on its frame
//!   and an unconnected `tick` faults nothing.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use signal_core::{EventPayload, Frame, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent, RealtimeConfig, Schedule};
use signal_output::{Driver, NullDriver, Render, realtime, render_blocks};
use signal_project::live::reload;
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile, provenance};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// `round_half_up(numerator / denominator)`, written out independently.
fn exact(numerator: u128, denominator: u128) -> u64 {
    let (q, r) = (numerator / denominator, numerator % denominator);
    (if 2 * r >= denominator { q + 1 } else { q }) as u64
}

/// The frame of tick `g` at a constant tempo from frame 0 (96 ppq).
fn tick_frame(g: u64, sample_rate: u64, bpm: u64) -> u64 {
    exact(
        u128::from(g) * 60_000 * u128::from(sample_rate),
        u128::from(bpm * 1000 * 96),
    )
}

/// A project at `rate`: a transport (`bpm`), the sequencers on its ticks
/// naming `melody` and `beat`, `count` probes on their event outputs, the
/// melody's position (÷16) and gate into the sink, and `extra` lines.
fn project_text(rate: u32, bpm: f64, extra: &str, wiring: &str) -> String {
    format!(
        r#"schema_version = 1

[project]
name = "patterns"
sample_rate = {rate}
seed = 9

[instances]
clock = {{ module = "std.transport", bpm = {bpm} }}
melody = {{ module = "std.note_seq", pattern = "melody" }}
drums = {{ module = "std.step_seq8", pattern = "beat", swing = 50 }}
on = {{ module = "std.count" }}
off = {{ module = "std.count" }}
kick = {{ module = "std.count" }}
hat = {{ module = "std.count" }}
accent = {{ module = "std.count" }}
env = {{ module = "std.exp_env" }}
out = {{ module = "std.audio_out" }}
{extra}
[wiring]
connections = [
  {{ from = "clock.tick", to = "melody.tick" }},
  {{ from = "clock.tick", to = "drums.tick" }},
  {{ from = "melody.on", to = "on.in" }},
  {{ from = "melody.off", to = "off.in" }},
  {{ from = "drums.t1", to = "kick.in" }},
  {{ from = "drums.t2", to = "hat.in" }},
  {{ from = "drums.accent", to = "accent.in" }},
  {{ from = "drums.t1", to = "env.trigger" }},
  {{ from = "melody.position", to = "out.l", scale = 0.0625 }},
  {{ from = "env.out", to = "out.r" }},
{wiring}]
"#
    )
}

/// Notes at beats 0, 1 and 2.5; one at 3.75 crossing the 4-beat loop end
/// would be taken over by the note at 0, so the crossing case has its own
/// pattern ([`CROSSING`]).
const MELODY: &str = r#"schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 60, start = 0, length = 0.5, velocity = 1 },
  { key = 62, start = 1, length = 0.5, velocity = 0.8 },
  { key = 64, start = 2.5, length = 0.5, velocity = 0.6 },
]
"#;

/// One note at 1.5 beats for 1 beat in a 2-beat loop: it crosses the loop
/// end and ends at 0.5 of the next loop.
const CROSSING: &str = r#"schema_version = 1
kind = "notes"
length = 2
notes = [{ key = 69, start = 1.5, length = 1, velocity = 1 }]
"#;

/// 16ths: the kick on 1 and an accented 3; the hats on every step.
const BEAT: &str = r#"schema_version = 1
kind = "steps"
steps = 4
step = 0.25

[lanes]
kick = "x.X."
hat = "xxxx"

[levels]
kick = 0.8
hat = 0.5
"#;

fn project(text: &str, melody: &str) -> Project {
    project_with(text, melody, BEAT)
}

fn project_with(text: &str, melody: &str, beat: &str) -> Project {
    Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text: text.to_owned(),
        },
        SourceFile {
            path: "patterns/melody.toml".to_owned(),
            text: melody.to_owned(),
        },
        SourceFile {
            path: "patterns/beat.toml".to_owned(),
            text: beat.to_owned(),
        },
    ])
    .unwrap()
}

fn config(rate: u32, threads: usize) -> EngineConfig {
    EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(rate, 9)
    }
}

/// Renders `frames` of `first` in blocks of `k` on `threads` threads, with
/// `external` events, swapping to `next` at frame `at` if given.
fn render(
    first: &Project,
    next: Option<(&Project, u64)>,
    frames: u64,
    (k, threads): (usize, usize),
    external: Vec<ExternalEvent>,
) -> Render {
    let registry = registry();
    let plan = first.plan(&registry).unwrap().plan;
    let mut engine = Engine::new(
        &plan,
        &registry,
        config(first.sample_rate, threads),
        external,
    )
    .unwrap();
    let mut swap = next.map(|(project, at)| {
        let mut plan = project.plan(&registry).unwrap().plan;
        plan.version = 1;
        (engine.prepare_swap(&plan, &registry).unwrap(), at)
    });
    let render = render_blocks(&mut engine, frames, k, |engine| match &mut swap {
        Some((prepared, at)) if !prepared.is_committed() => {
            if engine.next_frame().0 == *at {
                engine.commit_swap(prepared, Frame(*at))?;
                Ok(None)
            } else {
                Ok(Some(Frame(*at)))
            }
        }
        _ => Ok(None),
    })
    .unwrap();
    assert_eq!(engine.log_overflows(), 0);
    render
}

/// The events of the log delivered to `target`, as (frame, payload).
fn events(render: &Render, target: &str) -> Vec<(u64, Value)> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|v| v["type"] == "event" && v["target"] == target)
        .map(|v| (v["frame"].as_u64().unwrap(), v["payload"].clone()))
        .collect()
}

fn frames_of(render: &Render, target: &str) -> Vec<u64> {
    events(render, target)
        .into_iter()
        .filter(|(_, p)| p == "trigger")
        .map(|(f, _)| f)
        .collect()
}

fn faults(render: &Render) -> Vec<String> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\""))
        .map(str::to_owned)
        .collect()
}

/// The left channel: the melody's position ÷ 16.
fn positions(render: &Render) -> Vec<f64> {
    render.samples.chunks(2).map(|s| s[0] * 16.0).collect()
}

/// AC9: notes fire on the tick formula's frames at 120 and 132 bpm, the
/// loop wraps after 4 beats, and `off` follows each note's length.
#[test]
fn notes_fire_on_the_tick_frames_at_two_tempos() {
    for bpm in [120, 132] {
        let text = project_text(48_000, bpm as f64, "", "");
        let frames = tick_frame(2 * 384, 48_000, bpm);
        let rendered = render(&project(&text, MELODY), None, frames, (256, 1), Vec::new());
        assert!(faults(&rendered).is_empty(), "{:?}", faults(&rendered));
        // Notes at ticks 0, 96, 240 of each 384-tick loop; 48 ticks long.
        let starts: Vec<u64> = [0, 96, 240, 384, 480, 624]
            .iter()
            .map(|&g| tick_frame(g, 48_000, bpm))
            .collect();
        assert_eq!(frames_of(&rendered, "/on.in"), starts, "{bpm} bpm");
        let ends: Vec<u64> = [48, 144, 288, 432, 528, 672]
            .iter()
            .map(|&g| tick_frame(g, 48_000, bpm))
            .collect();
        assert_eq!(frames_of(&rendered, "/off.in"), ends, "{bpm} bpm");
        // The playhead: tick 96 is position 1 beat; the loop wraps at 384.
        let position = positions(&rendered);
        assert_eq!(position[tick_frame(96, 48_000, bpm) as usize], 1.0);
        assert_eq!(
            position[tick_frame(383, 48_000, bpm) as usize],
            383.0 / 96.0
        );
        assert_eq!(position[tick_frame(384, 48_000, bpm) as usize], 0.0);
    }
}

/// AC9: across a tempo change (120 → 150 bpm, set at frame 30 000) the
/// notes follow the transport's ticks re-anchored on beat 2 (frame 48 000).
#[test]
fn notes_follow_a_tempo_change() {
    let text = project_text(48_000, 120.0, "", "");
    let change = ExternalEvent {
        frame: Frame(30_000),
        sequence: 1,
        target: "/clock.bpm".parse().unwrap(),
        payload: EventPayload::Set(150.0),
    };
    let tick = |g: u64| {
        if g < 192 {
            tick_frame(g, 48_000, 120)
        } else {
            48_000 + exact(u128::from(g - 192) * 60_000 * 48_000, 150_000 * 96)
        }
    };
    let frames = tick(400);
    let rendered = render(&project(&text, MELODY), None, frames, (64, 1), vec![change]);
    let starts: Vec<u64> = [0, 96, 240, 384].iter().map(|&g| tick(g)).collect();
    assert_eq!(frames_of(&rendered, "/on.in"), starts);
    assert_eq!(
        starts[2],
        48_000 + 48 * 200,
        "after the change, 200 frames a tick"
    );
}

/// AC9: a note crossing the loop end ends in the next loop, after its whole
/// length; the loop wraps.
#[test]
fn a_note_crossing_the_loop_end_ends_in_the_next_loop() {
    let text = project_text(48_000, 120.0, "", "");
    let frames = tick_frame(3 * 192, 48_000, 120);
    let rendered = render(
        &project(&text, CROSSING),
        None,
        frames,
        (256, 1),
        Vec::new(),
    );
    // Loop of 192 ticks; the note from tick 144 for 96 ticks.
    let at = |g| tick_frame(g, 48_000, 120);
    assert_eq!(frames_of(&rendered, "/on.in"), [at(144), at(336), at(528)]);
    assert_eq!(frames_of(&rendered, "/off.in"), [at(240), at(432)]);
    let position = positions(&rendered);
    assert_eq!(
        position[at(240) as usize],
        0.5,
        "the note ends at 0.5 of the next loop"
    );
    assert_eq!(rendered.samples.chunks(2).count() as u64, frames);
}

/// AC9: swing delays the even 16ths by a whole number of ticks (50 % of 24
/// is 12); the kick fires `Set(level)` then a trigger, ×1.5 when accented,
/// which fires `std.exp_env`; `accent` fires on the accented step.
#[test]
fn steps_swing_and_accent() {
    let text = project_text(48_000, 120.0, "", "");
    let frames = tick_frame(2 * 96, 48_000, 120);
    let rendered = render(&project(&text, MELODY), None, frames, (256, 1), Vec::new());
    let at = |g| tick_frame(g, 48_000, 120);
    // Steps at ticks 0, 24, 48, 72; the 2nd and 4th 12 ticks late.
    assert_eq!(
        frames_of(&rendered, "/hat.in"),
        [
            at(0),
            at(36),
            at(48),
            at(84),
            at(96),
            at(132),
            at(144),
            at(180)
        ]
    );
    let kick = events(&rendered, "/kick.in");
    let set = |v: f64| serde_json::json!({ "set": v });
    assert_eq!(
        kick,
        [
            (at(0), set(0.8)),
            (at(0), Value::from("trigger")),
            (at(48), set(0.8 * 1.5)),
            (at(48), Value::from("trigger")),
            (at(96), set(0.8)),
            (at(96), Value::from("trigger")),
            (at(144), set(0.8 * 1.5)),
            (at(144), Value::from("trigger")),
        ]
    );
    assert_eq!(frames_of(&rendered, "/accent.in"), [at(48), at(144)]);
    // The envelope on t1 fires on the kick's frames (its attack starts from
    // the level before, so it rises after the trigger frame).
    let env: Vec<f64> = rendered.samples.chunks(2).map(|s| s[1]).collect();
    assert!(env[at(0) as usize + 20] > 0.5);
    assert!(env[at(96) as usize + 20] > env[at(96) as usize - 1]);
}

/// The operator note's step example (`[levels]`, 16 lanes' cells), verbatim:
/// it loads and plays; the kick carries its level 0.9 (×1.5 on `X`), the
/// snare, not in `[levels]`, level 1; swing 50 % delays the odd indices.
#[test]
fn the_levels_example_loads_and_renders() {
    const EXAMPLE: &str = r#"schema_version = 1
kind = "steps"
steps = 16
step = 0.25

[lanes]
kick  = "X...x...x...x..x"
snare = "....X.......X..."

[levels]
kick = 0.9
"#;
    let text = project_text(48_000, 120.0, "", "");
    let frames = tick_frame(384, 48_000, 120);
    let rendered = render(
        &project_with(&text, MELODY, EXAMPLE),
        None,
        frames,
        (64, 1),
        Vec::new(),
    );
    assert!(faults(&rendered).is_empty(), "{:?}", faults(&rendered));
    let at = |g| tick_frame(g, 48_000, 120);
    let set = |v: f64| serde_json::json!({ "set": v });
    let sets = |target: &str| -> Vec<(u64, Value)> {
        events(&rendered, target)
            .into_iter()
            .filter(|(_, p)| p != "trigger")
            .collect()
    };
    assert_eq!(
        sets("/kick.in"),
        [
            (at(0), set(0.9 * 1.5)),
            (at(96), set(0.9)),
            (at(192), set(0.9)),
            (at(288), set(0.9)),
            (at(15 * 24 + 12), set(0.9)),
        ]
    );
    assert_eq!(sets("/hat.in"), [(at(96), set(1.5)), (at(288), set(1.5))]);
}

/// AC4, AC5: a pattern edit while playing is a swap at an exact frame that
/// keeps the sequencer's position (no restart) and plays the new notes from
/// there; bit-identical for k = 1, 64, 256 on 1, 2, 4 and 8 threads.
#[test]
fn a_pattern_edit_keeps_the_position_and_is_bit_identical_in_blocks_and_on_threads() {
    let text = project_text(48_000, 120.0, "", "");
    let old = project(&text, MELODY);
    let edited = MELODY.replace("key = 64, start = 2.5", "key = 67, start = 3");
    let new = project(&text, &edited);
    let at = |g| tick_frame(g, 48_000, 120);
    let swap = at(200) + 7;
    let frames = at(400);
    let reference = render(&old, Some((&new, swap)), frames, (1, 1), Vec::new());
    let position = positions(&reference);
    // The position goes on through the swap frame.
    assert_eq!(position[swap as usize], 200.0 / 96.0);
    assert_eq!(position[at(201) as usize], 201.0 / 96.0);
    // The edited note: at tick 288 (beat 3), not 240 (beat 2.5).
    let starts = frames_of(&reference, "/on.in");
    assert_eq!(starts, [at(0), at(96), at(288), at(384)]);
    // The same up to the swap as without it.
    let unedited = render(&old, None, frames, (1, 1), Vec::new());
    assert_eq!(
        unedited.samples[..2 * swap as usize],
        reference.samples[..2 * swap as usize]
    );
    let hashes = |r: &Render| (r.audio_fnv1a64(), r.log_fnv1a64());
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let rendered = render(&old, Some((&new, swap)), frames, (k, threads), Vec::new());
            assert_eq!(
                hashes(&rendered),
                hashes(&reference),
                "k = {k}, {threads} threads"
            );
        }
    }
}

/// AC4, AC5: a live run on the null driver, its pattern file edited and
/// reloaded while playing (the watch's reload), records a session that
/// replays to exactly what was played; the session keeps the pattern of
/// each version.
#[test]
fn a_live_pattern_edit_is_recorded_and_replays_bit_identically() {
    let registry = registry();
    let dir = std::env::temp_dir().join(format!("signal-pattern-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("patterns")).unwrap();
    let rate = 8_000;
    fs::write(dir.join("project.toml"), project_text(rate, 120.0, "", "")).unwrap();
    fs::write(dir.join("patterns/melody.toml"), MELODY).unwrap();
    fs::write(dir.join("patterns/beat.toml"), BEAT).unwrap();
    let mut current = Project::load(&dir).unwrap();
    let mut plan = current.plan(&registry).unwrap().plan;
    plan.version = 0;
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(rate, current.seed)).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &current.files);
    let recorder =
        SessionRecorder::new(&current, &registry, rate, current.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(rate, 2, 3);
    driver.start(runner).unwrap();
    let run = |driver: &mut NullDriver,
               controller: &mut signal_output::Controller,
               live: &mut LiveRecorder,
               blocks: u64| {
        for _ in 0..blocks {
            driver.run(256, || {}).unwrap();
            controller.poll(live);
        }
    };
    run(&mut driver, &mut controller, &mut live, 20);
    let edited = MELODY.replace("key = 64, start = 2.5", "key = 67, start = 3");
    fs::write(dir.join("patterns/melody.toml"), &edited).unwrap();
    let applied = reload(&mut controller, &mut live, &mut current, &dir, &registry).unwrap();
    assert_eq!(
        (applied.sets, applied.swap, applied.removed),
        (0, Some(1), 0)
    );
    run(&mut driver, &mut controller, &mut live, 40);
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);

    let recorder = live.into_recorder();
    let swaps: Vec<u64> = recorder
        .edits()
        .iter()
        .filter(|e| matches!(e.command, session::Command::Swap { .. }))
        .map(|e| e.frame.0)
        .collect();
    assert_eq!(swaps.len(), 1);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("patterns/live");
    let _ = fs::remove_dir_all(&out);
    recorder.write(&out).unwrap();
    assert_eq!(
        fs::read_to_string(out.join("projects/v1/patterns/melody.toml")).unwrap(),
        edited
    );
    assert_eq!(
        fs::read_to_string(out.join("projects/v0/patterns/melody.toml")).unwrap(),
        MELODY
    );
    let session = Session::load(&out, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, driver.output(), "replay equals what was played");
    // The position went on through the swap: no restart at the edit.
    let swap = swaps[0] as usize;
    let left: Vec<f32> = samples.chunks(2).map(|s| s[0]).collect();
    assert!(left[swap] >= left[swap - 1] || left[swap] == 0.0);
    assert!(left[swap] > 0.0, "the position is not reset by the edit");
    let _ = fs::remove_dir_all(&dir);
}

/// auditor-1-005 (AC8): steps shorter than a tick share it, and every one of
/// them fires on it, the playhead showing the last. At ppq 1 (a tick a beat,
/// 24 000 frames at 120 bpm): 16 steps of 0.125 put steps 0–3 on tick 0, so
/// a hit only on step 3 fires at frame 0 with `step` 3; 256 steps of 0.01
/// put steps 0–49 on tick 0 and 150–249 on tick 2 (a loop of 3 ticks).
#[test]
fn steps_sharing_a_tick_all_fire_on_it() {
    const TEXT: &str = r#"schema_version = 1

[project]
name = "dense-steps"
sample_rate = 48000

[instances]
clock = { module = "std.transport", bpm = 120, ppq = 1 }
drums = { module = "std.step_seq8", pattern = "beat", ppq = 1 }
kick = { module = "std.count" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "clock.tick", to = "drums.tick" },
  { from = "drums.t1", to = "kick.in" },
  { from = "drums.step", to = "out.l", scale = 0.001 },
]
"#;
    let load = |cells: &str, step: f64| {
        let beat = format!(
            "schema_version = 1\nkind = \"steps\"\nsteps = {}\nstep = {step}\n\n[lanes]\nkick = \"{cells}\"\n",
            cells.len()
        );
        Project::from_sources(vec![
            SourceFile {
                path: "project.toml".to_owned(),
                text: TEXT.to_owned(),
            },
            SourceFile {
                path: "patterns/beat.toml".to_owned(),
                text: beat,
            },
        ])
        .unwrap()
    };
    let step_at = |r: &Render, frame: u64| (r.samples[2 * frame as usize] * 1000.0).round();

    let mut cells = vec!['.'; 16];
    cells[3] = 'x';
    let cells: String = cells.into_iter().collect();
    let rendered = render(&load(&cells, 0.125), None, 48_001, (64, 1), Vec::new());
    assert!(faults(&rendered).is_empty(), "{:?}", faults(&rendered));
    // A loop of round_half_up(16 · 0.125) = 2 ticks: step 3 again at tick 2.
    assert_eq!(frames_of(&rendered, "/kick.in"), [0, 48_000]);
    assert_eq!(step_at(&rendered, 0), 3.0);
    assert_eq!(step_at(&rendered, 24_000), 11.0, "steps 4–11 on tick 1");

    let mut cells = vec!['.'; 256];
    for i in [10, 40, 49, 160, 249] {
        cells[i] = 'x';
    }
    let cells: String = cells.into_iter().collect();
    let rendered = render(&load(&cells, 0.01), None, 72_001, (256, 1), Vec::new());
    assert!(faults(&rendered).is_empty());
    // One hit per frame on t1 however many steps fire on its tick.
    assert_eq!(frames_of(&rendered, "/kick.in"), [0, 48_000, 72_000]);
    assert_eq!(step_at(&rendered, 0), 49.0);
    assert_eq!(step_at(&rendered, 24_000), 149.0);
    assert_eq!(step_at(&rendered, 48_000), 249.0);
}

/// closure-AC8 (auditor-2-r2-step-at-loop-end-never-fires): a step whose
/// firing tick is at or past the loop length wraps to the next loop's tick
/// `firing − L` instead of being lost, from the second loop on.
/// - 1/64-beat steps at ppq 96 (1.5 ticks a step), 16 steps, swing 25: the
///   loop is 24 ticks (250 frames each at 120 bpm and 48 kHz); step 15's
///   grid tick is 23 (the playhead reaches it there) and swing delays it to
///   24 = L, so it fires on tick 0 of every loop after the first.
/// - 16 steps of 0.125 at ppq 1 (24 000 frames a tick): steps 12–15 round to
///   tick 2 = L and fire on tick 0 of every loop after the first.
#[test]
fn steps_at_the_loop_end_wrap_to_the_next_loop() {
    const TEXT: &str = r#"schema_version = 1

[project]
name = "loop-end"
sample_rate = 48000

[instances]
clock = { module = "std.transport", bpm = 120, ppq = PPQ }
drums = { module = "std.step_seq8", pattern = "beat", ppq = PPQ, swing = SWING }
kick = { module = "std.count" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "clock.tick", to = "drums.tick" },
  { from = "drums.t1", to = "kick.in" },
  { from = "drums.step", to = "out.l", scale = 0.001 },
]
"#;
    let load = |ppq: &str, swing: &str, cells: &str, step: f64| {
        let beat = format!(
            "schema_version = 1\nkind = \"steps\"\nsteps = {}\nstep = {step}\n\n[lanes]\nkick = \"{cells}\"\n",
            cells.len()
        );
        Project::from_sources(vec![
            SourceFile {
                path: "project.toml".to_owned(),
                text: TEXT.replace("PPQ", ppq).replace("SWING", swing),
            },
            SourceFile {
                path: "patterns/beat.toml".to_owned(),
                text: beat,
            },
        ])
        .unwrap()
    };
    let step_at = |r: &Render, frame: u64| (r.samples[2 * frame as usize] * 1000.0).round();

    // 1/64-beat steps, a hit only on the last (swung) step.
    let tick = |g: u64| g * 250;
    let rendered = render(
        &load("96", "25", "...............x", 0.015625),
        None,
        tick(3 * 24) + 1,
        (64, 1),
        Vec::new(),
    );
    assert!(faults(&rendered).is_empty(), "{:?}", faults(&rendered));
    assert_eq!(
        frames_of(&rendered, "/kick.in"),
        [tick(24), tick(48), tick(72)]
    );
    assert_eq!(
        step_at(&rendered, tick(23)),
        15.0,
        "the playhead reaches it"
    );
    assert_eq!(step_at(&rendered, tick(47)), 15.0);
    assert_eq!(step_at(&rendered, tick(24)), 0.0);
    // The same pattern without swing never wraps: step 15 fires on tick 23.
    let rendered = render(
        &load("96", "0", "...............x", 0.015625),
        None,
        tick(3 * 24) + 1,
        (64, 1),
        Vec::new(),
    );
    assert_eq!(
        frames_of(&rendered, "/kick.in"),
        [tick(23), tick(47), tick(71)]
    );

    // Sub-tick steps: hits on steps 12–15, which round up to the loop end.
    let rendered = render(
        &load("1", "0", "............xxxx", 0.125),
        None,
        96_001,
        (256, 1),
        Vec::new(),
    );
    assert!(faults(&rendered).is_empty());
    // One hit per frame on t1; ticks 2 and 4 are the next loops' tick 0.
    assert_eq!(frames_of(&rendered, "/kick.in"), [48_000, 96_000]);
    // Steps 12–15 share their tick with the next loop's steps 0–3, the last
    // in time, which the playhead shows.
    assert_eq!(step_at(&rendered, 48_000), 3.0);
}

/// At 30 Hz, 400 bpm and 96 ppq, 21 or 22 ticks fall in a frame.
mod dense {
    use super::*;

    fn text(wiring: &str, extra: &str) -> String {
        format!(
            "schema_version = 1\n\n[project]\nname = \"dense\"\nsample_rate = 30\n\n[instances]\n\
             clock = {{ module = \"std.transport\", bpm = 400 }}\n\
             beats = {{ module = \"std.count\" }}\n\
             out = {{ module = \"std.audio_out\" }}\n{extra}\n\
             [wiring]\nconnections = [\n  {{ from = \"clock.beat\", to = \"beats.in\" }},\n{wiring}]\n"
        )
    }

    fn load(text: &str, melody: &str) -> Project {
        project(text, melody)
    }

    /// Ticks per frame by the formula (400 bpm, 30 Hz, 96 ppq).
    fn ticks_by(frame: u64) -> u64 {
        (0..)
            .map(|g: u128| exact(g * 60_000 * 30, 400_000 * 96))
            .take_while(|&f| f <= frame)
            .count() as u64
    }

    /// An unconnected tick faults nothing, and the beats are on their
    /// formula frames.
    #[test]
    fn an_unconnected_tick_faults_nothing() {
        let project = load(&text("", ""), MELODY);
        let rendered = render(&project, None, 60, (7, 1), Vec::new());
        assert!(faults(&rendered).is_empty(), "{:?}", faults(&rendered));
        let beats: Vec<u64> = (0..)
            .map(|k: u128| exact(k * 60_000 * 30, 400_000))
            .take_while(|&f| f < 60)
            .collect();
        assert_eq!(frames_of(&rendered, "/beats.in"), beats);
    }

    /// `tick` into `std.count`: the values delivered (Trigger 1, Set v) sum to
    /// the formula's tick count on every frame; at most 2 events a frame.
    #[test]
    fn every_tick_is_counted_by_value() {
        let project = load(
            &text(
                "  { from = \"clock.tick\", to = \"ticks.in\" },\n",
                "ticks = { module = \"std.count\" }\n",
            ),
            MELODY,
        );
        let rendered = render(&project, None, 30, (64, 1), Vec::new());
        assert!(faults(&rendered).is_empty());
        let mut sum = 0;
        let mut by_frame = std::collections::BTreeMap::new();
        for (frame, payload) in events(&rendered, "/ticks.in") {
            let value = match &payload {
                Value::String(s) if s == "trigger" => 1,
                other => other["set"].as_f64().unwrap() as u64,
            };
            *by_frame.entry(frame).or_insert(0) += 1;
            sum += value;
            let expected_before: u64 = ticks_by(frame);
            assert!(sum <= expected_before);
        }
        assert_eq!(sum, ticks_by(29));
        assert!(by_frame.values().all(|&n| n <= 2));
        assert_eq!(by_frame.len(), 30);
    }

    /// `tick` into `std.note_seq`: its position counts every tick. Tick 96
    /// (beat 1) is on frame 5, the first frame whose end position is at least
    /// 1 beat; that frame ends at tick 117 (117/96 beats).
    #[test]
    fn the_sequencer_counts_every_tick() {
        let project = load(
            &text(
                "  { from = \"clock.tick\", to = \"melody.tick\" },\n  { from = \"melody.position\", to = \"out.l\", scale = 0.0625 },\n",
                "melody = { module = \"std.note_seq\", pattern = \"melody\" }\n",
            ),
            &MELODY.replace("length = 4", "length = 64"),
        );
        let rendered = render(&project, None, 10, (1, 1), Vec::new());
        assert!(faults(&rendered).is_empty());
        let position: Vec<f64> = rendered.samples.chunks(2).map(|s| s[0] * 16.0).collect();
        let first = position.iter().position(|&p| p >= 1.0).unwrap();
        assert_eq!(first, 5);
        assert_eq!(position[5], 117.0 / 96.0);
        for (frame, &p) in position.iter().enumerate() {
            assert_eq!(
                p,
                (ticks_by(frame as u64) - 1) as f64 / 96.0,
                "frame {frame}"
            );
        }
    }
}

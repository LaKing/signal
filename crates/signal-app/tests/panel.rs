//! M2.5-2: the parameter panel without a window, through the null driver, on
//! copies of `projects/kick` in a temporary folder (the committed files are
//! only read). A drag sends ramped `set`s through the Controller; the release
//! writes the file once; the file watch's reload then applies nothing; reset
//! goes back to the default; parameters inside a composite are read-only.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use signal_app::panel::{InstanceView, ParameterView, RAMP_MS, ramp_frames};
use signal_app::setup::Setup;
use signal_app::state::{AppState, Backend, Command, View};
use signal_core::{Curve, EventPayload};
use signal_project::live::WATCH_INTERVAL_MS;
use signal_project::session;

fn kick() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick")
}

/// A copy of `projects/kick` and a Setup file, of their own for each test.
fn copy(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("signal-panel-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dir = root.join("kick");
    fs::create_dir_all(dir.join("composites")).unwrap();
    fs::copy(kick().join("project.toml"), dir.join("project.toml")).unwrap();
    for entry in fs::read_dir(kick().join("composites")).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(
            &path,
            dir.join("composites").join(path.file_name().unwrap()),
        )
        .unwrap();
    }
    let setup = root.join("setup.toml");
    Setup::default().save(&setup).unwrap();
    (dir, setup)
}

fn state(dir: &Path, setup: &Path) -> AppState {
    AppState::new(
        setup.to_owned(),
        Some(dir.to_owned()),
        Backend::Null(Vec::new()),
    )
}

fn play(state: &mut AppState) {
    state.apply(Command::Play);
    let view = state.view();
    assert!(view.playing, "refused: {:?}", view.message);
}

fn instance<'v>(view: &'v View, name: &str) -> &'v InstanceView {
    view.panel.iter().find(|i| i.name == name).unwrap()
}

fn parameter<'v>(view: &'v View, name: &str, parameter: &str) -> &'v ParameterView {
    instance(view, name)
        .parameters
        .iter()
        .find(|p| p.name == parameter)
        .unwrap()
}

fn project_toml(dir: &Path) -> String {
    fs::read_to_string(dir.join("project.toml")).unwrap()
}

fn drag(instance: &str, parameter: &str, value: f64) -> Command {
    Command::Drag {
        instance: instance.to_owned(),
        parameter: parameter.to_owned(),
        value,
    }
}

fn release(instance: &str, parameter: &str) -> Command {
    Command::Release {
        instance: instance.to_owned(),
        parameter: parameter.to_owned(),
    }
}

fn reset(instance: &str, parameter: &str) -> Command {
    Command::Reset {
        instance: instance.to_owned(),
        parameter: parameter.to_owned(),
    }
}

/// The ramps the engine applied, as (target, to, frames).
fn ramps(state: &AppState) -> Vec<(String, f64, u64)> {
    state
        .edits()
        .into_iter()
        .filter_map(|e| match e.command {
            session::Command::Event {
                target,
                payload: EventPayload::Ramp { to, frames, curve },
            } => {
                assert_eq!(curve, Curve::Linear);
                Some((target.to_string(), to, frames))
            }
            _ => None,
        })
        .collect()
}

/// Plays `frames` frames and drains them.
fn run(state: &mut AppState, frames: u64) {
    state.run_null(frames).unwrap();
    state.tick();
}

/// Waits out the watch interval, then lets the watch look.
fn watch(state: &mut AppState) -> (String, bool) {
    thread::sleep(Duration::from_millis(WATCH_INTERVAL_MS + 20));
    state.tick();
    state.view().reload.unwrap()
}

/// SC1–SC3: the top-level instances with their parameters' unit, range,
/// default and base; the composite `drums` shows what it exports, editable,
/// and its inner parameters read-only; modulated parameters show the base and
/// the effective value from the runner.
#[test]
fn the_panel_lists_instances_parameters_and_effective_values() {
    let (dir, setup) = copy("list");
    let mut state = state(&dir, &setup);
    let view = state.view();
    let names: Vec<&str> = view.panel.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["accent", "drums", "lfo", "out", "transport"]);

    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.in_file, gain.editable), (0.75, true, true));
    assert!(!gain.modulated);
    assert_eq!(gain.effective, None, "no effective value while stopped");

    let drums = instance(&view, "drums");
    let exported: Vec<&str> = drums.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(exported, ["kick_decay", "kick_drive"]);
    let decay = parameter(&view, "drums", "kick_decay");
    assert_eq!(
        (decay.default, decay.base),
        (420.0, 420.0),
        "kick.toml's value"
    );
    assert_eq!(decay.unit, Some("ms"));
    assert!(decay.editable && decay.modulated && !decay.in_file);
    let drive = parameter(&view, "drums", "kick_drive");
    assert_eq!(drive.default, 2.5);
    assert!(drive.modulated);
    let inner = drums
        .inner
        .iter()
        .find(|p| p.name == "kick/amp_env.decay_ms")
        .unwrap();
    assert!(
        !inner.editable,
        "parameters inside a composite are read-only"
    );
    assert_eq!(inner.base, 420.0);
    assert!(drums.inner.iter().all(|p| !p.editable));
    assert!(instance(&view, "out").inner.is_empty());

    play(&mut state);
    run(&mut state, 48_000);
    let view = state.view();
    assert_eq!(parameter(&view, "out", "gain").effective, Some(0.75));
    let decay = parameter(&view, "drums", "kick_decay");
    let effective = decay.effective.expect("reported by the runner");
    assert_eq!(decay.base, 420.0);
    assert!(
        (300.0..=540.0).contains(&effective),
        "420 ± 120: {effective}"
    );
    assert!(
        drums_inner(&view, "kick/amp_env.decay_ms")
            .effective
            .is_some()
    );
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

fn drums_inner<'v>(view: &'v View, name: &str) -> &'v ParameterView {
    instance(view, "drums")
        .inner
        .iter()
        .find(|p| p.name == name)
        .unwrap()
}

/// AC3 and AC2: dragging `out.gain` sends each value as a 5 ms ramp through
/// the Controller and writes nothing; the release writes the file once, with
/// one value's text changed; the watch's reload of it applies nothing; the
/// runner reports the new value.
#[test]
fn a_drag_sends_ramps_and_the_release_writes_once() {
    let (dir, setup) = copy("drag");
    let original = project_toml(&dir);
    let mut state = state(&dir, &setup);
    play(&mut state);
    run(&mut state, 4_096);
    assert_eq!(ramp_frames(96_000), 480, "{RAMP_MS} ms at 96 kHz");

    for value in [0.7, 0.6, 0.5] {
        state.apply(drag("out", "gain", value));
        run(&mut state, 1_024);
    }
    assert_eq!(
        ramps(&state),
        [
            ("/out.gain".to_owned(), 0.7, 480),
            ("/out.gain".to_owned(), 0.6, 480),
            ("/out.gain".to_owned(), 0.5, 480),
        ]
    );
    let view = state.view();
    assert_eq!(view.writes, 0, "dragging writes nothing");
    assert_eq!(project_toml(&dir), original);
    assert_eq!(
        parameter(&view, "out", "gain").base,
        0.5,
        "the dragged base"
    );

    state.apply(release("out", "gain"));
    state.apply(release("out", "gain"));
    let view = state.view();
    assert_eq!(view.writes, 1, "the release writes once");
    assert_eq!(view.message, None);
    let written = project_toml(&dir);
    assert_eq!(
        written,
        original.replace(
            "out = { module = \"std.audio_out\", gain = 0.75 }",
            "out = { module = \"std.audio_out\", gain = 0.5 }"
        )
    );
    assert!(!dir.join(".project.toml.tmp").exists());

    // The file watch sees the write; the diff to the running project is
    // empty, so nothing is sent again.
    let before = state.edits().len();
    assert_eq!(watch(&mut state), ("no change to apply".to_owned(), false));
    run(&mut state, 4_096);
    assert_eq!(state.edits().len(), before);
    let view = state.view();
    let gain = parameter(&view, "out", "gain");
    assert_eq!(
        (gain.base, gain.in_file, gain.effective),
        (0.5, true, Some(0.5))
    );
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// AC4 and SC4: `drums`'s exported `kick_decay` is inserted into its inline
/// table on release; reset ramps back to the default and removes the key,
/// leaving the original file; the reloads apply nothing.
#[test]
fn an_exported_parameter_is_inserted_and_reset() {
    let (dir, setup) = copy("reset");
    let original = project_toml(&dir);
    let mut state = state(&dir, &setup);
    play(&mut state);
    state.apply(drag("drums", "kick_decay", 500.0));
    state.apply(release("drums", "kick_decay"));
    run(&mut state, 4_096);
    assert_eq!(
        ramps(&state),
        [("/drums/kick/amp_env.decay_ms".to_owned(), 500.0, 480)]
    );
    assert!(project_toml(&dir).contains("drums = { module = \"drums\", kick_decay = 500 }\n"));
    let view = state.view();
    let decay = parameter(&view, "drums", "kick_decay");
    assert_eq!(
        (decay.base, decay.default, decay.in_file),
        (500.0, 420.0, true)
    );
    assert_eq!(drums_inner(&view, "kick/amp_env.decay_ms").base, 500.0);
    assert_eq!(watch(&mut state), ("no change to apply".to_owned(), false));

    state.apply(reset("drums", "kick_decay"));
    run(&mut state, 4_096);
    assert_eq!(
        ramps(&state).last().unwrap().1,
        420.0,
        "back to the default"
    );
    assert_eq!(project_toml(&dir), original);
    let view = state.view();
    assert_eq!(view.writes, 2);
    let decay = parameter(&view, "drums", "kick_decay");
    assert_eq!((decay.base, decay.in_file), (420.0, false));
    assert_eq!(watch(&mut state), ("no change to apply".to_owned(), false));
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// SC2: a parameter inside a composite is refused, and neither sent nor
/// written. Stopped, a release still writes (and nothing is sent).
#[test]
fn a_parameter_inside_a_composite_is_read_only() {
    let (dir, setup) = copy("inner");
    let original = project_toml(&dir);
    let mut state = state(&dir, &setup);
    play(&mut state);
    state.apply(drag("drums", "kick/amp_env.decay_ms", 300.0));
    let message = state.view().message.unwrap();
    assert!(message.contains("is not editable here"), "{message}");
    state.apply(release("drums", "kick/amp_env.decay_ms"));
    state.apply(reset("drums", "kick/amp_env.decay_ms"));
    run(&mut state, 4_096);
    assert!(ramps(&state).is_empty());
    assert_eq!(state.view().writes, 0);
    assert_eq!(project_toml(&dir), original);

    state.apply(Command::Stop);
    state.apply(drag("out", "gain", 9.0));
    state.apply(release("out", "gain"));
    let view = state.view();
    assert_eq!(view.writes, 1);
    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.max), (4.0, 4.0), "clamped to its range");
    assert!(project_toml(&dir).contains("gain = 4 }"));
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// auditor-1-1: a drag whose ramp the Controller refuses (a full command
/// queue) is still written on release, but the running project does not
/// take it, so the watch's reload sends it and playback ends at the file's
/// value.
#[test]
fn a_refused_drag_is_written_and_applied_by_the_reload() {
    let (dir, setup) = copy("refused");
    let mut state = state(&dir, &setup);
    play(&mut state);
    let mut refused = false;
    for _ in 0..4_096 {
        state.apply(drag("out", "gain", 0.5));
        if state.view().message.is_some() {
            refused = true;
            break;
        }
    }
    assert!(refused, "the queue fills without playing");
    state.apply(drag("out", "gain", 0.25));
    let message = state.view().message.unwrap();
    assert!(message.contains("the command queue is full"), "{message}");
    state.apply(release("out", "gain"));
    assert_eq!(state.view().writes, 1);
    assert!(project_toml(&dir).contains("gain = 0.25 }"));

    run(&mut state, 4_096);
    assert_eq!(
        parameter(&state.view(), "out", "gain").effective,
        Some(0.5),
        "the engine has the last value it took"
    );
    assert_eq!(watch(&mut state), ("applied 1 sets".to_owned(), false));
    run(&mut state, 4_096);
    let view = state.view();
    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.effective), (0.25, Some(0.25)));
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// auditor-1-1: the engine took earlier drag values (0.5) and the release
/// back to the file's own value (0.75) is refused. The running project
/// takes what the engine holds, so the reload still sends 0.75.
#[test]
fn a_refused_release_to_the_original_value_is_applied_by_the_reload() {
    let (dir, setup) = copy("refused-back");
    let original = project_toml(&dir);
    let mut state = state(&dir, &setup);
    play(&mut state);
    let mut refused = false;
    for _ in 0..4_096 {
        state.apply(drag("out", "gain", 0.5));
        if state.view().message.is_some() {
            refused = true;
            break;
        }
    }
    assert!(refused, "the queue fills without playing");
    state.apply(drag("out", "gain", 0.75));
    assert!(
        state
            .view()
            .message
            .unwrap()
            .contains("command queue is full")
    );
    state.apply(release("out", "gain"));
    assert_eq!(state.view().writes, 1);
    assert_eq!(project_toml(&dir), original, "gain = 0.75, as before");

    run(&mut state, 4_096);
    assert_eq!(parameter(&state.view(), "out", "gain").effective, Some(0.5));
    assert_eq!(watch(&mut state), ("applied 1 sets".to_owned(), false));
    run(&mut state, 4_096);
    let view = state.view();
    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.effective), (0.75, Some(0.75)));
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// Drags `out.gain` to `value` until the Controller refuses one; returns how
/// many it took.
fn fill_queue(state: &mut AppState, value: f64) -> usize {
    for taken in 0..4_096 {
        state.apply(drag("out", "gain", value));
        if state.view().message.is_some() {
            return taken;
        }
    }
    panic!("the queue never filled");
}

/// A kick copy whose `drums.kick_decay` fans out to `amp_env.decay_ms`
/// (420 in kick.toml) and `pitch_env.decay_ms` (35), unmodulated.
fn fan_out(name: &str) -> (PathBuf, PathBuf) {
    let (dir, setup) = copy(name);
    let kick = dir.join("composites/kick.toml");
    let text = fs::read_to_string(&kick).unwrap().replace(
        "decay = \"amp_env.decay_ms\"",
        "decay = [\"amp_env.decay_ms\", \"pitch_env.decay_ms\"]",
    );
    fs::write(&kick, text).unwrap();
    let project = project_toml(&dir).replace(
        "  { from = \"lfo.out\", to = \"drums.kick_decay\", scale = 120 },\n",
        "",
    );
    fs::write(dir.join("project.toml"), project).unwrap();
    (dir, setup)
}

/// auditor-1-1 (partial fan-out): `amp_env` takes 550 and `pitch_env` is
/// refused it, then the drag back to the file's 500 is refused and released.
/// The reload sends 500 to both targets.
#[test]
fn a_partly_refused_fan_out_is_applied_to_every_target_by_the_reload() {
    let (dir, setup) = fan_out("partial");
    let text = project_toml(&dir).replace(
        "drums = { module = \"drums\" }",
        "drums = { module = \"drums\", kick_decay = 500 }",
    );
    fs::write(dir.join("project.toml"), &text).unwrap();
    let mut state = state(&dir, &setup);
    play(&mut state);
    // The queue's room, then all but one slot of it (0.75 is out.gain's own
    // value, so those drags change nothing).
    let room = fill_queue(&mut state, 0.75);
    run(&mut state, 4_096);
    for _ in 1..room {
        state.apply(drag("out", "gain", 0.75));
    }
    assert_eq!(state.view().message, None);
    state.apply(drag("drums", "kick_decay", 550.0));
    let message = state.view().message.unwrap();
    assert!(
        message.starts_with("/drums/kick/pitch_env.decay_ms: the command queue is full"),
        "{message}"
    );
    state.apply(drag("drums", "kick_decay", 500.0));
    state.apply(release("drums", "kick_decay"));
    assert_eq!(project_toml(&dir), text, "kick_decay = 500, as before");

    run(&mut state, 4_096);
    let view = state.view();
    assert_eq!(
        drums_inner(&view, "kick/amp_env.decay_ms").effective,
        Some(550.0)
    );
    assert_eq!(
        drums_inner(&view, "kick/pitch_env.decay_ms").effective,
        Some(500.0)
    );
    assert_eq!(watch(&mut state), ("applied 2 sets".to_owned(), false));
    run(&mut state, 4_096);
    let view = state.view();
    for name in ["kick/amp_env.decay_ms", "kick/pitch_env.decay_ms"] {
        let p = drums_inner(&view, name);
        assert_eq!((p.base, p.effective), (500.0, Some(500.0)), "{name}");
    }
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// auditor-1-3: drags to 0.5 fill the queue and Reset (without a release)
/// is refused. The file keeps no gain (default 1); the reload sends it.
#[test]
fn a_refused_reset_is_applied_by_the_reload() {
    let (dir, setup) = copy("refused-reset");
    let text = project_toml(&dir).replace(
        "out = { module = \"std.audio_out\", gain = 0.75 }",
        "out = { module = \"std.audio_out\" }",
    );
    fs::write(dir.join("project.toml"), &text).unwrap();
    let mut state = state(&dir, &setup);
    play(&mut state);
    fill_queue(&mut state, 0.5);
    state.apply(reset("out", "gain"));
    let message = state.view().message.unwrap();
    assert!(message.contains("the command queue is full"), "{message}");
    assert_eq!(state.view().writes, 1);
    assert_eq!(project_toml(&dir), text);

    run(&mut state, 4_096);
    assert_eq!(parameter(&state.view(), "out", "gain").effective, Some(0.5));
    assert_eq!(watch(&mut state), ("applied 1 sets".to_owned(), false));
    run(&mut state, 4_096);
    let view = state.view();
    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.effective), (1.0, Some(1.0)));
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// auditor-1-4: the watch sees the release's write while the queue is still
/// full, so its reload is refused; once playback drains the queue, the
/// reload is tried again without another change to the file.
#[test]
fn a_refused_reload_is_retried_without_another_change() {
    let (dir, setup) = copy("retry");
    let original = project_toml(&dir);
    let mut state = state(&dir, &setup);
    play(&mut state);
    fill_queue(&mut state, 0.5);
    state.apply(drag("out", "gain", 0.75));
    state.apply(release("out", "gain"));
    assert_eq!(project_toml(&dir), original);

    let (message, error) = watch(&mut state);
    assert!(error && message.contains("nothing was sent"), "{message}");
    assert!(message.ends_with("retrying"), "{message}");
    run(&mut state, 4_096);
    assert_eq!(parameter(&state.view(), "out", "gain").effective, Some(0.5));
    assert_eq!(watch(&mut state), ("applied 1 sets".to_owned(), false));
    run(&mut state, 4_096);
    let view = state.view();
    let gain = parameter(&view, "out", "gain");
    assert_eq!((gain.base, gain.effective), (0.75, Some(0.75)));
    assert_eq!(project_toml(&dir), original);
    // Nothing is pending: the next interval reloads nothing and sends nothing.
    let edits = state.edits().len();
    assert_eq!(watch(&mut state), ("applied 1 sets".to_owned(), false));
    run(&mut state, 4_096);
    assert_eq!(state.edits().len(), edits);
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// auditor-1-2: an exported parameter that fans out to targets with
/// different values in the composite's file; reset puts each target back
/// to its own value, and the reload finds nothing to apply.
#[test]
fn reset_restores_each_target_of_an_export() {
    let (dir, setup) = copy("fanout");
    let kick = dir.join("composites/kick.toml");
    let text = fs::read_to_string(&kick).unwrap().replace(
        "decay = \"amp_env.decay_ms\"",
        "decay = [\"amp_env.decay_ms\", \"pitch_env.decay_ms\"]",
    );
    fs::write(&kick, text).unwrap();
    let project = project_toml(&dir).replace(
        "  { from = \"lfo.out\", to = \"drums.kick_decay\", scale = 120 },\n",
        "",
    );
    fs::write(dir.join("project.toml"), project).unwrap();
    let original = project_toml(&dir);

    let mut state = state(&dir, &setup);
    play(&mut state);
    state.apply(drag("drums", "kick_decay", 500.0));
    state.apply(release("drums", "kick_decay"));
    run(&mut state, 4_096);
    let view = state.view();
    for name in ["kick/amp_env.decay_ms", "kick/pitch_env.decay_ms"] {
        let p = drums_inner(&view, name);
        assert_eq!((p.base, p.effective), (500.0, Some(500.0)), "{name}");
    }
    assert_eq!(watch(&mut state), ("no change to apply".to_owned(), false));

    state.apply(reset("drums", "kick_decay"));
    run(&mut state, 4_096);
    assert_eq!(project_toml(&dir), original);
    let view = state.view();
    for (name, own) in [
        ("kick/amp_env.decay_ms", 420.0),
        ("kick/pitch_env.decay_ms", 35.0),
    ] {
        let p = drums_inner(&view, name);
        assert_eq!((p.base, p.effective), (own, Some(own)), "{name}");
    }
    assert_eq!(watch(&mut state), ("no change to apply".to_owned(), false));
    state.apply(Command::Stop);
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

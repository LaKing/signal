//! M2.5-2 parameter write-back on copies of `projects/kick` in a temporary
//! folder (the committed files are only read): one value's text changes,
//! comments, ordering and spacing stay; a missing key is inserted at a
//! stable position; and after the write the M2-2 diff between the running
//! project and the reloaded folder is empty, so the M2-4 reload applies
//! nothing twice.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::{EventPayload, PortAddress, Registry};
use signal_engine::{RealtimeConfig, Stamp};
use signal_output::{Driver, NullDriver, realtime};
use signal_project::live::{Applied, reload};
use signal_project::session::{LiveRecorder, SessionRecorder};
use signal_project::{ParameterChange, Project, ProjectEdit, diff, provenance, write_parameter};

/// `drums`'s exported parameters, in declared (name) order.
const DRUMS: &[&str] = &["kick_decay", "kick_drive"];
/// `std.audio_out`'s parameters.
const OUT: &[&str] = &["gain"];

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn kick() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick")
}

/// A copy of `projects/kick` of its own for each test.
fn copy(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-write-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
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
    dir
}

fn project_toml(dir: &Path) -> String {
    fs::read_to_string(dir.join("project.toml")).unwrap()
}

/// The one line that differs, and in it the old and new text between the
/// longest common prefix and suffix.
fn changed_text(before: &str, after: &str) -> (String, String) {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    assert_eq!(a.len(), b.len(), "no line added or removed");
    assert_eq!(before.ends_with('\n'), after.ends_with('\n'));
    let changed: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    assert_eq!(changed.len(), 1, "exactly one line changes:\n{after}");
    let (x, y) = (a[changed[0]], b[changed[0]]);
    let prefix = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
    let suffix = x[prefix..]
        .bytes()
        .rev()
        .zip(y[prefix..].bytes().rev())
        .take_while(|(p, q)| p == q)
        .count();
    (
        x[prefix..x.len() - suffix].to_owned(),
        y[prefix..y.len() - suffix].to_owned(),
    )
}

fn comments(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains('#')).collect()
}

fn value(project: &Project, instance: &str, parameter: &str) -> Option<f64> {
    let def = project.root.instances.iter().find(|i| i.name == instance)?;
    def.parameters.get(parameter).copied()
}

/// AC1: `out.gain`'s value text changes and nothing else; the file loads
/// with the new value.
#[test]
fn writing_out_gain_changes_only_its_value_text() {
    let dir = copy("gain");
    let before = project_toml(&dir);
    let after = write_parameter(&before, "out", "gain", Some(0.6), OUT).unwrap();
    assert_eq!(changed_text(&before, &after), ("75".into(), "6".into()));
    assert!(after.contains("out = { module = \"std.audio_out\", gain = 0.6 }\n"));
    assert_eq!(comments(&before), comments(&after));
    fs::write(dir.join("project.toml"), &after).unwrap();
    let project = Project::load(&dir).unwrap();
    project.plan(&registry()).unwrap();
    assert_eq!(value(&project, "out", "gain"), Some(0.6));
    fs::remove_dir_all(&dir).unwrap();
}

/// AC1 and AC4: `drums`'s exposed `kick_decay` is not in the file, so it is
/// inserted into `drums`'s inline table; written again, only its value text
/// changes. Reset removes it, and the file is the original again.
#[test]
fn writing_drums_kick_decay_inserts_then_changes_one_value() {
    let dir = copy("decay");
    let original = project_toml(&dir);
    let inserted = write_parameter(&original, "drums", "kick_decay", Some(500.0), DRUMS).unwrap();
    assert_eq!(
        changed_text(&original, &inserted),
        (String::new(), ", kick_decay = 500".into())
    );
    assert!(inserted.contains("drums = { module = \"drums\", kick_decay = 500 }\n"));
    let changed = write_parameter(&inserted, "drums", "kick_decay", Some(437.5), DRUMS).unwrap();
    assert_eq!(
        changed_text(&inserted, &changed),
        ("500".into(), "437.5".into())
    );
    assert_eq!(comments(&original), comments(&changed));
    fs::write(dir.join("project.toml"), &changed).unwrap();
    let project = Project::load(&dir).unwrap();
    project.plan(&registry()).unwrap();
    assert_eq!(value(&project, "drums", "kick_decay"), Some(437.5));

    let reset = write_parameter(&changed, "drums", "kick_decay", None, DRUMS).unwrap();
    assert_eq!(reset, original, "reset to default removes the key");
    let reset = write_parameter(&original, "out", "gain", None, OUT).unwrap();
    assert!(reset.contains("out = { module = \"std.audio_out\" }\n"));
    fs::remove_dir_all(&dir).unwrap();
}

/// AC4: the inserted key's position follows the declared order, not the
/// order the keys were written in; the keys already there do not move.
#[test]
fn an_inserted_key_takes_a_stable_position() {
    let text = project_toml(&kick());
    let one = write_parameter(&text, "drums", "kick_decay", Some(400.0), DRUMS).unwrap();
    let one = write_parameter(&one, "drums", "kick_drive", Some(3.0), DRUMS).unwrap();
    let two = write_parameter(&text, "drums", "kick_drive", Some(3.0), DRUMS).unwrap();
    let two = write_parameter(&two, "drums", "kick_decay", Some(400.0), DRUMS).unwrap();
    assert_eq!(one, two);
    assert!(one.contains("drums = { module = \"drums\", kick_decay = 400, kick_drive = 3 }\n"));

    // An existing key is never moved, even when it is out of declared order.
    let order = ["a", "b", "c"];
    let text = "schema_version = 1\n[instances]\nx = { module = \"m\", c = 3 }  # keep\n";
    let written = write_parameter(text, "x", "a", Some(1.0), &order).unwrap();
    assert_eq!(
        written,
        "schema_version = 1\n[instances]\nx = { module = \"m\", a = 1, c = 3 }  # keep\n"
    );
    // Values that are not integers are written so they read back exactly.
    let written = write_parameter(text, "x", "c", Some(0.1 + 0.2), &order).unwrap();
    assert!(written.contains("c = 0.30000000000000004 }"), "{written}");
    let written = write_parameter(text, "x", "c", Some(-0.0), &order).unwrap();
    assert!(written.contains("c = -0.0 }"), "{written}");
    assert!(write_parameter(text, "y", "c", Some(1.0), &order).is_err());
    assert!(write_parameter(text, "x", "c", Some(f64::NAN), &order).is_err());
}

/// AC2: while the copy plays, the value the panel sent is written back and
/// the running project becomes `with_parameter`'s. The M2-2 diff between it
/// and the reloaded folder is empty, and the M2-4 reload (`--watch`) sends
/// nothing; without the update the same reload would send the value again.
#[test]
fn after_write_back_the_reload_applies_nothing_twice() {
    let registry = registry();
    let dir = copy("reload");
    let mut current = Project::load(&dir).unwrap();
    let stale = Project::load(&dir).unwrap();
    let mut plan = current.plan(&registry).unwrap().plan;
    plan.version = 0;
    let rate = current.sample_rate;
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(rate, current.seed)).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &current.files);
    let recorder =
        SessionRecorder::new(&current, &registry, rate, current.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(rate, 2, 3);
    driver.start(runner).unwrap();

    // What the panel sends while dragging, then writes on release.
    let port: PortAddress = "/drums/kick/amp_env.decay_ms".parse().unwrap();
    controller
        .send_event(port.clone(), EventPayload::Set(500.0), Stamp::Default)
        .unwrap();
    driver.run(4096, || {}).unwrap();
    controller.poll(&mut live);
    let text = project_toml(&dir);
    let written = write_parameter(&text, "drums", "kick_decay", Some(500.0), DRUMS).unwrap();
    fs::write(dir.join("project.toml"), written).unwrap();
    current = current
        .with_parameter("drums", "kick_decay", Some(500.0), DRUMS)
        .unwrap();

    let reloaded = Project::load(&dir).unwrap();
    assert!(diff(&current, &reloaded, &registry).unwrap().is_empty());
    match diff(&stale, &reloaded, &registry).unwrap() {
        ProjectEdit::Parameters(changes) => assert_eq!(
            changes,
            [ParameterChange {
                port,
                payload: EventPayload::Set(500.0)
            }],
            "without the update, the value would be applied twice"
        ),
        edit => panic!("{edit:?}"),
    }
    let applied = reload(&mut controller, &mut live, &mut current, &dir, &registry).unwrap();
    assert_eq!(applied, Applied::default());
    assert_eq!(applied.to_string(), "no change to apply");
    fs::remove_dir_all(&dir).unwrap();
}

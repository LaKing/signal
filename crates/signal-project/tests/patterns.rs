//! M2.11-1 (ADR 0028 §1–4): pattern files are project files. They load and
//! validate with file:line:column errors (unknown keys included), an
//! instance names one with the reserved key `pattern`, the type's library
//! compiles it into the instance's table at plan build, a changed pattern is
//! a swap that keeps every instance, and they are hashed, packed and watched
//! like the other project files. Fixtures are written here; no committed
//! project is touched.

use std::fs;
use std::path::PathBuf;

use signal_core::Registry;
use signal_project::archive::{pack, unpack};
use signal_project::live::ProjectWatcher;
use signal_project::{Project, ProjectEdit, SourceFile, diff, read_sources};

const PROJECT: &str = r#"schema_version = 1

[project]
name = "seq"
sample_rate = 48000

[instances]
clock = { module = "std.transport", bpm = 120 }
melody = { module = "std.note_seq", pattern = "melody" }
drums = { module = "std.step_seq8", pattern = "beat" }
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "clock.tick", to = "melody.tick" },
  { from = "clock.tick", to = "drums.tick" },
  { from = "melody.hz", to = "osc.freq" },
  { from = "osc.out", to = "out.l" },
  { from = "osc.out", to = "out.r" },
]
"#;

const MELODY: &str = r#"schema_version = 1
kind = "notes"
length = 4

[[notes]]
key = 64
start = 1
length = 0.5
velocity = 0.8

[[notes]]
key = 60
start = 0
length = 1
velocity = 1
"#;

const BEAT: &str = r#"schema_version = 1
kind = "steps"
steps = 4
step = 0.25

[lanes]
kick = "x..X"
hat = ".x.x"

[levels]
hat = 0.5
"#;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn sources(files: &[(&str, &str)]) -> Vec<SourceFile> {
    files
        .iter()
        .map(|(path, text)| SourceFile {
            path: (*path).to_owned(),
            text: (*text).to_owned(),
        })
        .collect()
}

fn valid() -> Vec<(&'static str, &'static str)> {
    vec![
        ("project.toml", PROJECT),
        ("patterns/melody.toml", MELODY),
        ("patterns/beat.toml", BEAT),
    ]
}

/// The diagnostics of loading and planning `files`, one per line.
fn errors(files: &[(&str, &str)]) -> String {
    let diagnostics = match Project::from_sources(sources(files)) {
        Err(diagnostics) => diagnostics,
        Ok(project) => match project.plan(&registry()) {
            Err(diagnostics) => diagnostics,
            Ok(_) => panic!("expected errors"),
        },
    };
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `valid()` with `path` replaced by `text` (or added).
fn with(path: &'static str, text: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut files: Vec<_> = valid().into_iter().filter(|(p, _)| *p != path).collect();
    files.push((path, text));
    files
}

/// A folder of its own for each test.
fn folder(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-patterns-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    dir
}

/// Both kinds load; the library compiles them into the tables of the
/// instances that name them, and nothing else gets one.
#[test]
fn patterns_load_and_become_their_instances_tables() {
    let project = Project::from_sources(sources(&valid())).unwrap();
    let paths: Vec<&str> = project.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["project.toml", "patterns/beat.toml", "patterns/melody.toml"]
    );
    let plan = project.plan(&registry()).unwrap().plan;
    let table = |address: &str| {
        plan.instances
            .iter()
            .find(|i| i.address.to_string() == address)
            .unwrap()
            .table
            .clone()
    };
    let melody = table("/melody").unwrap();
    // Row 0 the length; then [start, length, key, velocity, hz] by start.
    assert_eq!(melody.row(0)[0], 4.0);
    assert_eq!(melody.row(1)[..4], [0.0, 1.0, 60.0, 1.0]);
    assert_eq!(melody.row(2)[..4], [1.0, 0.5, 64.0, 0.8]);
    let beat = table("/drums").unwrap();
    assert_eq!(beat.rows(), 5);
    // Lanes in file order: kick, then hat.
    assert_eq!(beat.row(0)[..4], [4.0, 0.25, 1.0, 0.5]);
    assert_eq!(beat.row(4)[..2], [2.0, 1.0]);
    assert!(table("/osc").is_none() && table("/clock").is_none());
}

/// Unknown keys, bad values and overlapping notes are errors at the text.
#[test]
fn pattern_files_are_validated_at_their_text() {
    let cases = [
        (
            MELODY.replace("velocity = 1\n", "velocity = 1\ncolour = 2\n"),
            "patterns/melody.toml:16:1: error: unknown field `colour`, expected one of `key`, `start`, `length`, `velocity`",
        ),
        (
            MELODY.replace("length = 4", "length = 4\ntempo = 3"),
            "patterns/melody.toml:4:1: error: unknown field `tempo`, expected one of `schema_version`, `kind`, `length`, `notes`",
        ),
        (
            MELODY.replace("velocity = 0.8", "velocity = 1.5"),
            "patterns/melody.toml:9:12: error: velocity must be from 0 to 1",
        ),
        (
            MELODY.replace("start = 1\n", "start = -1\n"),
            "patterns/melody.toml:7:9: error: start must be a finite number of beats, at least 0",
        ),
        (
            MELODY.replace("length = 0.5", "length = 0"),
            "patterns/melody.toml:8:10: error: length must be a finite number of beats above 0",
        ),
        (
            MELODY.replace("length = 4", "length = 0"),
            "patterns/melody.toml:3:10: error: length must be above 0 and at most 65536 beats",
        ),
        (
            MELODY.replace("key = 64\nstart = 1", "key = 60\nstart = 0.5"),
            "patterns/melody.toml:5:1: error: this note overlaps an earlier note on key 60",
        ),
        (
            MELODY.replace("key = 64", "key = 64.0"),
            "patterns/melody.toml:6:7: error: invalid type: floating point `64.0`, expected i64",
        ),
        (
            MELODY.replace("key = 64", "key = 64.5"),
            "patterns/melody.toml:6:7: error: invalid type: floating point `64.5`, expected i64",
        ),
        (
            MELODY.replace("key = 64", "key = 128"),
            "patterns/melody.toml:6:7: error: key must be an integer from 0 to 127",
        ),
        (
            "schema_version = 1\nkind = \"notes\"\nlength = 4\n".to_owned(),
            "patterns/melody.toml:1:1: error: missing field `notes`",
        ),
        (
            MELODY.replace("kind = \"notes\"", "kind = \"chords\""),
            "patterns/melody.toml:2:8: error: unknown pattern kind; it is \"notes\" or \"steps\"",
        ),
        (
            MELODY.replace("schema_version = 1", "schema_version = 2"),
            "patterns/melody.toml:1:18: error: unsupported schema_version 2; this version of Signal reads 1",
        ),
    ];
    let mut wrong = Vec::new();
    for (text, expected) in &cases {
        let text: &'static str = Box::leak(text.clone().into_boxed_str());
        let got = errors(&with("patterns/melody.toml", text));
        if got != *expected {
            wrong.push(got);
        }
    }
    let steps = [
        (
            BEAT.replace("kick = \"x..X\"", "kick = \"x..\""),
            "patterns/beat.toml:7:8: error: lane `kick` has 3 cells; the pattern has 4 steps",
        ),
        (
            BEAT.replace("kick = \"x..X\"", "kick = \"x..o\""),
            "patterns/beat.toml:7:8: error: lane `kick`: `o` is not a cell; use `.` (rest), `x` (hit) or `X` (accent)",
        ),
        (
            BEAT.replace("hat = 0.5", "hat = 2"),
            "patterns/beat.toml:11:7: error: level of lane `hat` must be from 0 to 1",
        ),
        (
            BEAT.replace("hat = 0.5", "hat = 0.5\nsnare = 1"),
            "patterns/beat.toml:12:1: error: `snare` in [levels] is not a lane; every key of [levels] names a lane of [lanes]",
        ),
        (
            BEAT.replace("hat = \".x.x\"", "hat = { cells = \".x.x\", level = 0.5 }"),
            "patterns/beat.toml:8:7: error: invalid type: map, expected a string",
        ),
        (
            BEAT.replace("steps = 4", "steps = 0"),
            "patterns/beat.toml:3:9: error: steps must be from 1 to 256",
        ),
        (
            BEAT.replace("step = 0.25", "step = 0.25\nswing = 50"),
            "patterns/beat.toml:5:1: error: unknown field `swing`, expected one of `schema_version`, `kind`, `steps`, `step`, `lanes`, `levels`",
        ),
    ];
    for (text, expected) in &steps {
        let text: &'static str = Box::leak(text.clone().into_boxed_str());
        let got = errors(&with("patterns/beat.toml", text));
        if got != *expected {
            wrong.push(got);
        }
    }
    assert!(wrong.is_empty(), "got:\n{}", wrong.join("\n"));
    assert_eq!(
        errors(&with("patterns/Beat-2.toml", BEAT)),
        "patterns/Beat-2.toml:1:1: error: invalid pattern name `Beat-2`; a pattern file is patterns/<name>.toml, and names use a-z, 0-9, _ and start with a-z"
    );
}

/// Operator notes 1–2: `notes = []` is a pattern of no notes; the steps
/// example of note 1 (`[levels]`) loads verbatim, a lane missing from
/// `[levels]` has level 1.
#[test]
fn empty_notes_and_the_levels_example_load() {
    let empty = "schema_version = 1\nkind = \"notes\"\nlength = 4\nnotes = []\n";
    let example = r#"schema_version = 1
kind = "steps"
steps = 16
step = 0.25

[lanes]
kick  = "X...x...x...x..x"
snare = "....X.......X..."

[levels]
kick = 0.9
"#;
    let files = with("patterns/melody.toml", empty);
    let files: Vec<_> = files
        .into_iter()
        .filter(|(p, _)| *p != "patterns/beat.toml")
        .chain([("patterns/beat.toml", example)])
        .collect();
    let plan = Project::from_sources(sources(&files))
        .unwrap()
        .plan(&registry())
        .unwrap()
        .plan;
    let table = |address: &str| {
        plan.instances
            .iter()
            .find(|i| i.address.to_string() == address)
            .unwrap()
            .table
            .clone()
            .unwrap()
    };
    assert_eq!(table("/melody").rows(), 1, "only the header row");
    let beat = table("/drums");
    assert_eq!(beat.row(0)[..4], [16.0, 0.25, 0.9, 1.0]);
    assert_eq!(beat.row(1)[..2], [2.0, 0.0]);
    assert_eq!(beat.row(5)[..2], [1.0, 2.0]);
}

/// AC3: `pattern` is reserved; a missing file, a wrong kind and a type that
/// takes none are errors naming the instance and the file.
#[test]
fn the_pattern_key_is_checked() {
    let project = |from: &str, to: &str| -> &'static str {
        Box::leak(PROJECT.replace(from, to).into_boxed_str())
    };
    assert_eq!(
        errors(&with(
            "project.toml",
            project("pattern = \"melody\"", "pattern = \"tune\"")
        )),
        "project.toml:9:47: error: melody: pattern `tune`: patterns/tune.toml does not exist"
    );
    assert_eq!(
        errors(&with(
            "project.toml",
            project("pattern = \"melody\"", "pattern = \"beat\"")
        )),
        "project.toml:9:47: error: melody: `std.note_seq` takes a `notes` pattern; patterns/beat.toml is `steps`"
    );
    assert_eq!(
        errors(&with(
            "project.toml",
            project(
                "osc = { module = \"std.sine_osc\" }",
                "osc = { module = \"std.sine_osc\", pattern = \"melody\" }"
            )
        )),
        "project.toml:11:44: error: osc: `std.sine_osc` takes no pattern, so it cannot take patterns/melody.toml"
    );
    // A key outside 0..127 is refused at the text, before any library.
    let high = Box::leak(MELODY.replace("key = 64", "key = 130").into_boxed_str());
    assert_eq!(
        errors(&with("patterns/melody.toml", high)),
        "patterns/melody.toml:6:7: error: key must be an integer from 0 to 127"
    );
    // A composite takes no pattern, and cannot export one.
    let voice = r#"schema_version = 1

[composite]
name = "voice"

[instances]
seq = { module = "std.note_seq", pattern = "melody" }

[exports]
inputs = { tick = "seq.tick" }
outputs = { hz = "seq.hz" }
parameters = { pattern = "seq.ppq" }
"#;
    let mut files = with(
        "project.toml",
        project(
            "osc = { module = \"std.sine_osc\" }",
            "osc = { module = \"std.sine_osc\" }\nv = { module = \"voice\", pattern = \"melody\" }",
        ),
    );
    files.push(("composites/voice.toml", voice));
    assert_eq!(
        errors(&files),
        "project.toml:12:35: error: v: `pattern` gives a primitive instance its data; `voice` is a composite, which takes no pattern\n\
         composites/voice.toml:12:16: error: a composite cannot export a parameter named `pattern`: the key is reserved in instance tables"
    );
    // Inside a composite, an instance names a project pattern as at the top.
    files.retain(|(p, _)| !p.starts_with("project"));
    files.push((
        "project.toml",
        project(
            "osc = { module = \"std.sine_osc\" }",
            "osc = { module = \"std.sine_osc\" }\nv = { module = \"voice\" }",
        ),
    ));
    files.retain(|(p, _)| *p != "composites/voice.toml");
    files.push((
        "composites/voice.toml",
        Box::leak(
            voice
                .replace("parameters = { pattern = \"seq.ppq\" }\n", "")
                .into_boxed_str(),
        ),
    ));
    let project = Project::from_sources(sources(&files)).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let inner = plan
        .instances
        .iter()
        .find(|i| i.address.to_string() == "/v/seq")
        .unwrap();
    assert_eq!(inner.table.as_ref().unwrap().rows(), 3);
}

/// A changed pattern is a structural edit that keeps every instance; the
/// same pattern rewritten (comments, order) changes nothing.
#[test]
fn a_changed_pattern_is_a_swap_that_keeps_every_instance() {
    let registry = registry();
    let old = Project::from_sources(sources(&valid())).unwrap();
    let edited = with(
        "patterns/melody.toml",
        Box::leak(MELODY.replace("key = 64", "key = 67").into_boxed_str()),
    );
    let new = Project::from_sources(sources(&edited)).unwrap();
    match diff(&old, &new, &registry).unwrap() {
        ProjectEdit::Structural {
            removed,
            added,
            sets,
            plan,
        } => {
            assert!(removed.is_empty() && added.is_empty() && sets.is_empty());
            assert_eq!(plan.instances.len(), 5);
        }
        other => panic!("not a swap: {other:?}"),
    }
    let reordered = with(
        "patterns/melody.toml",
        Box::leak(format!("# the same notes\n{MELODY}").into_boxed_str()),
    );
    let same = Project::from_sources(sources(&reordered)).unwrap();
    assert!(diff(&old, &same, &registry).unwrap().is_empty());
}

/// Patterns are read from `patterns/` (their `.toml` files only), packed
/// into a `.signal` file and watched.
#[test]
fn patterns_are_read_packed_and_watched() {
    let dir = folder("files", &valid());
    fs::write(dir.join("patterns/notes.txt"), "not a pattern").unwrap();
    fs::create_dir_all(dir.join("patterns/old")).unwrap();
    fs::write(dir.join("patterns/old/x.toml"), "ignored").unwrap();
    let paths: Vec<String> = read_sources(&dir)
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(
        paths,
        ["project.toml", "patterns/beat.toml", "patterns/melody.toml"]
    );

    let packed = pack(&dir).unwrap();
    let into = dir.with_extension("unpacked");
    let _ = fs::remove_dir_all(&into);
    let unpacked = unpack(&packed, &into).unwrap();
    assert_eq!(
        unpacked.files,
        ["patterns/beat.toml", "patterns/melody.toml", "project.toml"]
    );
    assert_eq!(
        fs::read_to_string(into.join("patterns/beat.toml")).unwrap(),
        BEAT
    );
    let _ = fs::remove_dir_all(&into);

    let mut watcher = ProjectWatcher::new(&dir).unwrap();
    assert!(!watcher.poll().unwrap());
    fs::write(dir.join("patterns/beat.toml"), BEAT.replace("x..X", "xx.X")).unwrap();
    assert!(watcher.poll().unwrap());

    #[cfg(unix)]
    {
        let linked = folder("linked", &[("project.toml", PROJECT)]);
        std::os::unix::fs::symlink(dir.join("patterns"), linked.join("patterns")).unwrap();
        assert!(pack(&linked).is_err(), "a linked patterns/ is refused");
        let _ = fs::remove_dir_all(&linked);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// auditor-1-007 (AC2): every TOML example of `project-format.md` §12 loads
/// verbatim as a pattern file, and reads as a pattern.
#[test]
fn every_example_of_the_format_loads() {
    let doc = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/project-format.md"),
    )
    .unwrap();
    let section = &doc[doc.find("## 12.").unwrap()..];
    let section = &section[..section.find("\n## 13.").unwrap_or(section.len())];
    let examples: Vec<&str> = section
        .split("```toml\n")
        .skip(1)
        .map(|block| &block[..block.find("```").unwrap()])
        .collect();
    assert_eq!(
        examples.len(),
        3,
        "the notes, inline notes and steps examples"
    );
    for (i, example) in examples.iter().enumerate() {
        assert!(
            signal_project::pattern_kind(example).is_some(),
            "example {i} does not read:\n{example}"
        );
        let project = Project::from_sources(sources(&[
            (
                "project.toml",
                "schema_version = 1\n\n[project]\nname = \"doc\"\n",
            ),
            ("patterns/example.toml", example),
        ]));
        assert!(project.is_ok(), "example {i}: {:?}", project.err());
    }
}

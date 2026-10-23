//! AGENTS.md rule 1, ADR 0022 (M2.7-9): the engine holds no audio output
//! policy. No code line of `src/` and nothing in the manifest's
//! `[dependencies]` names the audio sink, its `gain`, its channel mapping,
//! the device conversion or the WAV writer: those are the audio adapter's
//! (`signal-output`). The engine hands every sink's inputs to the driver
//! generically (`Engine::sinks`, `SinkOutput`).
//!
//! Deliberately not flagged (port kinds and artifact names, not output
//! policy): `PortKind::Audio` and the fade bookkeeping built on it
//! (`output_audio`), and the render manifest's `audio_fnv1a64` and
//! `device_conversion` fields, which name the artifact files and whose
//! values the adapter supplies. Comment lines are skipped, so prose may name
//! the words; `mpsc::channel` is std's.

use std::fs;
use std::path::{Path, PathBuf};

/// Whole identifier words (`_` and alphanumerics continue a word).
const WORDS: [&str; 7] = [
    "gain",
    "GAIN",
    "audio_out",
    "channel",
    "channels",
    "device_sample",
    "hound",
];

/// Anywhere in a code line.
const PARTS: [&str; 3] = ["audio_sink", "AudioSink", "AudioChannels"];

/// Allowed spellings that contain a word above.
const ALLOWED: [&str; 1] = ["mpsc::channel"];

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// Whether `line` holds `word` as a whole identifier word.
fn has_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        let continues = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !continues(before) && !continues(after)
    })
}

/// The audio names in `line`, after removing the allowed spellings.
fn offences(line: &str) -> Vec<&'static str> {
    let mut line = line.to_owned();
    for allowed in ALLOWED {
        line = line.replace(allowed, "");
    }
    let words = WORDS.into_iter().filter(|w| has_word(&line, w));
    let parts = PARTS.into_iter().filter(|p| line.contains(p));
    words.chain(parts).collect()
}

#[test]
fn the_engine_names_no_audio_output_policy() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&crate_dir.join("src"), &mut files);
    assert!(files.len() > 15, "the walk found {} files", files.len());

    let mut found = Vec::new();
    for file in &files {
        let name = file.strip_prefix(crate_dir).unwrap_or(file).display();
        let text = fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for word in offences(code) {
                found.push(format!("{name}:{}: `{word}` in {code}", n + 1));
            }
        }
    }

    // The manifest's dependencies (dev-dependencies are its tests', which
    // use the adapter).
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    let dependencies = manifest
        .split("\n[")
        .find(|section| section.starts_with("dependencies]"))
        .expect("a [dependencies] section");
    for line in dependencies.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        for word in offences(line) {
            found.push(format!("Cargo.toml [dependencies]: `{word}` in {line}"));
        }
    }

    assert!(
        found.is_empty(),
        "audio output policy in signal-engine (ADR 0022):\n{}",
        found.join("\n")
    );
}

/// The check finds what it looks for.
#[test]
fn the_check_recognises_the_names() {
    assert_eq!(
        offences("pub(crate) const GAIN: &str = \"gain\";"),
        ["gain", "GAIN"]
    );
    assert_eq!(offences("for channel in 0..width {"), ["channel"]);
    assert_eq!(
        offences("let sinks = engine.audio_sinks();"),
        ["audio_sink"]
    );
    assert_eq!(offences("hound = { workspace = true }"), ["hound"]);
    assert!(offences("let (done, dropped) = mpsc::channel();").is_empty());
    assert!(
        offences("// the gain of a fade").len() == 1,
        "callers skip comments"
    );
    assert!(offences("again, against the gainst").is_empty());
}

/// Musical words (ADR 0028, AGENTS.md rule 1): notes, keys, steps and the
/// rest of a pattern's vocabulary belong to `signal-std` and to the project
/// format's parsing. The instance table path is rows of numbers only.
const MUSICAL: [&str; 16] = [
    "velocity",
    "ppq",
    "swing",
    "note",
    "notes",
    "pattern",
    "patterns",
    "midi",
    "MIDI",
    "piano",
    "bpm",
    "tempo",
    "beat",
    "beats",
    "accent",
    "step_seq8",
];

/// Core, graph and engine name no musical concept in a code line (M2.11-1
/// AC12): the instance table they carry is domain-free.
#[test]
fn core_graph_and_engine_name_no_musical_concept() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for name in ["signal-core", "signal-graph", "signal-engine"] {
        let mut files = Vec::new();
        rust_files(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name}: no sources found");
        for file in &files {
            let text = fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for word in MUSICAL.iter().filter(|w| has_word(code, w)) {
                    found.push(format!("{}:{}: `{word}` in {code}", file.display(), n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "musical concepts in a domain-free crate:\n{}",
        found.join("\n")
    );
    assert!(has_word("let velocity = 1.0;", "velocity"));
}

/// Project words (ADR 0035 §1, ADR 0037 §5, AGENTS.md rule 1): an included
/// project and a slot are resolved by `signal-project` into ordinary
/// composite instances before the graph sees them; the core, the graph and
/// the engine know a generic scope root, the `ports_of` query and nothing
/// else. The ADRs also list `slot`, `interface` and `load`, but those are
/// already these crates' own generic words (the engine's per-instance state
/// slots, a composite's resolved exports in the flattener, atomic loads), so
/// the check names the project format's spellings of the two concepts
/// instead (ADR 0037, note of 2026-10-23).
const PROJECT_WORDS: [&str; 9] = [
    "project",
    "projects",
    "include",
    "includes",
    "included",
    "interfaces",
    "SlotResolution",
    "SlotResolutions",
    "InterfaceFile",
];

/// Core, graph and engine name no project or slot concept in a code line.
#[test]
fn core_graph_and_engine_name_no_project_or_slot() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for name in ["signal-core", "signal-graph", "signal-engine"] {
        let mut files = Vec::new();
        rust_files(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name}: no sources found");
        for file in &files {
            let text = fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for word in PROJECT_WORDS.iter().filter(|w| has_word(code, w)) {
                    found.push(format!("{}:{}: `{word}` in {code}", file.display(), n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "project or slot concepts in a domain-free crate:\n{}",
        found.join("\n")
    );
    assert!(has_word("let project = load();", "project"));
    assert!(!has_word("include_str!(\"x\")", "include"));
}

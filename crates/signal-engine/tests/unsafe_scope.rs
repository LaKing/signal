//! AGENTS.md rule 9 (ADR 0017, ADR 0019 §4, ADR 0020, ADR 0036): `unsafe`
//! code appears only in the four audited modules, and `#![allow(unsafe_code)]`
//! only there.
//! Every `.rs` file under `crates/` and `lab/` is read; comment lines are
//! skipped, so prose may name the word.

use std::fs;
use std::path::{Path, PathBuf};

/// The audited modules, relative to the repository root.
const AUDITED: [&str; 4] = [
    "crates/signal-contract/src/alloc_counter.rs",
    "crates/signal-audio/src/rt_thread.rs",
    "crates/signal-engine/src/shared.rs",
    "crates/signal-audio/src/device_props.rs",
];

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

/// Whether `line` holds the keyword as a word (not `unsafe_code`).
fn has_keyword(line: &str, keyword: &str) -> bool {
    line.match_indices(keyword).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + keyword.len()..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !word(before) && !word(after)
    })
}

#[test]
fn unsafe_appears_only_in_the_audited_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let keyword = concat!("un", "safe");
    let allow = concat!("allow(un", "safe_code)");
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    rust_files(&root.join("lab"), &mut files);
    assert!(files.len() > 50, "the walk found {} files", files.len());

    let mut offenders = Vec::new();
    let mut audited_found = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let relative = relative.trim_start_matches("./").to_owned();
        let audited = AUDITED.contains(&relative.as_str());
        let text = fs::read_to_string(file).unwrap();
        let mut uses = false;
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if has_keyword(code, keyword) || code.contains(allow) {
                uses = true;
                if !audited {
                    offenders.push(format!("{relative}:{}: {}", n + 1, code));
                }
            }
        }
        if audited && uses {
            audited_found.push(relative);
        }
    }
    assert!(
        offenders.is_empty(),
        "the keyword outside the audited modules:\n{offenders:#?}"
    );
    // The four audited modules do use it.
    for module in AUDITED {
        assert!(audited_found.iter().any(|f| f == module), "{module}");
    }
}

/// The lines of `text` that use the keyword without a comment holding
/// `SAFETY:` right above them (attributes may stand between), and how many
/// lines use it.
fn without_safety_comments(text: &str) -> (usize, Vec<String>) {
    let keyword = concat!("un", "safe");
    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
    let mut uses = 0;
    let mut missing = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if line.starts_with("//") || !has_keyword(line, keyword) {
            continue;
        }
        uses += 1;
        let mut above = lines[..n]
            .iter()
            .rev()
            .skip_while(|l| l.starts_with("#["))
            .take_while(|l| l.starts_with("//"));
        if !above.any(|l| l.contains("SAFETY:")) {
            missing.push(format!("{}: {line}", n + 1));
        }
    }
    (uses, missing)
}

/// ADR 0036: every use of the keyword in `device_props.rs` follows a
/// `SAFETY:` comment, and the module only reads: it never sets a property.
#[test]
fn device_props_only_reads_with_safety_comments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = fs::read_to_string(root.join(AUDITED[3])).unwrap();
    let (uses, missing) = without_safety_comments(&text);
    assert!(uses >= 1, "device_props.rs uses the keyword {uses} times");
    assert!(
        missing.is_empty(),
        "without a SAFETY comment:\n{missing:#?}"
    );
    let code: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();
    assert!(
        !code.iter().any(|l| l.contains("SetPropertyData")),
        "device_props.rs sets a property"
    );
}

/// ADR 0019 §4 (M2.6-3): every line of `rt_thread.rs` that uses the keyword
/// follows a comment holding `SAFETY:` (attributes may stand between), and
/// `os_workgroup_join` and `os_workgroup_leave` are declared there in an
/// `extern "C"` block.
#[test]
fn every_unsafe_use_in_rt_thread_has_a_safety_comment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = fs::read_to_string(root.join(AUDITED[1])).unwrap();
    let keyword = concat!("un", "safe");
    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
    let mut uses = 0;
    let mut missing = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if line.starts_with("//") || !has_keyword(line, keyword) {
            continue;
        }
        uses += 1;
        let mut above = lines[..n]
            .iter()
            .rev()
            .skip_while(|l| l.starts_with("#["))
            .take_while(|l| l.starts_with("//"));
        if !above.any(|l| l.contains("SAFETY:")) {
            missing.push(format!("{}: {line}", n + 1));
        }
    }
    assert!(uses >= 5, "rt_thread.rs uses the keyword {uses} times");
    assert!(
        missing.is_empty(),
        "without a SAFETY comment:\n{missing:#?}"
    );

    let block = format!("{keyword} extern \"C\" {{");
    let start = text.find(&block).expect("an extern \"C\" block");
    let end = start + text[start..].find("\n}").expect("its end");
    let declarations = &text[start..end];
    for function in ["fn os_workgroup_join(", "fn os_workgroup_leave("] {
        assert!(declarations.contains(function), "{function} is declared");
    }
}

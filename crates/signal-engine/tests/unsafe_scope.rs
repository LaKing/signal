//! AGENTS.md rule 9 (ADR 0017, ADR 0019 §4, ADR 0020): `unsafe` code appears
//! only in the three audited modules, and `#![allow(unsafe_code)]` only there.
//! Every `.rs` file under `crates/` and `lab/` is read; comment lines are
//! skipped, so prose may name the word.

use std::fs;
use std::path::{Path, PathBuf};

/// The audited modules, relative to the repository root.
const AUDITED: [&str; 3] = [
    "crates/signal-contract/src/alloc_counter.rs",
    "crates/signal-audio/src/rt_thread.rs",
    "crates/signal-engine/src/shared.rs",
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
    // The two modules that exist today do use it (rt_thread.rs is M2.6-3).
    for module in [AUDITED[0], AUDITED[2]] {
        assert!(audited_found.iter().any(|f| f == module), "{module}");
    }
}

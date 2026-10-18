//! M2.10-3 AC8 (ADR 0027 §4, versioning): every library composite's output
//! is pinned. Each renders its cases (`common::cases`: its fixed stimulus at
//! its defaults, long enough for its whole signal path, `common::frames`;
//! `lfo_shapes` once per shape with every output), 48 kHz, seed 1. Its
//! pinned hash is the FNV-1a 64 of the audio as `audio.wav` stores it (of
//! the case hashes, one per line, where it has several), and must equal its
//! row in `tests/pinned.toml`.
//!
//! `CHANGELOG.md` must account for every row ([`check_changelog`]): a hash
//! is named, with its composite, by an entry under the current `## MAJOR.MINOR`
//! section, or by one under an earlier section (the output has not changed
//! since). So an output change needs a new row **and** an entry under the
//! current version. Earlier sections are history: each one's FNV-1a 64 is
//! recorded in `tests/released.toml` when the next MINOR starts, and must
//! not change.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{cases, faults, project_with, render};
use signal_core::fnv1a64;

fn crate_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

/// The pinned hash of `lib.<name>`: its one case's audio hash, or the hash
/// of its cases' audio hashes, one per line.
fn pinned_hash(name: &str) -> String {
    let hashes: Vec<String> = cases(name)
        .iter()
        .map(|case| {
            let project = project_with(name, &case.parameters, &case.instances, &case.connections);
            let rendered = render(&project);
            assert_eq!(
                faults(&rendered),
                Vec::<String>::new(),
                "lib.{name} ({})",
                case.parameters
            );
            hex(rendered.audio_fnv1a64())
        })
        .collect();
    match hashes.as_slice() {
        [one] => one.clone(),
        many => hex(fnv1a64(many.join("\n").as_bytes())),
    }
}

/// A `## <MAJOR.MINOR>` section of the changelog: its version and its text
/// (the lines after the heading, up to the next `## ` heading).
struct Section {
    version: String,
    text: String,
}

fn is_version(text: &str) -> bool {
    let mut parts = text.split('.');
    let numeric =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    numeric(parts.next()) && numeric(parts.next()) && parts.next().is_none()
}

/// The version sections of `changelog`, in file order (other `## ` sections,
/// such as "Versioning", are not versions and are skipped).
fn sections(changelog: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.extend(current.take());
            let heading = heading.trim();
            current = is_version(heading).then(|| Section {
                version: heading.to_owned(),
                text: String::new(),
            });
        } else if let Some(section) = &mut current {
            section.text.push_str(line);
            section.text.push('\n');
        }
    }
    sections.extend(current);
    for section in &mut sections {
        section.text = section.text.trim().to_owned();
    }
    sections
}

/// Whether `text` has an entry line naming `lib.<name>` with `hash`.
fn names(text: &str, name: &str, hash: &str) -> bool {
    let entry = format!("`lib.{name}`");
    text.lines()
        .any(|line| line.contains(&entry) && line.contains(hash))
}

/// What is wrong with `changelog` for the `pinned` rows (name → hash) at the
/// library `version`, with `released` (version → FNV-1a 64 of its section's
/// text) the frozen history.
fn check_changelog(
    changelog: &str,
    version: &str,
    pinned: &BTreeMap<String, String>,
    released: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let sections = sections(changelog);
    let current = sections.iter().find(|s| s.version == version);
    if current.is_none() {
        errors.push(format!("CHANGELOG.md has no `## {version}` section"));
    }
    let earlier: Vec<&Section> = sections.iter().filter(|s| s.version != version).collect();
    for section in &earlier {
        let hash = hex(fnv1a64(section.text.as_bytes()));
        match released.get(&section.version) {
            None => errors.push(format!(
                "`## {}` is an earlier version: record its hash in tests/released.toml (`\"{}\" = \"{hash}\"`); it is history and must not change",
                section.version, section.version
            )),
            Some(recorded) if *recorded != hash => errors.push(format!(
                "`## {}` changed (its hash {hash}, released as {recorded}): earlier sections are history; a change goes under `## {version}`",
                section.version
            )),
            Some(_) => {}
        }
    }
    for old in released.keys() {
        if !earlier.iter().any(|s| s.version == *old) {
            errors.push(format!(
                "`## {old}` is in tests/released.toml but no longer in CHANGELOG.md: earlier sections are history"
            ));
        }
    }
    for (name, hash) in pinned {
        let now = current.is_some_and(|s| names(&s.text, name, hash));
        let before = earlier.iter().any(|s| names(&s.text, name, hash));
        if !now && !before {
            errors.push(format!(
                "`lib.{name}` renders {hash}, which no CHANGELOG.md entry names: an output change needs an entry naming `lib.{name}` with {hash} under `## {version}`"
            ));
        }
    }
    errors
}

#[test]
fn every_library_composite_renders_its_pinned_hash() {
    let pinned: BTreeMap<String, String> = toml::from_str(&crate_file("tests/pinned.toml"))
        .expect("tests/pinned.toml: a table of `name = \"hash\"`");
    let released: BTreeMap<String, String> = toml::from_str(&crate_file("tests/released.toml"))
        .expect("tests/released.toml: a table of `\"MAJOR.MINOR\" = \"hash\"`");

    let fresh: BTreeMap<String, String> = signal_lib::composites()
        .iter()
        .map(|file| (file.name.to_owned(), pinned_hash(file.name)))
        .collect();
    let table: String = fresh
        .iter()
        .map(|(name, hash)| format!("{name} = \"{hash}\"\n"))
        .collect();
    assert_eq!(
        pinned, fresh,
        "a library composite's output changed, or one was added or removed. \
         An output change is a new name, or a MINOR change with a CHANGELOG.md entry \
         (CHANGELOG.md). Fresh table:\n{table}"
    );
    let errors = check_changelog(
        &crate_file("CHANGELOG.md"),
        signal_lib::VERSION,
        &pinned,
        &released,
    );
    assert_eq!(errors, Vec::<String>::new());
}

/// The changelog rule's failure modes, on made-up changelogs: a version 0.2
/// with 0.1 released.
mod changelog_rule {
    use super::*;

    const V01: &str =
        "- `lib.echo` (delay): `1111111111111111`\n- `lib.kick` (generator): `2222222222222222`";

    fn changelog(v01: &str, v02: &str) -> String {
        format!(
            "# Changelog\n\n## Versioning\n\n- `lib.<name>` rules.\n\n## 0.2\n\n{v02}\n\n## 0.1\n\n{v01}\n"
        )
    }

    fn released() -> BTreeMap<String, String> {
        BTreeMap::from([("0.1".to_owned(), hex(fnv1a64(V01.as_bytes())))])
    }

    fn pinned(echo: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("echo".to_owned(), echo.to_owned()),
            ("kick".to_owned(), "2222222222222222".to_owned()),
        ])
    }

    #[test]
    fn unchanged_rows_need_no_new_entry() {
        let text = changelog(V01, "Nothing changed.");
        assert_eq!(
            check_changelog(&text, "0.2", &pinned("1111111111111111"), &released()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_change_with_an_entry_under_the_current_version_passes() {
        let text = changelog(
            V01,
            "- `lib.echo` (delay), darker repeats: `3333333333333333`",
        );
        assert_eq!(
            check_changelog(&text, "0.2", &pinned("3333333333333333"), &released()),
            Vec::<String>::new()
        );
    }

    /// A changed row without an entry under `## 0.2` fails.
    #[test]
    fn a_changed_row_without_a_current_entry_fails() {
        let text = changelog(V01, "Nothing changed.");
        let errors = check_changelog(&text, "0.2", &pinned("3333333333333333"), &released());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("`lib.echo`") && errors[0].contains("## 0.2"),
            "{errors:?}"
        );
    }

    /// Editing the 0.1 line in place to the new hash fails: history.
    #[test]
    fn an_edited_earlier_section_fails() {
        let edited = V01.replace("1111111111111111", "3333333333333333");
        let text = changelog(&edited, "Nothing changed.");
        let errors = check_changelog(&text, "0.2", &pinned("3333333333333333"), &released());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("`## 0.1` changed"), "{errors:?}");
    }

    /// An earlier section not recorded as released, or a released one
    /// removed, fails; so does a missing current section.
    #[test]
    fn unrecorded_or_removed_history_fails() {
        let text = changelog(V01, "Nothing changed.");
        let errors = check_changelog(&text, "0.2", &pinned("1111111111111111"), &BTreeMap::new());
        assert!(
            errors.iter().any(|e| e.contains("record its hash")),
            "{errors:?}"
        );

        let only_current =
            "## 0.2\n\n- `lib.echo`: `1111111111111111`\n- `lib.kick`: `2222222222222222`\n";
        let errors = check_changelog(
            only_current,
            "0.2",
            &pinned("1111111111111111"),
            &released(),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("no longer in CHANGELOG.md")),
            "{errors:?}"
        );

        let errors = check_changelog(&text, "0.3", &pinned("1111111111111111"), &released());
        assert!(
            errors.iter().any(|e| e.contains("no `## 0.3` section")),
            "{errors:?}"
        );
    }
}

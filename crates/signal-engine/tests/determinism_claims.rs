//! M2.7-9 (the re-review's determinism qualification): every claim that the
//! output does not depend on how frames are split carries its one exception,
//! a panic in a `process` call of several frames (`docs/semantics.md` §14),
//! and AGENTS.md rule 3 states that exception precisely. Each claim is found
//! by a phrase of its own; its paragraph must name §14. A claim that is
//! reworded so that its phrase is gone fails here, rather than passing
//! unchecked.

use std::fs;
use std::path::Path;

/// (file, a phrase of the claim) for every partition-invariance claim about
/// the engine's output outside §13 and §14, which state the exception.
/// Claims about the thread count alone, and the module contract's
/// requirement that a primitive be partition-invariant, need none.
const CLAIMS: [(&str, &str); 9] = [
    (
        "docs/semantics.md",
        "Every later engine, including the real-time one, must produce the same results bit for bit",
    ),
    (
        "docs/semantics.md",
        "the engine runs them in blocks of *k* frames with the same result bit for bit",
    ),
    (
        "docs/semantics.md",
        "for any sequence of callback sizes (*k*-partition invariance, ADR 0002)",
    ),
    (
        "docs/testing-and-evaluation.md",
        "and requires the per-frame result bit for bit",
    ),
    (
        "docs/realtime-rules.md",
        "The output is bit-identical whatever the blocks",
    ),
    ("docs/semantics.md", "The block size changes nothing above"),
    (
        "docs/semantics.md",
        "on any machine with the same build), for every *k* and thread count",
    ),
    (
        "docs/roadmap.md",
        "output bit-identical across *k*, thread counts and callback sizes (goldens, V1–V11)",
    ),
    (
        "docs/00-vision.md",
        "block execution with a deterministic multi-core scheduler, bit-identical to the per-frame engine",
    ),
];

fn read(file: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join(file)).unwrap()
}

/// The paragraph (or list item, or table row) of `text` that holds
/// `phrase`.
fn paragraph<'a>(text: &'a str, phrase: &str) -> Option<&'a str> {
    text.split("\n\n")
        .flat_map(|p| p.split("\n- "))
        .flat_map(|p| p.split("\n|"))
        .find(|p| p.contains(phrase))
}

#[test]
fn every_partition_claim_names_the_exception() {
    let mut problems = Vec::new();
    for (file, phrase) in CLAIMS {
        match paragraph(&read(file), phrase) {
            None => problems.push(format!("{file}: the claim \"{phrase}\" is gone")),
            Some(p) if !p.contains("§14") => {
                problems.push(format!("{file}: \"{phrase}\" does not name §14"))
            }
            Some(_) => {}
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// AGENTS.md rule 3 says which call, from which frame, and that the thread
/// count is not part of the exception.
#[test]
fn agents_rule_3_states_the_exception_precisely() {
    let agents = read("AGENTS.md");
    let rule = agents
        .lines()
        .find(|l| l.starts_with("3. **Determinism.**"))
        .expect("AGENTS.md rule 3");
    for needed in [
        "`process` call of more than one frame",
        "first frame",
        "§14",
        "thread count",
    ] {
        assert!(rule.contains(needed), "rule 3 lacks \"{needed}\": {rule}");
    }
}

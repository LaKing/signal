//! ADR 0056 verification 4: `std.markov` steps by a chain pattern's weights.
//! A transport's ticks step a three-state chain with no self-moves; the
//! state, written to a discrete report slot, changes on every step, and the
//! moves out of each state follow its row within 1 % over about 10⁵ steps.
//! The same seed renders the same states, another seed others.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FRAMES: usize = 800_000;

fn project(dir: &Path) {
    fs::create_dir_all(dir.join("patterns")).unwrap();
    fs::write(
        dir.join("patterns/moves.toml"),
        "schema_version = 1\nkind = \"chain\"\nstates = [\"a\", \"b\", \"c\"]\n\
         [weights]\na = [0, 1, 3]\nb = [1, 0, 1]\nc = [2, 2, 0]\n",
    )
    .unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"chain\"\nsample_rate = 48000\nseed = 5\n\
         [instances]\nclock = { module = \"std.transport\", bpm = 400, ppq = 960 }\n\
         chain = { module = \"std.markov\", pattern = \"moves\" }\n\
         out = { module = \"std.audio_out\", gain = 0 }\n\
         report = { device = { play = [\"state\"], role = \"report\" } }\n\
         [wiring]\nconnections = [\n  { from = \"clock.tick\", to = \"chain.step\" },\n  { from = \"chain.state\", to = \"report.state\" },\n]\n",
    )
    .unwrap();
}

fn states(dir: &Path, out: &str, seed: &str) -> Vec<f32> {
    let out = dir.join(out);
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["render", dir.to_str().unwrap(), "--frames"])
        .arg(FRAMES.to_string())
        .args(["--seed", seed, "--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    hound::WavReader::open(out.join("devices/report.wav"))
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_chain_moves_by_its_weights_and_its_seed() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("markov-chain");
    let _ = fs::remove_dir_all(&dir);
    project(&dir);
    let run = states(&dir, "a", "5");
    // Every step moves (no self-weights): count the moves out of each state.
    let mut moves = [[0usize; 3]; 3];
    for pair in run.windows(2) {
        let (from, to) = (pair[0] as usize, pair[1] as usize);
        if from != to {
            moves[from][to] += 1;
        }
    }
    let total: usize = moves.iter().flatten().sum();
    assert!(total > 90_000, "{total} steps");
    let weights = [[0.0, 1.0, 3.0], [1.0, 0.0, 1.0], [2.0, 2.0, 0.0]];
    for (from, row) in moves.iter().enumerate() {
        let out: usize = row.iter().sum();
        let sum: f64 = weights[from].iter().sum();
        for (to, &count) in row.iter().enumerate() {
            let expected = weights[from][to] / sum;
            let seen = count as f64 / out as f64;
            assert!(
                (seen - expected).abs() < 0.01,
                "{from}→{to}: {seen} against {expected}"
            );
        }
    }
    assert_eq!(
        run,
        states(&dir, "again", "5"),
        "the same seed, the same states"
    );
    assert_ne!(run, states(&dir, "other", "6"), "another seed, others");
}

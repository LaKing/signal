//! External review (Codex), item 2: a run that records no session keeps
//! no history. A long loop of live edits and file reloads (sets and swaps)
//! with recording off leaves the run's recorder holding a bounded number of
//! entries (no version's files or plan, no command, no boundary record);
//! with recording on, the same loop is kept whole, and the session it
//! writes replays to the audio the run played.

mod common;

use std::fs;

use common::{Options, Rig, receive, rig, send};
use serde_json::json;
use signal_core::Registry;
use signal_engine::ControlSink;
use signal_project::session::{Session, replay};
use signal_ui::Finished;

const ROUNDS: usize = 24;

/// The tone project with `c` added (a swap) or not, and `a` at `freq`.
fn text(freq: u32, with_c: bool) -> String {
    let mut text = common::TONE.replace("freq = 440", &format!("freq = {freq}"));
    if with_c {
        text = text.replace(
            "out = { module",
            "c = { module = \"std.sine_osc\", freq = 110 }\nout = { module",
        );
    }
    text
}

/// Plays `ROUNDS` rounds of live sets and file edits (every other one a
/// swap), each waited for; the run's end and the audio it played.
fn play(rig: &mut Rig) -> (Finished, Vec<f32>) {
    let (mut ws, _) = rig.client();
    let mut audio = Vec::new();
    let path = rig.dir.join("project.toml");
    let mut id = 1;
    for round in 0..ROUNDS {
        for step in 0..4 {
            // Low enough that nothing clips (the device clamps; the replay does not).
            let value = 0.1 + 0.002 * (round * 4 + step) as f64;
            send(
                &mut ws,
                json!({"type": "set", "address": "/out.gain", "value": value, "id": id}),
            );
            id += 1;
        }
        fs::write(&path, text(300 + round as u32, round % 2 == 0)).unwrap();
        // Until the reload of this edit has been pushed.
        let mut reloaded = false;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !reloaded {
            assert!(
                std::time::Instant::now() < until,
                "no reload in round {round}"
            );
            let host = rig.host();
            host.run_null(256, |samples| audio.extend_from_slice(samples))
                .unwrap();
            host.step();
            while let Some(got) = receive(&mut ws) {
                if let Some(reload) = got.of("reload") {
                    assert_ne!(reload["sets"], 0, "{reload}");
                    reloaded = true;
                }
                assert!(got.of("reload.error").is_none(), "{:?}", got.json());
            }
        }
    }
    // Every report past its frame.
    for _ in 0..16 {
        let host = rig.host();
        host.run_null(256, |samples| audio.extend_from_slice(samples))
            .unwrap();
        host.step();
    }
    let finished = rig.host.take().unwrap().finish().unwrap();
    (finished, audio)
}

#[test]
fn a_run_that_records_nothing_keeps_no_history() {
    let mut rig = rig("history-off", Options::default());
    let (finished, _) = play(&mut rig);
    let mut live = finished.live;
    live.status(&finished.status);
    live.finish(finished.status.next_frame).unwrap();
    assert!(!live.recorder().keeps_history());
    // Nothing waits; nothing of the run's history is held.
    assert_eq!(live.retained(), 0, "{} entries held", live.retained());
    assert!(live.recorder().edits().is_empty());
    // Plan versions stay monotonic without the plans: one swap a round.
    assert_eq!(live.recorder().next_version(), 1 + ROUNDS as u64);
    let refused = live.into_recorder().write(&rig.dir.join("session"));
    assert!(refused.is_err(), "a run without history writes no session");
}

#[test]
fn a_recorded_run_keeps_its_history_and_replays() {
    let mut rig = rig(
        "history-on",
        Options {
            recording: true,
            ..Options::default()
        },
    );
    let (finished, audio) = play(&mut rig);
    let mut live = finished.live;
    live.status(&finished.status);
    live.finish(finished.status.next_frame).unwrap();
    assert!(live.recorder().keeps_history());
    assert!(
        live.retained() > ROUNDS * 5,
        "every version and command kept: {}",
        live.retained()
    );
    assert_eq!(live.recorder().next_version(), 1 + ROUNDS as u64);
    let dir = rig.dir.parent().unwrap().join("session");
    let _ = fs::remove_dir_all(&dir);
    live.into_recorder().write(&dir).unwrap();
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(session.versions.len(), 1 + ROUNDS);
    let frames = finished.status.next_frame.0;
    let replayed = replay(&session, &registry, Some(frames), 1).unwrap();
    let render = replayed.render;
    assert_eq!(render.frames, frames);
    // The frames the client's hello was stepped through were not captured:
    // the capture is the end of the run. The device took f32 samples.
    let replayed: Vec<f32> = render.samples.iter().map(|s| *s as f32).collect();
    let skipped = replayed.len() - audio.len();
    assert!(skipped <= 4 * 256 * 2, "{skipped} samples not captured");
    assert!(
        audio[..] == replayed[skipped..],
        "the replay is the audio the run played"
    );
}

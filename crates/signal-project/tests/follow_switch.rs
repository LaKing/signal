//! ADR 0037 test 6 (and ADR 0035 test 6): a deck's playhead following a
//! host, switched by swaps at every phase of a beat. A `std.step_seq8` with
//! a hit on every 16th follows the deck's position, its `reset` on the
//! deck's `jump`; a `std.counter` counts its hits. Wiring the deck to a
//! host, rewiring it to another host 0.1 beats ahead (`sources_changed` and
//! the playhead's switch `jump`), and letting go of it (`release` 0 and 1):
//! nothing fires retroactively, nothing twice, and every hit lands on the
//! frame the follow rule gives for the position the deck plays (the next one
//! on time). With `release` 0 the position goes on without a step; with 1 it
//! steps to the deck's own clock, an unfollowed playhead's position.

use signal_core::{EventPayload, Frame, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent};
use signal_graph::ExecutionPlan;
use signal_project::{Project, SourceFile};

const RATE: u32 = 48_000;
/// One beat at 120 bpm.
const BEAT: u64 = 24_000;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The deck following `follow` (`None`: nothing), with `release`.
fn plan(follow: Option<&str>, release: f64, version: u64) -> ExecutionPlan {
    let wires = follow.map_or(String::new(), |host| {
        format!(
            "  {{ from = \"{host}.position\", to = \"deck.follow\" }},\n  {{ from = \"{host}.jump\", to = \"deck.follow_jump\" }},\n"
        )
    });
    let text = format!(
        "schema_version = 1\n[project]\nname = \"switch\"\nsample_rate = {RATE}\nseed = 1\n[instances]\n\
         h1 = {{ module = \"std.playhead\", bpm = 120 }}\n\
         h2 = {{ module = \"std.playhead\", bpm = 120 }}\n\
         deck = {{ module = \"std.playhead\", bpm = 120, release = {release} }}\n\
         drums = {{ module = \"std.step_seq8\", pattern = \"beat\" }}\n\
         hits = {{ module = \"std.counter\" }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n{wires}\
           {{ from = \"deck.position\", to = \"drums.follow\" }},\n\
           {{ from = \"deck.jump\", to = \"drums.reset\" }},\n\
           {{ from = \"drums.t1\", to = \"hits.trigger\" }},\n\
           {{ from = \"hits.count\", to = \"out.l\", scale = 0.0001 }},\n]\n"
    );
    let beat = "schema_version = 1\nkind = \"steps\"\nsteps = 16\nstep = 0.25\n[lanes]\nkick = \"xxxxxxxxxxxxxxxx\"\n";
    let project = Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text,
        },
        SourceFile {
            path: "patterns/beat.toml".to_owned(),
            text: beat.to_owned(),
        },
    ])
    .unwrap();
    let mut plan = project.plan(&registry()).unwrap().plan;
    plan.version = version;
    plan
}

/// Per frame: the deck's position, the hits so far, and the own clock of
/// an unfollowed playhead (`/h1`'s, equal to the deck's own).
struct Trace {
    position: Vec<f64>,
    hits: Vec<f64>,
    own: Vec<f64>,
}

/// Runs `frames` frames from `first`, swapping to each `(frame, plan)`;
/// `/h2` is seeked 0.1 beats ahead on frame 0, in blocks of `k`.
fn run(first: &ExecutionPlan, swaps: &[(u64, ExecutionPlan)], frames: u64, k: usize) -> Trace {
    let registry = registry();
    let events = vec![ExternalEvent {
        frame: Frame(0),
        sequence: 1,
        target: "/h2.seek".parse().unwrap(),
        payload: EventPayload::Set(0.1),
    }];
    let mut engine = Engine::new(first, &registry, EngineConfig::new(RATE, 1), events).unwrap();
    let mut trace = Trace {
        position: Vec::new(),
        hits: Vec::new(),
        own: Vec::new(),
    };
    while engine.next_frame().0 < frames {
        let now = engine.next_frame().0;
        if let Some((_, next)) = swaps.iter().find(|(at, _)| *at == now) {
            engine.drain_log(&mut |_| {});
            let mut prepared = engine.prepare_swap(next, &registry).unwrap();
            engine.commit_swap(&mut prepared, Frame(now)).unwrap();
        }
        let until = swaps
            .iter()
            .map(|(at, _)| *at)
            .filter(|&at| at > now)
            .min()
            .unwrap_or(frames);
        let len = k.min(usize::try_from(until - now).unwrap());
        assert_eq!(engine.run_block(len), len, "{:?}", engine.failure());
        let index = |name: &str| engine.signal_output_index(&name.parse().unwrap()).unwrap();
        let (p, h, o) = (
            index("/deck.position"),
            index("/hits.count"),
            index("/h1.position"),
        );
        for f in 0..len {
            trace.position.push(engine.signal_output_at(p, f).unwrap());
            trace.hits.push(engine.signal_output_at(h, f).unwrap());
            trace.own.push(engine.signal_output_at(o, f).unwrap());
        }
        engine.drain_log(&mut |_| {});
    }
    trace
}

/// The hits the follow rule gives on frame `n` (not a seek) for the
/// positions played: every 16th `b` with `p(n − 1) ≤ b < p(n)` (4·x is
/// exact, so this counts exactly).
fn rule(position: &[f64], n: usize) -> f64 {
    let (from, to) = (position[n - 1], position[n]);
    if to <= from {
        return 0.0;
    }
    (4.0 * to).ceil() - (4.0 * from).ceil()
}

/// Every frame's hits equal the rule's, but on the seek frames, where
/// nothing fires; no frame has more than one.
fn check(trace: &Trace, seeks: &[u64], what: &str) {
    for n in 1..trace.hits.len() {
        let fired = trace.hits[n] - trace.hits[n - 1];
        let expected = if seeks.contains(&(n as u64)) {
            0.0
        } else {
            rule(&trace.position, n)
        };
        assert_eq!(fired, expected, "{what}: frame {n}");
        assert!(fired <= 1.0, "{what}: frame {n} fired {fired}");
    }
}

#[test]
fn rewiring_at_every_phase_of_a_beat_fires_nothing_retroactively() {
    let on_h1 = plan(Some("h1"), 0.0, 0);
    let on_h2 = plan(Some("h2"), 0.0, 1);
    let phases = [0, 1, 2, 2_999, 5_999, 6_000, 6_001, 11_999, 17_000, 23_999];
    for phase in phases {
        let at = 2 * BEAT + phase;
        for k in [1, 64, 256] {
            if k != 64 && phase % 6_000 > 2 {
                continue;
            }
            let trace = run(&on_h1, &[(at, on_h2.clone())], at + BEAT, k);
            let what = format!("rewired at phase {phase}, k {k}");
            // A copy of h1, then of h2 (0.1 beats ahead) from the swap.
            assert_eq!(trace.position[at as usize - 1], trace.own[at as usize - 1]);
            assert_eq!(
                trace.position[at as usize],
                0.1 + (at as f64 * 120.0) / 2_880_000.0,
                "{what}"
            );
            // The rewire is a seek (the switch jump): nothing on its frame,
            // and the first frame (0) one too.
            check(&trace, &[0, at], &what);
            // The 16th crossed by the jump does not fire late; the next one
            // fires on time.
            let total: f64 = trace.hits[trace.hits.len() - 1];
            assert!(total >= 8.0, "{what}: {total}");
        }
    }
}

#[test]
fn letting_go_continues_or_returns_as_release_says() {
    for release in [0.0, 1.0] {
        let following = plan(Some("h2"), release, 0);
        let free = plan(None, release, 1);
        for phase in [0, 1, 3_000, 5_999, 6_000, 6_001, 17_000] {
            let at = 2 * BEAT + phase;
            let trace = run(&following, &[(at, free.clone())], at + BEAT, 64);
            let what = format!("release {release} at phase {phase}");
            let at = at as usize;
            if release == 0.0 {
                // Continue: no step, then the deck's own speed (the same
                // 120 bpm here), so the next hit is on time with no seek
                // but the switch jump on the frame itself, which moves
                // nothing.
                assert_eq!(trace.position[at], trace.position[at - 1], "{what}");
                assert_eq!(
                    trace.position[at + 1],
                    trace.position[at - 1] + 120.0 / 2_880_000.0,
                    "{what}"
                );
            } else {
                // Return: the own clock's position, which ran underneath
                // from frame 0: an unfollowed playhead's (h1's).
                assert_eq!(trace.position[at], trace.own[at], "{what}");
                assert_eq!(trace.position[at + 100], trace.own[at + 100], "{what}");
            }
            check(&trace, &[0, at as u64], &what);
        }
    }
}

/// Connecting a follower mid-run (a host wired into the deck) is a switch
/// too: on the swap frame nothing fires, and it plays the host's beats from
/// the next frame.
#[test]
fn wiring_a_host_in_is_a_seek() {
    let free = plan(None, 0.0, 0);
    let following = plan(Some("h2"), 0.0, 1);
    for phase in [0, 1, 6_000, 6_001, 13_000] {
        let at = BEAT + phase;
        let trace = run(&free, &[(at, following.clone())], at + BEAT, 64);
        let at = at as usize;
        assert_eq!(trace.position[at], 0.1 + (at as f64 * 120.0) / 2_880_000.0);
        check(&trace, &[0, at as u64], &format!("wired at phase {phase}"));
    }
}

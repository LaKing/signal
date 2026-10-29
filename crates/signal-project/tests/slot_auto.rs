//! ADR 0045, phase 1 (offline): slot ports typed by their connections.
//!
//! - Resolution (test 1): an input from its host source, else its content,
//!   else the default; an output from its content, else its host
//!   consumers; fan-in by the first `from` string; an empty output's
//!   fan-out; `map`; an own inside before and after a frame wire; nested
//!   content; unit and range copied; a parameter and a partly pinned open
//!   port are no known end; `{}` under `parameters`.
//! - Cycles and order (test 2): two empty slots wired to each other, the
//!   triangle and the chain of §3, a slot wired to itself, permuted entries.
//! - Conflicts and checks (test 3): errors at the wire, naming the slot's
//!   port and the content's (the `locate.rs` mapping); rates that divide
//!   plan with a converter, others and events refused; pinned fields
//!   checked; `auto = false`, `auto` on a parameter, in a library template,
//!   a `voice` field.
//! - Equivalence (test 4): auto ports plan byte-identically to the pinned
//!   and the explicit forms; `{}` under today's wiring plans as the
//!   generic signal it meant; a project template's `{}` is auto in every
//!   slot from it.
//!
//! Projects are read from memory; the one that loads a project is written
//! into a fresh folder under the target's temporary directory.

use std::fs;
use std::path::PathBuf;

use signal_core::{ModuleType, ParameterDescriptor, PortDescriptor, PortShape, RateDivisor};
use signal_core::{PortKind, Registry};
use signal_graph::ExecutionPlan;
use signal_project::{AutoEnd, Field, InterfacePort, Planned, Project, Role, SourceFile};

/// The std library and three retimed copies: `test.slow` (`std.const` at
/// rate divisor 64), `test.slow48` (`std.gain` at 48) and `test.slow_edge`
/// (`std.edge` at 2, so its events run at 2).
fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    for (base, name, rate) in [
        ("std.const", "test.slow", 64),
        ("std.gain", "test.slow48", 48),
        ("std.edge", "test.slow_edge", 2),
    ] {
        let module = *registry.get(base).unwrap();
        let mut descriptor = module.descriptor;
        let rate = RateDivisor::new(rate).unwrap();
        descriptor.type_name = name;
        descriptor.ports = Box::leak(
            descriptor
                .ports
                .iter()
                .map(|p| PortDescriptor { rate, ..*p })
                .collect(),
        );
        descriptor.parameters = Box::leak(
            descriptor
                .parameters
                .iter()
                .map(|p| ParameterDescriptor { rate, ..*p })
                .collect(),
        );
        registry
            .register(ModuleType {
                descriptor,
                create: module.create,
            })
            .unwrap();
    }
    registry
}

/// A project of `instances` (lines of `[instances]`) and `wires` (`from`,
/// `to`), one wire per line.
fn host(instances: &str, wires: &[(&str, &str)]) -> String {
    let wires: String = wires
        .iter()
        .map(|(from, to)| format!("  {{ from = \"{from}\", to = \"{to}\" }},\n"))
        .collect();
    format!(
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\n{instances}\n[wiring]\nconnections = [\n{wires}]\n"
    )
}

fn sources(text: &str, extra: &[(&str, &str)]) -> Vec<SourceFile> {
    std::iter::once(("project.toml", text))
        .chain(extra.iter().copied())
        .map(|(path, text)| SourceFile {
            path: path.to_owned(),
            text: text.to_owned(),
        })
        .collect()
}

/// What loading and planning gives: the plan, or the diagnostics as text.
fn try_plan(text: &str, extra: &[(&str, &str)]) -> Result<Planned, Vec<String>> {
    let text_of = |found: Vec<signal_project::Diagnostic>| {
        found.iter().map(ToString::to_string).collect::<Vec<_>>()
    };
    let project = Project::from_sources(sources(text, extra)).map_err(text_of)?;
    project.plan(&registry()).map_err(text_of)
}

fn planned(text: &str, extra: &[(&str, &str)]) -> Planned {
    try_plan(text, extra).unwrap_or_else(|e| panic!("{text}\n{e:#?}"))
}

fn errors(text: &str, extra: &[(&str, &str)]) -> Vec<String> {
    match try_plan(text, extra) {
        Ok(_) => panic!("plans:\n{text}"),
        Err(found) => found,
    }
}

/// The plan as `signal render --plan-json` would dump it.
fn dump(plan: &ExecutionPlan) -> String {
    serde_json::to_string_pretty(plan).unwrap()
}

/// The resolved port `slot.port` (`slot` as `SlotResolution::slot`).
fn port<'p>(planned: &'p Planned, slot: &str, name: &str) -> &'p InterfacePort {
    planned
        .slots
        .iter()
        .find(|s| s.slot == slot)
        .unwrap_or_else(|| panic!("no slot {slot}"))
        .interface
        .port(name)
        .unwrap_or_else(|| panic!("{slot} has no {name}"))
}

/// Where an auto port's type comes from.
fn from(port: &InterfacePort) -> (AutoEnd, Option<&str>) {
    let auto = port.auto.as_ref().expect("an auto port");
    (auto.from.end, auto.from.port.as_deref())
}

/// The 1-based line of `text` that holds `needle`.
fn line(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no `{needle}`"))
        + 1
}

fn has(found: &[String], parts: &[&str]) -> bool {
    found
        .iter()
        .any(|p| parts.iter().all(|part| p.contains(part)))
}

/// A composite `thru`: `x` into a `std.gain`, its output `out`.
const THRU: &str = r#"schema_version = 1
[composite]
name = "thru"
[instances]
g = { module = "std.gain" }
[exports]
inputs = { x = "g.in" }
outputs = { out = "g.out" }
"#;

/// A composite `ticks`: its output `out` is a `std.edge`'s events, its
/// input `x` the edge's signal input.
const TICKS: &str = r#"schema_version = 1
[composite]
name = "ticks"
[instances]
e = { module = "std.edge" }
[exports]
inputs = { x = "e.in" }
outputs = { out = "e.rise" }
"#;

const OUT: &str = "out = { module = \"std.audio_out\" }";

/// Test 1: an input is typed by its host source, else by the content's
/// port, else the default; an output by the content, else by its host
/// consumer; every field not pinned is inferred, and the port says so.
#[test]
fn inputs_and_outputs_take_their_defining_end() {
    // The host source wins over the content for an input; the content
    // over the host consumer for an output.
    let text = host(
        &format!(
            "c = {{ module = \"std.const\" }}\nfx = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = {{}} }} }}, load = \"std.gain\" }}\n{OUT}"
        ),
        &[("c.out", "fx.in"), ("fx.out", "out.l")],
    );
    let p = planned(&text, &[]);
    let input = port(&p, "/fx", "in");
    assert_eq!(from(input), (AutoEnd::Host, Some("/c.out")));
    assert_eq!(
        input.kind,
        PortKind::Generic,
        "the const's kind, not gain's"
    );
    assert_eq!(
        input.auto.as_ref().unwrap().inferred,
        [
            Field::Shape,
            Field::Rate,
            Field::Kind,
            Field::Unit,
            Field::Range
        ]
    );
    let output = port(&p, "/fx", "out");
    assert_eq!(from(output), (AutoEnd::Content, Some("out")));
    assert_eq!(output.kind, PortKind::Audio);

    // No host source: the content's port.
    let text = host(
        &format!("fx = {{ slot = {{ inputs = {{ in = {{}} }} }}, load = \"std.gain\" }}\n{OUT}"),
        &[],
    );
    let p = planned(&text, &[]);
    let input = port(&p, "/fx", "in");
    assert_eq!(from(input), (AutoEnd::Content, Some("in")));
    assert_eq!(input.kind, PortKind::Audio);

    // Empty: the output from its consumer; nothing at all: the default.
    let text = host(
        &format!(
            "fx = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = {{}} }} }} }}\n{OUT}"
        ),
        &[("fx.out", "out.l")],
    );
    let p = planned(&text, &[]);
    let output = port(&p, "/fx", "out");
    assert_eq!(from(output), (AutoEnd::Host, Some("/out.l")));
    assert_eq!(output.kind, PortKind::Audio);
    let input = port(&p, "/fx", "in");
    assert_eq!(from(input), (AutoEnd::Default, None));
    assert_eq!(
        (
            input.shape,
            input.rate,
            input.kind,
            &input.unit,
            input.range
        ),
        (PortShape::Signal, 1, PortKind::Generic, &None, None)
    );
}

/// Test 1: fan-in at several rates: the first `from` string (`str::cmp`)
/// of known type defines the input; two wires from one source are one end.
#[test]
fn fan_in_is_defined_by_the_first_source_as_written() {
    for (slow, fast, rate) in [("a", "b", 64), ("b", "a", 1)] {
        let text = host(
            &format!(
                "{slow} = {{ module = \"test.slow\" }}\n{fast} = {{ module = \"std.const\" }}\nfx = {{ slot = {{ inputs = {{ in = {{}} }} }} }}"
            ),
            &[
                (&format!("{fast}.out"), "fx.in"),
                (&format!("{slow}.out"), "fx.in"),
                (&format!("{slow}.out"), "fx.in"),
            ],
        );
        let p = planned(&text, &[]);
        let input = port(&p, "/fx", "in");
        assert_eq!(input.rate, rate, "{text}");
        assert_eq!(from(input), (AutoEnd::Host, Some("/a.out")));
    }
}

/// Test 1: an empty slot's output feeding several consumers takes the
/// first one's type; the others agree.
#[test]
fn an_empty_output_fans_out() {
    let text = host(
        "fx = { slot = { outputs = { out = {} } } }\nb = { module = \"std.counter\" }\na = { module = \"std.clock_div\" }",
        &[("fx.out", "b.trigger"), ("fx.out", "a.in")],
    );
    let p = planned(&text, &[]);
    let output = port(&p, "/fx", "out");
    assert_eq!(output.shape, PortShape::Events);
    assert_eq!(from(output), (AutoEnd::Host, Some("/a.in")));
}

/// Test 1: `map` renames the content end, `false` leaves none, and two
/// slot outputs mapped to one content output both take its type.
#[test]
fn map_names_the_content_end() {
    let text = host(
        &format!(
            "fx = {{ slot = {{ inputs = {{ x = {{}}, side = {{}} }}, outputs = {{ l = {{}}, r = {{}} }} }}, load = \"std.sine_osc\", map = {{ x = \"pm\", side = false, l = \"out\", r = \"out\" }} }}\n{OUT}"
        ),
        &[("fx.l", "out.l"), ("fx.r", "out.r")],
    );
    let p = planned(&text, &[]);
    let x = port(&p, "/fx", "x");
    assert_eq!(from(x), (AutoEnd::Content, Some("pm")));
    assert_eq!(
        (x.kind, x.unit.as_deref()),
        (PortKind::Phase, Some("cycles"))
    );
    assert_eq!(from(port(&p, "/fx", "side")), (AutoEnd::Default, None));
    for channel in ["l", "r"] {
        let p = port(&p, "/fx", channel);
        assert_eq!(from(p), (AutoEnd::Content, Some("out")));
        assert_eq!((p.kind, p.range), (PortKind::Audio, Some((-1.0, 1.0))));
    }
}

/// Test 1: an own inside (ADR 0037, editable insides): its input typed by
/// the host's wire even while nothing inside is wired to the frame; its
/// output by the host's consumer until the inside wires it, then by the
/// inner source.
#[test]
fn an_own_inside_is_typed_from_outside_then_from_inside() {
    let text = host(
        &format!(
            "e = {{ module = \"std.edge\" }}\nfx = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = {{}} }} }}, load = \"fx_inside\" }}\nc = {{ module = \"std.counter\" }}\n{OUT}"
        ),
        &[("e.rise", "fx.in"), ("fx.out", "out.l")],
    );
    let inside = |exports: &str| {
        format!(
            "schema_version = 1\n[composite]\nname = \"fx_inside\"\ninside_of = \"fx\"\n[instances]\nn = {{ module = \"std.counter\" }}\n[exports]\n{exports}"
        )
    };
    // Before a frame wire: nothing inside names the ports.
    let before = inside("");
    let p = planned(&text, &[("composites/fx_inside.toml", &before)]);
    let input = port(&p, "/fx", "in");
    assert_eq!(
        (input.shape, from(input)),
        (PortShape::Events, (AutoEnd::Host, Some("/e.rise")))
    );
    let output = port(&p, "/fx", "out");
    assert_eq!(from(output), (AutoEnd::Host, Some("/out.l")));
    // After: the counter's output wired to the frame's `out`.
    let after = inside("inputs = { in = \"n.trigger\" }\noutputs = { out = \"n.count\" }\n");
    let p = planned(&text, &[("composites/fx_inside.toml", &after)]);
    let output = port(&p, "/fx", "out");
    assert_eq!(from(output), (AutoEnd::Content, Some("out")));
    assert_eq!(output.range, Some((0.0, 999_999.0)), "the counter's range");
    assert_eq!(
        from(port(&p, "/fx", "in")),
        (AutoEnd::Host, Some("/e.rise"))
    );
}

/// Test 1: nested content: a composite's own slot is resolved at its level
/// first (its host wire qualified by the level), and its exports are then
/// known types for the host's slot.
#[test]
fn nested_content_resolves_level_by_level() {
    let rack = r#"schema_version = 1
[composite]
name = "rack"
[instances]
c = { module = "std.lfo" }
inner = { slot = { inputs = { in = {} }, outputs = { out = {} } }, load = "std.gain" }
[wiring]
connections = [ { from = "c.out", to = "inner.in" } ]
[exports]
outputs = { out = "inner.out" }
"#;
    let text = host(
        &format!("fx = {{ slot = {{ outputs = {{ out = {{}} }} }}, load = \"rack\" }}\n{OUT}"),
        &[("fx.out", "out.l")],
    );
    let p = planned(&text, &[("composites/rack.toml", rack)]);
    let inner = port(&p, "rack/inner", "in");
    assert_eq!(from(inner), (AutoEnd::Host, Some("rack/c.out")));
    assert_eq!(
        (inner.kind, inner.range),
        (PortKind::Control, Some((-1.0, 1.0)))
    );
    let outer = port(&p, "/fx", "out");
    assert_eq!(from(outer), (AutoEnd::Content, Some("out")));
    assert_eq!(outer.kind, PortKind::Audio, "gain's, through two levels");
}

/// Test 1: unit and range are copied from a primitive's port and through
/// an export; an exported parameter's own range (ADR 0042) is what
/// `ports_of` gives it; a project's tap channels carry neither.
#[test]
fn unit_and_range_are_copied() {
    let text = host(
        &format!(
            "ph = {{ module = \"std.phasor\" }}\nfx = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = {{}} }} }}, load = \"osc_box\" }}\n{OUT}"
        ),
        &[("ph.phase", "fx.in"), ("fx.out", "out.l")],
    );
    let osc_box = r#"schema_version = 1
[composite]
name = "osc_box"
[instances]
osc = { module = "std.sine_osc" }
[exports]
inputs = { in = "osc.pm" }
outputs = { out = "osc.out" }
parameters = { freq = { to = "osc.freq", min = 20, max = 2000 } }
"#;
    let p = planned(&text, &[("composites/osc_box.toml", osc_box)]);
    let input = port(&p, "/fx", "in");
    assert_eq!(
        (input.unit.as_deref(), input.kind),
        (Some("cycles"), PortKind::Phase),
        "the phasor's port"
    );
    let output = port(&p, "/fx", "out");
    assert_eq!(
        output.range,
        Some((-1.0, 1.0)),
        "the oscillator's, through the export"
    );

    let def = signal_graph::CompositeDef {
        name: "osc_box".to_owned(),
        instances: vec![signal_graph::InstanceDef {
            name: "osc".to_owned(),
            module: "std.sine_osc".to_owned(),
            ..Default::default()
        }],
        exports: signal_graph::Exports {
            outputs: vec![signal_graph::OutputExport {
                name: "out".to_owned(),
                from: "osc.out".to_owned(),
            }],
            parameters: vec![
                signal_graph::ParameterExport {
                    name: "freq".to_owned(),
                    to: vec!["osc.freq".to_owned()],
                    range: Some(signal_core::Range {
                        min: 20.0,
                        max: 2000.0,
                    }),
                    ..Default::default()
                },
                signal_graph::ParameterExport {
                    name: "hz".to_owned(),
                    to: vec!["osc.freq".to_owned()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let ports = signal_graph::ports_of("osc_box", &[def], &registry()).unwrap();
    let get = |name: &str| ports.iter().find(|p| p.name == name).unwrap();
    let range = |name: &str| get(name).range.map(|r| (r.min, r.max));
    assert_eq!(range("freq"), Some((20.0, 2000.0)), "its own range");
    let osc = registry();
    let freq = osc
        .get("std.sine_osc")
        .unwrap()
        .descriptor
        .parameters
        .iter()
        .find(|p| p.name == "freq")
        .unwrap();
    assert_eq!(
        range("hz"),
        Some((freq.range.min, freq.range.max)),
        "its target's"
    );
    assert_eq!(get("hz").unit.as_deref(), freq.unit);
    assert_eq!(range("out"), Some((-1.0, 1.0)));

    // A project's tap channels carry no unit and no range.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("slot-auto-taps");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("host")).unwrap();
    fs::create_dir_all(dir.join("deck")).unwrap();
    fs::write(
        dir.join("deck/project.toml"),
        "schema_version = 1\n[project]\nname = \"deck\"\n[instances]\nosc = { module = \"std.sine_osc\" }\nout = { module = \"std.audio_out\" }\n[wiring]\nconnections = [ { from = \"osc.out\", to = \"out.l\" } ]\n",
    )
    .unwrap();
    fs::write(
        dir.join("host/project.toml"),
        host(
            &format!(
                "deck = {{ slot = {{ outputs = {{ l = {{}} }} }}, load = \"../deck\" }}\n{OUT}"
            ),
            &[("deck.l", "out.l")],
        ),
    )
    .unwrap();
    let p = Project::load(&dir.join("host"))
        .unwrap()
        .plan(&registry())
        .unwrap();
    let l = port(&p, "/deck", "l");
    assert_eq!(from(l), (AutoEnd::Content, Some("l")));
    assert_eq!((&l.unit, l.range), (&None, None));
}

/// Test 1: a parameter as a consumer is no known end, nor is an open auto
/// port with a field pinned; `{}` under `parameters` is a parameter with
/// no unit and no group.
#[test]
fn parameters_and_open_ports_are_no_known_end() {
    let text = host(
        "fx = { slot = { outputs = { out = {} } } }\nosc = { module = \"std.sine_osc\" }",
        &[("fx.out", "osc.freq")],
    );
    let p = planned(&text, &[]);
    assert_eq!(from(port(&p, "/fx", "out")), (AutoEnd::Default, None));

    let text = host(
        "a = { slot = { outputs = { out = { auto = true, kind = \"gate\" } } } }\nb = { slot = { inputs = { in = {} } } }",
        &[("a.out", "b.in")],
    );
    let p = planned(&text, &[]);
    let a = port(&p, "/a", "out");
    assert_eq!(from(a), (AutoEnd::Default, None));
    assert_eq!(a.kind, PortKind::Gate, "pinned");
    assert!(a.pinned(Field::Kind) && !a.pinned(Field::Shape));
    let b = port(&p, "/b", "in");
    assert_eq!(from(b), (AutoEnd::Default, None));
    assert_eq!(b.kind, PortKind::Generic, "not a.out's pinned gate");

    let text = host(
        "fx = { slot = { parameters = { volume = {} } }, load = \"std.gain\", map = { volume = \"db\" } }",
        &[],
    );
    let p = planned(&text, &[]);
    let volume = port(&p, "/fx", "volume");
    assert_eq!(volume.role, Role::Parameter);
    assert_eq!(
        (&volume.unit, &volume.group, &volume.auto),
        (&None, &None, &None)
    );
}

/// Test 2: two empty auto slots wired to each other both default, neither
/// a known end for the other; a slot wired to itself likewise; loaded, its
/// output is the content's and its input that output's.
#[test]
fn ports_typed_only_by_each_other_default() {
    let text = host(
        "a = { slot = { inputs = { in = {} }, outputs = { out = {} } } }\nb = { slot = { inputs = { in = {} }, outputs = { out = {} } } }",
        &[("a.out", "b.in"), ("b.out", "a.in")],
    );
    let p = planned(&text, &[]);
    for (slot, name) in [("/a", "in"), ("/a", "out"), ("/b", "in"), ("/b", "out")] {
        assert_eq!(from(port(&p, slot, name)), (AutoEnd::Default, None));
    }
    let text = host(
        "fx = { slot = { inputs = { in = {} }, outputs = { out = {} } } }",
        &[("fx.out", "fx.in")],
    );
    let p = planned(&text, &[]);
    assert_eq!(from(port(&p, "/fx", "in")), (AutoEnd::Default, None));
    assert_eq!(from(port(&p, "/fx", "out")), (AutoEnd::Default, None));
    let text = host(
        "fx = { slot = { inputs = { x = {} }, outputs = { out = {} } }, load = \"ticks\" }",
        &[("fx.out", "fx.x")],
    );
    let found = errors(&text, &[("composites/ticks.toml", TICKS)]);
    // The content's events reach its own signal input: the graph's error,
    // at the wire, through the slot.
    assert!(
        has(
            &found,
            &[
                &format!("project.toml:{}:", line(&text, "fx.out")),
                "/fx.x (auto, from /fx.out) is events; it reaches ticks's x, a signal"
            ]
        ),
        "{found:#?}"
    );
}

/// Test 2, §3: the triangle (`a.out → b.in`, `a.out → seq.reset`): stage
/// 4 types `a.out` events from `seq.reset`, stage 5 types `b.in` from it,
/// and the project builds with no agreement error; the chain.
#[test]
fn the_triangle_and_the_chain_of_section_3() {
    let text = host(
        "a = { slot = { outputs = { out = {} } } }\nb = { slot = { inputs = { in = {} } } }\nseq = { module = \"std.counter\" }",
        &[("a.out", "b.in"), ("a.out", "seq.reset")],
    );
    let p = planned(&text, &[]);
    let a = port(&p, "/a", "out");
    assert_eq!(
        (a.shape, from(a)),
        (PortShape::Events, (AutoEnd::Host, Some("/seq.reset")))
    );
    let b = port(&p, "/b", "in");
    assert_eq!(
        (b.shape, from(b)),
        (PortShape::Events, (AutoEnd::Host, Some("/a.out")))
    );

    // osc.out → a.in, a.out → b.in, b loaded with an events input.
    let text = host(
        "osc = { module = \"std.sine_osc\" }\na = { slot = { inputs = { in = {} }, outputs = { out = {} } } }\nb = { slot = { inputs = { in = {} } }, load = \"std.counter\", map = { in = \"trigger\" } }",
        &[("osc.out", "a.in"), ("a.out", "b.in")],
    );
    let p = planned(&text, &[]);
    let a_in = port(&p, "/a", "in");
    assert_eq!(
        (a_in.kind, from(a_in)),
        (PortKind::Audio, (AutoEnd::Host, Some("/osc.out")))
    );
    let b_in = port(&p, "/b", "in");
    assert_eq!(
        (b_in.shape, from(b_in)),
        (PortShape::Events, (AutoEnd::Content, Some("trigger")))
    );
    let a_out = port(&p, "/a", "out");
    assert_eq!(
        (a_out.shape, from(a_out)),
        (PortShape::Events, (AutoEnd::Host, Some("/b.in")))
    );
}

/// Test 2: the same project with its entries permuted resolves
/// identically, and plans identically.
#[test]
fn permuted_entries_resolve_identically() {
    let instances = [
        "a = { slot = { inputs = { in = {} }, outputs = { out = {} } } }",
        "b = { slot = { inputs = { in = {} } }, load = \"std.counter\", map = { in = \"trigger\" } }",
        "c = { slot = { outputs = { out = {} } } }",
        "d = { slot = { outputs = { out = {} } } }",
        "osc = { module = \"std.sine_osc\" }",
        "seq = { module = \"std.counter\" }",
    ];
    let wires = [
        ("osc.out", "a.in"),
        ("a.out", "b.in"),
        ("c.out", "seq.reset"),
        ("d.out", "a.in"),
    ];
    let forward = host(&instances.join("\n"), &wires);
    let mut reversed_instances = instances;
    reversed_instances.reverse();
    let mut reversed_wires = wires;
    reversed_wires.reverse();
    let backward = host(&reversed_instances.join("\n"), &reversed_wires);
    let (one, two) = (planned(&forward, &[]), planned(&backward, &[]));
    // The interfaces and matches (`dropped` lists wires in their written
    // order, which is permuted too).
    let resolved = |p: &Planned| {
        let slots: Vec<_> = p.slots.iter().map(|s| (&s.interface, &s.matches)).collect();
        serde_json::to_string(&slots).unwrap()
    };
    assert_eq!(resolved(&one), resolved(&two));
    assert_eq!(dump(&one.plan), dump(&two.plan));
    // `a.in` has two sources; `d.out` < `osc.out`, but `d.out` (empty) is
    // known only from stage 4, so `osc.out` defines it (stage 2), and
    // `d.out` takes `a.in`'s type.
    assert_eq!(port(&one, "/d", "out").kind, PortKind::Audio);
    assert_eq!(
        port(&one, "/c", "out").shape,
        PortShape::Events,
        "from seq.reset"
    );
    assert_eq!(
        from(port(&one, "/a", "in")),
        (AutoEnd::Host, Some("/osc.out"))
    );
}

/// Test 3: host events into an auto input whose content takes a signal,
/// and a content's events into a host signal input: errors at the wires,
/// naming the slot's port, how it is typed and the other end.
#[test]
fn conflicts_are_errors_at_the_wire() {
    let text = host(
        "e = { module = \"std.edge\" }\nfx = { slot = { inputs = { x = {} } }, load = \"thru\" }",
        &[("e.rise", "fx.x")],
    );
    let found = errors(&text, &[("composites/thru.toml", THRU)]);
    assert_eq!(
        found,
        [format!(
            "project.toml:{}:3: error: /fx.x (auto, from /e.rise) is events; it reaches thru's x, a signal",
            line(&text, "fx.x")
        )]
    );
    let text = host(
        "fx = { slot = { outputs = { out = {} } }, load = \"ticks\" }\ng = { module = \"std.gain\" }",
        &[("fx.out", "g.in")],
    );
    let found = errors(&text, &[("composites/ticks.toml", TICKS)]);
    assert_eq!(
        found,
        [format!(
            "project.toml:{}:3: error: /fx.out (auto, from the content's out) is events; it reaches /g.in, a signal",
            line(&text, "g.in")
        )]
    );
}

/// Test 3: the known host ends of an empty slot's auto port that disagree
/// with its defining end are errors at their wires; open ones are not.
#[test]
fn disagreeing_ends_of_an_empty_slot_are_errors() {
    let text = host(
        "fx = { slot = { outputs = { out = {} } } }\na = { module = \"std.counter\" }\nb = { module = \"std.gain\" }\nc = { slot = { inputs = { in = {} } } }",
        &[
            ("fx.out", "a.reset"),
            ("fx.out", "b.in"),
            ("fx.out", "c.in"),
        ],
    );
    let found = errors(&text, &[]);
    assert_eq!(
        found,
        [format!(
            "project.toml:{}:12: error: `/fx.out` (auto) is events, from `/a.reset`; `/b.in` takes a signal",
            line(&text, "b.in")
        )]
    );
    let text = host(
        "fx = { slot = { inputs = { in = {} } } }\na = { module = \"std.edge\" }\nb = { module = \"std.const\" }",
        &[("a.rise", "fx.in"), ("b.out", "fx.in")],
    );
    let found = errors(&text, &[]);
    assert!(
        has(
            &found,
            &[
                &format!("project.toml:{}:", line(&text, "b.out")),
                "`/fx.in` (auto) is events, from `/a.rise`; `/b.out` sends a signal"
            ]
        ),
        "{found:#?}"
    );
}

/// Test 3: a rate that divides plans with the converter, byte-identically
/// to the explicit instance; one that does not, and events at another
/// rate, are `RateMismatch` at the wire, through the slot.
#[test]
fn rates_are_judged_at_the_wire() {
    let wires = [("s.out", "fx.in"), ("fx.out", "out.l")];
    let slot = host(
        &format!(
            "s = {{ module = \"test.slow\" }}\nfx = {{ slot = {{ inputs = {{ in = {{}} }}, outputs = {{ out = {{}} }} }}, load = \"std.gain\" }}\n{OUT}"
        ),
        &wires,
    );
    let explicit = host(
        &format!("s = {{ module = \"test.slow\" }}\nfx = {{ module = \"std.gain\" }}\n{OUT}"),
        &wires,
    );
    let (slot_planned, explicit_planned) = (planned(&slot, &[]), planned(&explicit, &[]));
    assert_eq!(dump(&slot_planned.plan), dump(&explicit_planned.plan));
    assert_eq!(port(&slot_planned, "/fx", "in").rate, 64);
    let warnings: Vec<String> = slot_planned
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        has(
            &warnings,
            &[
                &format!("project.toml:{}:", line(&slot, "fx.in")),
                "inserted converter"
            ]
        ),
        "{warnings:#?}"
    );

    let odd = slot.replace("load = \"std.gain\"", "load = \"test.slow48\"");
    let found = errors(&odd, &[]);
    assert!(
        has(
            &found,
            &[
                &format!("project.toml:{}:3: error:", line(&odd, "fx.in")),
                "/fx.in (auto, from /s.out) runs at rate divisor 64; it reaches test.slow48's in, at rate divisor 48: rate mismatch"
            ]
        ),
        "{found:#?}"
    );

    let events = host(
        "e = { module = \"test.slow_edge\" }\nfx = { slot = { inputs = { in = {} } }, load = \"std.counter\", map = { in = \"trigger\" } }",
        &[("e.rise", "fx.in")],
    );
    let found = errors(&events, &[]);
    assert!(
        has(
            &found,
            &[
                &format!("project.toml:{}:3: error:", line(&events, "fx.in")),
                "/fx.in (auto, from /e.rise) runs at rate divisor 2; it reaches std.counter's trigger, at rate divisor 1"
            ]
        ),
        "{found:#?}"
    );
}

/// Test 3: an end that contradicts a pinned field is an error at that
/// wire, not skipped; the field pinned on an auto port is checked against
/// the content as a typed port's is.
#[test]
fn pinned_fields_are_checked_as_before() {
    let text = host(
        "c = { module = \"std.const\" }\nfx = { slot = { inputs = { in = { auto = true, shape = \"events\" } } } }",
        &[("c.out", "fx.in")],
    );
    let found = errors(&text, &[]);
    assert_eq!(
        found,
        [format!(
            "project.toml:{}:26: error: /fx: `in` is declared events on the slot's interface; `c.out` is a signal",
            line(&text, "fx.in")
        )]
    );
    let text = host(
        "fx = { slot = { inputs = { in = { auto = true, rate = 4 } } }, load = \"std.gain\" }",
        &[],
    );
    let found = errors(&text, &[]);
    assert!(
        has(
            &found,
            &["`std.gain` `in` runs at rate divisor 1; the slot's interface declares `in` at 4"]
        ),
        "{found:#?}"
    );
    // A pinned kind warns; an inferred one does not.
    let text = host(
        &format!(
            "c = {{ module = \"std.const\" }}\nfx = {{ slot = {{ inputs = {{ in = {{ auto = true, kind = \"gate\" }}, side = {{}} }} }}, load = \"std.gain\" }}\n{OUT}"
        ),
        &[("c.out", "fx.side")],
    );
    let warnings: Vec<String> = planned(&text, &[])
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        has(
            &warnings,
            &["kind mismatch: `std.gain`'s `in` is audio; the slot's interface declares `in` gate"]
        ),
        "{warnings:#?}"
    );
    assert!(!has(&warnings, &["`side`"]), "{warnings:#?}");
}

/// Test 3: `auto = false`, `auto` on a parameter, an auto port in a library
/// template, and a `voice` field (ADR 0044, not built: an unknown field)
/// are errors at the key.
#[test]
fn auto_is_refused_where_it_means_nothing() {
    let cases = [
        (
            "fx = { slot = { inputs = { in = { auto = false } } } }",
            "`auto` takes only `true`",
        ),
        (
            "fx = { slot = { parameters = { level = { auto = true } } } }",
            "unknown field `auto`",
        ),
        (
            "fx = { slot = { inputs = { in = { auto = true, voice = \"each\" } } } }",
            "unknown field `voice`",
        ),
        (
            "fx = { slot = { inputs = { in = { range = [1] } } } }",
            "`range` is `[min, max]`, or `[]` for none",
        ),
    ];
    for (entry, message) in cases {
        let text = host(entry, &[]);
        let found = errors(&text, &[]);
        assert!(
            has(&found, &["project.toml:5:", message]),
            "{entry}: {found:#?}"
        );
    }
    // `unit = ""` and `range = []` pin none, and are typed fields elsewhere.
    let text = host(
        "fx = { slot = { inputs = { a = { auto = true, unit = \"\", range = [] }, b = { unit = \"\", range = [] } } } }",
        &[],
    );
    let p = planned(&text, &[]);
    let a = port(&p, "/fx", "a");
    assert_eq!((&a.unit, a.range), (&None, None));
    assert!(a.pinned(Field::Unit) && a.pinned(Field::Range) && !a.pinned(Field::Shape));
    let b = port(&p, "/fx", "b");
    assert_eq!(
        (b.unit.as_deref(), b.range, &b.auto),
        (Some(""), None, &None)
    );
}

/// Test 4: a slot with auto ports plans byte-identically to the slot with
/// the inferred types pinned and to the explicit instance; `{}` under
/// today's wiring (signals at rate 1) plans as the generic signal it meant.
#[test]
fn auto_ports_plan_as_the_pinned_and_the_explicit_forms() {
    let wires = [
        ("osc.out", "fx.x"),
        ("fx.out", "out.l"),
        ("fx.out", "out.r"),
    ];
    let form = |fx: &str| {
        host(
            &format!("osc = {{ module = \"std.sine_osc\" }}\nfx = {fx}\n{OUT}"),
            &wires,
        )
    };
    let explicit = planned(
        &form("{ module = \"thru\" }"),
        &[("composites/thru.toml", THRU)],
    );
    let forms = [
        "{ slot = { inputs = { x = {} }, outputs = { out = {} } }, load = \"thru\" }",
        "{ slot = { inputs = { x = { auto = true, kind = \"audio\" } }, outputs = { out = { auto = true, range = [] } } }, load = \"thru\" }",
        "{ slot = { inputs = { x = { auto = true, shape = \"signal\", rate = 1, kind = \"audio\", unit = \"\", range = [-1, 1] } }, outputs = { out = { auto = true, shape = \"signal\", rate = 1, kind = \"audio\", unit = \"\", range = [] } } }, load = \"thru\" }",
        "{ slot = { inputs = { x = \"audio\" }, outputs = { out = \"audio\" } }, load = \"thru\" }",
        "{ slot = { inputs = { x = \"generic\" }, outputs = { out = \"generic\" } }, load = \"thru\" }",
    ];
    for fx in forms {
        let p = planned(&form(fx), &[("composites/thru.toml", THRU)]);
        assert_eq!(dump(&p.plan), dump(&explicit.plan), "{fx}");
    }
    let p = planned(&form(forms[0]), &[("composites/thru.toml", THRU)]);
    let x = port(&p, "/fx", "x");
    assert_eq!(
        (x.kind, x.range, from(x)),
        (
            PortKind::Audio,
            Some((-1.0, 1.0)),
            (AutoEnd::Host, Some("/osc.out"))
        )
    );
}

/// Test 4: a project template holding `{}` makes an auto port in every
/// slot from it, each inferred from its own wiring.
#[test]
fn a_project_template_with_auto_ports_types_each_slot_by_its_wiring() {
    let bus =
        "schema_version = 1\n[interface]\nname = \"bus\"\n[inputs]\nin = {}\n[outputs]\nout = {}\n";
    let text = host(
        "e = { module = \"std.edge\" }\nc = { module = \"std.const\" }\none = { slot = \"bus\" }\ntwo = { slot = \"bus\" }",
        &[("e.rise", "one.in"), ("c.out", "two.in")],
    );
    let p = planned(&text, &[("interfaces/bus.toml", bus)]);
    assert_eq!(port(&p, "/one", "in").shape, PortShape::Events);
    assert_eq!(port(&p, "/two", "in").shape, PortShape::Signal);
    for slot in ["/one", "/two"] {
        assert_eq!(from(port(&p, slot, "out")), (AutoEnd::Default, None));
    }
    // The library's templates read, all typed.
    for template in signal_project::library_interfaces() {
        assert!(
            template.interface.ports.iter().all(|p| p.auto.is_none()),
            "{}",
            template.name
        );
    }
}

/// ADR 0045 §9: `describe`'s `content.slots` and `graph`'s interface ports
/// carry `auto` additively; a typed port's JSON is what it was.
#[test]
fn the_auto_field_is_reported() {
    let text = host(
        &format!(
            "c = {{ module = \"std.const\" }}\nfx = {{ slot = {{ inputs = {{ in = {{ auto = true, kind = \"control\", group = \"g\" }} }}, outputs = {{ out = \"audio\" }} }} }}\n{OUT}"
        ),
        &[("c.out", "fx.in"), ("fx.out", "out.l")],
    );
    let p = planned(&text, &[]);
    let json = serde_json::to_value(&p.slots[0].interface.ports).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            {"name": "in", "role": "input", "shape": "signal", "rate": 1, "kind": "control", "group": "g",
             "auto": {"inferred": ["shape", "rate", "unit", "range"], "from": {"end": "host", "port": "/c.out"}}},
            {"name": "out", "role": "output", "shape": "signal", "rate": 1, "kind": "audio"}
        ])
    );
    // `graph`: the slot's interface and its box's ports.
    let project = Project::from_sources(sources(&text, &[])).unwrap();
    let structure = project.structure(&p.plan, &registry());
    let fx = find(&structure, &|v| {
        v.get("name") == Some(&serde_json::json!("fx")) && v.get("slot").is_some()
    })
    .expect("the slot fx");
    assert_eq!(fx["slot"]["interface"]["ports"], json);
    let auto = &json[0]["auto"];
    assert_eq!(fx["ports"][0]["auto"], *auto);
    assert!(fx["ports"][1].get("auto").is_none());
}

/// The first value in `value` (depth first) that `wanted`.
fn find<'v>(
    value: &'v serde_json::Value,
    wanted: &dyn Fn(&serde_json::Value) -> bool,
) -> Option<&'v serde_json::Value> {
    if wanted(value) {
        return Some(value);
    }
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(|v| find(v, wanted)),
        serde_json::Value::Object(fields) => fields.values().find_map(|v| find(v, wanted)),
        _ => None,
    }
}

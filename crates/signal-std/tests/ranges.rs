//! ADR 0043, the owner's "MINDEN kapjon értelmezési tartományt vagy
//! értékkészletet" (everything gets a range or a value set): over the whole
//! registry, no parameter keeps the generic ±10⁶ range, every range is
//! finite and within the engine's magnitude bound, every discrete
//! parameter (a mode, a kind, a shape, a boundary, an interpolation, a
//! wrap, an on/off gate) has a value set, and every value set is
//! well-formed: inside its range, spanning it, its default one of it.

use signal_core::Registry;

/// The engine's magnitude bound (`docs/semantics.md` §7).
const BOUND: f64 = 1e6;

/// The parameter names that are choices: each must have a value set.
const DISCRETE: &[&str] = &[
    "mode",
    "kind",
    "shape",
    "boundary",
    "interp",
    "wrap",
    "on_change",
    "run",
];

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

#[test]
fn every_parameter_has_a_meaningful_range_or_a_value_set() {
    let registry = registry();
    let mut problems = Vec::new();
    let mut counted = 0;
    for module in registry.iter() {
        let d = &module.descriptor;
        for p in d.parameters {
            counted += 1;
            let at = format!("{}.{}", d.type_name, p.name);
            let r = p.range;
            if !(r.min.is_finite() && r.max.is_finite()) {
                problems.push(format!("{at}: range [{}, {}] is not finite", r.min, r.max));
            }
            if r.min.abs() > BOUND || r.max.abs() > BOUND {
                problems.push(format!(
                    "{at}: range [{}, {}] reaches beyond ±10⁶",
                    r.min, r.max
                ));
            }
            if r.min == -BOUND && r.max == BOUND {
                problems.push(format!("{at}: the generic ±10⁶ range"));
            }
            if !(r.min <= p.default && p.default <= r.max) {
                problems.push(format!("{at}: default {} outside its range", p.default));
            }
            if DISCRETE.contains(&p.name) && p.values.is_none() {
                problems.push(format!("{at}: a choice without a value set"));
            }
            if let Some(values) = p.values {
                if values.is_empty() {
                    problems.push(format!("{at}: an empty value set"));
                    continue;
                }
                let lo = values.iter().map(|v| v.value).fold(f64::INFINITY, f64::min);
                let hi = values
                    .iter()
                    .map(|v| v.value)
                    .fold(f64::NEG_INFINITY, f64::max);
                if lo != r.min || hi != r.max {
                    problems.push(format!(
                        "{at}: its values span [{lo}, {hi}], its range [{}, {}]",
                        r.min, r.max
                    ));
                }
                if !values.iter().any(|v| v.value == p.default) {
                    problems.push(format!("{at}: default {} not one of its values", p.default));
                }
                if values.iter().any(|v| v.label.trim().is_empty()) {
                    problems.push(format!("{at}: a value without a name"));
                }
                if values.windows(2).any(|w| w[0].value >= w[1].value) {
                    problems.push(format!("{at}: values not in increasing order"));
                }
            }
        }
    }
    assert!(counted > 100, "the registry's parameters: {counted}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_value_sets_name_what_the_rustdoc_names() {
    let registry = registry();
    let labels = |t: &str, p: &str| -> Vec<(f64, &'static str)> {
        let d = &registry.get(t).unwrap().descriptor;
        d.parameters
            .iter()
            .find(|q| q.name == p)
            .and_then(|q| q.values)
            .unwrap_or_default()
            .iter()
            .map(|v| (v.value, v.label))
            .collect()
    };
    assert_eq!(
        labels("std.osc", "shape"),
        [
            (0.0, "sine"),
            (1.0, "saw"),
            (2.0, "square"),
            (3.0, "triangle")
        ]
    );
    assert_eq!(labels("std.table4", "interp")[2], (2.0, "Hermite"));
    assert_eq!(
        labels("std.playhead", "run"),
        [(0.0, "stopped"), (1.0, "running")]
    );
    assert_eq!(
        labels("std.playhead", "release"),
        [(0.0, "continue"), (1.0, "return")]
    );
    assert_eq!(labels("std.biquad", "mode").len(), 8);
}

/// ADR 0043 (review of c89253a): snapping a value to a value set gives
/// exactly what the module's `process` decides for that value. Over the
/// whole registry, every discrete parameter is driven, with the default
/// random stimulus otherwise, at a sweep of constant bases (its values, the
/// halves between them and just beside, small positives, its range's
/// ends), and the render at each base must equal the render at its
/// snapped value ([`signal_core::ParameterDescriptor::snapped`]). A
/// parameter whose values all render alike under the stimulus is listed,
/// so the sweep cannot pass by not reaching it.
/// Parameters read only when the plan's wiring changes, which a stimulus of
/// one call cannot do: `std.playhead.release` is read on the frame
/// following stops (a swap that disconnects `follow`, ADR 0037 §6); its
/// snapping is checked against `process` in `playhead.rs`'s tests.
const WIRING_ONLY: &[&str] = &["std.playhead.release"];

#[test]
fn a_value_set_snaps_as_process_reads_it() {
    const FRAMES: usize = 512;
    let registry = registry();
    let mut problems = Vec::new();
    let mut unreached = Vec::new();
    let mut checked = 0;
    for module in registry.iter() {
        let d = &module.descriptor;
        for (index, p) in d.parameters.iter().enumerate() {
            let Some(values) = p.values else {
                continue;
            };
            let at = format!("{}.{}", d.type_name, p.name);
            let render = |base: f64| {
                let mut stimulus = signal_contract::stimulus::random(d, 7, FRAMES);
                stimulus.parameters[index] = vec![base; FRAMES];
                format!("{:?}", signal_contract::outputs(module, &stimulus, FRAMES))
            };
            let mut entries: Vec<f64> = values.iter().map(|v| v.value).collect();
            entries.sort_by(f64::total_cmp);
            let mut bases = entries.clone();
            for pair in entries.windows(2) {
                let half = (pair[0] + pair[1]) / 2.0;
                bases.extend([half, half - 1e-9, half + 1e-9]);
            }
            for v in &entries {
                bases.extend([v - 0.3, v + 0.3, v + 1e-9, v - 1e-9]);
            }
            bases.extend([1e-9, 0.3, 0.49999999999999994, p.range.min, p.range.max]);
            bases.retain(|b| p.range.min <= *b && *b <= p.range.max);
            for base in bases {
                let snapped = p.snapped(base);
                checked += 1;
                if render(base) != render(snapped) {
                    problems.push(format!(
                        "{at}: {base} snaps to {snapped}, but plays differently"
                    ));
                }
            }
            let renders: std::collections::BTreeSet<String> =
                entries.iter().map(|v| render(*v)).collect();
            if entries.len() > 1 && renders.len() < 2 && !WIRING_ONLY.contains(&at.as_str()) {
                unreached.push(at);
            }
        }
    }
    assert!(checked > 100, "only {checked} bases checked");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(
        unreached.is_empty(),
        "values that render alike under the stimulus: {unreached:?}"
    );
}

/// The measured cases of the review of c89253a, each snapped as `process`
/// reads it on `main` (ties upward, as `round_half_up`; anything positive
/// on for a parameter read as `> 0`).
#[test]
fn the_reviewed_values_snap_as_they_played() {
    let registry = registry();
    let snapped = |module: &str, name: &str, value: f64| {
        let d = &registry.get(module).unwrap().descriptor;
        d.parameters
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .snapped(value)
    };
    assert_eq!(snapped("std.osc", "shape", 0.5), 1.0, "saw, as before");
    // `x + ½` rounds to 1 here, so `round_half_up` read saw (review of 5b54fc5).
    assert_eq!(snapped("std.osc", "shape", 0.49999999999999994), 1.0);
    assert_eq!(snapped("std.fm_op", "mode", 0.5), 1.0);
    assert_eq!(snapped("std.playhead", "run", 0.5), 1.0);
    assert_eq!(snapped("std.playhead", "release", 0.5), 1.0);
    assert_eq!(snapped("std.playhead", "release", 0.4999), 0.0);
    assert_eq!(snapped("std.to_events", "on_change", 0.3), 1.0);
    assert_eq!(snapped("std.to_events", "on_change", 0.0), 0.0);
    assert_eq!(snapped("std.table16", "wrap", 0.4), 1.0);
    assert_eq!(snapped("std.table16", "interp", 1.5), 2.0);
    assert_eq!(snapped("std.table16", "interp", 1.4999), 1.0);
}

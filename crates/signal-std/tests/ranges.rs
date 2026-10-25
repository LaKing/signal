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
    assert_eq!(labels("std.biquad", "mode").len(), 8);
}

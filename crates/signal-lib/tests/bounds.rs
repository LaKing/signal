//! M2.10-3 AC12: no library composite faults at its parameters' extremes.
//! Each exported parameter's range is the one `signal modules` shows
//! (`signal_project::library_modules`, the interface `graph` gives); every
//! composite renders on its stimulus (for `common::frames`, long enough for
//! its whole signal path at its defaults) with all parameters at their
//! minimum, all at their maximum, and each one alone at its minimum and at
//! its maximum. Where an extreme would keep part of the path silent within
//! the render ([`base`]: `tape_echo` at a long repeat time never closes its
//! loop in 1 s), every case runs again with that parameter held where the
//! path is live (so "all at maximum" also runs with the loop closing). The engine checks every output value for being finite and within
//! ±1e6 (`docs/semantics.md` §7) and logs a fault otherwise; the test asserts
//! the log has none, and that the samples are within ±1e6. A parameter
//! exported to several targets with different ranges would make the plan
//! refuse the value at one of them, so the plan succeeding also shows that
//! fanned-out targets accept the range shown.

mod common;

use std::collections::BTreeMap;

use common::{faults, peak, project, registry, render};
use serde_json::Value;
use signal_project::library_modules;

/// The parameters held where `name`'s whole path is live in its render, in
/// a second run of every case (overriding the case's own value): `tape_echo`'s repeat
/// time at 20 ms, so its feedback loop runs about 50 trips in 1 s whatever
/// the feedback, tone, wow, flutter and mix.
fn base(name: &str) -> &'static [(&'static str, f64)] {
    match name {
        "tape_echo" => &[("time_ms", 20.0)],
        _ => &[],
    }
}

/// `key = value, …` for TOML.
fn assignments(parameters: &BTreeMap<String, f64>) -> String {
    parameters
        .iter()
        .map(|(name, value)| format!("{name} = {value:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders `parameters`, and again with [`base`] over them.
fn check_with_base(name: &str, case: &str, parameters: BTreeMap<String, f64>) {
    check(name, case, &assignments(&parameters));
    let mut held = parameters.clone();
    for (key, value) in base(name) {
        held.insert((*key).to_owned(), *value);
    }
    if held != parameters {
        check(name, &format!("{case}, base"), &assignments(&held));
    }
}

fn check(name: &str, case: &str, parameters: &str) {
    let rendered = render(&project(name, parameters));
    assert_eq!(
        faults(&rendered),
        Vec::<String>::new(),
        "lib.{name}, {case}: {parameters}"
    );
    let peak = peak(&rendered);
    assert!(peak <= 1e6, "lib.{name}, {case}: peak {peak}");
}

#[test]
fn no_fault_at_parameter_extremes() {
    for module in library_modules(&registry()).unwrap() {
        let name = module["module"]
            .as_str()
            .unwrap()
            .strip_prefix("lib.")
            .unwrap()
            .to_owned();
        let ranges: Vec<(String, f64, f64)> = module["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p: &Value| {
                let bound = |k: &str| p["range"][k].as_f64().unwrap();
                (
                    p["name"].as_str().unwrap().to_owned(),
                    bound("min"),
                    bound("max"),
                )
            })
            .collect();
        let all = |pick: fn(&(String, f64, f64)) -> f64| {
            ranges.iter().map(|r| (r.0.clone(), pick(r))).collect()
        };
        check_with_base(&name, "all at minimum", all(|r| r.1));
        check_with_base(&name, "all at maximum", all(|r| r.2));
        for (parameter, min, max) in &ranges {
            let one = |value: f64| BTreeMap::from([(parameter.clone(), value)]);
            check_with_base(&name, "one at minimum", one(*min));
            check_with_base(&name, "one at maximum", one(*max));
        }
    }
}

//! The gain ceiling (ADR 0029 §3, `docs/agent-interface.md` "Safety"): no
//! tool may cause a `std.audio_out` gain above `--max-gain`.
//!
//! A sink's effective gain is `clamp(base · m, 0, 4)` where `m` is the sum
//! of its gain port's signal connections, or the base alone when nothing is
//! connected (`docs/semantics.md` "The effective value"); a port that lost
//! connections to instances a swap removed holds the old `m`. Nothing bounds
//! `m` but that clamp, so a connected ("modulated") port has no known
//! effective value. For every gain port, a change is allowed only when:
//!
//! - **R1** (static after the change): `live_after ≤ max`, or the port was
//!   static before too and `live_after ≤ live_before`;
//! - **R2** (modulated after): `max ≥ 4`, or the port was modulated before,
//!   with the same connections and scales into it and the same fan-in (every
//!   instance with a path to it, their parameters, connections and pattern
//!   data), and `live_after ≤ live_before` (with `m` fixed, `clamp(base · m)`
//!   never grows as the base falls).
//!
//! `live_before` is the base the runner last reported (a `parameters`
//! stream). `live_after` is the value a `set`, `ramp` (its target: a ramp is
//! monotonic), `commit` or `revert` sends; for an edit of the files, the
//! file's new value when it changes (or the port is new, an absent key being
//! the declared default 1.0), else `live_before`: a reload sends a parameter
//! the file's value only when that value changes (`docs/ui-protocol.md`
//! "Unsaved").

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use signal_core::Registry;
use signal_project::{Project, SourceFile};

/// The parameter every `std.audio_out` gain is.
const SINK: &str = "std.audio_out";
const GAIN: &str = "gain";
/// The gain's declared range ends at 4: the effective gain never exceeds it.
const CLAMP: f64 = 4.0;

/// A gain port of a plan.
#[derive(Clone, Debug, PartialEq)]
struct Port {
    /// The base the plan gives it (the file's value, else the default).
    file: f64,
    /// Whether anything is connected into it.
    connected: bool,
    /// Its own sources and its fan-in, in a canonical text: equal texts are
    /// the same modulation.
    fanin: String,
    /// The instances with a path to it.
    instances: BTreeSet<String>,
}

/// Every gain port of a project's plan, by address (`/out.gain`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Gains {
    ports: BTreeMap<String, Port>,
    /// Every instance of the plan.
    instances: BTreeSet<String>,
}

impl Gains {
    /// The gain ports of the project `files` make, or why it does not load
    /// and plan (its diagnostics, one per line): then no change is judged,
    /// and every guard refuses (`docs/agent-interface.md` "Safety"). `dir`
    /// is the project's folder, where the projects the files include are
    /// found (ADR 0035); without one an include does not load.
    pub(crate) fn of(
        files: Vec<SourceFile>,
        dir: Option<&Path>,
        registry: &Registry,
    ) -> Result<Gains, String> {
        let lines = |diagnostics: Vec<signal_project::Diagnostic>| {
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let project = match dir {
            Some(dir) => Project::from_sources_in(files, dir),
            None => Project::from_sources(files),
        }
        .map_err(lines)?;
        let plan = project.plan(registry).map_err(lines)?.plan;
        let instances: BTreeSet<String> = plan
            .instances
            .iter()
            .map(|i| i.address.to_string())
            .collect();
        // Each instance's incoming connections: (its port, source, scale, shape).
        let mut into: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut feeders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (port, sources) in &plan.sources {
            let instance = port.instance().to_string();
            for source in sources {
                into.entry(instance.clone()).or_default().push(format!(
                    "{port}<-{}*{:?}:{:?}",
                    source.from,
                    source.scale.to_bits(),
                    source.shape
                ));
                feeders
                    .entry(instance.clone())
                    .or_default()
                    .insert(source.from.instance().to_string());
            }
        }
        let mut ports = BTreeMap::new();
        for sink in plan.instances.iter().filter(|i| i.type_name == SINK) {
            let address = format!("{}.{GAIN}", sink.address);
            let gain_sources: Vec<_> = plan
                .sources
                .iter()
                .filter(|(port, _)| port.as_str() == address)
                .flat_map(|(_, sources)| sources)
                .collect();
            // The fan-in: every instance with a path to the gain port.
            let mut reached = BTreeSet::new();
            let mut todo: Vec<String> = gain_sources
                .iter()
                .map(|s| s.from.instance().to_string())
                .collect();
            while let Some(next) = todo.pop() {
                if reached.insert(next.clone()) {
                    todo.extend(feeders.get(&next).into_iter().flatten().cloned());
                }
            }
            let mut fanin: Vec<String> = gain_sources
                .iter()
                .map(|s| {
                    format!(
                        "{address}<-{}*{:?}:{:?}",
                        s.from,
                        s.scale.to_bits(),
                        s.shape
                    )
                })
                .collect();
            for instance in plan
                .instances
                .iter()
                .filter(|i| reached.contains(&i.address.to_string()))
            {
                let parameters: Vec<String> = instance
                    .parameters
                    .iter()
                    .map(|(name, value)| format!("{name}={:?}", value.to_bits()))
                    .collect();
                // Its pattern's data, every value (a table's Debug shows its
                // shape only).
                let table = instance.table.as_ref().map(|t| {
                    let values: Vec<String> = t
                        .values()
                        .iter()
                        .map(|v| format!("{:?}", v.to_bits()))
                        .collect();
                    format!("{}:{}", t.width(), values.join(","))
                });
                fanin.push(format!(
                    "{} {} [{}] {table:?}",
                    instance.address,
                    instance.type_name,
                    parameters.join(","),
                ));
                fanin.extend(
                    into.get(&instance.address.to_string())
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
            ports.insert(
                address,
                Port {
                    file: sink.parameters.get(GAIN).copied().unwrap_or(1.0),
                    connected: !gain_sources.is_empty(),
                    fanin: fanin.join("\n"),
                    instances: reached,
                },
            );
        }
        Ok(Gains { ports, instances })
    }

    /// The base the files give gain port `address` (absent key: 1.0).
    pub(crate) fn file_value(&self, address: &str) -> Option<f64> {
        self.ports.get(address).map(|port| port.file)
    }

    /// The gain ports' addresses.
    pub(crate) fn addresses(&self) -> impl Iterator<Item = &str> {
        self.ports.keys().map(String::as_str)
    }

    /// Whether `address` is a gain port.
    pub(crate) fn is_gain(&self, address: &str) -> bool {
        self.ports.contains_key(address)
    }

    /// What `project_info` reports of each gain port.
    pub(crate) fn report(&self, live: &BTreeMap<String, f64>, max: f64) -> Vec<serde_json::Value> {
        self.ports
            .iter()
            .map(|(address, port)| {
                let live = live.get(address).copied();
                serde_json::json!({
                    "port": address,
                    "file": port.file,
                    "live": live,
                    "modulated": port.connected,
                    "aboveCeiling": port.connected && max < CLAMP
                        || live.unwrap_or(port.file) > max,
                })
            })
            .collect()
    }
}

/// A change of live values: the gain ports' new bases, and the instances
/// whose parameters or events it changes.
#[derive(Default)]
pub(crate) struct Live {
    /// New bases of gain ports, by address.
    pub(crate) bases: BTreeMap<String, f64>,
    /// Instances a `set`, `ramp` or `trigger` reaches (other than a gain).
    pub(crate) instances: BTreeSet<String>,
}

/// Whether `commit` may write `written` into a gain whose file value is
/// `file`: the file's own value is its baseline, so a commit never puts an
/// over-ceiling value there that the file did not already hold (whatever
/// the live value; `check` judges what it sends live).
pub(crate) fn commit_allowed(max: f64, written: f64, file: f64) -> bool {
    written <= max || written <= file
}

/// Why a change was refused, or `Ok`.
pub(crate) fn check(
    max: f64,
    before: Option<&Gains>,
    after: &Gains,
    live_before: &BTreeMap<String, f64>,
    live: &Live,
) -> Result<(), String> {
    for (address, port) in &after.ports {
        let old = before.and_then(|b| b.ports.get(address));
        // Unknown before (a new sink, a project that did not plan): taken as
        // modulated, the stricter case.
        let modulated_before = old.is_none_or(|p| p.connected);
        // Held modulation: a source of the port's whose instance is gone.
        let held = old.is_some_and(|old| {
            old.connected
                && old
                    .instances
                    .iter()
                    .any(|instance| !after.instances.contains(instance))
        });
        let modulated_after = port.connected || held;
        let measured = live_before.get(address).copied();
        let before_value = measured.or(old.map(|p| p.file)).unwrap_or(0.0);
        let after_value = match (live.bases.get(address), old) {
            (Some(value), _) => *value,
            (None, Some(old)) if old.file != port.file => port.file,
            (None, Some(_)) => before_value,
            (None, None) => port.file,
        }
        .clamp(0.0, CLAMP);
        let identical = old.is_some_and(|old| old.fanin == port.fanin)
            && live.instances.is_disjoint(&port.instances);
        let allowed = if modulated_after {
            max >= CLAMP || (modulated_before && identical && after_value <= before_value)
        } else {
            after_value <= max || (!modulated_before && after_value <= before_value)
        };
        if !allowed {
            let why = if modulated_after {
                format!(
                    "{address} is modulated (a connection into it{}), so its effective gain is bounded only by its range (4); the change raises its base or changes what modulates it",
                    if held && !port.connected {
                        " was removed with its source instance, which leaves the modulation held"
                    } else {
                        ""
                    }
                )
            } else if modulated_before {
                format!(
                    "{address} would be {after_value} with nothing modulating it, and it was modulated before"
                )
            } else {
                format!("{address} would go from {before_value} to {after_value}")
            };
            return Err(format!(
                "refused: this would put a std.audio_out gain above --max-gain {max}: {why}. Start `signal mcp` with a higher --max-gain to allow it."
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        let mut registry = Registry::new();
        signal_std::register(&mut registry).unwrap();
        registry
    }

    fn gains(text: &str) -> Gains {
        let files = vec![SourceFile {
            path: "project.toml".to_owned(),
            text: text.to_owned(),
        }];
        Gains::of(files, None, &registry()).unwrap()
    }

    const HEAD: &str =
        "schema_version = 1\n[project]\nname = \"t\"\nsample_rate = 48000\nseed = 1\n";

    fn project(instances: &str, connections: &str) -> String {
        format!("{HEAD}[instances]\n{instances}\n[wiring]\nconnections = [{connections}]\n")
    }

    #[test]
    fn a_new_sink_without_a_gain_key_has_the_default_1() {
        let after = gains(&project(
            "a = { module = \"std.sine_osc\" }\nout = { module = \"std.audio_out\" }",
            "{ from = \"a.out\", to = \"out.l\" }",
        ));
        assert!(check(0.25, None, &after, &BTreeMap::new(), &Live::default()).is_err());
        assert!(check(1.0, None, &after, &BTreeMap::new(), &Live::default()).is_ok());
    }

    /// auditor-1-commit-test-fixture: a live value already above the
    /// ceiling (the person's, attached) may not be committed over a lower
    /// file value, though committing it changes nothing live.
    #[test]
    fn commit_takes_the_file_as_its_baseline() {
        assert!(!commit_allowed(0.25, 0.3, 0.2), "live 0.3, file 0.2");
        assert!(commit_allowed(0.25, 0.5, 0.5), "the file already holds it");
        assert!(commit_allowed(0.25, 0.2, 0.5));
        assert!(commit_allowed(0.5, 0.3, 0.2));
    }

    #[test]
    fn modulation_bounds_only_by_the_clamp() {
        let modulated = project(
            "m = { module = \"std.const\", value = 2 }\nout = { module = \"std.audio_out\", gain = 0.1 }",
            "{ from = \"m.out\", to = \"out.gain\" }",
        );
        let before = gains(&modulated);
        let live = BTreeMap::from([("/out.gain".to_owned(), 0.1)]);
        let raise = Live {
            bases: BTreeMap::from([("/out.gain".to_owned(), 0.2)]),
            ..Live::default()
        };
        assert!(check(0.25, Some(&before), &before, &live, &raise).is_err());
        let lower = Live {
            bases: BTreeMap::from([("/out.gain".to_owned(), 0.05)]),
            ..Live::default()
        };
        assert!(check(0.25, Some(&before), &before, &live, &lower).is_ok());
        let upstream = gains(&modulated.replace("value = 2", "value = 3"));
        assert!(check(0.25, Some(&before), &upstream, &live, &Live::default()).is_err());
        assert!(check(4.0, Some(&before), &upstream, &live, &Live::default()).is_ok());
    }
}

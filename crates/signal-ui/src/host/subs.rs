//! Subscriptions: the parameter, meter, scope and vector streams and their
//! rates.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_core::{PortAddress, PortShape};
use signal_graph::ExecutionPlan;
use signal_output::{SCOPE_SLOTS, VECTOR_MAX_ROWS, VECTOR_SLOTS};

use crate::frames::{self, frame};

use super::app_mode::powered_off;
use super::failure::{Failure, fail};
use super::{Host, with};

/// The fastest parameter and meter streams.
const MAX_RATE_HZ: f64 = 60.0;

/// A message's `rate`, `default` without one: a number of at least 1.
pub(super) fn rate_of(message: &Value, default: f64) -> Result<f64, Failure> {
    let rate = match message.get("rate") {
        None => default,
        Some(rate) => rate.as_f64().unwrap_or(f64::NAN),
    };
    if rate.is_finite() && rate >= 1.0 {
        Ok(rate)
    } else {
        Err(fail("bad-rate", "rate must be a number of at least 1"))
    }
}

/// The interval of a parameter or meter stream: its `rate` (default 30)
/// from 1 to 60 per second, faster ones taken as 60.
pub(super) fn rate_interval(message: &Value) -> Result<Duration, Failure> {
    let rate = rate_of(message, 30.0)?.min(MAX_RATE_HZ);
    Duration::try_from_secs_f64(1.0 / rate).map_err(|e| fail("bad-rate", e.to_string()))
}

/// A scope's decimation at `sample_rate`: its `rate` in points per second
/// (default and at most the sample rate; at least 1).
pub(super) fn scope_decimation(message: &Value, sample_rate: u32) -> Result<u64, Failure> {
    let sample_rate = f64::from(sample_rate.max(1));
    let rate = rate_of(message, sample_rate)?.min(sample_rate);
    // From 1 to the sample rate, so this is a finite number of frames.
    Ok((sample_rate / rate).round().max(1.0) as u64)
}

/// A vector stream's decimation: its `rate` in snapshots per second
/// (default 60; at least 1, at most the port's tick rate), as a whole number
/// of the port's ticks, so every snapshot is a tick.
pub(super) fn vector_decimation(rate: f64, sample_rate: u32, divisor: u64) -> u64 {
    let divisor = divisor.max(1);
    let ticks = f64::from(sample_rate.max(1)) / divisor as f64;
    let rate = rate.min(ticks.max(1.0));
    divisor * (ticks / rate).round().max(1.0) as u64
}

/// Vector output `port` of `plan`: its elements and rate divisor
/// ([`signal_output::vector_shape`]); `None` without a plan.
pub(super) fn vector_shape(
    plan: Option<&ExecutionPlan>,
    port: &PortAddress,
) -> Option<(usize, u64)> {
    signal_output::vector_shape(plan?, port)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Level {
    peak: f32,
    sum_squares: f64,
    frames: u64,
}

pub(super) enum Stream {
    Parameters {
        ports: Vec<Option<PortAddress>>,
    },
    Meters {
        input: bool,
        levels: Vec<Level>,
        first: Option<u64>,
    },
    Scope {
        slot: usize,
        port: PortAddress,
        decimation: u64,
        bound: bool,
    },
    Vector {
        slot: usize,
        port: PortAddress,
        rows: usize,
        /// The snapshots per second asked for.
        rate: f64,
        decimation: u64,
        bound: bool,
    },
}

pub(super) struct Sub {
    pub(super) client: u64,
    every: Duration,
    last: Instant,
    pub(super) stream: Stream,
}

/// The clients' subscriptions by id, the id the next one gets, the scope
/// and vector slots they hold, and what the last `scopes` push said.
pub(super) struct Subscriptions {
    pub(super) by_id: BTreeMap<u32, Sub>,
    pub(super) next: u32,
    pub(super) scope_slots: [Option<u32>; SCOPE_SLOTS],
    pub(super) vector_slots: [Option<u32>; VECTOR_SLOTS],
    /// The free scope slots last pushed (`scopes`), and when they last rose.
    pub(super) scopes_pushed: Option<usize>,
    pub(super) scopes_freed: Option<Instant>,
    /// A scope slot was released since the last `scopes` push.
    pub(super) scopes_released: bool,
}

impl Default for Subscriptions {
    fn default() -> Subscriptions {
        Subscriptions {
            by_id: BTreeMap::new(),
            next: 1,
            scope_slots: [None; SCOPE_SLOTS],
            vector_slots: [None; VECTOR_SLOTS],
            scopes_pushed: None,
            scopes_freed: None,
            scopes_released: false,
        }
    }
}

impl Subscriptions {
    /// The subscription holding scope slot `slot`, and its client.
    fn of_scope_slot(&self, slot: usize) -> Option<(u32, u64)> {
        let sub = self.scope_slots.get(slot).copied().flatten()?;
        self.by_id.get(&sub).map(|s| (sub, s.client))
    }

    /// The subscription holding vector slot `slot`, and its client.
    fn of_vector_slot(&self, slot: usize) -> Option<(u32, u64)> {
        let sub = self.vector_slots.get(slot).copied().flatten()?;
        self.by_id.get(&sub).map(|s| (sub, s.client))
    }

    /// How many scope slots are free.
    fn free_scopes(&self) -> usize {
        self.scope_slots.iter().filter(|s| s.is_none()).count()
    }

    /// Client `client`'s subscriptions.
    fn of_client(&self, client: u64) -> Vec<u32> {
        self.by_id
            .iter()
            .filter(|(_, s)| s.client == client)
            .map(|(&id, _)| id)
            .collect()
    }

    /// Removes subscription `sub` and frees its slot (a scope slot's
    /// release restarts `sinceFreedMs`); the slot is stopped by the caller.
    fn remove(&mut self, sub: u32) -> Option<Sub> {
        let removed = self.by_id.remove(&sub);
        match &removed {
            Some(Sub {
                stream: Stream::Scope { slot, .. },
                ..
            }) => {
                if let Some(s) = self.scope_slots.get_mut(*slot) {
                    *s = None;
                }
                // Every release restarts the age, even if another client
                // takes the slot before the next push.
                self.scopes_freed = Some(Instant::now());
                self.scopes_released = true;
            }
            Some(Sub {
                stream: Stream::Vector { slot, .. },
                ..
            }) => {
                if let Some(s) = self.vector_slots.get_mut(*slot) {
                    *s = None;
                }
            }
            _ => {}
        }
        removed
    }
}

impl Host {
    pub(super) fn take_meters(&mut self) {
        for (meters, input) in [
            (std::mem::take(&mut self.meters), false),
            (std::mem::take(&mut self.inputs), true),
        ] {
            for m in &meters {
                for sub in self.subs.by_id.values_mut() {
                    if let Stream::Meters {
                        input: wants,
                        levels,
                        first,
                    } = &mut sub.stream
                        && *wants == input
                    {
                        if levels.len() <= m.channel {
                            levels.resize(m.channel + 1, Level::default());
                        }
                        if let Some(level) = levels.get_mut(m.channel) {
                            level.peak = level.peak.max(m.peak);
                            level.sum_squares += m.sum_squares;
                            level.frames += m.frames;
                        }
                        first.get_or_insert(m.first_frame.0);
                    }
                }
            }
        }
    }

    pub(super) fn take_blocks(&mut self) {
        for block in std::mem::take(&mut self.blocks) {
            let Some((sub, client)) = self.subs.of_scope_slot(block.slot) else {
                continue;
            };
            let values = block.values();
            let mut bytes = frame(
                frames::SCOPE,
                sub,
                block.first_frame.0,
                values.len() as u32,
                block.decimation as u32,
                values.len() * 4,
            );
            for v in values {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            self.send_binary(client, bytes);
        }
    }

    /// Vector frames (kind 4, M2.8-7): one per snapshot, its rows as `f32`.
    pub(super) fn take_snapshots(&mut self) {
        for snapshot in std::mem::take(&mut self.snapshots) {
            let Some((sub, client)) = self.subs.of_vector_slot(snapshot.slot) else {
                continue;
            };
            let values = snapshot.values();
            let mut bytes = frame(
                frames::VECTOR,
                sub,
                snapshot.frame.0,
                values.len() as u32,
                u32::try_from(snapshot.decimation).unwrap_or(u32::MAX),
                values.len() * 4,
            );
            for v in values {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            self.send_binary(client, bytes);
        }
    }

    /// The parameter and meter frames due.
    pub(super) fn emit(&mut self) {
        let now = Instant::now();
        let next_frame = self
            .run
            .as_ref()
            .map_or(0, |r| r.controller.status().next_frame.0);
        let mut out = Vec::new();
        for (&id, sub) in &mut self.subs.by_id {
            if now.duration_since(sub.last) < sub.every {
                continue;
            }
            match &mut sub.stream {
                Stream::Parameters { ports } => {
                    sub.last = now;
                    let mut bytes = frame(
                        frames::PARAMETERS,
                        id,
                        next_frame,
                        ports.len() as u32,
                        0,
                        ports.len() * 16,
                    );
                    for port in ports.iter() {
                        let value = port.as_ref().and_then(|p| self.values.get(p));
                        let (base, effective) =
                            value.map_or((f64::NAN, f64::NAN), |v| (v.base, v.effective));
                        bytes.extend_from_slice(&base.to_le_bytes());
                        bytes.extend_from_slice(&effective.to_le_bytes());
                    }
                    out.push((sub.client, bytes));
                }
                Stream::Meters { levels, first, .. } => {
                    let Some(start) = first.take() else {
                        continue;
                    };
                    sub.last = now;
                    let covered = levels.iter().map(|l| l.frames).max().unwrap_or(0);
                    let mut bytes = frame(
                        frames::METERS,
                        id,
                        start,
                        levels.len() as u32,
                        covered.min(u64::from(u32::MAX)) as u32,
                        levels.len() * 8,
                    );
                    for level in levels.iter_mut() {
                        let rms = if level.frames > 0 {
                            (level.sum_squares / level.frames as f64).sqrt() as f32
                        } else {
                            0.0
                        };
                        bytes.extend_from_slice(&level.peak.to_le_bytes());
                        bytes.extend_from_slice(&rms.to_le_bytes());
                        *level = Level::default();
                    }
                    out.push((sub.client, bytes));
                }
                Stream::Scope { .. } | Stream::Vector { .. } => {}
            }
        }
        for (client, bytes) in out {
            self.send_binary(client, bytes);
        }
    }

    /// The scope slots of a run, as `describe` and `scopes` give them:
    /// `free` now, and `sinceFreedMs` since the free count last rose.
    pub(super) fn scopes_json(&self) -> Value {
        json!({
            "slots": SCOPE_SLOTS,
            "free": self.subs.free_scopes(),
            "sinceFreedMs": self.subs.scopes_freed.map(|at| at.elapsed().as_millis()),
        })
    }

    /// Pushes `scopes` to every client whenever the free scope slot count
    /// changed since the last push, or a slot was released (`sinceFreedMs`
    /// restarted) though another was taken in the same step.
    pub(super) fn push_scopes(&mut self) {
        let free = self.subs.free_scopes();
        let released = std::mem::take(&mut self.subs.scopes_released);
        if self.subs.scopes_pushed == Some(free) && !released {
            return;
        }
        self.subs.scopes_pushed = Some(free);
        let message = with(self.scopes_json(), "type", json!("scopes"));
        self.broadcast(&message);
    }

    pub(super) fn subscribe(&mut self, client: u64, message: &Value) -> Result<Value, Failure> {
        if !self.powered() {
            return Err(powered_off());
        }
        let stream = message.get("stream").and_then(Value::as_str).unwrap_or("");
        let id = self.subs.next;
        let (stream, every, extra) = match stream {
            "parameters" => {
                let addresses: Vec<String> = message
                    .get("addresses")
                    .and_then(Value::as_array)
                    .ok_or_else(|| fail("bad-message", "parameters need `addresses`"))?
                    .iter()
                    .map(|a| a.as_str().unwrap_or("").to_owned())
                    .collect();
                let mut ports = Vec::new();
                for address in &addresses {
                    let aim = self.aim(address)?;
                    ports.push(aim.ports.into_iter().next());
                }
                (
                    Stream::Parameters { ports },
                    rate_interval(message)?,
                    json!({"addresses": addresses}),
                )
            }
            "meters" => {
                let input = match message.get("source").and_then(Value::as_str) {
                    None | Some("output") => false,
                    Some("input") => true,
                    Some(other) => {
                        return Err(fail("bad-message", format!("no meter source '{other}'")));
                    }
                };
                (
                    Stream::Meters {
                        input,
                        levels: Vec::new(),
                        first: None,
                    },
                    rate_interval(message)?,
                    json!({"source": if input { "input" } else { "output" }}),
                )
            }
            "scope" => {
                let address = message.get("port").and_then(Value::as_str).unwrap_or("");
                let port: PortAddress = self
                    .through_slot(address)?
                    .parse()
                    .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
                // A signal output, or a signal input (the sum of its sources).
                let is_signal = self.plan.as_ref().is_some_and(|plan| {
                    plan.instances
                        .iter()
                        .find(|i| i.address == port.instance())
                        .is_some_and(|i| {
                            i.descriptor
                                .ports
                                .iter()
                                .any(|p| p.name == port.port() && p.shape == PortShape::Signal)
                        })
                });
                if !is_signal {
                    return Err(fail(
                        "not-a-signal-output",
                        format!("{address} is not a signal output or input of the running plan"),
                    ));
                }
                let decimation = scope_decimation(message, self.rate)?;
                let slot = self
                    .subs
                    .scope_slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or_else(|| fail("no-scope-slot", "every scope slot is in use"))?;
                let run = self
                    .run
                    .as_mut()
                    .ok_or_else(|| fail("not-playing", "nothing plays"))?;
                run.controller
                    .scope_start(slot, port.clone(), decimation)
                    .map_err(|e| fail("busy", e.to_string()))?;
                if let Some(s) = self.subs.scope_slots.get_mut(slot) {
                    *s = Some(id);
                }
                (
                    Stream::Scope {
                        slot,
                        port,
                        decimation,
                        bound: false,
                    },
                    Duration::ZERO,
                    json!({"port": address, "decimation": decimation, "slot": slot}),
                )
            }
            "vector" => {
                let address = message.get("port").and_then(Value::as_str).unwrap_or("");
                let port: PortAddress = self
                    .through_slot(address)?
                    .parse()
                    .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
                // A vector output: its rows and rate divisor.
                let Some((rows, divisor)) = vector_shape(self.plan.as_ref(), &port) else {
                    return Err(fail(
                        "not-a-vector-output",
                        format!(
                            "{address} is not a vector output (of at most {VECTOR_MAX_ROWS} elements) of the running plan"
                        ),
                    ));
                };
                let rate = rate_of(message, 60.0)?;
                let decimation = vector_decimation(rate, self.rate, divisor);
                let slot = self
                    .subs
                    .vector_slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or_else(|| fail("no-vector-slot", "every vector slot is in use"))?;
                let (Some(plan), Some(run)) = (self.plan.as_ref(), self.run.as_mut()) else {
                    return Err(fail("not-playing", "nothing plays"));
                };
                run.controller
                    .vector_start(plan, slot, port.clone(), rows, decimation)
                    .map_err(|e| fail("busy", e.to_string()))?;
                if let Some(s) = self.subs.vector_slots.get_mut(slot) {
                    *s = Some(id);
                }
                (
                    Stream::Vector {
                        slot,
                        port,
                        rows,
                        rate,
                        decimation,
                        bound: false,
                    },
                    Duration::ZERO,
                    json!({"port": address, "rows": rows, "decimation": decimation, "slot": slot}),
                )
            }
            other => return Err(fail("bad-message", format!("no stream '{other}'"))),
        };
        self.subs.next = self.subs.next.wrapping_add(1).max(1);
        let kind = match &stream {
            Stream::Parameters { .. } => "parameters",
            Stream::Meters { .. } => "meters",
            Stream::Scope { .. } => "scope",
            Stream::Vector { .. } => "vector",
        };
        self.subs.by_id.insert(
            id,
            Sub {
                client,
                every,
                last: Instant::now(),
                stream,
            },
        );
        let reply = json!({"type": "subscribed", "sub": id, "stream": kind});
        let reply = match (reply, extra) {
            (Value::Object(mut a), Value::Object(b)) => {
                a.extend(b);
                Value::Object(a)
            }
            (reply, _) => reply,
        };
        Ok(reply)
    }

    pub(super) fn unsubscribe(&mut self, client: u64, message: &Value) -> Result<Value, Failure> {
        let sub = message
            .get("sub")
            .and_then(Value::as_u64)
            .and_then(|s| u32::try_from(s).ok())
            .ok_or_else(|| fail("bad-message", "unsubscribe needs `sub`"))?;
        match self.subs.by_id.get(&sub) {
            Some(s) if s.client == client => {}
            _ => return Err(fail("unknown-sub", format!("no subscription {sub}"))),
        }
        self.drop_sub(sub);
        Ok(json!({"type": "unsubscribed", "sub": sub}))
    }

    pub(super) fn drop_sub(&mut self, sub: u32) {
        match self.subs.remove(sub) {
            Some(Sub {
                stream: Stream::Scope { slot, .. },
                ..
            }) => {
                if let Some(run) = self.run.as_mut() {
                    let _ = run.controller.scope_stop(slot);
                }
            }
            Some(Sub {
                stream: Stream::Vector { slot, .. },
                ..
            }) => {
                if let Some(run) = self.run.as_mut() {
                    let _ = run.controller.vector_stop(slot);
                }
            }
            _ => {}
        }
    }

    /// Vector subscriptions whose port the plan now has with another number
    /// of elements or another rate divisor (a swap replaced its type) take
    /// the new shape, and their client is told (`vector.reshaped`), so a
    /// stream never carries another port's rows (the adapter refuses to
    /// bind a slot whose width differs from the port's). With `restart`,
    /// a reshaped slot is started again on the running Controller.
    pub(super) fn reshape_vectors(&mut self, restart: bool) {
        let plan = self.plan.as_ref();
        let mut told = Vec::new();
        for (&sub, s) in &mut self.subs.by_id {
            let Stream::Vector {
                slot,
                port,
                rows,
                rate,
                decimation,
                ..
            } = &mut s.stream
            else {
                continue;
            };
            let Some((n, divisor)) = vector_shape(plan, port) else {
                continue;
            };
            let d = vector_decimation(*rate, self.rate, divisor);
            if (n, d) == (*rows, *decimation) {
                continue;
            }
            (*rows, *decimation) = (n, d);
            if restart && let (Some(plan), Some(run)) = (plan, self.run.as_mut()) {
                let _ = run.controller.vector_start(plan, *slot, port.clone(), n, d);
            }
            told.push((
                s.client,
                json!({"type": "vector.reshaped", "sub": sub, "rows": n, "decimation": d}),
            ));
        }
        for (client, message) in told {
            self.send_text(client, &message);
        }
    }

    /// The scope and vector subscriptions, on a new run's Controller.
    pub(super) fn rearm(&mut self) {
        self.reshape_vectors(false);
        let (Some(plan), Some(run)) = (self.plan.as_ref(), self.run.as_mut()) else {
            return;
        };
        for sub in self.subs.by_id.values_mut() {
            if let Stream::Scope {
                slot,
                port,
                decimation,
                bound,
            } = &mut sub.stream
            {
                *bound = false;
                let _ = run.controller.scope_start(*slot, port.clone(), *decimation);
            }
            if let Stream::Vector {
                slot,
                port,
                rows,
                decimation,
                bound,
                ..
            } = &mut sub.stream
            {
                *bound = false;
                let _ = run
                    .controller
                    .vector_start(plan, *slot, port.clone(), *rows, *decimation);
            }
        }
    }

    pub(super) fn release(&mut self, client: u64) {
        for sub in self.subs.of_client(client) {
            self.drop_sub(sub);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::within;
    use serde_json::json;

    /// auditor-1-rate-panic: no client rate panics; the unsupported ones are
    /// refused with `bad-rate`.
    #[test]
    fn rates_are_checked_without_panicking() {
        fn refused<T>(result: Result<T, Failure>) -> bool {
            matches!(
                result,
                Err(Failure {
                    code: "bad-rate",
                    ..
                })
            )
        }
        for rate in [
            json!(1e-20),
            json!(1e-320),
            json!(0),
            json!(-1),
            json!(0.5),
            json!(f64::NAN),
            json!("NaN"),
            json!(null),
        ] {
            let message = json!({"rate": rate});
            assert!(refused(rate_interval(&message)), "{message}");
            assert!(refused(scope_decimation(&message, 48_000)), "{message}");
        }
        let huge = json!({"rate": 1e300});
        assert_eq!(
            rate_interval(&huge).ok(),
            Some(Duration::from_secs_f64(1.0 / 60.0))
        );
        assert_eq!(scope_decimation(&huge, 48_000).ok(), Some(1));
        let slow = json!({"rate": 1});
        assert_eq!(rate_interval(&slow).ok(), Some(Duration::from_secs(1)));
        assert_eq!(scope_decimation(&slow, 48_000).ok(), Some(48_000));
        assert_eq!(
            rate_interval(&json!({})).ok(),
            Some(Duration::from_secs_f64(1.0 / 30.0))
        );
        assert_eq!(scope_decimation(&json!({}), 48_000).ok(), Some(1));
        // A range whose ends cross (an export's targets) clamps, never panics.
        assert_eq!(within(5.0, 3.0, 1.0), 1.0);
        assert_eq!(within(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(
            scope_decimation(&json!({"rate": 4800}), 48_000).ok(),
            Some(10)
        );
    }
}

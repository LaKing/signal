//! M2.7-2: deterministic placement of GPU-backed instances on the pool's
//! devices, lent through the engine's [`Host`] hook.
//!
//! **The rule.** The devices are the pool's, sorted by id and fixed when it
//! opened (a lost device keeps its place). An instance pinned in the project
//! (`gpu = "<id or index>"`) runs on that device. Every other GPU-backed
//! instance, in address order, takes device *r* mod *n*, where *r* is its
//! rank among the plan's instances placed this way and *n* the number of
//! devices. A pin that names no device of the pool is reported and the
//! instance is placed by the rule. An address keeps the device it was first
//! given for the rest of the run, so a live edit never moves a survivor and
//! the engine's second preparation of a plan (its real-time mirror) gets the
//! same answer. Without devices nothing is lent: the instance has no channel
//! and its due results are lost, logged.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use signal_core::{Host, InstanceAddress, Lent};

use crate::DevicePool;

/// Where an instance runs, as the render manifest records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The instance's address.
    pub instance: String,
    /// The device's id; `None` without a device.
    pub device: Option<String>,
    /// The project's pin, as written, if any.
    pub pinned: Option<String>,
}

#[derive(Debug, Default)]
struct State {
    /// The device each address was first given.
    assigned: BTreeMap<String, Option<usize>>,
    /// Every warning made, so each is reported once.
    seen: BTreeSet<String>,
    /// Warnings not yet taken.
    pending: Vec<String>,
}

/// Lends GPU-backed instances their device (see the module documentation).
/// Set on the registry the engine is built with
/// ([`Registry::set_host`](signal_core::Registry::set_host)); keep it (and so
/// the pool) until the engine is dropped, so the pool is never dropped on the
/// audio thread.
#[derive(Debug)]
pub struct GpuHost {
    pool: Arc<DevicePool>,
    types: Vec<&'static str>,
    pins: BTreeMap<String, String>,
    state: Mutex<State>,
}

impl GpuHost {
    /// Places instances of `types` on `pool`'s devices, with the project's
    /// `pins` (address → device id or index).
    pub fn new(
        pool: Arc<DevicePool>,
        types: &[&'static str],
        pins: BTreeMap<String, String>,
    ) -> GpuHost {
        GpuHost {
            pool,
            types: types.to_vec(),
            pins,
            state: Mutex::new(State::default()),
        }
    }

    /// The pool.
    pub fn pool(&self) -> &Arc<DevicePool> {
        &self.pool
    }

    /// The warnings made since the last call, each once.
    pub fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut self.state().pending)
    }

    /// Every GPU-backed instance placed so far and its device, by address.
    pub fn placement(&self) -> Vec<Placed> {
        let devices = self.pool.devices();
        self.state()
            .assigned
            .iter()
            .map(|(instance, device)| Placed {
                instance: instance.clone(),
                device: device.and_then(|d| devices.get(d)).map(|d| d.id.clone()),
                pinned: self.pins.get(instance).cloned(),
            })
            .collect()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The device `pin` names: an index into the pool's devices, or an id.
    fn resolve(&self, pin: &str) -> Option<usize> {
        let devices = self.pool.devices();
        match pin.parse::<usize>() {
            Ok(index) => (index < devices.len()).then_some(index),
            Err(_) => devices.iter().position(|d| d.id == pin),
        }
    }
}

fn warn(state: &mut State, warning: String) {
    if state.seen.insert(warning.clone()) {
        state.pending.push(warning);
    }
}

impl Host for GpuHost {
    fn lend(&self, instances: &[(&InstanceAddress, &'static str)]) -> Vec<Option<Lent>> {
        let n = self.pool.devices().len();
        let ids: Vec<&str> = self.pool.devices().iter().map(|d| d.id.as_str()).collect();
        let mut state = self.state();
        let mut rank = 0;
        let mut lent: Vec<Option<Lent>> = vec![None; instances.len()];
        // By address, whatever order the instances are given in.
        let mut order: Vec<usize> = (0..instances.len()).collect();
        order.sort_by(|&a, &b| {
            let address = |i: usize| instances.get(i).map(|(address, _)| *address);
            address(a).cmp(&address(b))
        });
        for i in order {
            let Some(&(address, type_name)) = instances.get(i) else {
                continue;
            };
            let key = address.to_string();
            let pin = self.pins.get(&key);
            if !self.types.contains(&type_name) {
                if pin.is_some() {
                    warn(
                        &mut state,
                        format!("{key}: `gpu` is set, but `{type_name}` uses no GPU; ignored"),
                    );
                }
                continue;
            }
            let pinned = pin.and_then(|pin| self.resolve(pin));
            if let (Some(pin), None) = (pin, pinned) {
                warn(
                    &mut state,
                    format!(
                        "{key}: gpu = \"{pin}\" names no device of this machine's pool ({}); placed by the rule",
                        if ids.is_empty() {
                            "none".to_owned()
                        } else {
                            ids.join(", ")
                        }
                    ),
                );
            }
            let chosen = match pinned {
                Some(device) => Some(device),
                None => {
                    let device = (n > 0).then(|| rank % n);
                    rank += 1;
                    device
                }
            };
            let device = *state.assigned.entry(key).or_insert(chosen);
            if let Some(slot) = lent.get_mut(i) {
                *slot = device.map(|d| Arc::new(DevicePool::lease(&self.pool, d)) as Lent);
            }
        }
        lent
    }
}

//! The sound system's routing (ADR 0038 §4, §8): which device channel each
//! play side of a project plays on. Output policy (ADR 0022): the engine
//! hands every sink's inputs to the driver generically; which audio sinks
//! are heard, and where, is decided here, on the control thread, before a
//! run starts. The result is a fixed channel table the callback only reads.
//!
//! - **A play side** is an audio sink of the plan with its **role** (ADR
//!   0038 §1): `std.audio_out` is `main`; a device slot names its own. The
//!   project names roles, never hardware.
//! - **A role binding** is the machine's: the device channels (from 1) a
//!   role plays on, from the Setup (`[play.<role>]`, `output_channels` for
//!   `main`) or `devices.local.toml`. Its device is the clock device unless
//!   it names another.
//! - **Canonical order** is role name, then slot address. The run's audio
//!   frame is every play side's ports concatenated in that order, so the
//!   order never depends on the machine.
//! - **Unknown roles are not heard; they never fall back to `main`** (ADR
//!   0038 §4). One exception keeps "open and hear it at once": a project
//!   with no `main` side plays its first side in canonical order as main.
//! - **Two roles on one device channel are never summed:** the later in
//!   canonical order is refused for that channel and stays silent there.
//! - **One clock in v1** (ADR 0038 §8): a role bound to another device than
//!   the clock device is accepted as text but not heard. [`Placement`]
//!   keeps the device per role, so a per-device clock adapter (a later ADR)
//!   can take those roles instead of refusing them.
//! - **Layouts are not converted yet** (ADR 0038 §5, phase 2): a side
//!   whose width differs from its binding's channel count plays by
//!   position; the rest is reported.

use std::collections::{BTreeMap, BTreeSet};

use signal_core::InstanceAddress;

/// The default role (ADR 0038 §1): `std.audio_out`'s, and a device slot's
/// without `role`.
pub const MAIN: &str = "main";

/// An audio sink of the plan with its role: what the project plays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaySide {
    /// The sink instance.
    pub address: InstanceAddress,
    /// Its role (`main`, `cue`, …).
    pub role: String,
    /// Its signal inputs (its channels, in port order).
    pub width: usize,
}

/// Where a role's device is, as the machine found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The clock device: the device the run plays on.
    Clock,
    /// Another present device: another clock (ADR 0038 §8), not heard in
    /// v1. Its name, for the notice.
    Other(String),
    /// A device the binding names that is not present: its name and why.
    Missing(String, String),
}

/// The machine's binding of one role: its device and channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleBinding {
    /// Its device.
    pub placement: Placement,
    /// Its device channels, from 1, in the side's port order.
    pub channels: Vec<u16>,
}

impl RoleBinding {
    /// A binding on the clock device.
    pub fn clock(channels: Vec<u16>) -> RoleBinding {
        RoleBinding {
            placement: Placement::Clock,
            channels,
        }
    }
}

/// The machine's sound system: the binding of each role it describes. A
/// `main` it does not describe plays on the clock device's channels 1 to
/// its width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundSystem {
    /// By role.
    pub roles: BTreeMap<String, RoleBinding>,
}

/// Whether a play side is heard on this machine, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// On the clock device (some channels may still be refused).
    Clock,
    /// The machine describes no such role.
    Unrouted,
    /// Bound to another device: another clock (ADR 0038 §8).
    OtherClock(String),
    /// Bound to a device that is not present.
    Missing(String),
}

/// One play side as routed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedSide {
    /// The sink instance.
    pub address: InstanceAddress,
    /// Its role.
    pub role: String,
    /// The role whose binding it plays on: its own, or `main` for the first
    /// side of a project without `main`.
    pub played_as: String,
    /// Its channels.
    pub width: usize,
    /// Per port, the device channel (from 1) it plays on; `None`: not heard.
    pub channels: Vec<Option<u16>>,
    /// Whether it is heard.
    pub heard: Heard,
}

/// Every play side routed, in canonical order, and what to tell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routing {
    /// In canonical order: the order of the run's audio frame.
    pub sides: Vec<RoutedSide>,
    /// What the person is told (one line each).
    pub notices: Vec<String>,
}

impl Routing {
    /// The run's audio frame: every side's channels.
    pub fn width(&self) -> usize {
        self.sides.iter().map(|s| s.width).sum()
    }

    /// The sinks, in the frame's order.
    pub fn addresses(&self) -> Vec<InstanceAddress> {
        self.sides.iter().map(|s| s.address.clone()).collect()
    }

    /// Per channel of the run's audio frame, the device channel (from 0) it
    /// plays on, if any.
    pub fn targets(&self) -> Vec<Option<usize>> {
        self.sides
            .iter()
            .flat_map(|s| s.channels.iter())
            .map(|c| c.map(|c| usize::from(c).saturating_sub(1)))
            .collect()
    }

    /// The device channels the routing needs: up to its highest target, at
    /// least 1 (the width to choose the device's configuration for).
    pub fn device_width(&self) -> usize {
        self.targets()
            .into_iter()
            .flatten()
            .map(|t| t + 1)
            .max()
            .unwrap_or(1)
    }

    /// The side at `address`.
    pub fn side(&self, address: &InstanceAddress) -> Option<&RoutedSide> {
        self.sides.iter().find(|s| s.address == *address)
    }
}

/// Routes `sides` on the machine's `system` (ADR 0038 §4, §8), the clock
/// device having `device_channels` channels (`None`: not known, nothing is
/// refused for its range). See the module's rules.
pub fn route(sides: &[PlaySide], system: &SoundSystem, device_channels: Option<usize>) -> Routing {
    let mut ordered: Vec<&PlaySide> = sides.iter().collect();
    ordered.sort_by(|a, b| (&a.role, &a.address).cmp(&(&b.role, &b.address)));
    let has_main = ordered.iter().any(|s| s.role == MAIN);
    let mut notices = Vec::new();
    let mut used: BTreeMap<u16, String> = BTreeMap::new();
    let mut roles_seen: BTreeSet<&str> = BTreeSet::new();
    let mut routed = Vec::with_capacity(ordered.len());
    for (k, side) in ordered.iter().enumerate() {
        let who = format!("{} ({})", side.role, side.address);
        if !roles_seen.insert(side.role.as_str()) {
            // The plan refuses this (two versions of a role are not built);
            // routing never sums them either.
            notices.push(format!(
                "{who}: another output already plays role `{}`; not heard",
                side.role
            ));
            routed.push(unheard(side, Heard::Unrouted));
            continue;
        }
        let own = system.roles.get(&side.role);
        let played_as = if !has_main && k == 0 && own.is_none() {
            notices.push(format!(
                "{who}: the project has no `main` output; `{}` plays as main",
                side.role
            ));
            MAIN.to_owned()
        } else {
            side.role.clone()
        };
        let binding = match system.roles.get(&played_as) {
            Some(binding) => binding.clone(),
            None if played_as == MAIN => RoleBinding::clock(
                (1..=side.width)
                    .filter_map(|c| u16::try_from(c).ok())
                    .collect(),
            ),
            None => {
                notices.push(format!(
                    "{who}: not heard on this machine (nothing binds it: [play.{}] in the Setup)",
                    side.role
                ));
                routed.push(unheard(side, Heard::Unrouted));
                continue;
            }
        };
        match &binding.placement {
            Placement::Clock => {}
            Placement::Other(name) => {
                notices.push(format!(
                    "{who}: bound to {name}, another clock; not heard (make an aggregate device in \
                     Audio MIDI Setup with drift correction, or wait for clock adapters)"
                ));
                routed.push(unheard(side, Heard::OtherClock(name.clone())));
                continue;
            }
            Placement::Missing(name, reason) => {
                notices.push(format!(
                    "{who}: {name} is not present ({reason}); not heard"
                ));
                routed.push(unheard(side, Heard::Missing(name.clone())));
                continue;
            }
        }
        if binding.channels.len() != side.width {
            notices.push(format!(
                "{who}: {} channel{} on {} device channel{}; played by position (layouts are not \
                 converted yet)",
                side.width,
                plural(side.width),
                binding.channels.len(),
                plural(binding.channels.len())
            ));
        }
        let mut channels = Vec::with_capacity(side.width);
        for port in 0..side.width {
            let Some(&channel) = binding.channels.get(port) else {
                channels.push(None);
                continue;
            };
            if channel == 0 || device_channels.is_some_and(|n| usize::from(channel) > n) {
                notices.push(format!(
                    "{who}: device channel {channel} does not exist{}; not heard there",
                    device_channels.map_or(String::new(), |n| format!(" (the device has {n})"))
                ));
                channels.push(None);
                continue;
            }
            if channels.contains(&Some(channel)) {
                notices.push(format!(
                    "{who}: device channel {channel} is listed twice; not heard there again"
                ));
                channels.push(None);
                continue;
            }
            if let Some(owner) = used.get(&channel) {
                notices.push(format!(
                    "{who}: device channel {channel} already plays `{owner}`; two roles on one \
                     channel are never summed, so it is not heard there"
                ));
                channels.push(None);
                continue;
            }
            used.insert(channel, side.role.clone());
            channels.push(Some(channel));
        }
        routed.push(RoutedSide {
            address: side.address.clone(),
            role: side.role.clone(),
            played_as,
            width: side.width,
            channels,
            heard: Heard::Clock,
        });
    }
    Routing {
        sides: routed,
        notices,
    }
}

fn unheard(side: &PlaySide, heard: Heard) -> RoutedSide {
    RoutedSide {
        address: side.address.clone(),
        role: side.role.clone(),
        played_as: side.role.clone(),
        width: side.width,
        channels: vec![None; side.width],
        heard,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(address: &str, role: &str, width: usize) -> PlaySide {
        PlaySide {
            address: address.parse().unwrap(),
            role: role.to_owned(),
            width,
        }
    }

    fn system(roles: &[(&str, RoleBinding)]) -> SoundSystem {
        SoundSystem {
            roles: roles
                .iter()
                .map(|(r, b)| ((*r).to_owned(), b.clone()))
                .collect(),
        }
    }

    /// The owner's case: `main` on the SSL 2+'s Out 1–2, `cue` on Out 3–4.
    #[test]
    fn main_and_cue_play_on_every_channel_of_the_clock_device() {
        let sides = [side("/out", MAIN, 2), side("/cue", "cue", 2)];
        let system = system(&[("cue", RoleBinding::clock(vec![3, 4]))]);
        let routing = route(&sides, &system, Some(4));
        assert_eq!(routing.notices, Vec::<String>::new());
        // Canonical order: role name, so `cue` before `main`.
        let roles: Vec<&str> = routing.sides.iter().map(|s| s.role.as_str()).collect();
        assert_eq!(roles, ["cue", "main"]);
        assert_eq!(routing.targets(), [Some(2), Some(3), Some(0), Some(1)]);
        assert_eq!(routing.width(), 4);
        assert_eq!(routing.device_width(), 4);
    }

    /// One `std.audio_out` and no Setup: today's identity map.
    #[test]
    fn a_lone_main_plays_on_channels_1_to_its_width() {
        let routing = route(&[side("/out", MAIN, 2)], &SoundSystem::default(), Some(2));
        assert_eq!(routing.targets(), [Some(0), Some(1)]);
        assert!(routing.notices.is_empty());
    }

    #[test]
    fn an_unknown_role_is_not_heard_and_never_falls_back_to_main() {
        let sides = [side("/out", MAIN, 2), side("/click", "click", 1)];
        let routing = route(&sides, &SoundSystem::default(), Some(4));
        assert_eq!(routing.targets(), [None, Some(0), Some(1)]);
        assert_eq!(routing.sides[0].heard, Heard::Unrouted);
        assert_eq!(
            routing.notices,
            [
                "click (/click): not heard on this machine (nothing binds it: [play.click] in the Setup)"
            ]
        );
    }

    #[test]
    fn a_project_without_main_plays_its_first_role_as_main() {
        let sides = [side("/b", "stems", 2), side("/a", "monitor", 2)];
        let routing = route(&sides, &SoundSystem::default(), Some(2));
        assert_eq!(routing.sides[0].role, "monitor");
        assert_eq!(routing.sides[0].played_as, MAIN);
        assert_eq!(routing.targets(), [Some(0), Some(1), None, None]);
        assert_eq!(routing.notices.len(), 2);
    }

    /// ADR 0038 §4: the later in canonical order is refused there.
    #[test]
    fn two_roles_on_one_channel_are_never_summed() {
        let sides = [side("/out", MAIN, 2), side("/mon", "monitor", 2)];
        let system = system(&[("monitor", RoleBinding::clock(vec![2, 3]))]);
        let routing = route(&sides, &system, Some(4));
        // `main` < `monitor`: main takes 1 and 2, monitor keeps only 3.
        assert_eq!(routing.targets(), [Some(0), Some(1), None, Some(2)]);
        assert_eq!(
            routing.notices,
            [
                "monitor (/mon): device channel 2 already plays `main`; two roles on one channel are never summed, so it is not heard there"
            ]
        );
    }

    #[test]
    fn another_clock_and_a_missing_device_are_not_heard() {
        let sides = [
            side("/out", MAIN, 2),
            side("/a", "alt", 2),
            side("/z", "zone", 2),
        ];
        let system = system(&[
            (
                "alt",
                RoleBinding {
                    placement: Placement::Other("Fireface".to_owned()),
                    channels: vec![1, 2],
                },
            ),
            (
                "zone",
                RoleBinding {
                    placement: Placement::Missing("BlackHole".to_owned(), "not found".to_owned()),
                    channels: vec![1, 2],
                },
            ),
        ]);
        let routing = route(&sides, &system, Some(2));
        assert_eq!(
            routing.sides[0].heard,
            Heard::OtherClock("Fireface".to_owned())
        );
        assert_eq!(
            routing.sides[2].heard,
            Heard::Missing("BlackHole".to_owned())
        );
        assert_eq!(
            routing.targets(),
            [None, None, Some(0), Some(1), None, None]
        );
    }

    #[test]
    fn channels_out_of_range_or_of_another_width_are_reported() {
        let sides = [side("/out", MAIN, 2), side("/cue", "cue", 2)];
        let system = system(&[("cue", RoleBinding::clock(vec![5]))]);
        let routing = route(&sides, &system, Some(4));
        assert_eq!(routing.targets(), [None, None, Some(0), Some(1)]);
        assert_eq!(routing.notices.len(), 2);
        assert!(routing.notices[0].contains("played by position"));
        assert!(routing.notices[1].contains("device channel 5 does not exist (the device has 4)"));
    }

    /// The same project and sound system route the same whatever order the
    /// plan lists its sinks in.
    #[test]
    fn the_order_is_canonical() {
        let a = [side("/out", MAIN, 2), side("/cue", "cue", 2)];
        let b = [side("/cue", "cue", 2), side("/out", MAIN, 2)];
        let system = system(&[("cue", RoleBinding::clock(vec![3, 4]))]);
        assert_eq!(route(&a, &system, None), route(&b, &system, None));
    }
}

//! Telling same-named devices apart: labels for a picker, and finding a
//! saved device again. A saved device with an id is that id and nothing
//! else; without one, it is found only when its name (and then its
//! signature) leaves exactly one candidate. Nothing is chosen by position.

use crate::config::{DeviceInfo, Direction};

/// A device as a Setup remembers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRef {
    /// Its name.
    pub name: String,
    /// Its stable identifier ([`DeviceInfo::id`]), if it had one.
    pub id: Option<String>,
    /// Its [`signature`] when it was chosen.
    pub signature: String,
}

impl DeviceRef {
    /// The reference that finds `device` again.
    pub fn of(device: &DeviceInfo) -> DeviceRef {
        DeviceRef {
            name: device.name.clone(),
            id: device.id.clone(),
            signature: signature(device),
        }
    }
}

/// Where a [`DeviceRef`] is in a device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly this device (an index into the list).
    Found(usize),
    /// Several devices fit; none is chosen.
    Ambiguous {
        /// The devices that fit (indices into the list).
        candidates: Vec<usize>,
        /// Why, for people.
        reason: String,
    },
    /// No device fits.
    Missing {
        /// Why, for people.
        reason: String,
        /// The devices of the same name that are present (indices), which
        /// are not the saved one or cannot be told to be.
        same_name: Vec<usize>,
    },
}

/// The channels and rates of a device: `2 ch, 44100-96000 Hz`, from its
/// widest configuration and every rate range it reports.
pub fn signature(device: &DeviceInfo) -> String {
    let channels = device.configs.iter().map(|c| c.channels).max().unwrap_or(0);
    let mut rates: Vec<(u32, u32)> = device
        .configs
        .iter()
        .map(|c| (c.min_rate, c.max_rate))
        .collect();
    rates.sort_unstable();
    rates.dedup();
    let rates: Vec<String> = rates
        .iter()
        .map(|&(min, max)| {
            if min == max {
                min.to_string()
            } else {
                format!("{min}-{max}")
            }
        })
        .collect();
    format!("{channels} ch, {} Hz", rates.join(", "))
}

/// A label for each device of `direction` in `list`, as (index, label): its
/// name, or for a name several devices share, the name with the device's
/// place among them and its signature, `DELL U3219Q (#2 · 2 ch, 48000 Hz)`.
pub fn labels(list: &[DeviceInfo], direction: Direction) -> Vec<(usize, String)> {
    let of_direction: Vec<(usize, &DeviceInfo)> = list
        .iter()
        .enumerate()
        .filter(|(_, d)| d.direction == direction)
        .collect();
    of_direction
        .iter()
        .map(|&(index, device)| {
            let same: Vec<usize> = of_direction
                .iter()
                .filter(|(_, d)| d.name == device.name)
                .map(|&(i, _)| i)
                .collect();
            let label = if same.len() > 1 {
                let place = same.iter().position(|&i| i == index).unwrap_or(0) + 1;
                format!("{} (#{place} · {})", device.name, signature(device))
            } else {
                device.name.clone()
            };
            (index, label)
        })
        .collect()
}

/// Finds `wanted` among the devices of `direction` in `list`.
pub fn resolve(list: &[DeviceInfo], direction: Direction, wanted: &DeviceRef) -> Resolution {
    let kind = match direction {
        Direction::Output => "output",
        Direction::Input => "input",
    };
    let same_name: Vec<usize> = list
        .iter()
        .enumerate()
        .filter(|(_, d)| d.direction == direction && d.name == wanted.name)
        .map(|(i, _)| i)
        .collect();
    if let Some(id) = &wanted.id {
        let with_id: Vec<usize> = list
            .iter()
            .enumerate()
            .filter(|(_, d)| d.direction == direction && d.id.as_ref() == Some(id))
            .map(|(i, _)| i)
            .collect();
        return match with_id.as_slice() {
            [index] => Resolution::Found(*index),
            [] => Resolution::Missing {
                reason: format!(
                    "the saved {kind} '{}' (id {id}) is not present; choose a device",
                    wanted.name
                ),
                same_name,
            },
            _ => Resolution::Ambiguous {
                reason: format!(
                    "{} {kind} devices report the saved id {id}; choose a device",
                    with_id.len()
                ),
                candidates: with_id,
            },
        };
    }
    let candidates = if same_name.len() > 1 {
        same_name
            .iter()
            .copied()
            .filter(|&i| {
                list.get(i)
                    .is_some_and(|d| signature(d) == wanted.signature)
            })
            .collect()
    } else {
        same_name.clone()
    };
    match candidates.as_slice() {
        [index] => Resolution::Found(*index),
        [] if same_name.is_empty() => Resolution::Missing {
            reason: format!(
                "the saved {kind} '{}' is not present; choose a device",
                wanted.name
            ),
            same_name,
        },
        _ => Resolution::Ambiguous {
            reason: format!(
                "{} {kind} devices are named '{}' and the Setup has no id to tell them apart; choose a device",
                same_name.len(),
                wanted.name
            ),
            candidates: if candidates.is_empty() {
                same_name
            } else {
                candidates
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigRange;

    fn device(name: &str, id: &str, channels: u16, rate: u32) -> DeviceInfo {
        DeviceInfo {
            name: name.to_owned(),
            id: Some(id.to_owned()),
            direction: Direction::Output,
            default: false,
            configs: vec![ConfigRange {
                channels,
                min_rate: rate,
                max_rate: rate,
                buffer: Some((14, 4096)),
                sample_format: "f32".to_owned(),
            }],
        }
    }

    const DELL: &str = "DELL U3219Q";

    /// A and B: same name and signature; C: same name, another signature; D.
    fn list() -> Vec<DeviceInfo> {
        vec![
            device(DELL, "coreaudio:a", 2, 48_000),
            device(DELL, "coreaudio:b", 2, 48_000),
            device(DELL, "coreaudio:c", 8, 48_000),
            device("SSL 2+ MKII", "coreaudio:d", 4, 96_000),
        ]
    }

    fn saved(index: usize) -> DeviceRef {
        DeviceRef::of(&list()[index])
    }

    #[test]
    fn same_named_devices_get_distinct_labels() {
        let output = labels(&list(), Direction::Output);
        let text: Vec<&str> = output.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            text,
            [
                "DELL U3219Q (#1 · 2 ch, 48000 Hz)",
                "DELL U3219Q (#2 · 2 ch, 48000 Hz)",
                "DELL U3219Q (#3 · 8 ch, 48000 Hz)",
                "SSL 2+ MKII",
            ]
        );
        assert!(labels(&list(), Direction::Input).is_empty());
    }

    #[test]
    fn a_saved_id_finds_that_device() {
        let list = list();
        assert_eq!(
            resolve(&list, Direction::Output, &saved(1)),
            Resolution::Found(1)
        );
        assert_eq!(
            resolve(&list, Direction::Output, &saved(0)),
            Resolution::Found(0)
        );
    }

    #[test]
    fn a_disconnected_device_is_missing_and_its_twin_is_not_chosen() {
        let a = saved(0);
        let mut list = list();
        list.remove(0); // A is gone; B (same name and signature) is now index 0.
        assert_eq!(
            resolve(&list, Direction::Output, &a),
            Resolution::Missing {
                reason: "the saved output 'DELL U3219Q' (id coreaudio:a) is not present; choose a device"
                    .to_owned(),
                same_name: vec![0, 1],
            }
        );
    }

    #[test]
    fn without_an_id_same_named_devices_are_ambiguous_unless_the_signature_decides() {
        let list = list();
        let without_id = |index| DeviceRef {
            id: None,
            ..saved(index)
        };
        assert!(matches!(
            resolve(&list, Direction::Output, &without_id(0)),
            Resolution::Ambiguous { candidates, .. } if candidates == [0, 1]
        ));
        assert_eq!(
            resolve(&list, Direction::Output, &without_id(2)),
            Resolution::Found(2)
        );
        // A unique name needs no id.
        assert_eq!(
            resolve(&list, Direction::Output, &without_id(3)),
            Resolution::Found(3)
        );
    }

    #[test]
    fn a_duplicated_id_is_ambiguous() {
        let mut list = list();
        list[1].id = Some("coreaudio:a".to_owned());
        assert!(matches!(
            resolve(&list, Direction::Output, &saved(0)),
            Resolution::Ambiguous { candidates, .. } if candidates == [0, 1]
        ));
    }

    #[test]
    fn an_unknown_name_is_missing() {
        let wanted = DeviceRef {
            name: "Gone".to_owned(),
            id: None,
            signature: String::new(),
        };
        assert!(matches!(
            resolve(&list(), Direction::Output, &wanted),
            Resolution::Missing { same_name, .. } if same_name.is_empty()
        ));
    }
}

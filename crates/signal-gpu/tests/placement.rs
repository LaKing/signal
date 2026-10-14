//! AC1a of M2.7-2: placement on 3 noop devices is deterministic and is the
//! documented rule (`GpuHost`): pinned instances on their device, the others
//! round-robin over the devices sorted by id, by instance address.

use std::collections::BTreeMap;
use std::sync::Arc;

use signal_core::{Host, InstanceAddress, Lent};
use signal_gpu::{DevicePool, GpuHost, Lease, Placed};

const GPU: &str = "t.gpu";
const CPU: &str = "t.cpu";

fn addresses(names: &[&str]) -> Vec<InstanceAddress> {
    names.iter().map(|n| n.parse().unwrap()).collect()
}

/// Lends `instances` (address, type) and reads back each device index.
fn lend(host: &GpuHost, instances: &[(&InstanceAddress, &'static str)]) -> Vec<Option<usize>> {
    host.lend(instances)
        .iter()
        .map(|lent: &Option<Lent>| {
            lent.as_ref()
                .map(|l| l.downcast_ref::<Lease>().unwrap().device())
        })
        .collect()
}

fn host(pins: &[(&str, &str)]) -> GpuHost {
    let pool = Arc::new(DevicePool::noop(3).unwrap());
    let ids: Vec<&str> = pool.devices().iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["noop-0", "noop-1", "noop-2"], "sorted by id");
    let pins: BTreeMap<String, String> = pins
        .iter()
        .map(|(a, d)| ((*a).to_owned(), (*d).to_owned()))
        .collect();
    GpuHost::new(pool, &[GPU], pins)
}

#[test]
fn round_robin_by_address_over_devices_sorted_by_id() {
    let names = addresses(&["/a", "/b", "/c", "/d", "/e", "/f", "/x"]);
    let types = [GPU, GPU, GPU, GPU, GPU, GPU, CPU];
    let instances: Vec<_> = names.iter().zip(types).collect();
    let expected = vec![Some(0), Some(1), Some(2), Some(0), Some(1), Some(2), None];
    for run in 0..3 {
        let host = host(&[]);
        assert_eq!(lend(&host, &instances), expected, "run {run}");
        assert!(host.take_warnings().is_empty());
    }
    // The order the instances come in does not matter: the rule is by address.
    let host = host(&[]);
    let reversed: Vec<_> = instances.iter().rev().copied().collect();
    let mut got = lend(&host, &reversed);
    got.reverse();
    assert_eq!(got, expected);
}

#[test]
fn pins_by_id_or_index_and_an_unavailable_pin_falls_back() {
    let host = host(&[("/c", "noop-0"), ("/e", "2"), ("/f", "noop-9"), ("/x", "1")]);
    let names = addresses(&["/a", "/b", "/c", "/d", "/e", "/f", "/x"]);
    let types = [GPU, GPU, GPU, GPU, GPU, GPU, CPU];
    let instances: Vec<_> = names.iter().zip(types).collect();
    // Unpinned and the unavailable pin by the rule: /a 0, /b 1, /d 2, /f 0.
    let expected = vec![Some(0), Some(1), Some(0), Some(2), Some(2), Some(0), None];
    assert_eq!(lend(&host, &instances), expected);
    let warnings = host.take_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("/f") && w.contains("noop-9"))
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("/x") && w.contains("uses no GPU"))
    );
    // The engine's mirror prepares the same plan again: the same answer, and
    // no warning twice (auditor-2-002).
    assert_eq!(lend(&host, &instances), expected);
    assert!(host.take_warnings().is_empty());
    let placed = host.placement();
    assert_eq!(placed.len(), 6, "the GPU instances: {placed:?}");
    assert_eq!(
        placed[2],
        Placed {
            instance: "/c".to_owned(),
            device: Some("noop-0".to_owned()),
            pinned: Some("noop-0".to_owned()),
        }
    );
    assert_eq!(placed[5].pinned.as_deref(), Some("noop-9"));
    assert_eq!(placed[5].device.as_deref(), Some("noop-0"));
}

/// A live edit inserts an instance before the others: every survivor keeps
/// the device it was given; the new one is placed by its rank.
#[test]
fn a_survivor_keeps_its_device() {
    let host = host(&[]);
    let before = addresses(&["/b", "/c", "/d"]);
    let first: Vec<_> = before.iter().map(|a| (a, GPU)).collect();
    assert_eq!(lend(&host, &first), [Some(0), Some(1), Some(2)]);
    let after = addresses(&["/a", "/b", "/c", "/d"]);
    let second: Vec<_> = after.iter().map(|a| (a, GPU)).collect();
    assert_eq!(lend(&host, &second), [Some(0), Some(0), Some(1), Some(2)]);
}

/// Without devices nothing is lent, and the record says so.
#[test]
fn without_devices_nothing_is_lent() {
    let host = GpuHost::new(Arc::new(DevicePool::empty()), &[GPU], BTreeMap::new());
    let names = addresses(&["/a"]);
    assert_eq!(lend(&host, &[(&names[0], GPU)]), [None]);
    assert_eq!(host.placement()[0].device, None);
}

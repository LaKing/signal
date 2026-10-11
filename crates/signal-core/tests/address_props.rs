//! Property tests: address parse/display round-trips and ordering (ADR 0016 §1, §3).

use std::cmp::Ordering;

use proptest::prelude::*;
use signal_core::{InstanceAddress, PortAddress};

/// Segment names, mixing a tiny alphabet (to force shared prefixes and
/// prefix-of-segment cases like `a` vs `a_`) with the full grammar.
fn name() -> impl Strategy<Value = String> {
    prop_oneof!["[ab][ab0_]{0,2}", "[a-z][a-z0-9_]{0,8}"]
}

fn segments() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(name(), 1..5)
}

fn instance_text() -> impl Strategy<Value = String> {
    segments().prop_map(|s| format!("/{}", s.join("/")))
}

fn port_text() -> impl Strategy<Value = String> {
    (instance_text(), name()).prop_map(|(i, p)| format!("{i}.{p}"))
}

fn segment_order(a: &InstanceAddress, b: &InstanceAddress) -> Ordering {
    a.segments().cmp(b.segments())
}

proptest! {
    #[test]
    fn instance_round_trips(text in instance_text()) {
        let address: InstanceAddress = text.parse().unwrap();
        prop_assert_eq!(address.to_string(), text.clone());
        prop_assert_eq!(address.to_string().parse::<InstanceAddress>().unwrap(), address);
    }

    #[test]
    fn port_round_trips(text in port_text()) {
        let address: PortAddress = text.parse().unwrap();
        prop_assert_eq!(address.to_string(), text.clone());
        prop_assert_eq!(address.to_string().parse::<PortAddress>().unwrap(), address.clone());
        prop_assert_eq!(PortAddress::new(&address.instance(), address.port()).unwrap(), address);
    }

    /// Whatever parses, displays as exactly the text it came from.
    #[test]
    fn arbitrary_text_parses_only_canonically(text in "[/a-c0_.A]{0,10}") {
        if let Ok(address) = text.parse::<InstanceAddress>() {
            prop_assert_eq!(address.to_string(), text.clone());
        }
        if let Ok(address) = text.parse::<PortAddress>() {
            prop_assert_eq!(address.to_string(), text);
        }
    }

    #[test]
    fn instance_order_is_byte_order_and_segment_order(a in instance_text(), b in instance_text()) {
        let (x, y): (InstanceAddress, InstanceAddress) = (a.parse().unwrap(), b.parse().unwrap());
        prop_assert_eq!(x.cmp(&y), a.as_bytes().cmp(b.as_bytes()));
        prop_assert_eq!(x.cmp(&y), segment_order(&x, &y));
    }

    #[test]
    fn port_order_is_byte_order_and_instance_then_port(a in port_text(), b in port_text()) {
        let (x, y): (PortAddress, PortAddress) = (a.parse().unwrap(), b.parse().unwrap());
        prop_assert_eq!(x.cmp(&y), a.as_bytes().cmp(b.as_bytes()));
        let structured = segment_order(&x.instance(), &y.instance()).then(x.port().cmp(y.port()));
        prop_assert_eq!(x.cmp(&y), structured);
    }
}

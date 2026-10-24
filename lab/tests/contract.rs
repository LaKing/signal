//! The contract suite (`docs/module-contract.md`) on every `lab.*` primitive,
//! as on `std.*`. The suite's renderer waits for due results as the offline
//! engine does. `lab.gpu_partials` is lent a device (M2.7-2) and runs on the
//! GPU, or, where none is usable (AC10 of M2.6-4), on the noop device: then
//! every job travels the real path (submit, due, recycle, reset) but its
//! output is 0. The M3 `lab.tanh_*` candidates (`experiments/m3-tanh-aliasing/`)
//! run on the CPU and get the default settings, as `std.*` types do.

#![cfg(feature = "lab")]

mod common;

use signal_contract::{Check, Settings, stimulus};

signal_contract::install_counting_allocator!();

macro_rules! checks {
    ($($name:ident: $check:expr),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let gpu = common::device();
                $check.assert(
                    &common::module_type(),
                    stimulus::random,
                    &Settings::persisting(file!()).lending(gpu.lent()),
                );
            }
        )*
    };
}

mod gpu_partials {
    use super::*;

    checks! {
        partition_invariance: Check::PartitionInvariance,
        reset_determinism: Check::ResetDeterminism,
        finite_output: Check::FiniteOutput,
        no_allocation: Check::NoAllocation,
        state_output_honesty: Check::StateOutputHonesty,
        rate_agnostic: Check::RateAgnostic,
        no_subnormals: Check::NoSubnormals,
    }
}

/// A registered `lab.*` type.
fn lab_type(name: &str) -> signal_core::ModuleType {
    *common::registry()
        .get(name)
        .unwrap_or_else(|| panic!("{name} is registered"))
}

mod tanh_adaa2 {
    signal_contract::contract_tests!(super::lab_type("lab.tanh_adaa2"));
}

mod tanh_os2_adaa1 {
    signal_contract::contract_tests!(super::lab_type("lab.tanh_os2_adaa1"));
}

mod tanh_os4_adaa1 {
    signal_contract::contract_tests!(super::lab_type("lab.tanh_os4_adaa1"));
}

mod tanh_os8 {
    signal_contract::contract_tests!(super::lab_type("lab.tanh_os8"));
}

/// Every `lab.tanh_*` type has its contract tests above. The promoted
/// `lab.tanh_os8_adaa1` is gone from the lab: it is `std.tanh_shaper_os8`,
/// with the std contract suite (`crates/signal-std/tests/contract.rs`).
#[test]
fn every_tanh_candidate_has_contract_tests() {
    let listed = [
        "lab.tanh_adaa2",
        "lab.tanh_os2_adaa1",
        "lab.tanh_os4_adaa1",
        "lab.tanh_os8",
    ];
    assert_eq!(signal_lab::tanh_shapers::TYPES, listed);
    let registered: Vec<&str> = common::registry()
        .iter()
        .map(|t| t.descriptor.type_name)
        .filter(|n| n.starts_with("lab.tanh"))
        .collect();
    assert_eq!(registered.len(), listed.len(), "{registered:?}");
    assert!(common::registry().get("lab.tanh_os8_adaa1").is_none());
}

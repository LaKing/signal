//! The contract suite (`docs/module-contract.md`) on every `lab.*` primitive,
//! as on `std.*`. The suite's renderer waits for due results as the offline
//! engine does. `lab.gpu_partials` runs on the GPU, or, where none is usable
//! (AC10 of M2.6-4), on the noop device: then every job travels the real
//! path (submit, due, recycle, reset) but its output is 0.

#![cfg(feature = "lab")]

mod common;

use signal_contract::{Check, Settings, stimulus};

signal_contract::install_counting_allocator!();

macro_rules! checks {
    ($($name:ident: $check:expr),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                common::device();
                $check.assert(
                    &common::module_type(),
                    stimulus::random,
                    &Settings::persisting(file!()),
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

//! Signal contract suite: proves that a primitive honours the module contract
//! (`docs/module-contract.md`, `docs/realtime-rules.md`, ADR 0002, ADR 0016).
//! Use it as a dev-dependency only.
//!
//! A test file under `tests/` installs the counting allocator once and runs
//! every [`Check`] on a module type with one line:
//!
//! ```text
//! signal_contract::install_counting_allocator!();
//!
//! mod gain {
//!     signal_contract::contract_tests!(super::gain_type());
//! }
//! ```
//!
//! Each check is its own `#[test]` named after it (`gain::partition_invariance`,
//! …). A failure names the check, the module type, the rate, the seed and the
//! frame of the first difference, and proptest records the case in
//! `<test file stem>.proptest-regressions` beside the test file so the next run
//! replays it first. `tests/self_test.rs` is a complete example. `docs/module-contract.md`,
//! "Testing a primitive: the contract suite", says what each check proves.

pub mod alloc_counter;
mod checks;
mod render;
pub mod stimulus;

pub use checks::{Case, Check, Failure, RATES, Settings};
pub use stimulus::{Stimulus, StimulusFn};

/// The outputs of a fresh instance of `module_type` for the first `frames`
/// frames of `stimulus` (prepared at 48 kHz with seed 1, nothing lent, in
/// calls of 64 frames): every signal output's samples and every event
/// output's `(frame, payload)`; `None` when a call panicked. For tests that
/// compare two renders, such as that a discrete parameter's snapped value
/// plays exactly as the value itself (ADR 0043).
#[allow(clippy::type_complexity)]
pub fn outputs(
    module_type: &signal_core::ModuleType,
    stimulus: &Stimulus,
    frames: usize,
) -> Option<(Vec<Vec<f64>>, Vec<Vec<(u64, signal_core::EventPayload)>>)> {
    let mut instance = render::Instance::new(module_type, None, 48_000.0, 64, 1);
    let rendered = instance.render(stimulus, frames, &[64]);
    if rendered.panic.is_some() {
        return None;
    }
    Some((rendered.signals, rendered.events))
}

/// Expands to one `#[test]` per [`Check`], named after it, for a module type.
///
/// `contract_tests!(module_type)` uses the default [`stimulus::random`];
/// `contract_tests!(module_type, stimulus)` takes any [`StimulusFn`]. The
/// module type is an expression of type [`ModuleType`](signal_core::ModuleType),
/// evaluated in each test. To test several module types in one file, wrap each
/// invocation in its own `mod`. The test binary must also invoke
/// [`install_counting_allocator!`] once, or `no_allocation` fails.
#[macro_export]
macro_rules! contract_tests {
    ($module_type:expr $(,)?) => {
        $crate::contract_tests!($module_type, $crate::stimulus::random);
    };
    ($module_type:expr, $stimulus:expr $(,)?) => {
        #[test]
        fn partition_invariance() {
            $crate::Check::PartitionInvariance.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn reset_determinism() {
            $crate::Check::ResetDeterminism.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn finite_output() {
            $crate::Check::FiniteOutput.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn no_allocation() {
            $crate::Check::NoAllocation.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn state_output_honesty() {
            $crate::Check::StateOutputHonesty.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn rate_agnostic() {
            $crate::Check::RateAgnostic.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }

        #[test]
        fn no_subnormals() {
            $crate::Check::NoSubnormals.assert(
                &$module_type,
                $stimulus,
                &$crate::Settings::persisting(file!()),
            );
        }
    };
}

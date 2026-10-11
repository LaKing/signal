//! Every `std.*` primitive passes all seven checks of the contract suite
//! (`docs/module-contract.md`, "Testing a primitive"), `no_allocation` under
//! the counting allocator included. Each type is taken from the registry that
//! `signal_std::register` fills, so this also covers what is registered.

use signal_core::{ModuleType, Registry};

signal_contract::install_counting_allocator!();

/// The registered type `name`.
fn std_type(name: &str) -> ModuleType {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    *registry
        .get(name)
        .unwrap_or_else(|| panic!("{name} is registered"))
}

mod sine_osc {
    signal_contract::contract_tests!(super::std_type("std.sine_osc"));
}

mod exp_env {
    signal_contract::contract_tests!(super::std_type("std.exp_env"));
}

mod mul {
    signal_contract::contract_tests!(super::std_type("std.mul"));
}

mod add {
    signal_contract::contract_tests!(super::std_type("std.add"));
}

mod tanh_shaper {
    signal_contract::contract_tests!(super::std_type("std.tanh_shaper"));
}

mod dc_block {
    signal_contract::contract_tests!(super::std_type("std.dc_block"));
}

mod unit_delay {
    signal_contract::contract_tests!(super::std_type("std.unit_delay"));
}

mod lfo {
    signal_contract::contract_tests!(super::std_type("std.lfo"));
}

mod noise {
    signal_contract::contract_tests!(super::std_type("std.noise"));
}

mod transport {
    signal_contract::contract_tests!(super::std_type("std.transport"));
}

mod audio_out {
    signal_contract::contract_tests!(super::std_type("std.audio_out"));
}

mod osc {
    signal_contract::contract_tests!(super::std_type("std.osc"));
}

mod svf {
    signal_contract::contract_tests!(super::std_type("std.svf"));
}

mod onepole {
    signal_contract::contract_tests!(super::std_type("std.onepole"));
}

mod adsr {
    signal_contract::contract_tests!(super::std_type("std.adsr"));
}

mod lag {
    signal_contract::contract_tests!(super::std_type("std.lag"));
}

mod delay {
    signal_contract::contract_tests!(super::std_type("std.delay"));
}

mod mix {
    signal_contract::contract_tests!(super::std_type("std.mix"));
}

mod pan {
    signal_contract::contract_tests!(super::std_type("std.pan"));
}

mod gain {
    signal_contract::contract_tests!(super::std_type("std.gain"));
}

mod sub {
    signal_contract::contract_tests!(super::std_type("std.sub"));
}

mod min {
    signal_contract::contract_tests!(super::std_type("std.min"));
}

mod max {
    signal_contract::contract_tests!(super::std_type("std.max"));
}

mod abs {
    signal_contract::contract_tests!(super::std_type("std.abs"));
}

mod scale {
    signal_contract::contract_tests!(super::std_type("std.scale"));
}

mod clamp {
    signal_contract::contract_tests!(super::std_type("std.clamp"));
}

mod sample_hold {
    signal_contract::contract_tests!(super::std_type("std.sample_hold"));
}

mod edge {
    signal_contract::contract_tests!(super::std_type("std.edge"));
}

mod clock_div {
    signal_contract::contract_tests!(super::std_type("std.clock_div"));
}

mod step_seq {
    signal_contract::contract_tests!(super::std_type("std.step_seq"));
}

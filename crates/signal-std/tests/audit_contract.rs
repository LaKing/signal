//! Audit-only contract coverage for new input and layout primitives.
use signal_core::{ModuleType, Registry};
signal_contract::install_counting_allocator!();
fn get(name: &str) -> ModuleType {
    let mut r = Registry::new();
    signal_std::register(&mut r).unwrap();
    *r.get(name).unwrap()
}
mod audio_in {
    signal_contract::contract_tests!(super::get("std.audio_in"));
}
mod audio_in_mono {
    signal_contract::contract_tests!(super::get("std.audio_in_mono"));
}
mod audio_out_mono {
    signal_contract::contract_tests!(super::get("std.audio_out_mono"));
}
mod audio_out_quad {
    signal_contract::contract_tests!(super::get("std.audio_out_quad"));
}
mod audio_out_5_1 {
    signal_contract::contract_tests!(super::get("std.audio_out_5_1"));
}
mod audio_out_7_1 {
    signal_contract::contract_tests!(super::get("std.audio_out_7_1"));
}
mod audio_tap_mono {
    signal_contract::contract_tests!(super::get("std.audio_tap_mono"));
}
mod audio_tap_quad {
    signal_contract::contract_tests!(super::get("std.audio_tap_quad"));
}
mod audio_tap_5_1 {
    signal_contract::contract_tests!(super::get("std.audio_tap_5_1"));
}
mod audio_tap_7_1 {
    signal_contract::contract_tests!(super::get("std.audio_tap_7_1"));
}

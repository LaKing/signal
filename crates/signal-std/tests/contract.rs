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

mod tanh_shaper_os8 {
    signal_contract::contract_tests!(super::std_type("std.tanh_shaper_os8"));
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

mod playhead {
    signal_contract::contract_tests!(super::std_type("std.playhead"));
}

mod prefer {
    signal_contract::contract_tests!(super::std_type("std.prefer"));
}

mod audio_out {
    signal_contract::contract_tests!(super::std_type("std.audio_out"));
}

mod audio_tap {
    signal_contract::contract_tests!(super::std_type("std.audio_tap"));
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

mod ladder {
    signal_contract::contract_tests!(super::std_type("std.ladder"));
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

mod note_seq {
    signal_contract::contract_tests!(super::std_type("std.note_seq"));
}

mod step_seq8 {
    signal_contract::contract_tests!(super::std_type("std.step_seq8"));
}

mod string {
    signal_contract::contract_tests!(super::std_type("std.string"));
}

mod modal {
    signal_contract::contract_tests!(super::std_type("std.modal"));
}

mod mass_spring {
    signal_contract::contract_tests!(super::std_type("std.mass_spring"));
}

mod exciter {
    signal_contract::contract_tests!(super::std_type("std.exciter"));
}

mod count {
    signal_contract::contract_tests!(super::std_type("std.count"));
}

mod particles {
    signal_contract::contract_tests!(super::std_type("std.particles"));
}

mod mesh2d {
    signal_contract::contract_tests!(super::std_type("std.mesh2d"));
}

mod mesh2d_audio {
    signal_contract::contract_tests!(super::std_type("std.mesh2d_audio"));
}

mod body2d {
    signal_contract::contract_tests!(super::std_type("std.body2d"));
}

mod div {
    signal_contract::contract_tests!(super::std_type("std.div"));
}

mod neg {
    signal_contract::contract_tests!(super::std_type("std.neg"));
}

mod modulo {
    signal_contract::contract_tests!(super::std_type("std.mod"));
}

mod fract {
    signal_contract::contract_tests!(super::std_type("std.fract"));
}

mod floor {
    signal_contract::contract_tests!(super::std_type("std.floor"));
}

mod round {
    signal_contract::contract_tests!(super::std_type("std.round"));
}

mod sign {
    signal_contract::contract_tests!(super::std_type("std.sign"));
}

mod sqrt {
    signal_contract::contract_tests!(super::std_type("std.sqrt"));
}

mod exp {
    signal_contract::contract_tests!(super::std_type("std.exp"));
}

mod log {
    signal_contract::contract_tests!(super::std_type("std.log"));
}

mod pow {
    signal_contract::contract_tests!(super::std_type("std.pow"));
}

mod sin {
    signal_contract::contract_tests!(super::std_type("std.sin"));
}

mod cos {
    signal_contract::contract_tests!(super::std_type("std.cos"));
}

mod tan {
    signal_contract::contract_tests!(super::std_type("std.tan"));
}

mod tanh {
    signal_contract::contract_tests!(super::std_type("std.tanh"));
}

mod atan2 {
    signal_contract::contract_tests!(super::std_type("std.atan2"));
}

mod sin_cycles {
    signal_contract::contract_tests!(super::std_type("std.sin_cycles"));
}

mod constant {
    signal_contract::contract_tests!(super::std_type("std.const"));
}

mod sample_rate {
    signal_contract::contract_tests!(super::std_type("std.sample_rate"));
}

mod frames {
    signal_contract::contract_tests!(super::std_type("std.frames"));
}

mod compare {
    signal_contract::contract_tests!(super::std_type("std.compare"));
}

mod select {
    signal_contract::contract_tests!(super::std_type("std.select"));
}

mod and {
    signal_contract::contract_tests!(super::std_type("std.and"));
}

mod or {
    signal_contract::contract_tests!(super::std_type("std.or"));
}

mod not {
    signal_contract::contract_tests!(super::std_type("std.not"));
}

mod phasor {
    signal_contract::contract_tests!(super::std_type("std.phasor"));
}

mod counter {
    signal_contract::contract_tests!(super::std_type("std.counter"));
}

mod integrator {
    signal_contract::contract_tests!(super::std_type("std.integrator"));
}

mod buffer1k {
    signal_contract::contract_tests!(super::std_type("std.buffer1k"));
}

mod buffer64k {
    signal_contract::contract_tests!(super::std_type("std.buffer64k"));
}

mod buffer1m {
    signal_contract::contract_tests!(super::std_type("std.buffer1m"));
}

mod latch {
    signal_contract::contract_tests!(super::std_type("std.latch"));
}

mod to_events {
    signal_contract::contract_tests!(super::std_type("std.to_events"));
}

mod route {
    signal_contract::contract_tests!(super::std_type("std.route"));
}

mod random {
    signal_contract::contract_tests!(super::std_type("std.random"));
}

/// Every vector type and table (`signal_std::VECTOR_WIDTHS`), which
/// `vector_types_are_every_operation_at_every_width` checks against the
/// registry; each has its contract module below.
const VECTOR_TYPES: &[&str] = &[
    "std.pack2",
    "std.unpack2",
    "std.index2",
    "std.reduce2",
    "std.vadd2",
    "std.vmul2",
    "std.table2",
    "std.pack4",
    "std.unpack4",
    "std.index4",
    "std.reduce4",
    "std.vadd4",
    "std.vmul4",
    "std.table4",
    "std.pack8",
    "std.unpack8",
    "std.index8",
    "std.reduce8",
    "std.vadd8",
    "std.vmul8",
    "std.table8",
    "std.pack16",
    "std.unpack16",
    "std.index16",
    "std.reduce16",
    "std.vadd16",
    "std.vmul16",
    "std.table16",
    "std.pack64",
    "std.unpack64",
    "std.index64",
    "std.reduce64",
    "std.vadd64",
    "std.vmul64",
    "std.table64",
    "std.pack256",
    "std.unpack256",
    "std.index256",
    "std.reduce256",
    "std.vadd256",
    "std.vmul256",
    "std.table256",
    "std.pack1024",
    "std.unpack1024",
    "std.index1024",
    "std.reduce1024",
    "std.vadd1024",
    "std.vmul1024",
    "std.table1024",
];

#[test]
fn vector_types_are_every_operation_at_every_width() {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    let mut registered: Vec<&str> = registry
        .iter()
        .map(|t| t.descriptor.type_name)
        .filter(|name| {
            let op = name.trim_end_matches(|c: char| c.is_ascii_digit());
            let families = ["pack", "unpack", "index", "reduce", "vadd", "vmul", "table"];
            families.iter().any(|f| op == format!("std.{f}"))
        })
        .collect();
    let mut covered = VECTOR_TYPES.to_vec();
    registered.sort_unstable();
    covered.sort_unstable();
    assert_eq!(registered, covered);
}

mod pack2 {
    signal_contract::contract_tests!(super::std_type("std.pack2"));
}

mod unpack2 {
    signal_contract::contract_tests!(super::std_type("std.unpack2"));
}

mod index2 {
    signal_contract::contract_tests!(super::std_type("std.index2"));
}

mod reduce2 {
    signal_contract::contract_tests!(super::std_type("std.reduce2"));
}

mod vadd2 {
    signal_contract::contract_tests!(super::std_type("std.vadd2"));
}

mod vmul2 {
    signal_contract::contract_tests!(super::std_type("std.vmul2"));
}

mod table2 {
    signal_contract::contract_tests!(super::std_type("std.table2"));
}

mod pack4 {
    signal_contract::contract_tests!(super::std_type("std.pack4"));
}

mod unpack4 {
    signal_contract::contract_tests!(super::std_type("std.unpack4"));
}

mod index4 {
    signal_contract::contract_tests!(super::std_type("std.index4"));
}

mod reduce4 {
    signal_contract::contract_tests!(super::std_type("std.reduce4"));
}

mod vadd4 {
    signal_contract::contract_tests!(super::std_type("std.vadd4"));
}

mod vmul4 {
    signal_contract::contract_tests!(super::std_type("std.vmul4"));
}

mod table4 {
    signal_contract::contract_tests!(super::std_type("std.table4"));
}

mod pack8 {
    signal_contract::contract_tests!(super::std_type("std.pack8"));
}

mod unpack8 {
    signal_contract::contract_tests!(super::std_type("std.unpack8"));
}

mod index8 {
    signal_contract::contract_tests!(super::std_type("std.index8"));
}

mod reduce8 {
    signal_contract::contract_tests!(super::std_type("std.reduce8"));
}

mod vadd8 {
    signal_contract::contract_tests!(super::std_type("std.vadd8"));
}

mod vmul8 {
    signal_contract::contract_tests!(super::std_type("std.vmul8"));
}

mod table8 {
    signal_contract::contract_tests!(super::std_type("std.table8"));
}

mod pack16 {
    signal_contract::contract_tests!(super::std_type("std.pack16"));
}

mod unpack16 {
    signal_contract::contract_tests!(super::std_type("std.unpack16"));
}

mod index16 {
    signal_contract::contract_tests!(super::std_type("std.index16"));
}

mod reduce16 {
    signal_contract::contract_tests!(super::std_type("std.reduce16"));
}

mod vadd16 {
    signal_contract::contract_tests!(super::std_type("std.vadd16"));
}

mod vmul16 {
    signal_contract::contract_tests!(super::std_type("std.vmul16"));
}

mod table16 {
    signal_contract::contract_tests!(super::std_type("std.table16"));
}

mod pack64 {
    signal_contract::contract_tests!(super::std_type("std.pack64"));
}

mod unpack64 {
    signal_contract::contract_tests!(super::std_type("std.unpack64"));
}

mod index64 {
    signal_contract::contract_tests!(super::std_type("std.index64"));
}

mod reduce64 {
    signal_contract::contract_tests!(super::std_type("std.reduce64"));
}

mod vadd64 {
    signal_contract::contract_tests!(super::std_type("std.vadd64"));
}

mod vmul64 {
    signal_contract::contract_tests!(super::std_type("std.vmul64"));
}

mod table64 {
    signal_contract::contract_tests!(super::std_type("std.table64"));
}

mod pack256 {
    signal_contract::contract_tests!(super::std_type("std.pack256"));
}

mod unpack256 {
    signal_contract::contract_tests!(super::std_type("std.unpack256"));
}

mod index256 {
    signal_contract::contract_tests!(super::std_type("std.index256"));
}

mod reduce256 {
    signal_contract::contract_tests!(super::std_type("std.reduce256"));
}

mod vadd256 {
    signal_contract::contract_tests!(super::std_type("std.vadd256"));
}

mod vmul256 {
    signal_contract::contract_tests!(super::std_type("std.vmul256"));
}

mod table256 {
    signal_contract::contract_tests!(super::std_type("std.table256"));
}

mod pack1024 {
    signal_contract::contract_tests!(super::std_type("std.pack1024"));
}

mod unpack1024 {
    signal_contract::contract_tests!(super::std_type("std.unpack1024"));
}

mod index1024 {
    signal_contract::contract_tests!(super::std_type("std.index1024"));
}

mod reduce1024 {
    signal_contract::contract_tests!(super::std_type("std.reduce1024"));
}

mod vadd1024 {
    signal_contract::contract_tests!(super::std_type("std.vadd1024"));
}

mod vmul1024 {
    signal_contract::contract_tests!(super::std_type("std.vmul1024"));
}

mod table1024 {
    signal_contract::contract_tests!(super::std_type("std.table1024"));
}

/// The physical models stay finite when their parameters jump between range
/// limits on every frame (finding auditor-1-modal-modulation-overflow): a
/// unit impulse on every signal input and a trigger on every event input on
/// frame 0, then silence, with the named parameters alternating between two
/// values each frame and the others fixed or at their defaults. Run through
/// the contract's `finite_output` check at every rate, in calls of 1 frame.
mod modulation {
    use signal_contract::{Case, Check, Stimulus};
    use signal_core::{EventPayload, ModuleDescriptor};

    /// `alternating`: (name, even-frame value, odd-frame value); `fixed`:
    /// (name, value); every vector input row alternates between `rows.0`
    /// and `rows.1`.
    fn stimulus(
        descriptor: &ModuleDescriptor,
        frames: usize,
        alternating: &[(&str, f64, f64)],
        fixed: &[(&str, f64)],
        rows: (f64, f64),
    ) -> Stimulus {
        let ports = descriptor.ports.iter();
        let inputs = ports.filter(|p| p.direction == signal_core::Direction::In);
        let mut signals = Vec::new();
        let mut events = Vec::new();
        for port in inputs {
            match port.shape {
                signal_core::PortShape::Signal => {
                    let mut x = vec![0.0; frames];
                    x[0] = 1.0;
                    signals.push(x);
                }
                signal_core::PortShape::Events => events.push(vec![(0, EventPayload::Trigger)]),
                signal_core::PortShape::Vector(width) => {
                    let row: Vec<f64> = (0..frames)
                        .map(|n| if n % 2 == 0 { rows.0 } else { rows.1 })
                        .collect();
                    signals.extend((0..width).map(|_| row.clone()));
                }
                _ => {}
            }
        }
        let parameters = descriptor
            .parameters
            .iter()
            .map(|p| {
                if let Some(&(_, a, b)) = alternating.iter().find(|(n, ..)| *n == p.name) {
                    (0..frames)
                        .map(|n| if n % 2 == 0 { a } else { b })
                        .collect()
                } else {
                    let value = fixed.iter().find(|(n, _)| *n == p.name).map(|&(_, v)| v);
                    vec![value.unwrap_or(p.default); frames]
                }
            })
            .collect();
        Stimulus {
            signals,
            parameters,
            events,
        }
    }

    fn range(descriptor: &ModuleDescriptor, name: &str) -> (f64, f64) {
        let p = descriptor
            .parameters
            .iter()
            .find(|p| p.name == name)
            .unwrap();
        (p.range.min, p.range.max)
    }

    /// The auditor's stimulus: freq alternating 100 and 21 000 Hz, one string
    /// mode, decay_s 60, law 0, position ½.
    fn modal(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let fixed = [
            ("material", 0.0),
            ("modes", 1.0),
            ("decay_s", 60.0),
            ("law", 0.0),
            ("position", 0.5),
        ];
        stimulus(
            descriptor,
            frames,
            &[("freq", 100.0, 21_000.0)],
            &fixed,
            (0.0, 0.0),
        )
    }

    /// M2.10-8: every continuous parameter of the bank but the decay and
    /// its tilt between its range limits, and every vector row between 15 and 0
    /// (factors 16 and 1); 64 modes, decay 60 s and law 0 so the excitation
    /// keeps ringing (`modal_decay` moves those). `gain` is at its maximum on
    /// the even frames: on the odd ones `freq` 21 kHz times `mode_freq`'s 16
    /// puts every mode above Nyquist, where the bank's Nyquist fade makes it
    /// silent, so the loud frames are the low ones and the bank is heard.
    fn modal_all(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let alternating: Vec<(&str, f64, f64)> = descriptor
            .parameters
            .iter()
            .filter(|p| !["modes", "decay_s", "law", "damping_tilt"].contains(&p.name))
            .map(|p| match p.name {
                "freq" => ("freq", 100.0, 21_000.0),
                "gain" => ("gain", p.range.max, p.range.min),
                _ => (p.name, p.range.min, p.range.max),
            })
            .collect();
        let fixed = [("modes", 64.0), ("decay_s", 60.0), ("law", 0.0)];
        stimulus(descriptor, frames, &alternating, &fixed, (15.0, 0.0))
    }

    /// The decay between its limits: `decay_s` 60 and 0.01, `law` 0 and 3,
    /// `damping_tilt` 0 and 10,
    /// every vector row 63 and −1 (factors 64 and 0, `mode_decay` 64 and
    /// 1/64). The excitation dies out (checked for finiteness only).
    fn modal_decay(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let alternating = [
            ("decay_s", 60.0, 0.01),
            ("law", 0.0, 3.0),
            ("damping_tilt", 0.0, 10.0),
            ("freq", 100.0, 21_000.0),
        ];
        stimulus(
            descriptor,
            frames,
            &alternating,
            &[("modes", 64.0)],
            (63.0, -1.0),
        )
    }

    /// The same at the largest gain the parameters allow: decay 60 s, law 0,
    /// gain 4, rows between 15 and 63 (factors 16 and 64), with only the
    /// frequency, material and pickup moving.
    fn modal_loud(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let alternating = [
            ("freq", 100.0, 21_000.0),
            ("material", 0.0, 5.0),
            ("pickup", 0.01, 0.99),
            ("brightness", -1.0, 1.0),
        ];
        let fixed = [
            ("modes", 64.0),
            ("decay_s", 60.0),
            ("law", 0.0),
            ("gain", 4.0),
            ("pickup_mix", 1.0),
        ];
        stimulus(descriptor, frames, &alternating, &fixed, (15.0, 63.0))
    }

    /// M2.10-8: the exciter's level, hardness and added parameters between
    /// their range limits and its duration between 5 and 50 ms on every
    /// frame (a length change rebases the excitation every frame), as a bow
    /// and as a noise burst.
    fn exciter_with(descriptor: &ModuleDescriptor, frames: usize, kind: f64) -> Stimulus {
        let alternating: Vec<(&str, f64, f64)> = descriptor
            .parameters
            .iter()
            .filter(|p| p.name != "kind")
            .map(|p| match p.name {
                "duration_ms" => ("duration_ms", 5.0, 50.0),
                "level" => ("level", 1.0, 0.2),
                _ => (p.name, p.range.min, p.range.max),
            })
            .collect();
        stimulus(
            descriptor,
            frames,
            &alternating,
            &[("kind", kind)],
            (0.0, 0.0),
        )
    }

    fn exciter_bow(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        exciter_with(descriptor, frames, 3.0)
    }

    fn exciter_noise(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        exciter_with(descriptor, frames, 2.0)
    }

    /// freq, brightness and stiffness between their range limits, decay_s 60.
    fn string(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "freq");
        let alternating = [
            ("freq", lo, hi),
            ("brightness", 0.0, 1.0),
            ("stiffness", 1.0, 0.0),
        ];
        stimulus(
            descriptor,
            frames,
            &alternating,
            &[("decay_s", 60.0)],
            (0.0, 0.0),
        )
    }

    /// The same as [`string`] with only the frequency moving, between 20 Hz
    /// and 20 000 Hz, at full stiffness.
    fn string_freq(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let fixed = [("decay_s", 60.0), ("stiffness", 1.0), ("brightness", 1.0)];
        stimulus(
            descriptor,
            frames,
            &[("freq", 20.0, 20_000.0)],
            &fixed,
            (0.0, 0.0),
        )
    }

    /// The auditor's stimulus (finding auditor-1-mass-spring-modulation-
    /// overflow): spring_hz at its range maximum (clamped to sr/2π) on even
    /// frames and 0 on odd ones, so the frame-0 impulse drives a stiff chain;
    /// 32 masses, decay_s 60, fixed–fixed, default drive and pickup nodes.
    fn mass_spring(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "spring_hz");
        let fixed = [("masses", 32.0), ("decay_s", 60.0), ("boundary", 0.0)];
        stimulus(
            descriptor,
            frames,
            &[("spring_hz", hi, lo)],
            &fixed,
            (0.0, 0.0),
        )
    }

    /// The same with the boundary alternating too, heard at the free end.
    fn mass_spring_boundary(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "spring_hz");
        let alternating = [("spring_hz", hi, lo), ("boundary", 0.0, 1.0)];
        let fixed = [("masses", 32.0), ("decay_s", 60.0), ("pickup_node", 32.0)];
        stimulus(descriptor, frames, &alternating, &fixed, (0.0, 0.0))
    }

    fn finite(name: &str, stimulus: signal_contract::StimulusFn) {
        let case = Case {
            seed: 0,
            blocks: vec![1],
        };
        if let Err(failure) = Check::FiniteOutput.run_case(&super::std_type(name), stimulus, &case)
        {
            panic!("{failure}");
        }
    }

    /// Renders 4096 frames of `stimulus` at 44.1 kHz one frame at a time and
    /// asserts that the model is excited while it is modulated: the output
    /// moves on frames after the impulse, so the probe cannot pass on silence.
    fn excited(name: &str, stimulus: signal_contract::StimulusFn) {
        use signal_core::{Event, Inputs, Outputs, PrepareContext, Primitive, RngStream};
        let module_type = super::std_type(name);
        let frames = 4096;
        let input = stimulus(&module_type.descriptor, 0, frames);
        let Primitive::Direct(mut primitive) = (module_type.create)() else {
            panic!("{name} is direct");
        };
        primitive.prepare(&PrepareContext {
            sample_rate: 44_100.0,
            max_frames: 1,
            rng: RngStream::derive(0, &"/probe".parse().unwrap()),
            lent: None,
            generation: 0,
        });
        let mut moving = 0;
        let mut last = 0.0;
        for n in 0..frames {
            let signals: Vec<f64> = input.signals.iter().map(|s| s[n]).collect();
            let parameters: Vec<f64> = input.parameters.iter().map(|p| p[n]).collect();
            let mut events = Vec::new();
            let mut ends = Vec::new();
            for port in &input.events {
                for &(frame, payload) in port {
                    if frame == n as u64 {
                        events.push(Event { offset: 0, payload });
                    }
                }
                ends.push(events.len());
            }
            let mut out = [f64::NAN];
            primitive.process(
                &Inputs::new(1, &signals, &parameters, &events, &ends),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            if n > 0 && out[0] != last {
                moving += 1;
            }
            last = out[0];
        }
        println!(
            "{name}: the output moves on {moving} of {} frames",
            frames - 1
        );
        assert!(
            moving > frames / 2,
            "{name} is not excited: {moving} moving frames"
        );
    }

    #[test]
    fn modal_stays_finite_when_freq_alternates_every_frame() {
        excited("std.modal", modal);
        finite("std.modal", modal);
    }

    /// M2.10-8 (AC6, AC10): every parameter and vector row of the bank.
    #[test]
    fn modal_stays_finite_when_every_parameter_alternates_every_frame() {
        for stimulus in [modal_all, modal_loud] {
            excited("std.modal", stimulus);
            finite("std.modal", stimulus);
        }
        finite("std.modal", modal_decay);
    }

    /// M2.10-8 (AC7): the exciter under every parameter at once.
    #[test]
    fn exciter_stays_finite_when_its_parameters_alternate_every_frame() {
        for stimulus in [exciter_bow, exciter_noise] {
            finite("std.exciter", stimulus);
        }
    }

    #[test]
    fn string_stays_finite_when_its_parameters_alternate_every_frame() {
        for stimulus in [string, string_freq] {
            excited("std.string", stimulus);
            finite("std.string", stimulus);
        }
    }

    #[test]
    fn mass_spring_stays_finite_when_its_spring_alternates_every_frame() {
        for stimulus in [mass_spring, mass_spring_boundary] {
            excited("std.mass_spring", stimulus);
            finite("std.mass_spring", stimulus);
        }
    }
}

mod biquad {
    signal_contract::contract_tests!(super::std_type("std.biquad"));
}

mod comb {
    signal_contract::contract_tests!(super::std_type("std.comb"));
}

mod allpass_delay {
    signal_contract::contract_tests!(super::std_type("std.allpass_delay"));
}

mod fdn_reverb {
    signal_contract::contract_tests!(super::std_type("std.fdn_reverb"));
}

mod fm_op {
    signal_contract::contract_tests!(super::std_type("std.fm_op"));
}

mod wavetable {
    signal_contract::contract_tests!(super::std_type("std.wavetable"));
}

/// ADR 0033: the contract suite feeds no connections (`Inputs::with_connected`
/// is the engine's), so following a position is checked here: both
/// sequencers, `follow` connected, through forward motion, a stop, a seek
/// marked on `reset`, a jump backwards and a reverse, in calls of 1, 7 and
/// 256 frames, allocate and free nothing in `process`, and give the same
/// output in every partition.
mod follow_mode {
    use signal_contract::alloc_counter::Guard;
    use signal_core::{
        DirectPrimitive, Event, EventPayload, EventSink, Inputs, Outputs, PrepareContext,
        RngStream, Table,
    };
    use signal_std::pattern::{compile_notes, compile_steps};
    use signal_std::{NoteSeq, StepSeq8};

    const FRAMES: usize = 4096;

    /// The followed position per frame: forward at 1/256 beat a frame, held
    /// from 1000 to 1200, a seek to 3.5 at 2000 (marked on `reset`), back to
    /// 0.25 at 3000, backwards from 3500.
    fn positions() -> Vec<f64> {
        let mut p = 0.0;
        (0..FRAMES)
            .map(|n| {
                match n {
                    1000..1200 => {}
                    2000 => p = 3.5,
                    3000 => p = 0.25,
                    3500.. => p -= 1.0 / 512.0,
                    _ => p += 1.0 / 256.0,
                }
                p
            })
            .collect()
    }

    fn context() -> PrepareContext {
        PrepareContext {
            sample_rate: 48_000.0,
            max_frames: 256,
            rng: RngStream::derive(1, &"/f".parse().unwrap()),
            lent: None,
            generation: 0,
        }
    }

    /// Every output of `seq` over the timeline in calls of `block`.
    fn run<P: DirectPrimitive + Default>(
        table: &Table,
        parameters: &[f64],
        signals: usize,
        sinks: usize,
        block: usize,
    ) -> (Vec<u64>, Vec<u64>) {
        let mut seq = P::default();
        seq.prepare(&context());
        let positions = positions();
        let mut connected = [false; 16];
        connected[2] = true;
        let mut out = vec![0.0; signals * 256];
        let mut sinks = vec![EventSink::per_frame(16, 256); sinks];
        let (mut seen, mut events) = (Vec::new(), Vec::new());
        let mut start = 0;
        while start < FRAMES {
            let k = block.min(FRAMES - start);
            let params: Vec<f64> = parameters
                .iter()
                .flat_map(|&v| std::iter::repeat_n(v, k))
                .collect();
            let resets: Vec<Event> = (start..start + k)
                .filter(|&n| n == 2000)
                .map(|n| Event {
                    offset: (n - start) as u32,
                    payload: EventPayload::Trigger,
                })
                .collect();
            for sink in &mut sinks {
                sink.start_call(k);
            }
            let ends = [0, resets.len()];
            let inputs = Inputs::new(k, &positions[start..start + k], &params, &resets, &ends)
                .with_table(Some(table))
                .with_connected(&connected);
            let guard = Guard::start();
            seq.process(
                &inputs,
                &mut Outputs::new(k, &mut out[..signals * k], &mut sinks),
            );
            let counts = guard.counts();
            drop(guard);
            assert!(counts.is_zero(), "{counts:?}");
            // Frame by frame, then each output's events as (port, frame).
            for i in 0..k {
                seen.extend((0..signals).map(|r| out[r * k + i].to_bits()));
            }
            for (port, sink) in sinks.iter_mut().enumerate() {
                events.extend(
                    sink.events()
                        .iter()
                        .map(|e| ((start as u64 + u64::from(e.offset)) << 8) | port as u64),
                );
                sink.clear();
            }
            start += k;
        }
        events.sort_unstable();
        (seen, events)
    }

    #[test]
    fn following_allocates_nothing_and_ignores_the_partition() {
        let notes = Table::new(
            4,
            vec![
                60.0, 0.0, 0.5, 1.0, 62.0, 0.75, 1.0, 0.5, 64.0, 2.0, 0.25, 0.8, 65.0, 3.5, 1.0,
                1.0,
            ],
        )
        .unwrap();
        let notes = compile_notes(&[4.0], &notes).unwrap();
        let steps = Table::new(5, vec![1.0, 1.0, 0.0, 2.0, 0.0, 0.5, 0.0, 1.0, 1.0, 1.0]).unwrap();
        let steps = compile_steps(&[4.0, 0.25], &steps).unwrap();
        let notes_ref = run::<NoteSeq>(&notes, &[96.0], 5, 2, 256);
        let steps_ref = run::<StepSeq8>(&steps, &[96.0, 1.5, 50.0], 1, 9, 256);
        assert!(notes_ref.1.len() > 4 && steps_ref.1.len() > 8, "they play");
        for block in [1, 7] {
            assert_eq!(run::<NoteSeq>(&notes, &[96.0], 5, 2, block), notes_ref);
            assert_eq!(
                run::<StepSeq8>(&steps, &[96.0, 1.5, 50.0], 1, 9, block),
                steps_ref
            );
        }
    }
}

/// ADR 0039: `std.note_seq`'s `audition` notes merged into the voice, both
/// counting ticks and following a position (with a stop, a seek on `reset`
/// and a reverse), in calls of 1, 7 and 256 frames: `process` allocates and
/// frees nothing, and the output is the same in every partition.
mod audition {
    use signal_contract::alloc_counter::Guard;
    use signal_core::{
        DirectPrimitive, Event, EventPayload, EventSink, Inputs, Outputs, PrepareContext,
        RngStream, Table,
    };
    use signal_std::pattern::compile_notes;
    use signal_std::{NoteSeq, audition_value};

    const FRAMES: usize = 4096;

    /// The audition timeline: presses, a slide, a stray note off, a press
    /// and release on one frame, a `Trigger` release, junk values.
    fn auditions() -> Vec<(usize, EventPayload)> {
        let on = |key, velocity| EventPayload::Set(audition_value(key, velocity, true));
        let off = |key| EventPayload::Set(audition_value(key, 0.0, false));
        vec![
            (100, on(72, 0.8)),
            (300, on(74, 0.4)),
            (301, off(72)),
            (700, off(74)),
            (1100, on(48, 1.0)),
            (1150, off(48)),
            (2000, on(50, 0.6)),
            (2000, off(50)),
            (2500, on(55, 0.9)),
            (2600, EventPayload::Set(-1.0)),
            (2600, EventPayload::Set(0.5)),
            (3100, EventPayload::Trigger),
            (3600, on(57, 0.3)),
            (3900, off(57)),
        ]
    }

    fn positions() -> Vec<f64> {
        let mut p = 0.0;
        (0..FRAMES)
            .map(|n| {
                match n {
                    1000..1200 => {}
                    2000 => p = 3.5,
                    3500.. => p -= 1.0 / 512.0,
                    _ => p += 1.0 / 256.0,
                }
                p
            })
            .collect()
    }

    /// Every output over the timeline in calls of `block`; following a
    /// position when `follow`, else a tick every frame.
    fn run(table: &Table, follow: bool, block: usize) -> (Vec<u64>, Vec<u64>) {
        let mut seq = NoteSeq::default();
        seq.prepare(&PrepareContext {
            sample_rate: 48_000.0,
            max_frames: 256,
            rng: RngStream::derive(1, &"/a".parse().unwrap()),
            lent: None,
            generation: 0,
        });
        let positions = positions();
        let auditions = auditions();
        let mut connected = [false; 16];
        connected[2] = follow;
        let mut out = vec![0.0; 5 * 256];
        let mut sinks = vec![EventSink::per_frame(16, 256); 2];
        let (mut seen, mut events) = (Vec::new(), Vec::new());
        let mut start = 0;
        while start < FRAMES {
            let k = block.min(FRAMES - start);
            let local = |n: usize| (n - start) as u32;
            let mut input: Vec<Event> = Vec::new();
            if !follow {
                input.extend((start..start + k).map(|n| Event {
                    offset: local(n),
                    payload: EventPayload::Trigger,
                }));
            }
            let ticks = input.len();
            if follow {
                input.extend((start..start + k).filter(|&n| n == 2000).map(|n| Event {
                    offset: local(n),
                    payload: EventPayload::Trigger,
                }));
            }
            let resets = input.len();
            input.extend(
                auditions
                    .iter()
                    .filter(|(n, _)| (start..start + k).contains(n))
                    .map(|&(n, payload)| Event {
                        offset: local(n),
                        payload,
                    }),
            );
            let ends = [ticks, resets, input.len()];
            let params = vec![96.0; k];
            let follow_in = if follow {
                &positions[start..start + k]
            } else {
                &[][..]
            };
            for sink in &mut sinks {
                sink.start_call(k);
            }
            let inputs = Inputs::new(k, follow_in, &params, &input, &ends)
                .with_table(Some(table))
                .with_connected(&connected);
            let guard = Guard::start();
            seq.process(&inputs, &mut Outputs::new(k, &mut out[..5 * k], &mut sinks));
            let counts = guard.counts();
            drop(guard);
            assert!(counts.is_zero(), "{counts:?}");
            for i in 0..k {
                seen.extend((0..5).map(|r| out[r * k + i].to_bits()));
            }
            for (port, sink) in sinks.iter_mut().enumerate() {
                events.extend(
                    sink.events()
                        .iter()
                        .map(|e| ((start as u64 + u64::from(e.offset)) << 8) | port as u64),
                );
                sink.clear();
            }
            start += k;
        }
        events.sort_unstable();
        (seen, events)
    }

    #[test]
    fn auditions_allocate_nothing_and_ignore_the_partition() {
        let notes = Table::new(
            4,
            vec![
                60.0, 0.0, 0.5, 1.0, 62.0, 0.75, 1.0, 0.5, 64.0, 2.0, 0.25, 0.8, 65.0, 3.5, 1.0,
                1.0,
            ],
        )
        .unwrap();
        let notes = compile_notes(&[4.0], &notes).unwrap();
        for follow in [false, true] {
            let reference = run(&notes, follow, 256);
            // The auditions are heard: key 72 sounds from frame 100.
            assert_eq!(f64::from_bits(reference.0[100 * 5 + 1]), 72.0, "{follow}");
            for block in [1, 7] {
                assert_eq!(run(&notes, follow, block), reference, "{follow} {block}");
            }
        }
    }
}

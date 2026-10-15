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
    /// (name, value).
    fn stimulus(
        descriptor: &ModuleDescriptor,
        frames: usize,
        alternating: &[(&str, f64, f64)],
        fixed: &[(&str, f64)],
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
        stimulus(descriptor, frames, &[("freq", 100.0, 21_000.0)], &fixed)
    }

    /// freq, brightness and stiffness between their range limits, decay_s 60.
    fn string(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "freq");
        let alternating = [
            ("freq", lo, hi),
            ("brightness", 0.0, 1.0),
            ("stiffness", 1.0, 0.0),
        ];
        stimulus(descriptor, frames, &alternating, &[("decay_s", 60.0)])
    }

    /// The same as [`string`] with only the frequency moving, between 20 Hz
    /// and 20 000 Hz, at full stiffness.
    fn string_freq(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let fixed = [("decay_s", 60.0), ("stiffness", 1.0), ("brightness", 1.0)];
        stimulus(descriptor, frames, &[("freq", 20.0, 20_000.0)], &fixed)
    }

    /// The auditor's stimulus (finding auditor-1-mass-spring-modulation-
    /// overflow): spring_hz at its range maximum (clamped to sr/2π) on even
    /// frames and 0 on odd ones, so the frame-0 impulse drives a stiff chain;
    /// 32 masses, decay_s 60, fixed–fixed, default drive and pickup nodes.
    fn mass_spring(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "spring_hz");
        let fixed = [("masses", 32.0), ("decay_s", 60.0), ("boundary", 0.0)];
        stimulus(descriptor, frames, &[("spring_hz", hi, lo)], &fixed)
    }

    /// The same with the boundary alternating too, heard at the free end.
    fn mass_spring_boundary(descriptor: &ModuleDescriptor, _seed: u64, frames: usize) -> Stimulus {
        let (lo, hi) = range(descriptor, "spring_hz");
        let alternating = [("spring_hz", hi, lo), ("boundary", 0.0, 1.0)];
        let fixed = [("masses", 32.0), ("decay_s", 60.0), ("pickup_node", 32.0)];
        stimulus(descriptor, frames, &alternating, &fixed)
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

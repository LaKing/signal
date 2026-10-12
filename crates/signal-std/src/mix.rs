//! `std.mix`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

const fn gain(name: &'static str) -> signal_core::ParameterDescriptor {
    parameter(name, PortKind::Gain, None, 0.0, 4.0, 1.0, Combine::Multiply)
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.mix",
    doc: "Four-input mixer: out = gain_a·a + gain_b·b + gain_c·c + gain_d·d.",
    ports: &[
        signal_port("a", Direction::In, PortKind::Audio, None, None),
        signal_port("b", Direction::In, PortKind::Audio, None, None),
        signal_port("c", Direction::In, PortKind::Audio, None, None),
        signal_port("d", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        gain("gain_a"),
        gain("gain_b"),
        gain("gain_c"),
        gain("gain_d"),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.mix`: the weighted sum of four signals.
///
/// **Ports**
/// - `a`, `b`, `c`, `d` (signal in, audio, unbounded); unconnected they read 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `gain_a` … `gain_d` (gain, linear, no unit, range `[0, 4]`, default 1,
///   combine `multiply`): the factor on the input of the same letter.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = gain_a[n]·a[n] + gain_b[n]·b[n] + gain_c[n]·c[n] +
/// gain_d[n]·d[n]`, summed left to right. Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Mix;

impl DirectPrimitive for Mix {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let term = |port: usize, n: usize| {
            at(inputs.parameter(port), n, 1.0) * at(inputs.signal(port), n, 0.0)
        };
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = term(0, n) + term(1, n) + term(2, n) + term(3, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn out_is_the_weighted_sum() {
        let (a, b, c, d) = ([1.0, 2.0], [10.0, 20.0], [100.0, 200.0], [-1.0, 0.5]);
        let (ga, gb, gc, gd) = ([1.0, 1.0], [0.5, 2.0], [0.0, 4.0], [3.0, 1.0]);
        let mut mix = Mix;
        mix.prepare(&context(48_000.0, 2, 0));
        let run = render(
            &mut mix,
            &Timeline {
                signals: &[&a, &b, &c, &d],
                parameters: &[&ga, &gb, &gc, &gd],
                triggers: &[],
            },
            2,
            1,
        );
        assert_eq!(
            run.signals[0],
            [1.0 + 5.0 + 0.0 - 3.0, 2.0 + 40.0 + 800.0 + 0.5]
        );
    }
}

//! `std.compare`, `std.select`, `std.and`, `std.or` and `std.not`: the core
//! set's comparison and selection (ADR 0027 §1). A gate is a signal of kind
//! `gate`; any value above 0 is true, and the gates these types output are
//! exactly 0 or 1. Feeding a gate input from an output of another kind is
//! accepted with the graph's usual kind-mismatch warning (none from a
//! `generic` output, which matches any kind). Direct, at rate
//! divisor 1, category `core`; unconnected inputs read 0. Stateless; constant
//! work per frame.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortDescriptor, PortKind, PrepareContext, Range,
};

use crate::math::{bounded, stateless};
use crate::{at, parameter, round_half_up, signal_port};

/// The comparisons `mode` selects (ADR 0043: its value set).
const MODE_VALUES: &[signal_core::ValueLabel] = &[
    crate::named(0.0, "lt"),
    crate::named(1.0, "le"),
    crate::named(2.0, "eq"),
    crate::named(3.0, "ge"),
    crate::named(4.0, "gt"),
    crate::named(5.0, "ne"),
];

/// A gate output: 0 or 1.
const GATE_OUT: PortDescriptor = signal_port(
    "out",
    Direction::Out,
    PortKind::Gate,
    None,
    Some(Range { min: 0.0, max: 1.0 }),
);

/// 1 for true, 0 for false.
fn gate(on: bool) -> f64 {
    if on { 1.0 } else { 0.0 }
}

/// The comparison `mode` selects (see [`Compare`]).
pub(crate) fn compare(mode: f64, a: f64, b: f64) -> f64 {
    gate(match round_half_up(mode) as u8 {
        0 => a < b,
        1 => a <= b,
        2 => a == b,
        3 => a >= b,
        4 => a > b,
        _ => a != b,
    })
}

const COMPARE: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.compare",
    doc: "Compares a with b: out = 1 if (a op b) else 0, op by mode: 0 lt, 1 le, 2 eq, 3 ge, 4 gt, 5 ne.",
    ports: &[
        signal_port("a", Direction::In, PortKind::Generic, None, None),
        signal_port("b", Direction::In, PortKind::Generic, None, None),
        GATE_OUT,
    ],
    parameters: &[crate::choice(
        parameter(
            "mode",
            PortKind::Generic,
            None,
            0.0,
            5.0,
            0.0,
            Combine::Replace,
        ),
        MODE_VALUES,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.compare`: a comparison as a gate.
///
/// **Ports**
/// - `a`, `b` (signal in, generic, unbounded); unconnected they read 0.
/// - `out` (signal out, gate, `[0, 1]`).
///
/// **Parameters**
/// - `mode` (generic, no unit, choice, range `[0, 5]`, default 0, combine
///   `replace`): `round_half_up(mode)` selects 0 `lt`, 1 `le`, 2 `eq`, 3 `ge`,
///   4 `gt`, 5 `ne`.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = 1` if `a[n] op b[n]`, else `0`, with IEEE 754
/// comparisons: `−0` equals `+0`. Stateless; constant work per frame.
///
/// **Modulated mode:** the mode is read per frame, so a mode ramped or
/// modulated from one value to another passes through every mode between
/// them (from `lt` to `ne` it is `le`, `eq`, `ge` and `gt` on the way).
///
/// **Operands:** ordered: `lt` is `a < b`, never `b < a`.
#[derive(Clone, Debug, Default)]
pub struct Compare;

impl DirectPrimitive for Compare {
    fn describe() -> ModuleDescriptor {
        COMPARE
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (a, b, mode) = (inputs.signal(0), inputs.signal(1), inputs.parameter(0));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = compare(at(mode, n, 0.0), at(a, n, 0.0), at(b, n, 0.0));
        }
    }
}

/// `std.select`: one of two signals, chosen by a gate.
///
/// **Ports**
/// - `gate` (signal in, gate, unbounded), `a`, `b` (signal in, generic,
///   unbounded); unconnected they read 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = a[n]` if `gate[n] > 0`, else `b[n]`, clamped to
/// `±1e6` ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND), which only an input that a
/// fan-in sum or a connection scale took beyond it reaches), so the type
/// never faults on its own. Stateless; constant work per frame.
///
/// **Operands:** ordered: `a` is chosen when the gate is open, `b` otherwise.
#[derive(Clone, Debug, Default)]
pub struct Select;

impl DirectPrimitive for Select {
    fn describe() -> ModuleDescriptor {
        stateless(
            "std.select",
            "Selects by a gate: out = a if gate > 0, else b.",
            SELECT_PORTS,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (g, a, b) = (inputs.signal(0), inputs.signal(1), inputs.signal(2));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = bounded(if at(g, n, 0.0) > 0.0 {
                at(a, n, 0.0)
            } else {
                at(b, n, 0.0)
            });
        }
    }
}

/// The ports of `std.select`.
const SELECT_PORTS: &[PortDescriptor] = &[
    signal_port("gate", Direction::In, PortKind::Gate, None, None),
    signal_port("a", Direction::In, PortKind::Generic, None, None),
    signal_port("b", Direction::In, PortKind::Generic, None, None),
    signal_port("out", Direction::Out, PortKind::Generic, None, None),
];

/// The ports of `std.not`.
const NOT_PORTS: &[PortDescriptor] = &[
    signal_port("in", Direction::In, PortKind::Gate, None, None),
    GATE_OUT,
];

/// The ports of `std.and` and `std.or`.
const GATE_PAIR: &[PortDescriptor] = &[
    signal_port("a", Direction::In, PortKind::Gate, None, None),
    signal_port("b", Direction::In, PortKind::Gate, None, None),
    GATE_OUT,
];

/// `out[n] = f(a[n] > 0, b[n] > 0)` as a gate.
fn process_gates(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, f: fn(bool, bool) -> bool) {
    let (a, b) = (inputs.signal(0), inputs.signal(1));
    for (n, out) in outputs.signal(0).iter_mut().enumerate() {
        *out = gate(f(at(a, n, 0.0) > 0.0, at(b, n, 0.0) > 0.0));
    }
}

/// `std.and`: both gates open.
///
/// **Ports**
/// - `a`, `b` (signal in, gate, unbounded); unconnected they read 0.
/// - `out` (signal out, gate, `[0, 1]`).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = 1` if `a[n] > 0` and `b[n] > 0`, else `0`.
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct And;

impl DirectPrimitive for And {
    fn describe() -> ModuleDescriptor {
        stateless(
            "std.and",
            "Gate and: out = 1 if a > 0 and b > 0, else 0.",
            GATE_PAIR,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        process_gates(inputs, outputs, |a, b| a && b);
    }
}

/// `std.or`: either gate open.
///
/// **Ports**
/// - `a`, `b` (signal in, gate, unbounded); unconnected they read 0.
/// - `out` (signal out, gate, `[0, 1]`).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = 1` if `a[n] > 0` or `b[n] > 0`, else `0`.
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Or;

impl DirectPrimitive for Or {
    fn describe() -> ModuleDescriptor {
        stateless(
            "std.or",
            "Gate or: out = 1 if a > 0 or b > 0, else 0.",
            GATE_PAIR,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        process_gates(inputs, outputs, |a, b| a || b);
    }
}

/// `std.not`: the inverted gate.
///
/// **Ports**
/// - `in` (signal in, gate, unbounded); unconnected it reads 0.
/// - `out` (signal out, gate, `[0, 1]`).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = 0` if `in[n] > 0`, else `1` (so an unconnected
/// `std.not` outputs 1). Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Not;

impl DirectPrimitive for Not {
    fn describe() -> ModuleDescriptor {
        stateless("std.not", "Gate not: out = 0 if in > 0, else 1.", NOT_PORTS)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = gate(at(x, n, 0.0) <= 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn run<P: DirectPrimitive>(mut p: P, signals: &[&[f64]], parameters: &[&[f64]]) -> Vec<f64> {
        let frames = signals[0].len();
        p.prepare(&context(48_000.0, 3, 0));
        let timeline = Timeline {
            signals,
            parameters,
            triggers: &[],
        };
        render(&mut p, &timeline, frames, 3).signals.remove(0)
    }

    /// AC7: every mode on (less, equal, greater, −0 vs +0), as a 0/1 gate.
    #[test]
    fn compare_modes() {
        let a = [1.0, 2.0, 3.0, -0.0];
        let b = [2.0, 2.0, 2.0, 0.0];
        let expect: [(f64, [f64; 4]); 6] = [
            (0.0, [1.0, 0.0, 0.0, 0.0]),
            (1.0, [1.0, 1.0, 0.0, 1.0]),
            (2.0, [0.0, 1.0, 0.0, 1.0]),
            (3.0, [0.0, 1.0, 1.0, 1.0]),
            (4.0, [0.0, 0.0, 1.0, 0.0]),
            (5.0, [1.0, 0.0, 1.0, 0.0]),
        ];
        for (mode, out) in expect {
            assert_eq!(run(Compare, &[&a, &b], &[&[mode; 4]]), out, "mode {mode}");
        }
        // The mode rounds half up: 3.5 is gt, 3.49 is ge.
        assert_eq!(compare(3.5, 2.0, 2.0), 0.0);
        assert_eq!(compare(3.49, 2.0, 2.0), 1.0);
        assert_eq!(COMPARE.ports[2].kind, PortKind::Gate);
    }

    /// AC7: a when the gate is above 0, else b.
    #[test]
    fn select_by_gate() {
        let g = [1.0, 0.0, -1.0, 1e-300];
        let a = [10.0, 11.0, 12.0, 13.0];
        let b = [20.0, 21.0, 22.0, 23.0];
        assert_eq!(run(Select, &[&g, &a, &b], &[]), [10.0, 21.0, 22.0, 13.0]);
        // Beyond the bound, clamped.
        let big = [1.0, 0.0];
        assert_eq!(
            run(Select, &[&big, &[3e6, 0.0], &[0.0, -3e6]], &[]),
            [1e6, -1e6]
        );
    }

    /// AC7: truth tables, > 0 is true.
    #[test]
    fn and_or_not_truth_tables() {
        let a = [0.0, 0.0, 0.5, 2.0, -1.0];
        let b = [0.0, 1.0, 0.0, 1e-9, 1.0];
        assert_eq!(run(And, &[&a, &b], &[]), [0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(run(Or, &[&a, &b], &[]), [0.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(run(Not, &[&a], &[]), [1.0, 1.0, 0.0, 0.0, 1.0]);
    }
}

//! The core set's vector types and `std.table<L>` (ADR 0027 §1), one type per
//! operation and width: `std.pack4`, `std.unpack16`, `std.index64`,
//! `std.reduce1024`, `std.vadd2`, `std.vmul256`, `std.table8` and so on, the
//! width a suffix as in the converters' `vector4`. Direct, at rate divisor 1,
//! category `core`; every output is clamped to `±1e6`, the engine's magnitude
//! bound (`docs/semantics.md` §7), so they never fault on their own.
//!
//! **Registration.** A port's width is part of a `'static` descriptor, which
//! stable `const` Rust cannot make generic over the width, so each width's
//! descriptors (and their element names `e0…`, `v0…`) are made once per
//! process, the first time [`types`] is asked, and leaked, as the rate
//! converters' are (`signal_core::converter`). The widths are the list in the
//! one invocation of `widths!` below, [`VECTOR_WIDTHS`]. **To add a width**,
//! add it to that list (at most `MAX_VECTOR_WIDTH`); every operation and the
//! table then exist at it. A width not in the list is an unknown module in a
//! project, and a compile error in Rust (`Pack::<3>`; [`describe`]).

/// A table's ends: clamped or wrapped (ADR 0043).
const WRAP_VALUES: &[signal_core::ValueLabel] =
    &[crate::named(0.0, "clamp"), crate::named(1.0, "wrap")];

use std::sync::OnceLock;

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType,
    Outputs, ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive,
};

use crate::math::bounded;
use crate::memory::Table;
use crate::{at, parameter, port_at, signal_port};

/// The operations, each a type per width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Pack,
    Unpack,
    Index,
    Reduce,
    Vadd,
    Vmul,
    Table,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Pack => "pack",
            Op::Unpack => "unpack",
            Op::Index => "index",
            Op::Reduce => "reduce",
            Op::Vadd => "vadd",
            Op::Vmul => "vmul",
            Op::Table => "table",
        }
    }

    fn doc(self) -> &'static str {
        match self {
            Op::Pack => "Packs N signals e0..e(N-1) into a vector of N, element by element.",
            Op::Unpack => "Unpacks a vector of N into N signals e0..e(N-1).",
            Op::Index => {
                "Reads a vector at a fractional element position, linear between neighbours, clamped to [0, N-1]."
            }
            Op::Reduce => "Sum, min, max and mean of a vector's elements, in element order.",
            Op::Vadd => "Elementwise sum of two vectors: out[e] = a[e] + b[e].",
            Op::Vmul => "Elementwise product of two vectors: out[e] = a[e] * b[e].",
            Op::Table => {
                "Read-only table of values v0..v(L-1), read at a fractional position (none, linear or Hermite), clamped or wrapped."
            }
        }
    }
}

/// Declares [`VECTOR_WIDTHS`] and makes every operation's type at each of
/// them: the one list of widths.
macro_rules! widths {
    ($($w:literal),*) => {
        /// The widths every vector operation and `std.table<L>` is registered
        /// at (ADR 0027 §1). Add one here to add a width.
        pub const VECTOR_WIDTHS: &[u32] = &[$($w),*];

        fn make_types() -> Vec<ModuleType> {
            let mut types = Vec::new();
            $(types.extend(family::<$w>());)*
            types
        }
    };
}

widths!(2, 4, 8, 16, 64, 256, 1024);

/// Every vector type and table at every width, made once per process.
pub(crate) fn types() -> &'static [ModuleType] {
    static TYPES: OnceLock<Vec<ModuleType>> = OnceLock::new();
    TYPES.get_or_init(make_types)
}

/// Whether `width` is in `widths` (a `const fn`, for the guard in
/// [`describe`]).
const fn is_width_of(widths: &[u32], width: usize) -> bool {
    match widths {
        [] => false,
        [first, rest @ ..] => *first as usize == width || is_width_of(rest, width),
    }
}

/// The descriptor of `std.<op><N>` (for [`DirectPrimitive::describe`]).
///
/// **An unregistered width cannot be reached silently.** The `const` block
/// is evaluated when `describe::<N>` is instantiated, so `Pack::<3>` or
/// `Table::<5>` used anywhere (a registration, a test) fails to compile
/// ("evaluation panicked: vector width not in VECTOR_WIDTHS"). The lookup
/// therefore always finds the registered descriptor; its fallback, for the
/// type system's sake, makes the same type's descriptor (`op` at `N`),
/// never another type's.
pub(crate) fn describe<const N: usize>(op: Op) -> ModuleDescriptor {
    const {
        assert!(
            is_width_of(VECTOR_WIDTHS, N),
            "vector width not in VECTOR_WIDTHS: add it to `widths!`"
        );
    }
    let name = format!("{}{N}", op.name());
    types()
        .iter()
        .map(|t| t.descriptor)
        .find(|d| d.type_name.strip_prefix("std.") == Some(name.as_str()))
        .unwrap_or_else(|| descriptor(op, N))
}

/// The types of one width.
fn family<const N: usize>() -> [ModuleType; 7] {
    let make = |op, create| ModuleType {
        descriptor: descriptor(op, N),
        create,
    };
    [
        make(Op::Pack, || Primitive::Direct(Box::new(Pack::<N>))),
        make(Op::Unpack, || Primitive::Direct(Box::new(Unpack::<N>))),
        make(Op::Index, || Primitive::Direct(Box::new(Index::<N>))),
        make(Op::Reduce, || Primitive::Direct(Box::new(Reduce::<N>))),
        make(Op::Vadd, || Primitive::Direct(Box::new(Vadd::<N>))),
        make(Op::Vmul, || Primitive::Direct(Box::new(Vmul::<N>))),
        make(Op::Table, || Primitive::Direct(Box::new(Table::<N>))),
    ]
}

/// The leaked element names `prefix0 … prefix{n−1}`, made once per prefix.
fn element_names(prefix: char, n: usize) -> &'static [&'static str] {
    static E: OnceLock<Vec<&'static str>> = OnceLock::new();
    static V: OnceLock<Vec<&'static str>> = OnceLock::new();
    let widest = VECTOR_WIDTHS.iter().copied().max().unwrap_or(0) as usize;
    let names = if prefix == 'e' { &E } else { &V };
    let names = names.get_or_init(|| {
        (0..widest)
            .map(|i| &*Box::leak(format!("{prefix}{i}").into_boxed_str()))
            .collect()
    });
    names.get(..n).unwrap_or(names)
}

/// The descriptor of `op` at `width`, its slices leaked.
fn descriptor(op: Op, width: usize) -> ModuleDescriptor {
    let vector = |name, direction| {
        port_at(
            name,
            direction,
            PortShape::Vector(width as u32),
            1,
            PortKind::Generic,
            None,
            None,
        )
    };
    let signal = |name, direction| signal_port(name, direction, PortKind::Generic, None, None);
    let elements = |direction| {
        element_names('e', width)
            .iter()
            .map(move |&e| signal(e, direction))
    };
    let ports: Vec<PortDescriptor> = match op {
        Op::Pack => elements(Direction::In)
            .chain([vector("out", Direction::Out)])
            .collect(),
        Op::Unpack => [vector("in", Direction::In)]
            .into_iter()
            .chain(elements(Direction::Out))
            .collect(),
        Op::Index => vec![
            vector("in", Direction::In),
            signal("pos", Direction::In),
            signal("out", Direction::Out),
        ],
        Op::Reduce => vec![
            vector("in", Direction::In),
            signal("sum", Direction::Out),
            signal("min", Direction::Out),
            signal("max", Direction::Out),
            signal("mean", Direction::Out),
        ],
        Op::Vadd | Op::Vmul => vec![
            vector("a", Direction::In),
            vector("b", Direction::In),
            vector("out", Direction::Out),
        ],
        Op::Table => vec![signal("pos", Direction::In), signal("out", Direction::Out)],
    };
    let parameters: Vec<ParameterDescriptor> = match op {
        Op::Table => element_names('v', width)
            .iter()
            .map(|&v| {
                parameter(
                    v,
                    PortKind::Generic,
                    None,
                    -crate::GENERIC,
                    crate::GENERIC,
                    0.0,
                    Combine::Add,
                )
            })
            .chain([
                crate::choice(
                    parameter(
                        "interp",
                        PortKind::Generic,
                        None,
                        0.0,
                        2.0,
                        1.0,
                        Combine::Replace,
                    ),
                    crate::INTERP_VALUES,
                ),
                crate::choice(
                    parameter(
                        "wrap",
                        PortKind::Gate,
                        None,
                        0.0,
                        1.0,
                        0.0,
                        Combine::Replace,
                    ),
                    WRAP_VALUES,
                ),
            ])
            .collect(),
        _ => Vec::new(),
    };
    ModuleDescriptor {
        type_name: Box::leak(format!("std.{}{width}", op.name()).into_boxed_str()),
        doc: op.doc(),
        ports: Box::leak(ports.into_boxed_slice()),
        parameters: Box::leak(parameters.into_boxed_slice()),
        feedthrough: Feedthrough::Direct,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: Some(crate::category::CORE),
    }
}

/// `out[r] = clamp(f(a[r], b[r]), ±1e6)` over every row of the output.
fn elementwise(outputs: &mut Outputs<'_>, a: &[f64], b: &[f64], f: fn(f64, f64) -> f64) {
    for (r, out) in outputs.signals().flatten().enumerate() {
        *out = bounded(f(at(a, r, 0.0), at(b, r, 0.0)));
    }
}

/// `std.pack<N>` (e.g. `std.pack4`): N signals into a vector.
///
/// **Ports:** `e0` … `e{N−1}` (signal in, generic; unconnected read 0),
/// `out` (vector of N out, generic). **Parameters:** none. **Feedthrough:**
/// direct.
///
/// **Formula:** `out[e][n] = clamp(e_e[n], ±1e6)`. Constant work per element
/// and frame.
#[derive(Clone, Debug, Default)]
pub struct Pack<const N: usize>;

/// `std.unpack<N>` (e.g. `std.unpack4`): a vector into N signals.
///
/// **Ports:** `in` (vector of N in, generic; unconnected reads 0), `e0` …
/// `e{N−1}` (signal out, generic). **Parameters:** none. **Feedthrough:**
/// direct.
///
/// **Formula:** `e_e[n] = clamp(in[e][n], ±1e6)`. Constant work per element
/// and frame.
#[derive(Clone, Debug, Default)]
pub struct Unpack<const N: usize>;

/// `std.vadd<N>` (e.g. `std.vadd4`): the elementwise sum of two vectors.
///
/// **Ports:** `a`, `b` (vector of N in, generic; unconnected read 0), `out`
/// (vector of N out, generic). **Parameters:** none. **Feedthrough:** direct.
///
/// **Formula:** `out[e][n] = clamp(a[e][n] + b[e][n], ±1e6)`. Commutative.
/// Constant work per element and frame.
#[derive(Clone, Debug, Default)]
pub struct Vadd<const N: usize>;

/// `std.vmul<N>` (e.g. `std.vmul4`): the elementwise product of two vectors.
///
/// **Ports:** `a`, `b` (vector of N in, generic; unconnected read 0), `out`
/// (vector of N out, generic). **Parameters:** none. **Feedthrough:** direct.
///
/// **Formula:** `out[e][n] = clamp(a[e][n]·b[e][n], ±1e6)`. Commutative.
/// Constant work per element and frame.
#[derive(Clone, Debug, Default)]
pub struct Vmul<const N: usize>;

/// `std.index<N>` (e.g. `std.index4`): a vector read at a fractional element
/// position.
///
/// **Ports:** `in` (vector of N in, generic), `pos` (signal in, generic, in
/// elements), `out` (signal out, generic); unconnected inputs read 0.
/// **Parameters:** none. **Feedthrough:** direct.
///
/// **Formula** (`x_e = in[e][n]`, the element as received, a fan-in sum
/// included): `p = clamp(pos[n], 0, N − 1)`, `i = ⌊p⌋`, `t = p − i`,
/// `out[n] = clamp((1 − t)·x_i + t·x_{min(i+1, N−1)}, ±1e6)`, and exactly
/// `clamp(x_i, ±1e6)` at an integer position (`t = 0`). Each weighted term
/// is at most its element in magnitude, so finite elements never overflow on
/// the way (as `x_i + t·(x_{i+1} − x_i)` could, the difference of two large
/// elements of opposite sign); the sum overflows only where the result
/// itself is beyond the double range, and the clamp then gives its sign. Only
/// the result is clamped, so the type is total and never faults. Constant
/// work per frame.
#[derive(Clone, Debug, Default)]
pub struct Index<const N: usize>;

/// `std.reduce<N>` (e.g. `std.reduce4`): a vector's sum, minimum, maximum
/// and mean.
///
/// **Ports:** `in` (vector of N in, generic; unconnected reads 0), `sum`,
/// `min`, `max`, `mean` (signal out, generic). **Parameters:** none.
/// **Feedthrough:** direct.
///
/// **Formula** (`x_e = in[e][n]`, the element as received, a fan-in sum
/// included; folded in element order `e = 0 … N − 1`, so the result never
/// depends on anything else): `s = ((x_0 + x_1) + …) + x_{N−1}`;
/// `sum[n] = clamp(s, ±1e6)`, `mean[n] = clamp(s / N, ±1e6)`, `min[n]` and
/// `max[n]` the smallest and largest `x_e`, clamped. Only the results are
/// clamped, so elements beyond the bound still count (`[2e6, −1e6]` sums to
/// 1e6). The sum of finite elements is finite or, past `f64::MAX`, stays
/// `±∞`, which the clamp maps to `±1e6`, so the type is total and never
/// faults. Work proportional to N per frame.
#[derive(Clone, Debug, Default)]
pub struct Reduce<const N: usize>;

/// The `DirectPrimitive` impl shared by every vector type.
macro_rules! vector_type {
    ($name:ident, $op:expr, |$s:ident, $inputs:ident, $outputs:ident| $process:block) => {
        impl<const N: usize> DirectPrimitive for $name<N> {
            fn describe() -> ModuleDescriptor {
                describe::<N>($op)
            }

            fn prepare(&mut self, _ctx: &PrepareContext) {}

            fn reset(&mut self) {}

            fn process(&mut self, $inputs: &Inputs<'_>, $outputs: &mut Outputs<'_>) {
                let $s = N;
                $process
            }
        }
    };
}

vector_type!(Pack, Op::Pack, |n, inputs, outputs| {
    elementwise(outputs, inputs.rows(0, n), &[], |x, _| x);
});

vector_type!(Unpack, Op::Unpack, |n, inputs, outputs| {
    elementwise(outputs, inputs.rows(0, n), &[], |x, _| x);
});

vector_type!(Vadd, Op::Vadd, |n, inputs, outputs| {
    elementwise(outputs, inputs.rows(0, n), inputs.rows(n, n), |a, b| a + b);
});

vector_type!(Vmul, Op::Vmul, |n, inputs, outputs| {
    elementwise(outputs, inputs.rows(0, n), inputs.rows(n, n), |a, b| a * b);
});

vector_type!(Index, Op::Index, |n, inputs, outputs| {
    let (k, x, pos) = (inputs.frames(), inputs.rows(0, n), inputs.signal(n));
    let last = n.saturating_sub(1);
    for (f, out) in outputs.signal(0).iter_mut().enumerate() {
        let p = at(pos, f, 0.0).clamp(0.0, last as f64);
        let whole = libm::floor(p);
        let i = whole as usize;
        let t = p - whole;
        let y0 = at(x, i * k + f, 0.0);
        *out = bounded(if t == 0.0 {
            y0
        } else {
            // Each term is at most its element in magnitude, so finite
            // neighbours never overflow (a difference of two could).
            (1.0 - t) * y0 + t * at(x, (i + 1).min(last) * k + f, 0.0)
        });
    }
});

vector_type!(Reduce, Op::Reduce, |n, inputs, outputs| {
    let (k, x) = (inputs.frames(), inputs.rows(0, n));
    let rows = outputs.rows(0, 4);
    for f in 0..k {
        let (mut sum, mut min, mut max) = (0.0, f64::INFINITY, f64::NEG_INFINITY);
        for e in 0..n {
            let v = at(x, e * k + f, 0.0);
            sum += v;
            min = min.min(v);
            max = max.max(v);
        }
        let mean = if n > 0 { sum / n as f64 } else { 0.0 };
        if n == 0 {
            (min, max) = (0.0, 0.0);
        }
        for (r, v) in [sum, min, max, mean].into_iter().enumerate() {
            if let Some(out) = rows.get_mut(r * k + f) {
                *out = bounded(v);
            }
        }
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> &'static ModuleType {
        types()
            .iter()
            .find(|t| t.descriptor.type_name == name)
            .unwrap_or_else(|| panic!("{name}"))
    }

    /// Runs `name` for `frames` frames on `signals` (rows, frame-major per row).
    fn run(name: &str, frames: usize, signals: &[f64], parameters: &[f64]) -> Vec<f64> {
        let t = find(name);
        let rows: usize = t
            .descriptor
            .ports
            .iter()
            .filter(|p| p.direction == Direction::Out)
            .map(|p| p.shape.rows())
            .sum();
        let mut out = vec![f64::NAN; rows * frames];
        match (t.create)() {
            Primitive::Direct(mut p) => p.process(
                &Inputs::new(frames, signals, parameters, &[], &[]),
                &mut Outputs::new(frames, &mut out, &mut []),
            ),
            Primitive::StateOutput(_) => unreachable!(),
        }
        out
    }

    /// AC6: every operation and the table exist at every width, named with
    /// the width as a suffix, and nothing else does.
    #[test]
    fn every_width_is_registered() {
        assert_eq!(VECTOR_WIDTHS, [2, 4, 8, 16, 64, 256, 1024]);
        assert_eq!(types().len(), 7 * VECTOR_WIDTHS.len());
        for w in VECTOR_WIDTHS {
            for op in ["pack", "unpack", "index", "reduce", "vadd", "vmul", "table"] {
                let d = find(&format!("std.{op}{w}")).descriptor;
                assert_eq!(d.category, Some(crate::category::CORE));
            }
            let pack = find(&format!("std.pack{w}")).descriptor;
            assert_eq!(pack.ports.len(), *w as usize + 1);
            assert_eq!(pack.ports.last().unwrap().shape, PortShape::Vector(*w));
            assert_eq!(
                find(&format!("std.table{w}")).descriptor.parameters.len(),
                *w as usize + 2
            );
        }
        assert!(
            types()
                .iter()
                .all(|t| t.descriptor.type_name != "std.pack3")
        );
        // Made once: the same leaked descriptor every time.
        assert!(std::ptr::eq(types(), types()));
        assert_eq!(Pack::<4>::describe().type_name, "std.pack4");
        assert_eq!(Table::<16>::describe().type_name, "std.table16");
    }

    /// AC6, AC10: pack and unpack copy, clamped; vadd and vmul are
    /// elementwise, clamped.
    #[test]
    fn pack_unpack_vadd_vmul() {
        // Two frames, rows e0, e1.
        assert_eq!(
            run("std.pack2", 2, &[1.0, 2.0, 3e6, -4.0], &[]),
            [1.0, 2.0, 1e6, -4.0]
        );
        assert_eq!(
            run("std.unpack2", 2, &[1.0, 2.0, -3e6, 4.0], &[]),
            [1.0, 2.0, -1e6, 4.0]
        );
        let ab = [1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 1e6, 1e6];
        assert_eq!(run("std.vadd2", 2, &ab, &[]), [11.0, 22.0, 1e6, 1e6]);
        assert_eq!(run("std.vmul2", 2, &ab, &[]), [10.0, 40.0, 1e6, 1e6]);
    }

    /// AC6: index reads linearly between elements, exact at integers,
    /// clamped to the ends.
    #[test]
    fn index_reads_a_fractional_element() {
        // One frame per position: rows e0..e3, then pos.
        let mut signals = Vec::new();
        let pos = [0.0, 1.5, 2.25, 3.0, -1.0, 9.0];
        for e in [1.0, 2.0, 4.0, 8.0] {
            signals.extend([e; 6]);
        }
        signals.extend(pos);
        assert_eq!(
            run("std.index4", 6, &signals, &[]),
            [1.0, 3.0, 5.0, 8.0, 1.0, 8.0]
        );
    }

    /// AC6, AC10: reduce in element order, its sum clamped at width 1024.
    #[test]
    fn reduce_sums_in_element_order_and_clamps() {
        let out = run("std.reduce4", 1, &[1.0, -2.0, 4.0, 0.5], &[]);
        assert_eq!(out, [3.5, -2.0, 4.0, 0.875]);
        // Elements beyond the bound (a fan-in sum) count as received; only
        // the results are clamped (auditor-1-vector-premature-clamping).
        let out = run("std.reduce2", 1, &[2e6, -1e6], &[]);
        assert_eq!(out, [1e6, -1e6, 1e6, 5e5]);
        let out = run(
            "std.index2",
            3,
            &[2e6, 2e6, 2e6, -1e6, -1e6, -1e6, 0.5, 0.0, 1.0],
            &[],
        );
        assert_eq!(out, [5e5, 1e6, -1e6]);
        // Totality at the extremes: a sum past f64::MAX clamps; index of
        // extreme finite neighbours, at integer and fractional positions
        // (auditor-1-index-intermediate-overflow: their difference overflows,
        // the weighted terms do not), is the true interpolation, clamped.
        let out = run("std.reduce2", 1, &[f64::MAX, f64::MAX], &[]);
        assert_eq!(out, [1e6, 1e6, 1e6, 1e6]);
        let extremes = [[f64::MAX; 4], [-f64::MAX; 4], [0.0, 0.5, 0.75, 1.0]].concat();
        let out = run("std.index2", 4, &extremes, &[]);
        assert_eq!(out, [1e6, 0.0, -1e6, -1e6]);
        let out = run("std.reduce1024", 1, &[1e6; 1024], &[]);
        assert_eq!(out, [1e6, 1e6, 1e6, 1e6]);
        // Order matters in floating point: (1e16 + 1) − 1e16 vs 1 + (1e16 − 1e16).
        let ordered = run("std.reduce4", 1, &[0.1, 0.2, 0.3, -0.6], &[]);
        assert_eq!(ordered[0], ((0.1 + 0.2) + 0.3) - 0.6);
    }

    /// AC3: a table is read through its registered type, at its width.
    #[test]
    fn table_types_read_their_values() {
        let mut parameters = vec![0.0, 10.0];
        parameters.extend([1.0, 0.0]);
        assert_eq!(run("std.table2", 1, &[0.5], &parameters), [5.0]);
    }
}

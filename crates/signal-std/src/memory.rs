//! `std.buffer<C>` and `std.table<L>`: the core set's memory (ADR 0027 §1).
//! Direct, at rate divisor 1, category `core`. Values are clamped to `±1e6`,
//! the engine's magnitude bound (`docs/semantics.md` §7), when they are
//! written and when they are read, so neither type faults on its own.
//!
//! **Interpolation** (both types, parameter `interp`, rounded with
//! `round_half_up`): 0 none (the element at `⌊p⌋`), 1 linear, 2 Hermite,
//! `std.delay`'s 4-point, 3rd-order (Catmull-Rom) interpolator. With
//! `y₋₁ … y₂` the elements at `⌊p⌋ − 1 … ⌊p⌋ + 2` and `t = p − ⌊p⌋`:
//!
//! ```text
//! linear  = y₀ + t·(y₁ − y₀)
//! c₁ = ½·(y₁ − y₋₁),  c₂ = y₋₁ − 2.5·y₀ + 2·y₁ − ½·y₂,  c₃ = ½·(y₂ − y₋₁) + 1.5·(y₀ − y₁)
//! hermite = ((c₃·t + c₂)·t + c₁)·t + y₀
//! ```
//!
//! Each is exact at an integer position (`t = 0`). The elements are within
//! `±1e6` (a buffer clamps on write, a table's values are parameters of that
//! range), so no difference or coefficient of either formula can overflow. The neighbours wrap
//! around the buffer, and are clamped to the ends of a table unless it wraps.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortDescriptor, PortKind, PrepareContext,
};

use crate::math::bounded;
use crate::{at, parameter, round_half_up, signal_port};

/// The interpolation `interp` selects: 0 none, 1 linear, 2 Hermite.
pub(crate) fn interpolation(interp: f64) -> u8 {
    round_half_up(interp).clamp(0.0, 2.0) as u8
}

/// The value at position `p` (`⌊p⌋` is an element), interpolated as
/// `mode` says, of the elements `element` gives (it maps every integer to an
/// element, wrapping or clamping).
pub(crate) fn interpolate(mode: u8, p: f64, element: impl Fn(i64) -> f64) -> f64 {
    let whole = libm::floor(p);
    let t = p - whole;
    let i = whole as i64;
    match mode {
        0 => element(i),
        1 => {
            let y0 = element(i);
            y0 + t * (element(i + 1) - y0)
        }
        _ => {
            let (ym1, y0, y1, y2) = (element(i - 1), element(i), element(i + 1), element(i + 2));
            let c1 = 0.5 * (y1 - ym1);
            let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
            let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
            ((c3 * t + c2) * t + c1) * t + y0
        }
    }
}

/// `x` modulo `len` (floored), in `[0, len)`: a remainder that rounds up to
/// `len` (a tiny negative `x`) is 0.
pub(crate) fn wrapped(x: f64, len: usize) -> f64 {
    let len = len as f64;
    let p = x.rem_euclid(len);
    if p >= len { 0.0 } else { p }
}

/// The capacities `std.buffer<C>` is registered at, in frames: `std.buffer1k`
/// (2¹⁰), `std.buffer64k` (2¹⁶) and `std.buffer1m` (2²⁰, 8 MiB, about
/// `std.delay`'s buffer at 96 kHz; 21.8 s at 48 kHz). The capacity is the
/// type's: `prepare` cannot see parameter values (`PrepareContext`), so the
/// memory it allocates is fixed by the type name, and `size` chooses how
/// much of it is in use, frame by frame. **To add a capacity**, add it here
/// and give it a descriptor in [`Buffer::DESCRIPTOR`] and a registration in
/// `register`; an unlisted capacity does not compile.
pub const BUFFER_CAPACITIES: [usize; 3] = [1 << 10, 1 << 16, 1 << 20];

/// `std.buffer`'s read taps.
const TAPS: usize = 4;

const BUFFER_DOC: &str = "Circular memory of `size` frames (read every frame, up to the type's capacity): writes value at index while gate > 0, then four taps read at fractional positions (none, linear or Hermite).";

const BUFFER_PORTS: &[PortDescriptor] = &[
    signal_port("index", Direction::In, PortKind::Generic, None, None),
    signal_port("value", Direction::In, PortKind::Generic, None, None),
    signal_port("gate", Direction::In, PortKind::Gate, None, None),
    signal_port("read0", Direction::In, PortKind::Generic, None, None),
    signal_port("read1", Direction::In, PortKind::Generic, None, None),
    signal_port("read2", Direction::In, PortKind::Generic, None, None),
    signal_port("read3", Direction::In, PortKind::Generic, None, None),
    signal_port("out0", Direction::Out, PortKind::Generic, None, None),
    signal_port("out1", Direction::Out, PortKind::Generic, None, None),
    signal_port("out2", Direction::Out, PortKind::Generic, None, None),
    signal_port("out3", Direction::Out, PortKind::Generic, None, None),
];

/// A buffer's largest `size`: its capacity, at most 10⁶ frames (ADR 0043:
/// within the engine's magnitude bound; `std.buffer1m` holds 2²⁰ frames but
/// takes a size up to 10⁶).
const fn size_max(capacity: usize) -> f64 {
    if capacity > 1_000_000 {
        1_000_000.0
    } else {
        capacity as f64
    }
}

/// The `'static` descriptor of the buffer named `$name` of `$capacity`
/// frames: `size` ranges over `[1, capacity]` and defaults to the capacity.
macro_rules! buffer_descriptor {
    ($name:literal, $capacity:expr) => {
        ModuleDescriptor {
            type_name: $name,
            doc: BUFFER_DOC,
            ports: BUFFER_PORTS,
            parameters: &[
                parameter(
                    "size",
                    PortKind::Generic,
                    Some("frames"),
                    1.0,
                    size_max($capacity),
                    size_max($capacity),
                    Combine::Replace,
                ),
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
            ],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: Some(crate::category::CORE),
        }
    };
}

const BUFFER_1K: ModuleDescriptor = buffer_descriptor!("std.buffer1k", BUFFER_CAPACITIES[0]);
const BUFFER_64K: ModuleDescriptor = buffer_descriptor!("std.buffer64k", BUFFER_CAPACITIES[1]);
const BUFFER_1M: ModuleDescriptor = buffer_descriptor!("std.buffer1m", BUFFER_CAPACITIES[2]);

/// `std.buffer1k`: a [`Buffer`] of 1024 frames.
pub type Buffer1k = Buffer<{ BUFFER_CAPACITIES[0] }>;
/// `std.buffer64k`: a [`Buffer`] of 65 536 frames.
pub type Buffer64k = Buffer<{ BUFFER_CAPACITIES[1] }>;
/// `std.buffer1m`: a [`Buffer`] of 1 048 576 frames.
pub type Buffer1m = Buffer<{ BUFFER_CAPACITIES[2] }>;

/// The frames in use on a frame whose `size` is `size`: `round_half_up`,
/// clamped to `[1, C]` (a NaN is 1).
fn frames_in_use(size: f64, capacity: usize) -> usize {
    (round_half_up(size).clamp(1.0, capacity as f64) as usize).clamp(1, capacity)
}

/// `std.buffer<C>` (`std.buffer1k`, `std.buffer64k`, `std.buffer1m`; C one of
/// [`BUFFER_CAPACITIES`]): a circular memory of C frames, of which the first
/// N(n) are in use on frame *n*, with one write and four reads.
///
/// **Ports**
/// - `index` (signal in, generic): the write position; `⌊index⌋ mod N`.
/// - `value` (signal in, generic): the value written, clamped to `±1e6`.
/// - `gate` (signal in, gate): writes on frames where `gate > 0`.
/// - `read0` … `read3` (signal in, generic): the read positions of the four
///   taps, fractional, modulo N.
/// - `out0` … `out3` (signal out, generic): the taps.
///
/// Unconnected inputs read 0, so an unconnected gate never writes.
///
/// **Parameters**
/// - `size` (generic, frames, range `[1, C]`, default C, combine `replace`):
///   N(n), rounded with `round_half_up` and clamped to `[1, C]`, **read every
///   frame** like any other parameter: a `set`, a ramp, a modulation or a
///   live edit of the file takes effect on its frame, so the running instance
///   always uses the N its file and connections say (AGENTS.md rule 4).
/// - `interp` (generic, choice, range `[0, 2]`, default 1, combine
///   `replace`): 0 none, 1 linear, 2 Hermite (the module documentation).
///
/// **Feedthrough:** direct (a tap reads this frame's write).
///
/// **Formula** (memory `m` of C frames, zero after `prepare`; per frame *n*,
/// N = N(n)):
///
/// ```text
/// write:  if gate[n] > 0:  m[⌊index[n]⌋ mod N] = clamp(value[n], ±1e6)
/// read:   out_r[n] = clamp(interp(m[0..N], read_r[n] mod N), ±1e6)    r = 0 … 3
/// ```
///
/// **Read-after-write within a frame:** the write comes first, so a tap
/// reading the written element on the same frame reads the new value; a
/// loop through a buffer therefore needs a `std.unit_delay`. Positions and
/// interpolation neighbours wrap around the N frames in use.
///
/// **When N changes:** the buffer holds no position of its own (the write
/// and read positions are inputs), so every position is simply taken modulo
/// the new N from that frame on. The contents are kept: shrinking N hides
/// the elements at N and above without clearing them, and growing it again
/// shows them as they were (zero if never written).
///
/// **Live swap:** a surviving instance keeps its contents (ADR 0006,
/// `docs/semantics.md` §12), and its N follows `size` as on every frame; an
/// added one starts at zero.
///
/// **Allocation and memory:** only in `prepare`, which allocates C frames
/// and writes every one of them (a store of an opaque zero, which the
/// compiler cannot turn into a lazily zeroed allocation), so the pages are
/// touched on the control thread and `process` takes no first-touch page
/// fault (assumption: the system may still compress or page out memory
/// under pressure; nothing locks it). `reset` zeroes the elements written
/// since `prepare` or the last `reset` (up to the highest written index),
/// bounded work. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Buffer<const C: usize> {
    memory: Vec<f64>,
    /// One past the highest index written since `prepare` or `reset`: what
    /// `reset` must zero.
    touched: usize,
}

impl<const C: usize> Buffer<C> {
    /// The type's descriptor. A capacity not in [`BUFFER_CAPACITIES`] has
    /// none and fails to compile where `describe` is instantiated.
    pub const DESCRIPTOR: ModuleDescriptor = match C {
        1024 => BUFFER_1K,
        65_536 => BUFFER_64K,
        1_048_576 => BUFFER_1M,
        _ => panic!("std.buffer: capacity not in BUFFER_CAPACITIES"),
    };
}

impl<const C: usize> DirectPrimitive for Buffer<C> {
    fn describe() -> ModuleDescriptor {
        Self::DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        if self.memory.len() != C {
            self.memory = Vec::with_capacity(C);
        }
        // Every element stored, with a value the optimiser cannot see, so the
        // pages are touched here and not on the audio thread.
        let zero = std::hint::black_box(0.0);
        self.memory.clear();
        self.memory.resize(C, zero);
        self.touched = 0;
    }

    fn reset(&mut self) {
        if let Some(used) = self.memory.get_mut(..self.touched) {
            used.fill(0.0);
        }
        self.touched = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (index, value, gate) = (inputs.signal(0), inputs.signal(1), inputs.signal(2));
        let (size, interp) = (inputs.parameter(0), inputs.parameter(1));
        for n in 0..inputs.frames() {
            // N; 0 only before `prepare`, when there is no memory.
            let len = frames_in_use(at(size, n, C as f64), C).min(self.memory.len());
            if len > 0 && at(gate, n, 0.0) > 0.0 {
                let w = wrapped(libm::floor(at(index, n, 0.0)), len) as usize;
                if let Some(slot) = self.memory.get_mut(w) {
                    *slot = bounded(at(value, n, 0.0));
                    self.touched = self.touched.max(w + 1);
                }
            }
            let mode = interpolation(at(interp, n, 1.0));
            let memory = &self.memory;
            let element = |i: i64| {
                let i = i.rem_euclid(len.max(1) as i64) as usize;
                memory.get(i).copied().unwrap_or(0.0)
            };
            for tap in 0..TAPS {
                let y = if len > 0 {
                    let p = wrapped(at(inputs.signal(3 + tap), n, 0.0), len);
                    bounded(interpolate(mode, p, element))
                } else {
                    0.0
                };
                if let Some(out) = outputs.signal(tap).get_mut(n) {
                    *out = y;
                }
            }
        }
    }
}

/// `std.table<L>` (L one of [`VECTOR_WIDTHS`](crate::VECTOR_WIDTHS), e.g.
/// `std.table16`): a read-only table of L values, read at a fractional
/// position. Its descriptor is made by `crate::vector`.
///
/// **Ports**
/// - `pos` (signal in, generic): the position, in elements; unconnected it
///   reads 0.
/// - `out` (signal out, generic).
///
/// **Parameters**
/// - `v0` … `v{L−1}` (generic, range `[−10⁵, 10⁵]` (ADR 0043), default 0, combine
///   `add`): the values, the instance's parameters in the project file,
///   validated when the project loads (out of range is an error).
/// - `interp` (generic, choice, range `[0, 2]`, default 1, combine
///   `replace`): 0 none, 1 linear, 2 Hermite (the module documentation).
/// - `wrap` (gate, range `[0, 1]`, default 0, combine `replace`): above 0,
///   the position and the neighbours wrap modulo L (a cycle, e.g. a
///   wavetable); otherwise the position is clamped to `[0, L − 1]` and the
///   neighbours to the ends.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = clamp(interp(v, pos[n]), ±1e6)`, with the values
/// of frame *n*: a Hermite read can overshoot the values, and is clamped.
/// Read-only and stateless; it allocates nothing. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Table<const L: usize>;

impl<const L: usize> DirectPrimitive for Table<L> {
    fn describe() -> ModuleDescriptor {
        crate::vector::describe::<L>(crate::vector::Op::Table)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let pos = inputs.signal(0);
        let (interp, wrap) = (inputs.parameter(L), inputs.parameter(L + 1));
        let last = L.saturating_sub(1);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let wraps = at(wrap, n, 0.0) > 0.0;
            let element = |i: i64| {
                let i = if wraps {
                    i.rem_euclid(L.max(1) as i64) as usize
                } else {
                    i.clamp(0, last as i64) as usize
                };
                at(inputs.parameter(i), n, 0.0)
            };
            let x = at(pos, n, 0.0);
            let p = if wraps {
                wrapped(x, L.max(1))
            } else {
                x.clamp(0.0, last as f64)
            };
            *out = bounded(interpolate(interpolation(at(interp, n, 1.0)), p, element));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;

    /// A buffer's run of `frames` frames in calls of `block`: its four taps.
    struct Feed<'a> {
        index: &'a [f64],
        value: &'a [f64],
        gate: &'a [f64],
        reads: [&'a [f64]; 4],
        size: &'a [f64],
        interp: f64,
    }

    fn run(buffer: &mut Buffer1k, feed: &Feed<'_>, block: usize) -> Vec<Vec<f64>> {
        let frames = feed.index.len();
        let mut taps = vec![Vec::new(); TAPS];
        let mut start = 0;
        while start < frames {
            let k = block.min(frames - start);
            let rows = [feed.index, feed.value, feed.gate]
                .into_iter()
                .chain(feed.reads);
            let signals: Vec<f64> = rows.flat_map(|r| r[start..start + k].to_vec()).collect();
            let mut parameters = feed.size[start..start + k].to_vec();
            parameters.extend(vec![feed.interp; k]);
            let mut out = vec![f64::NAN; TAPS * k];
            buffer.process(
                &Inputs::new(k, &signals, &parameters, &[], &[]),
                &mut Outputs::new(k, &mut out, &mut []),
            );
            for (tap, chunk) in taps.iter_mut().zip(out.chunks(k)) {
                tap.extend_from_slice(chunk);
            }
            start += k;
        }
        taps
    }

    fn prepared() -> Buffer1k {
        let mut b = Buffer1k::default();
        b.prepare(&context(48_000.0, 8, 0));
        b
    }

    /// AC2: the write comes before the reads, so a tap at the written index
    /// reads the new value on the same frame; gate 0 writes nothing; indices
    /// wrap modulo N.
    #[test]
    fn writes_before_it_reads_and_wraps() {
        let mut b = prepared();
        let at_write = [0.0, 1.0, 2.0, 1.0, 9.0];
        let feed = Feed {
            index: &[0.0, 1.0, 2.0, 1.0, 9.0],
            value: &[10.0, 11.0, 12.0, 13.0, 14.0],
            gate: &[1.0, 1.0, 1.0, 0.0, 1.0],
            reads: [&at_write, &[0.0; 5], &[-1.0; 5], &[4.0; 5]],
            size: &[4.0; 5],
            interp: 0.0,
        };
        let taps = run(&mut b, &feed, 2);
        assert_eq!(taps[0], [10.0, 11.0, 12.0, 11.0, 14.0]);
        assert_eq!(taps[1], [10.0, 10.0, 10.0, 10.0, 10.0]);
        // −1 is element 3, never written; 4 is element 0.
        assert_eq!(taps[2], [0.0; 5]);
        assert_eq!(taps[3], [10.0; 5]);
        // Index 9 mod 4 = 1: the last write replaced element 1.
        assert_eq!(b.memory[..4], [10.0, 14.0, 12.0, 0.0]);
    }

    /// AC2: none, linear and Hermite read the same elements at integer
    /// positions, and differ between them as their formulas say.
    #[test]
    fn interpolates_none_linear_and_hermite() {
        let element = |i: i64| [0.0, 1.0, 4.0, 9.0][i.rem_euclid(4) as usize];
        for mode in 0..3 {
            for i in 0..4 {
                assert_eq!(interpolate(mode, i as f64, element), element(i));
            }
        }
        assert_eq!(interpolate(0, 1.5, element), 1.0);
        assert_eq!(interpolate(1, 1.5, element), 2.5);
        // Catmull-Rom of 0, 1, 4, 9 at ½: exact for this quadratic, 2.25.
        assert_eq!(interpolate(2, 1.5, element), 2.25);
    }

    /// `size` is read every frame (review decision 1): a write at index 5
    /// lands at 5 mod N(n), and a read at 5 reads 5 mod N(n). Shrinking N
    /// hides element 5 without clearing it; growing N shows it again.
    #[test]
    fn size_is_read_every_frame() {
        let mut b = prepared();
        let feed = Feed {
            index: &[5.0, 5.0, 0.0, 0.0, 0.0],
            value: &[1.0, 2.0, 0.0, 0.0, 0.0],
            gate: &[1.0, 1.0, 0.0, 0.0, 0.0],
            reads: [&[5.0; 5], &[1.0; 5], &[0.0; 5], &[0.0; 5]],
            size: &[8.0, 4.0, 4.0, 8.0, 3.6],
            interp: 0.0,
        };
        let taps = run(&mut b, &feed, 2);
        // Frame 0: N = 8, element 5 = 1. Frame 1: N = 4, element 1 = 2.
        // Frame 2: N = 4, a read at 5 is element 1. Frame 3: N = 8, element 5
        // is still 1. Frame 4: 3.6 rounds to N = 4.
        assert_eq!(taps[0], [1.0, 2.0, 2.0, 1.0, 2.0]);
        assert_eq!(taps[1], [0.0, 2.0, 2.0, 2.0, 2.0]);
        assert_eq!(b.memory[..8], [0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // Out of range sizes clamp to [1, C]; a NaN is 1.
        assert_eq!(frames_in_use(0.0, 1024), 1);
        assert_eq!(frames_in_use(5e6, 1024), 1024);
        assert_eq!(frames_in_use(f64::NAN, 1024), 1);
        assert_eq!(frames_in_use(2.5, 1024), 3);
        // The same run in one call and frame by frame gives the same taps.
        let mut whole = prepared();
        assert_eq!(run(&mut whole, &feed, 5), taps);
        let mut single = prepared();
        assert_eq!(run(&mut single, &feed, 1), taps);
    }

    /// `reset` returns to the state `prepare` left: every written element
    /// zero, including those beyond the current N.
    #[test]
    fn reset_zeroes_every_written_element() {
        let mut b = prepared();
        let feed = Feed {
            index: &[700.0, 3.0],
            value: &[1.0, 2.0],
            gate: &[1.0, 1.0],
            reads: [&[0.0; 2]; 4],
            size: &[1024.0, 4.0],
            interp: 0.0,
        };
        run(&mut b, &feed, 2);
        assert_eq!((b.memory[700], b.memory[3], b.touched), (1.0, 2.0, 701));
        b.reset();
        assert_eq!(b.touched, 0);
        assert!(b.memory.iter().all(|&v| v == 0.0));
    }

    /// `prepare` allocates the type's capacity and writes every element
    /// (the prefault); a second `prepare` zeroes it again without a new
    /// allocation. That the pages are resident cannot be checked portably
    /// (it needs `mincore`, i.e. `unsafe`); the store of an opaque zero is
    /// what makes it so (the type's documentation).
    #[test]
    fn prepare_allocates_and_writes_the_capacity() {
        let mut b = Buffer64k::default();
        b.prepare(&context(48_000.0, 8, 0));
        assert_eq!(b.memory.len(), 65_536);
        assert!(b.memory.iter().all(|&v| v == 0.0));
        b.memory[65_535] = 3.0;
        b.touched = 65_536;
        let before = b.memory.as_ptr();
        b.prepare(&context(48_000.0, 8, 0));
        assert_eq!(b.memory.as_ptr(), before, "no second allocation");
        assert!(b.memory.iter().all(|&v| v == 0.0));
        assert_eq!(b.touched, 0);
        assert_eq!(Buffer1k::describe().type_name, "std.buffer1k");
        assert_eq!(Buffer1m::describe().type_name, "std.buffer1m");
        for (d, capacity) in [
            (Buffer1k::describe(), 1024.0),
            (Buffer64k::describe(), 65_536.0),
            // ADR 0043: a size within the engine's bound, 10⁶ of its 2²⁰.
            (Buffer1m::describe(), 1_000_000.0),
        ] {
            let size = &d.parameters[0];
            assert_eq!((size.range.max, size.default), (capacity, capacity));
        }
    }

    /// AC10: writes and reads are clamped to ±1e6, including a Hermite
    /// overshoot.
    #[test]
    fn values_are_clamped_on_write_and_read() {
        let mut b = prepared();
        let feed = Feed {
            index: &[0.0, 1.0, 2.0, 3.0, 0.0],
            value: &[0.0, 1e6, 5e6, 0.0, 0.0],
            gate: &[1.0, 1.0, 1.0, 1.0, 0.0],
            reads: [&[2.0; 5], &[1.5; 5], &[0.0; 5], &[0.0; 5]],
            size: &[4.0; 5],
            interp: 2.0,
        };
        let taps = run(&mut b, &feed, 5);
        assert_eq!(taps[0][4], 1e6);
        let unclamped = interpolate(2, 1.5, |i| [0.0, 1e6, 1e6, 0.0][i.rem_euclid(4) as usize]);
        assert!(unclamped > 1e6);
        assert_eq!(taps[1][4], 1e6);
    }

    fn table4(values: [f64; 4], pos: &[f64], interp: f64, wrap: f64) -> Vec<f64> {
        let k = pos.len();
        let mut parameters: Vec<f64> = values.iter().flat_map(|&v| vec![v; k]).collect();
        parameters.extend(vec![interp; k]);
        parameters.extend(vec![wrap; k]);
        let mut out = vec![f64::NAN; k];
        Table::<4>.process(
            &Inputs::new(k, pos, &parameters, &[], &[]),
            &mut Outputs::new(k, &mut out, &mut []),
        );
        out
    }

    /// AC3: a table reads its parameter values at fractional positions,
    /// clamped to the ends or wrapped, interpolated, and clamped to ±1e6.
    #[test]
    fn table_reads_its_values() {
        let v = [1.0, 2.0, 4.0, 8.0];
        assert_eq!(
            table4(v, &[0.0, 1.5, 2.25, -3.0, 7.0], 1.0, 0.0),
            [1.0, 3.0, 5.0, 1.0, 8.0]
        );
        assert_eq!(table4(v, &[3.5, -0.5, 5.0], 1.0, 1.0), [4.5, 4.5, 2.0]);
        assert_eq!(table4(v, &[1.75], 0.0, 0.0), [2.0]);
        let overshoot = table4([0.0, 1e6, 1e6, 0.0], &[1.5], 2.0, 1.0);
        assert_eq!(overshoot, [1e6]);
    }
}

//! `std.body2d`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PortShape, PrepareContext,
};

use crate::bridge::{BRIDGE_ROWS, Since, effect, lost};
use crate::svf::flush;
use crate::{VISUAL_DIVISOR, at, parameter_at, port_at};

/// Bodies: 0–3 circles, 4–5 boxes.
pub(crate) const BODIES: usize = 6;
const CIRCLES: usize = 4;
/// The most substeps a tick takes.
const MAX_SUBSTEPS: usize = 64;
const D: u32 = VISUAL_DIVISOR;

const GRAVITY: usize = 0;
const RESTITUTION: usize = 1;
const MAX_SPEED: usize = 2;
const KICK_SPEED: usize = 3;
const RADIUS: usize = 4;
const HALF: usize = 5;

/// Where the bodies start: circles in a row above, boxes below.
const START: [[f64; 2]; BODIES] = [
    [-0.6, 0.5],
    [-0.2, 0.5],
    [0.2, 0.5],
    [0.6, 0.5],
    [-0.4, -0.5],
    [0.4, -0.5],
];

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.body2d",
    doc: "Four rigid circles and two rigid, axis-aligned boxes colliding in the box [-1, 1]² at the visual rate (rate divisor 800), with gravity and restitution, kicked through a std.count reading; deterministic symplectic integration in substeps, so nothing tunnels at speeds up to max_speed.",
    ports: &[
        port_at(
            "kick",
            Direction::In,
            PortShape::Vector(BRIDGE_ROWS as u32),
            1,
            PortKind::Generic,
            None,
            None,
        ),
        port_at(
            "bodies",
            Direction::Out,
            PortShape::Vector(4 * BODIES as u32),
            D,
            PortKind::Control,
            None,
            None,
        ),
        port_at(
            "vel",
            Direction::Out,
            PortShape::Vector(2 * BODIES as u32),
            D,
            PortKind::Control,
            None,
            None,
        ),
        port_at(
            "lost",
            Direction::Out,
            PortShape::Signal,
            D,
            PortKind::Control,
            None,
            None,
        ),
    ],
    parameters: &[
        parameter_at(
            "gravity",
            D,
            PortKind::Control,
            Some("1/s²"),
            -50.0,
            50.0,
            -2.0,
            Combine::Add,
        ),
        parameter_at(
            "restitution",
            D,
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.6,
            Combine::Add,
        ),
        parameter_at(
            "max_speed",
            D,
            PortKind::Control,
            Some("1/s"),
            0.1,
            50.0,
            8.0,
            Combine::Add,
        ),
        parameter_at(
            "kick_speed",
            D,
            PortKind::Control,
            Some("1/s"),
            0.0,
            50.0,
            3.0,
            Combine::Multiply,
        ),
        parameter_at(
            "radius",
            D,
            PortKind::Control,
            None,
            0.02,
            0.3,
            0.08,
            Combine::Add,
        ),
        parameter_at(
            "half",
            D,
            PortKind::Control,
            None,
            0.02,
            0.3,
            0.1,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.body2d`: four rigid circles (bodies 0–3) and two rigid boxes (4–5),
/// axis-aligned and not rotating, colliding with each other and with the
/// walls of the box [−1, 1]², at the visual rate (divisor 800). No friction.
///
/// **Ports**
/// - `kick` (reading in, operator decision operator-request:007): the
///   reading of one `std.count` at scale 1, the only connection the plan
///   accepts there ([`crate::bridge`]). At each tick
///   every body gets `S × kick_speed` of speed along its own direction
///   `θ_i = π/2 + 0.4·(i − 2.5)` (mostly up), `S` the summed values of the
///   events its counter counted since the reading it applied last; nothing
///   is capped.
/// - `bodies` (vector of 24 out): per body `[x, y, size, kind]`, kind 0 a
///   circle (size its radius), 1 a box (size its half-width).
/// - `vel` (vector of 12 out): per body `[vx, vy]`.
/// - `lost` (out): readings of `kick` not applied because their sum may
///   have wrapped (2¹⁷ events or more since the one applied last), counted
///   since the start or the last reset, at most 2¹⁹ − 1.
///
/// **Parameters** (at divisor 800): `gravity` (along y, default −2),
/// `restitution` *e* (`[0, 1]`, default 0.6), `max_speed` (default 8),
/// `kick_speed` (default 3, `multiply`), `radius` (circles,
/// default 0.08), `half` (boxes, default 0.1). Mass is proportional to
/// area. Bodies start at rest, circles at y = ½, boxes at y = −½.
///
/// **Integration** (h = 800 / sample rate). A tick takes
/// `S = ⌈2·v_max·h / size_min⌉` substeps (at most 64, `size_min` the
/// smaller of `radius` and `half`), with `v_max = min(max_speed,
/// 64·size_min / (2h))`; each substep of `δ = h/S`, in this order:
///
/// 1. `v_y ← v_y + δ·gravity`, then every speed is limited to `v_max`;
///    `p ← p + δ·v` (symplectic Euler, velocity first);
/// 2. **walls**: a body past a wall is put back against it, and its
///    velocity into the wall is reflected times *e*;
/// 3. **pairs** in index order (0–1, 0–2, …, 4–5), each once: if they
///    overlap by `d > 0` along the contact normal `n` (circle–circle: along
///    the centres; circle–box: from the box's nearest point to the centre,
///    or the box's nearest face when the centre is inside; box–box: the axis
///    of least overlap), they are moved apart along `n` by `d`, shared by
///    inverse mass, and if they approach (`(v_j − v_i)·n < 0`) they get the
///    impulse `J = −(1 + e)·(v_j − v_i)·n / (1/m_i + 1/m_j)`. If they do not
///    overlap at the end of the substep but touched during it (a **swept**
///    contact: the pair's relative path over the substep, a straight
///    segment, enters the Minkowski sum of the two shapes, a circle of radius
///    `r_i + r_j`, a box of half-width `h_i + h_j`, or a box of half-width
///    `h` rounded by `r`), `n` is the outward normal where it first entered
///    and they get the same impulse, without a move;
/// 4. walls again, for positions only.
///
/// **No tunnelling.** Every contact during a substep is found, at its end
/// (an overlap) or on the way (the swept test), however shallow: a pass
/// that grazes another body by any depth gets its impulse. A body moves at
/// most `v_max·δ ≤ size_min/2` per substep and two bodies close at most
/// `size_min`, less than the sum of their sizes, so the swept segment never
/// crosses a body whole between two looks; walls clamp every substep. So
/// at any speed up to `max_speed` (the documented bound, itself limited to
/// `64·size_min/(2h)`) a body never crosses a wall or another body. Known
/// limit, not fixed (M2.8-10): a large change of `radius` or `half` within
/// one tick can put bodies into overlap, which is then resolved by
/// positional correction without a sweep. A resting stack may jitter (one pass per substep). Velocities are flushed
/// to 0 below 1e-9. The order of every operation is fixed, so the result is
/// bit-exact.
#[derive(Clone, Debug)]
pub struct Body2d {
    h: f64,
    p: [[f64; 2]; BODIES],
    v: [[f64; 2]; BODIES],
    /// What `kick` applied last.
    kicks: Since,
}

impl Default for Body2d {
    fn default() -> Body2d {
        Body2d {
            h: f64::from(D) / 48_000.0,
            p: START,
            v: [[0.0; 2]; BODIES],
            kicks: Since::default(),
        }
    }
}

/// The parameters of one tick.
#[derive(Clone, Copy, Debug)]
struct Tick {
    gravity: f64,
    e: f64,
    max_speed: f64,
    radius: f64,
    half: f64,
}

impl Tick {
    fn size(&self, i: usize) -> f64 {
        if i < CIRCLES { self.radius } else { self.half }
    }

    fn inverse_mass(&self, i: usize) -> f64 {
        let s = self.size(i);
        if i < CIRCLES {
            1.0 / (std::f64::consts::PI * s * s)
        } else {
            1.0 / (4.0 * s * s)
        }
    }
}

/// The contact normal (from `a` to `b`) and depth of bodies `a` and `b`,
/// if they overlap.
fn contact(a: (usize, [f64; 2]), b: (usize, [f64; 2]), tick: &Tick) -> Option<([f64; 2], f64)> {
    let ((i, pa), (j, pb)) = (a, b);
    let (sa, sb) = (tick.size(i), tick.size(j));
    let d = [pb[0] - pa[0], pb[1] - pa[1]];
    match (i < CIRCLES, j < CIRCLES) {
        (true, true) => {
            let dist = libm::sqrt(d[0] * d[0] + d[1] * d[1]);
            let depth = sa + sb - dist;
            (depth > 0.0).then(|| match dist > 0.0 {
                true => ([d[0] / dist, d[1] / dist], depth),
                false => ([0.0, 1.0], depth),
            })
        }
        (false, false) => {
            let ox = sa + sb - d[0].abs();
            let oy = sa + sb - d[1].abs();
            let sign = |x: f64| if x < 0.0 { -1.0 } else { 1.0 };
            (ox > 0.0 && oy > 0.0).then(|| match ox <= oy {
                true => ([sign(d[0]), 0.0], ox),
                false => ([0.0, sign(d[1])], oy),
            })
        }
        (circle_a, _) => {
            // One circle and one box: the normal points from the box to
            // the circle, then is turned to point from a to b.
            let (c, bx, r, s, flip) = match circle_a {
                true => (pa, pb, sa, sb, -1.0),
                false => (pb, pa, sb, sa, 1.0),
            };
            let q = [
                c[0].clamp(bx[0] - s, bx[0] + s),
                c[1].clamp(bx[1] - s, bx[1] + s),
            ];
            let off = [c[0] - q[0], c[1] - q[1]];
            let dist = libm::sqrt(off[0] * off[0] + off[1] * off[1]);
            let (n, depth) = if dist > 0.0 {
                ([off[0] / dist, off[1] / dist], r - dist)
            } else {
                // The centre is inside the box: out through the nearest face.
                let rel = [c[0] - bx[0], c[1] - bx[1]];
                let (gx, gy) = (s - rel[0].abs(), s - rel[1].abs());
                let sign = |x: f64| if x < 0.0 { -1.0 } else { 1.0 };
                match gx <= gy {
                    true => ([sign(rel[0]), 0.0], r + gx),
                    false => ([0.0, sign(rel[1])], r + gy),
                }
            };
            (depth > 0.0).then_some(([flip * n[0], flip * n[1]], depth))
        }
    }
}

/// When the path `q0 + t·d`, t in [0, 1], first enters the box of
/// half-widths `hx`, `hy` centred at 0, and its outward normal there; `None`
/// if it starts inside or never enters.
fn box_entry(q0: [f64; 2], d: [f64; 2], hx: f64, hy: f64) -> Option<(f64, [f64; 2])> {
    let (mut enter, mut exit, mut axis) = (f64::NEG_INFINITY, f64::INFINITY, 0);
    for (k, ((q, d), h)) in q0.into_iter().zip(d).zip([hx, hy]).enumerate() {
        if d == 0.0 {
            if q.abs() >= h {
                return None;
            }
            continue;
        }
        let (a, b) = ((-h - q) / d, (h - q) / d);
        let (lo, hi) = (a.min(b), a.max(b));
        if lo > enter {
            (enter, axis) = (lo, k);
        }
        exit = exit.min(hi);
    }
    if !(0.0..=1.0).contains(&enter) || enter >= exit {
        return None;
    }
    let at = |k: usize| q0.get(k).zip(d.get(k)).map_or(0.0, |(q, d)| q + enter * d);
    let sign = |x: f64| if x < 0.0 { -1.0 } else { 1.0 };
    let n = if axis == 0 {
        [sign(at(0)), 0.0]
    } else {
        [0.0, sign(at(1))]
    };
    Some((enter, n))
}

/// When the path `q0 + t·d`, t in [0, 1], first enters the circle of radius
/// `r` around `c`, and its outward normal there; `None` if it starts inside
/// or never enters.
fn circle_entry(q0: [f64; 2], d: [f64; 2], c: [f64; 2], r: f64) -> Option<(f64, [f64; 2])> {
    let p = [q0[0] - c[0], q0[1] - c[1]];
    let a = d[0] * d[0] + d[1] * d[1];
    let b = 2.0 * (p[0] * d[0] + p[1] * d[1]);
    let s = p[0] * p[0] + p[1] * p[1] - r * r;
    if s <= 0.0 || a == 0.0 {
        return None;
    }
    let disc = b * b - 4.0 * a * s;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - libm::sqrt(disc)) / (2.0 * a);
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let at = [p[0] + t * d[0], p[1] + t * d[1]];
    Some((t, [at[0] / r, at[1] / r]))
}

/// The earlier of two entries.
fn first(a: Option<(f64, [f64; 2])>, b: Option<(f64, [f64; 2])>) -> Option<(f64, [f64; 2])> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 < x.0 { y } else { x }),
        (x, None) => x,
        (None, y) => y,
    }
}

/// The swept contact of bodies `a` and `b` over a substep (`docs` of
/// [`Body2d`], step 3): their relative position (`b` from `a`) moves from
/// `q0` to `q1`; the normal (from `a` to `b`) where it first enters the
/// Minkowski sum of their shapes, if it does.
fn swept(a: usize, b: usize, q0: [f64; 2], q1: [f64; 2], tick: &Tick) -> Option<[f64; 2]> {
    let (sa, sb) = (tick.size(a), tick.size(b));
    let d = [q1[0] - q0[0], q1[1] - q0[1]];
    let entry = match (a < CIRCLES, b < CIRCLES) {
        (true, true) => circle_entry(q0, d, [0.0, 0.0], sa + sb),
        (false, false) => box_entry(q0, d, sa + sb, sa + sb),
        (circle_a, _) => {
            // The circle's position from the box's: a box of half-width h
            // rounded by r is two boxes and four corner circles.
            let (h, r, flip) = if circle_a {
                (sb, sa, -1.0)
            } else {
                (sa, sb, 1.0)
            };
            let (q0, d) = ([flip * q0[0], flip * q0[1]], [flip * d[0], flip * d[1]]);
            let mut entry = first(box_entry(q0, d, h + r, h), box_entry(q0, d, h, h + r));
            for c in [[h, h], [-h, h], [h, -h], [-h, -h]] {
                entry = first(entry, circle_entry(q0, d, c, r));
            }
            entry.map(|(t, n)| (t, [flip * n[0], flip * n[1]]))
        }
    };
    entry.map(|(_, n)| n)
}

impl Body2d {
    /// Puts bodies back inside the walls; with `bounce`, reflects their
    /// velocity into a wall times `e`.
    fn walls(&mut self, tick: &Tick, bounce: bool) {
        for (i, (p, v)) in self.p.iter_mut().zip(self.v.iter_mut()).enumerate() {
            let lim = (1.0 - tick.size(i)).max(0.0);
            for (p, v) in p.iter_mut().zip(v.iter_mut()) {
                if *p < -lim {
                    *p = -lim;
                    if bounce && *v < 0.0 {
                        *v *= -tick.e;
                    }
                } else if *p > lim {
                    *p = lim;
                    if bounce && *v > 0.0 {
                        *v *= -tick.e;
                    }
                }
            }
        }
    }

    /// Every pair once, in index order; `before` are the positions at the
    /// start of the substep, for the swept test.
    fn pairs(&mut self, tick: &Tick, before: &[[f64; 2]; BODIES]) {
        for i in 0..BODIES {
            for j in i + 1..BODIES {
                let (Some(&pa), Some(&pb)) = (self.p.get(i), self.p.get(j)) else {
                    continue;
                };
                let (Some(&a0), Some(&b0)) = (before.get(i), before.get(j)) else {
                    continue;
                };
                let found = contact((i, pa), (j, pb), tick).or_else(|| {
                    let q0 = [b0[0] - a0[0], b0[1] - a0[1]];
                    let q1 = [pb[0] - pa[0], pb[1] - pa[1]];
                    swept(i, j, q0, q1, tick).map(|n| (n, 0.0))
                });
                let Some((n, depth)) = found else {
                    continue;
                };
                let (wa, wb) = (tick.inverse_mass(i), tick.inverse_mass(j));
                let w = wa + wb;
                let (Some(&va), Some(&vb)) = (self.v.get(i), self.v.get(j)) else {
                    continue;
                };
                let vn = (vb[0] - va[0]) * n[0] + (vb[1] - va[1]) * n[1];
                let impulse = if vn < 0.0 {
                    -(1.0 + tick.e) * vn / w
                } else {
                    0.0
                };
                let along = |x: &mut [f64; 2], scale: f64| {
                    for (x, n) in x.iter_mut().zip(n) {
                        *x += scale * n;
                    }
                };
                if let Some(p) = self.p.get_mut(i) {
                    along(p, -depth * wa / w);
                }
                if let Some(p) = self.p.get_mut(j) {
                    along(p, depth * wb / w);
                }
                if let Some(v) = self.v.get_mut(i) {
                    along(v, -impulse * wa);
                }
                if let Some(v) = self.v.get_mut(j) {
                    along(v, impulse * wb);
                }
            }
        }
    }

    /// Speeds limited to `v_max`.
    fn limit(&mut self, v_max: f64) {
        for v in &mut self.v {
            let speed = libm::sqrt(v[0] * v[0] + v[1] * v[1]);
            if speed > v_max {
                let scale = v_max / speed;
                v[0] *= scale;
                v[1] *= scale;
            }
        }
    }

    /// The speed bound and substeps of a tick.
    fn substeps(&self, tick: &Tick) -> (f64, usize) {
        let size = tick.radius.min(tick.half);
        let v_max = tick
            .max_speed
            .min(MAX_SUBSTEPS as f64 * size / (2.0 * self.h));
        let substeps = libm::ceil(2.0 * v_max * self.h / size).clamp(1.0, MAX_SUBSTEPS as f64);
        (v_max, substeps as usize)
    }

    /// One tick with `kick` speed.
    fn tick(&mut self, kick: f64, tick: &Tick) {
        let (v_max, substeps) = self.substeps(tick);
        if kick != 0.0 {
            for (i, v) in self.v.iter_mut().enumerate() {
                let theta = std::f64::consts::FRAC_PI_2 + 0.4 * (i as f64 - 2.5);
                v[0] += kick * libm::cos(theta);
                v[1] += kick * libm::sin(theta);
            }
        }
        let delta = self.h / substeps as f64;
        for _ in 0..substeps {
            for v in &mut self.v {
                v[1] += delta * tick.gravity;
            }
            self.limit(v_max);
            let before = self.p;
            for (p, v) in self.p.iter_mut().zip(&self.v) {
                p[0] += delta * v[0];
                p[1] += delta * v[1];
            }
            self.walls(tick, true);
            self.pairs(tick, &before);
            self.walls(tick, false);
        }
        self.limit(v_max);
        for v in &mut self.v {
            v[0] = flush(v[0]);
            v[1] = flush(v[1]);
        }
    }
}

impl DirectPrimitive for Body2d {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.h = f64::from(D) / ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        let h = self.h;
        *self = Body2d::default();
        self.h = h;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ticks = inputs.frames();
        let p = |index: usize, t: usize| {
            let fallback = DESCRIPTOR.parameters.get(index).map_or(0.0, |d| d.default);
            at(inputs.parameter(index), t, fallback)
        };
        let kick = inputs.rows(0, BRIDGE_ROWS);
        for t in 0..ticks {
            let tick = Tick {
                gravity: p(GRAVITY, t),
                e: p(RESTITUTION, t).clamp(0.0, 1.0),
                max_speed: p(MAX_SPEED, t).max(0.0),
                radius: p(RADIUS, t).max(1e-3),
                half: p(HALF, t).max(1e-3),
            };
            let kicked = self.kicks.take(kick, ticks, t);
            self.tick(effect(kicked, p(KICK_SPEED, t)), &tick);
            for (i, (pos, vel)) in self.p.iter().zip(&self.v).enumerate() {
                let kind = if i < CIRCLES { 0.0 } else { 1.0 };
                let record = [pos[0], pos[1], tick.size(i), kind];
                for (r, x) in record.into_iter().enumerate() {
                    if let Some(out) = outputs.signal(4 * i + r).get_mut(t) {
                        *out = x;
                    }
                }
                for (axis, &x) in vel.iter().enumerate() {
                    if let Some(out) = outputs.signal(4 * BODIES + 2 * i + axis).get_mut(t) {
                        *out = x;
                    }
                }
            }
            if let Some(out) = outputs.signal(6 * BODIES).get_mut(t) {
                *out = lost(&[&self.kicks]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Tick {
        Tick {
            gravity: 0.0,
            e: 1.0,
            max_speed: 8.0,
            radius: 0.05,
            half: 0.05,
        }
    }

    fn ready() -> Body2d {
        let mut bodies = Body2d::default();
        bodies.prepare(&crate::testing::context(48_000.0, 256, 0));
        bodies
    }

    /// M2.8-9 AC7: a kick window that may have wrapped (2¹⁷ events since the
    /// reading applied last) applies nothing and is counted on `lost`, which
    /// a reset clears.
    #[test]
    fn a_window_that_may_have_wrapped_is_counted_lost() {
        use crate::bridge::Reading;
        use signal_core::Inputs;
        let mut bodies = ready();
        let tick = |bodies: &mut Body2d, n: u64, age: u64| {
            let reading = Reading {
                stamp: 9,
                age,
                n,
                sum: u128::from(n) << 96,
                ..Reading::default()
            };
            let mut out = [0.0; 6 * BODIES + 1];
            bodies.process(
                &Inputs::new(1, &reading.encode(), &[], &[], &[]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            out[6 * BODIES]
        };
        assert_eq!(tick(&mut bodies, 0, 5), 0.0);
        assert_eq!(tick(&mut bodies, 1 << 17, 6), 1.0);
        assert_eq!(tick(&mut bodies, (1 << 17) + 1, 7), 1.0);
        bodies.reset();
        assert_eq!(tick(&mut bodies, (1 << 17) + 2, 8), 0.0);
        // At its limit `lost` stays 2¹⁹ − 1, inside the fault guard
        // (finding auditor-1-007).
        bodies.kicks = Since::with_lost(crate::bridge::LOST_MAX);
        assert_eq!(
            tick(&mut bodies, (1 << 17) + 3, 9),
            f64::from(crate::bridge::LOST_MAX)
        );
    }

    /// Parks every body but `keep` far apart in the corners, at rest.
    fn park(bodies: &mut Body2d, keep: &[usize]) {
        let spots = [
            [-0.9, -0.9],
            [0.9, -0.9],
            [-0.9, 0.9],
            [0.9, 0.9],
            [0.0, -0.9],
            [0.0, 0.9],
        ];
        for (i, spot) in spots.into_iter().enumerate() {
            if !keep.contains(&i) {
                bodies.p[i] = spot;
                bodies.v[i] = [0.0; 2];
            }
        }
    }

    /// AC2, no tunnelling: a body at the speed bound aimed at a wall, a
    /// resting circle and a resting box, from 64 starting offsets, never
    /// ends beyond them, and no body ever leaves the walls.
    #[test]
    fn nothing_tunnels_at_the_speed_bound() {
        let tick = Tick { e: 0.5, ..quiet() };
        for offset in 0..64 {
            let start = -0.5 + offset as f64 * 0.003_1;
            for target in [None, Some(1usize), Some(4usize)] {
                let mut bodies = ready();
                let (v_max, _) = bodies.substeps(&tick);
                park(&mut bodies, &[0, target.unwrap_or(0)]);
                bodies.p[0] = [start, 0.0];
                bodies.v[0] = [v_max, 0.0];
                if let Some(t) = target {
                    bodies.p[t] = [0.5, 0.0];
                }
                for _ in 0..200 {
                    bodies.tick(0.0, &tick);
                    for (i, p) in bodies.p.iter().enumerate() {
                        let lim = 1.0 - tick.size(i) + 1e-12;
                        assert!(
                            p[0].abs() <= lim && p[1].abs() <= lim,
                            "body {i} out: {p:?}"
                        );
                    }
                    if let Some(t) = target {
                        assert!(
                            bodies.p[0][0] < bodies.p[t][0],
                            "offset {offset}: passed body {t}"
                        );
                    }
                }
            }
        }
    }

    /// Finding auditor-1-006: a circle passing another at 8 (the speed
    /// bound) so that their centres come within 0.09999 of each other
    /// (radii 0.05 + 0.05) touches it on the way, though they overlap at no
    /// substep's end: the contact is found and both velocities change. Other
    /// grazes, and the swept box cases, likewise; a pass that clears by 1e-5
    /// touches nothing.
    #[test]
    fn a_grazing_pass_is_a_contact() {
        let tick = Tick {
            radius: 0.05,
            half: 0.05,
            ..quiet()
        };
        let run = |a: usize, b: usize, start: [f64; 2], ticks: usize| {
            let mut bodies = ready();
            park(&mut bodies, &[a, b]);
            bodies.p[a] = start;
            bodies.v[a] = [8.0, 0.0];
            bodies.p[b] = [0.0, 0.0];
            bodies.v[b] = [0.0, 0.0];
            for _ in 0..ticks {
                bodies.tick(0.0, &tick);
            }
            (bodies.v[a], bodies.v[b], bodies.p[b])
        };
        // A contact pushed the other body (a frictionless box sliding past
        // a box moves it sideways without an impulse along the motion).
        let touched = |(_, v, p): ([f64; 2], [f64; 2], [f64; 2])| v != [0.0; 2] || p != [0.0; 2];
        let (moving, still, _) = run(0, 1, [-1.0 / 90.0, 0.099_99], 1);
        println!("after the graze: {moving:?} {still:?}");
        assert_ne!(moving, [8.0, 0.0]);
        assert_ne!(still, [0.0, 0.0]);
        // From clear of each other, across the other body within reach.
        for y in [0.05, 0.09, 0.099, 0.099_999] {
            for (a, b) in [(0, 1), (4, 5), (0, 4)] {
                assert!(
                    touched(run(a, b, [-0.4, y], 6)),
                    "bodies {a} and {b} at {y}"
                );
            }
        }
        // Out of reach (sizes 0.05 + 0.05): nothing touches.
        for (a, b) in [(0, 1), (4, 5), (0, 4)] {
            let result = run(a, b, [-0.4, 0.100_01], 6);
            assert_eq!(result.0, [8.0, 0.0], "{a}, {b}: a clear pass");
            assert!(!touched(result), "{a}, {b}: a clear pass");
        }
    }

    /// AC2: an elastic collision of two circles without gravity keeps
    /// momentum and kinetic energy.
    #[test]
    fn an_elastic_collision_keeps_momentum_and_energy() {
        let tick = quiet();
        let mut bodies = ready();
        park(&mut bodies, &[0, 1]);
        bodies.p[0] = [-0.3, 0.01];
        bodies.p[1] = [0.3, -0.01];
        bodies.v[0] = [2.0, 0.0];
        bodies.v[1] = [-1.0, 0.0];
        let m = 1.0 / tick.inverse_mass(0);
        let momentum = |b: &Body2d| [m * (b.v[0][0] + b.v[1][0]), m * (b.v[0][1] + b.v[1][1])];
        let energy = |b: &Body2d| {
            0.5 * m
                * (b.v[0][0].powi(2) + b.v[0][1].powi(2) + b.v[1][0].powi(2) + b.v[1][1].powi(2))
        };
        let (p0, e0) = (momentum(&bodies), energy(&bodies));
        for _ in 0..30 {
            bodies.tick(0.0, &tick);
        }
        assert!(bodies.v[0][0] < 0.0, "they collided: {:?}", bodies.v);
        let (p1, e1) = (momentum(&bodies), energy(&bodies));
        assert!((p1[0] - p0[0]).abs() <= 1e-12 * m && (p1[1] - p0[1]).abs() <= 1e-12 * m);
        assert!((e1 / e0 - 1.0).abs() <= 1e-12);
    }

    /// Two runs from the same state are bit-identical, and every body stays
    /// inside the walls through kicks and settling.
    #[test]
    fn runs_are_deterministic_and_bounded() {
        let tick = Tick {
            gravity: -2.0,
            e: 0.6,
            max_speed: 8.0,
            radius: 0.08,
            half: 0.1,
        };
        let run = || {
            let mut bodies = ready();
            let mut trace = Vec::new();
            for n in 0..2000 {
                bodies.tick(if n % 120 == 0 { 3.0 } else { 0.0 }, &tick);
                for (i, p) in bodies.p.iter().enumerate() {
                    let lim = 1.0 - tick.size(i) + 1e-12;
                    assert!(p[0].abs() <= lim && p[1].abs() <= lim);
                }
                trace.push(bodies.p);
            }
            trace
        };
        assert_eq!(run(), run());
    }
}

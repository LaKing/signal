import test from 'node:test';
import assert from 'node:assert/strict';
import { FLAT, LOOP_OUT, REACH_MIN, REACH_TALL, cordCurve, curveBox, hull, split, curveData, curveDistance, flatten, wireCurve, wireGeometry, wirePoints } from './schematic-wire.js';
import { bake, bakeHolds, bakedMatrix, bakedPath } from './schematic-draw.js';
import { hitTest } from './schematic-hit.js';
import { toScreen } from './schematic-view.js';

/** The point at `t` of a cubic, and its derivative. */
const at = ([p0, c1, c2, p3], t) => {
  const s = 1 - t;
  return { x: s * s * s * p0.x + 3 * s * s * t * c1.x + 3 * s * t * t * c2.x + t * t * t * p3.x, y: s * s * s * p0.y + 3 * s * s * t * c1.y + 3 * s * t * t * c2.y + t * t * t * p3.y };
};
const pieces = (curve) => {
  let from = curve.start;
  return curve.segs.map((s) => {
    const piece = s.c1 ? [from, s.c1, s.c2, s.to] : [from, from, s.to, s.to];
    from = s.to;
    return piece;
  });
};
/** Dense samples of the whole curve. */
const samples = (curve, n = 200) => pieces(curve).flatMap((c) => Array.from({ length: n + 1 }, (_, i) => at(c, i / n)));
/** The unit direction from `a` to `b`. */
const dir = (a, b) => {
  const l = Math.hypot(b.x - a.x, b.y - a.y);
  return { x: (b.x - a.x) / l, y: (b.y - a.y) / l };
};
/** A piece's direction where it starts and where it ends (its first and last distinct control points). */
const starts = (c) => dir(c[0], c.slice(1).find((p) => p.x !== c[0].x || p.y !== c[0].y));
const ends = (c) => dir(c.slice(0, 3).reverse().find((p) => p.x !== c[3].x || p.y !== c[3].y), c[3]);
const near = (a, b, eps = 1e-9) => Math.abs(a.x - b.x) <= eps && Math.abs(a.y - b.y) <= eps;

const FORWARD = [
  [{ x: 0, y: 0 }, { x: 300, y: 40 }],
  [{ x: 0, y: 0 }, { x: 72, y: 400 }],
  [{ x: 0, y: 100 }, { x: 72, y: -300 }],
  [{ x: 0, y: 0 }, { x: 10, y: 200 }],
  [{ x: 5, y: 5 }, { x: 900, y: 5 }],
];

test('a forward wire leaves its output and enters its input horizontally, its ends exact', () => {
  for (const [a, b] of FORWARD) {
    const curve = wireCurve(a, b);
    assert.equal(curve.segs.length, 1, 'one cubic');
    const [{ c1, c2, to }] = curve.segs;
    assert.deepEqual([curve.start, to], [a, b]);
    assert.equal(c1.y, a.y, 'leaves horizontally');
    assert.ok(c1.x > a.x, 'to the right');
    assert.equal(c2.y, b.y, 'enters horizontally');
    assert.ok(c2.x < b.x, 'from the left');
    const points = flatten(curve);
    assert.deepEqual([points[0], points.at(-1)], [a, b], 'the flattened ends are the pins');
  }
});

test('a forward wire is a patch cord: its handles reach out however close the boxes, its box exact', () => {
  for (const [a, b] of FORWARD) {
    const curve = wireCurve(a, b);
    const reach = curve.segs[0].c1.x - a.x;
    assert.equal(curve.segs[0].to.x - curve.segs[0].c2.x, reach, 'the same at both ends');
    assert.ok(reach >= REACH_MIN, 'never collapses');
    assert.ok(reach >= (b.x - a.x) / 2 && reach >= Math.min(Math.abs(b.y - a.y) / 2, REACH_TALL));
    // The box: every point of the curve in it, and each of its edges reached.
    const box = curveBox(curve);
    const all = samples(curve, 4000);
    assert.ok(all.every((p) => p.x >= box.x0 - 1e-9 && p.x <= box.x1 + 1e-9 && p.y >= box.y0 - 1e-9 && p.y <= box.y1 + 1e-9));
    for (const [edge, value] of [['x0', Math.min(...all.map((p) => p.x))], ['x1', Math.max(...all.map((p) => p.x))], ['y0', Math.min(...all.map((p) => p.y))], ['y1', Math.max(...all.map((p) => p.y))]]) {
      assert.ok(Math.abs(box[edge] - value) < 1e-3, `${edge} ${box[edge]} vs ${value}`);
    }
    assert.deepEqual(wireGeometry(a, b).box, box);
  }
  // A long, flat wire bows half its run.
  assert.equal(wireCurve({ x: 0, y: 0 }, { x: 400, y: 10 }).segs[0].c1.x, 200);
  // Two boxes side by side (an output at 482, an input at 497, 240 below):
  // an S swinging out past both, not a line down their edges.
  const close = wireCurve({ x: 482, y: 100 }, { x: 497, y: 340 });
  assert.deepEqual([close.segs[0].c1.x, close.segs[0].c2.x], [482 + REACH_TALL, 497 - REACH_TALL]);
  const box = curveBox(close);
  assert.ok(box.x1 > 497 + 20 && box.x0 < 482 - 20, `swings out: ${JSON.stringify(box)}`);
});

test('a feedback wire loops out right, down below its lane, back and in from the left, its turns rounded', () => {
  const cases = [
    [{ x: 500, y: 100 }, { x: 0, y: 40 }, 300],
    [{ x: 500, y: 40 }, { x: 0, y: 100 }, 180],
    // A back edge whose input is right of its output.
    [{ x: 0, y: 40 }, { x: 400, y: 60 }, 200],
    // Output right above the input: the loop's run along the lane is short.
    [{ x: 100, y: 0 }, { x: 100 + 2 * LOOP_OUT, y: 30 }, 120],
    [{ x: 0, y: 0 }, { x: 50, y: 30 }, 120],
    [{ x: 0, y: 0 }, { x: 30, y: 30 }, 120],
  ];
  for (const [a, b, under] of cases) {
    const curve = wireCurve(a, b, { feedback: true, under });
    const all = pieces(curve);
    assert.deepEqual([curve.start, curve.segs.at(-1).to], [a, b], 'its ends are the pins');
    // Leaves to the right, enters from the left, both horizontally.
    assert.ok(near(starts(all[0]), { x: 1, y: 0 }), 'leaves right');
    assert.ok(near(ends(all.at(-1)), { x: 1, y: 0 }), 'enters from the left');
    // Down to the lane and back.
    const ys = samples(curve).map((p) => p.y);
    assert.ok(Math.abs(Math.max(...ys) - under) < 1e-9, 'along its lane, no lower');
    // Rounded: the direction is continuous wherever two pieces meet.
    for (let i = 1; i < all.length; i++) {
      assert.ok(near(ends(all[i - 1]), starts(all[i]), 1e-12), `a corner at ${JSON.stringify(all[i][0])}`);
    }
  }
});

test('the curves are deterministic, and the geometry is one function', () => {
  const a = { x: 3, y: 7 };
  const b = { x: 250, y: 90 };
  assert.deepEqual(wireCurve(a, b), wireCurve({ y: 7, x: 3 }, { y: 90, x: 250 }));
  assert.deepEqual(wireGeometry(a, b, { feedback: true, under: 200 }), wireGeometry(a, b, { feedback: true, under: 200 }));
  assert.deepEqual(wirePoints(a, b), flatten(wireCurve(a, b)));
  // The flattened points lie on the curve, every one of them, and within FLAT of it between.
  const curve = wireCurve(a, b);
  for (const p of flatten(curve)) assert.ok(curveDistance(curve, p, 0.01) < 1e-3);
  const points = flatten(curve);
  for (let i = 1; i < points.length; i++) {
    const mid = { x: (points[i - 1].x + points[i].x) / 2, y: (points[i - 1].y + points[i].y) / 2 };
    assert.ok(curveDistance(curve, mid, 10) <= FLAT, `chord ${i} bows ${curveDistance(curve, mid, 10)}`);
  }
});

test('the distance to a curve is exact near it and Infinity past the limit', () => {
  const curve = wireCurve({ x: 0, y: 0 }, { x: 200, y: 100 }, {});
  const [c] = pieces(curve);
  for (const t of [0.1, 0.37, 0.5, 0.9]) {
    const p = at(c, t);
    assert.ok(curveDistance(curve, p, 0.01) < 1e-3, `t ${t}`);
    const off = { x: p.x, y: p.y - 3 };
    const want = Math.min(...samples(curve, 4000).map((s) => Math.hypot(s.x - off.x, s.y - off.y)));
    assert.ok(Math.abs(curveDistance(curve, off, 4) - want) < 4 / 16 + 1e-3, `t ${t}`);
  }
  assert.equal(curveDistance(curve, { x: 100, y: 500 }, 4), Infinity);
});

test('a wire is hit on its curve at any zoom, not on its flattened points', () => {
  const curve = wireCurve({ x: 0, y: 0 }, { x: 300, y: 200 });
  const wire = { id: 0, from: { node: 'a', port: 'out' }, to: { node: 'b', port: 'in' }, written: { from: 'a.out', to: 'b.in' }, curve, points: flatten(curve) };
  // The view's centre on the curve's middle; flattened, the polyline is off by up to FLAT, at k = 1e4 a long way.
  const mid = at(pieces(curve)[0], 0.4321);
  for (const k of [1, 1e4, 1e9]) {
    const view = { k, ox: mid.x, oy: mid.y, x: 500, y: 300 };
    const drawn = { levels: [{ path: '', level: '', transform: view, nodes: [], wires: [wire] }] };
    assert.equal(hitTest(drawn, { x: 500, y: 302 }).kind, 'wire', `k ${k}`);
    assert.equal(hitTest(drawn, { x: 500, y: 312 }).kind, 'background', `k ${k}`);
  }
});

/** Path data as `[{op, points}]`. */
function parse(d) {
  return [...d.matchAll(/([MLC])([^MLC]*)/g)].map(([, op, args]) => ({ op, points: args.trim().split(/\s+/).map((s) => s.split(',').map(Number)).map(([x, y]) => ({ x, y })) }));
}

test('a curve is baked as Béziers, exact when unclipped and when far larger than the screen', () => {
  const curve = wireCurve({ x: 0, y: 0 }, { x: 1000, y: 500 });
  const viewport = { w: 1000, h: 600 };
  // Fitted: one M and one C, the curve's own control points baked.
  const fitted = { k: 0.5, ox: 500, oy: 250, x: 500, y: 300 };
  const whole = bake(fitted, viewport);
  const d = bakedPath(whole, flatten(curve), curve);
  assert.deepEqual(parse(d).map((c) => c.op), ['M', 'C']);
  assert.equal(whole.clipped, false);
  // At k = 1e5 the curve is 1e8 px long: clipped, its pieces within the clip
  // box, and the part on screen the curve itself (to 0.01 px).
  const mid = at(pieces(curve)[0], 0.5);
  const deep = { k: 1e5, ox: mid.x, oy: mid.y, x: 500, y: 300 };
  const baked = bake(deep, viewport);
  const path = parse(bakedPath(baked, flatten(curve), curve));
  assert.ok(baked.clipped);
  assert.ok(path.length < 200, `${path.length} commands`);
  const { clip } = baked;
  for (const c of path) for (const p of c.points) assert.ok(p.x >= clip.x0 - 1e-6 && p.x <= clip.x1 + 1e-6 && p.y >= clip.y0 - 1e-6 && p.y <= clip.y1 + 1e-6);
  // Back to the screen: the curve's points near the screen are on what is drawn.
  const screen = (p) => toScreen(deep, { x: baked.q.x + (p.x - baked.shift.x) / baked.m, y: baked.q.y + (p.y - baked.shift.y) / baked.m });
  const drawn = [];
  for (const c of path) {
    const points = c.points.map(screen);
    if (c.op === 'M') drawn.push({ start: points[0], segs: [] });
    else drawn.at(-1).segs.push(c.op === 'C' ? { c1: points[0], c2: points[1], to: points[2] } : { to: points[0] });
  }
  assert.ok(path.some((c) => c.op === 'C'), 'Béziers, not a polyline');
  const [c] = pieces(curve);
  for (const t of [0.5 - 4e-6, 0.5 - 1e-6, 0.5, 0.5 + 1e-6, 0.5 + 4e-6]) {
    const p = toScreen(deep, at(c, t));
    assert.ok(Math.abs(p.x - 500) < 1000, `t ${t} is on screen`);
    const gap = Math.min(...drawn.map((one) => curveDistance(one, p, 0.05)));
    assert.ok(gap < 0.01, `t ${t}: ${gap} px off`);
  }
});

test('a clipped curve stays on its curve at every zoom its bake holds for', () => {
  // A forward wire and a feedback loop (its corners included), far larger than the screen.
  const forward = wireCurve({ x: 0, y: 0 }, { x: 1000, y: 500 });
  const loop = wireCurve({ x: 1000, y: 0 }, { x: 0, y: 50 }, { feedback: true, under: 400 });
  const viewport = { w: 1000, h: 600 };
  const cases = [[forward, at(pieces(forward)[0], 0.3)], [forward, at(pieces(forward)[0], 0.05)], [loop, { x: 1020, y: 384 }], [loop, { x: 1020, y: 340 }]];
  for (const [curve, centre] of cases) {
    const view = { k: 2e5, ox: centre.x, oy: centre.y, x: 500, y: 300 };
    const baked = bake(view, viewport);
    const path = parse(bakedPath(baked, flatten(curve), curve));
    assert.ok(baked.clipped);
    // Out, as far as the bake holds, and in; panned to every part of the
    // curve it holds for.
    const views = [];
    const targets = pieces(curve).flatMap((c) => Array.from({ length: 40 }, (_, i) => at(c, i / 40)));
    for (let k = view.k * 8; k > view.k / 1e5; k /= 2) {
      for (const t of targets) {
        const v = { k, ox: t.x, oy: t.y, x: 500, y: 300 };
        if (bakeHolds(baked, v, viewport)) views.push(v);
      }
    }
    for (const v of views) {
      const mx = bakedMatrix(baked, v);
      const screen = (p) => ({ x: p.x * mx.k + mx.x, y: p.y * mx.k + mx.y });
      const drawn = [];
      for (const c of path) {
        const points = c.points.map(screen);
        if (c.op === 'M') drawn.push({ start: points[0], segs: [] });
        else drawn.at(-1).segs.push(c.op === 'C' ? { c1: points[0], c2: points[1], to: points[2] } : { to: points[0] });
      }
      // Points of the curve on screen are on what is drawn, within 0.01 px.
      const onScreen = { start: toScreen(v, curve.start), segs: curve.segs.map((sg) => (sg.c1 ? { c1: toScreen(v, sg.c1), c2: toScreen(v, sg.c2), to: toScreen(v, sg.to) } : { to: toScreen(v, sg.to) })) };
      let checked = 0;
      // The parts of the curve on screen, a point about every 20 px.
      const visit = (c) => {
        const box = hull(c);
        if (box.x1 < 0 || box.x0 > viewport.w || box.y1 < 0 || box.y0 > viewport.h) return;
        if (Math.max(box.x1 - box.x0, box.y1 - box.y0) > 20) {
          for (const half of split(c)) visit(half);
          return;
        }
        const p = c[0];
        if (p.x < 0 || p.x > viewport.w || p.y < 0 || p.y > viewport.h) return;
        const gap = Math.min(...drawn.map((one) => curveDistance(one, p, 0.05)));
        assert.ok(gap < 0.01, `k ${v.k}: ${gap} px off at ${JSON.stringify(p)}`);
        checked++;
      };
      for (const c of pieces(onScreen)) visit(c);
      assert.ok(checked > 0, `k ${v.k}: something on screen`);
    }
  }
});

test('the cord being dragged is a wire’s curve, a loop when dragged back', () => {
  const a = { x: 100, y: 100 };
  const forward = cordCurve(a, { x: 300, y: 50 });
  assert.equal(forward.segs.length, 1);
  assert.deepEqual(forward, wireCurve(a, { x: 300, y: 50 }));
  const back = cordCurve(a, { x: 20, y: 140 });
  assert.ok(back.segs.length > 1);
  assert.ok(samples(back).every((p) => p.y <= 140 + 32 + 1e-9));
  assert.match(curveData(forward), /^M100,100 C[-\d.e]+,100 [-\d.e]+,50 300,50$/);
});

// ADR 0047 §6 (phase 2): the board's orthogonal routing, net labels and measures (ui/lib/board-route.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CLEAR, LABEL_STUB, NET_DESTS, NET_SPAN, legCrossings, measure, orthGeometry, polylineMetrics, routeWires, simplify, trackPreferences, zCorners } from './board-route.js';
import { STUB, TRACK } from './board-layout.js';

/** The segments of a route's corners. */
const segments = (c) => c.slice(1).map((q, i) => [c[i], q]);

/** Whether segment p–q passes through the inside of `box`. */
function through([p, q], box) {
  const x0 = Math.min(p.x, q.x);
  const x1 = Math.max(p.x, q.x);
  const y0 = Math.min(p.y, q.y);
  const y1 = Math.max(p.y, q.y);
  return x0 < box.x + box.w && x1 > box.x && y0 < box.y + box.h && y1 > box.y;
}

/** Two columns of boxes and a blocker between them: wires a → c, b → c, a → d. */
function board() {
  const boxes = [
    { id: 'a', x: 0, y: 0, w: 100, h: 80 },
    { id: 'b', x: 0, y: 200, w: 100, h: 80 },
    { id: 'block', x: 160, y: 60, w: 60, h: 160 },
    { id: 'c', x: 300, y: 0, w: 100, h: 80 },
    { id: 'd', x: 300, y: 220, w: 100, h: 80 },
  ];
  const pin = (id, side, row) => {
    const b = boxes.find((x) => x.id === id);
    return { x: side === 'out' ? b.x + b.w : b.x, y: b.y + 40 + row * 16 };
  };
  const wires = [
    { id: 0, net: 'a.out', from: 'a', to: 'c', a: pin('a', 'out', 0), b: pin('c', 'in', 1), span: 1 },
    { id: 1, net: 'b.out', from: 'b', to: 'c', a: pin('b', 'out', 0), b: pin('c', 'in', 0), span: 1 },
    { id: 2, net: 'a.out', from: 'a', to: 'd', a: pin('a', 'out', 0), b: pin('d', 'in', 0), span: 1 },
  ];
  return { boxes, wires };
}

test('no route passes under a box, and turns only on verticals of their own track (test 6)', () => {
  const { boxes, wires } = board();
  const { routes } = routeWires(boxes, wires);
  const verticals = [];
  routes.forEach((r, i) => {
    assert.equal(r.labelled, false, `wire ${i} routed`);
    const w = wires[i];
    assert.deepEqual(r.corners[0], w.a);
    assert.deepEqual(r.corners.at(-1), w.b);
    segments(r.corners).forEach(([p, q], k, all) => {
      assert.ok(p.x === q.x || p.y === q.y, 'orthogonal');
      for (const box of boxes) {
        if ((k === 0 && box.id === w.from) || (k === all.length - 1 && box.id === w.to)) continue;
        assert.ok(!through([p, q], { x: box.x - CLEAR + 1, y: box.y - CLEAR + 1, w: box.w + 2 * CLEAR - 2, h: box.h + 2 * CLEAR - 2 }), `wire ${i} under ${box.id}`);
      }
      if (p.x === q.x) verticals.push({ x: p.x, lo: Math.min(p.y, q.y), hi: Math.max(p.y, q.y), net: w.net });
    });
    // It leaves and enters straight for at least a stub.
    assert.ok(r.corners[1].x - w.a.x >= STUB || r.corners.length === 2);
    assert.ok(w.b.x - r.corners.at(-2).x >= STUB || r.corners.length === 2);
  });
  for (const v of verticals) {
    for (const o of verticals) {
      if (v === o || v.net === o.net) continue;
      const overlap = v.lo < o.hi && o.lo < v.hi;
      assert.ok(!overlap || Math.abs(v.x - o.x) >= TRACK - 0.01, `two nets on one track at x=${v.x}`);
    }
  }
});

test('a net’s wires from one pin share their first vertical (a trunk)', () => {
  const boxes = [
    { id: 'a', x: 0, y: 0, w: 100, h: 80 },
    { id: 'c', x: 300, y: 0, w: 100, h: 80 },
    { id: 'd', x: 300, y: 200, w: 100, h: 80 },
  ];
  const a = { x: 100, y: 40 };
  const wires = [
    { id: 0, net: 'n', from: 'a', to: 'c', a, b: { x: 300, y: 56 }, span: 1 },
    { id: 1, net: 'n', from: 'a', to: 'd', a, b: { x: 300, y: 240 }, span: 1 },
  ];
  const { routes } = routeWires(boxes, wires);
  assert.equal(routes[0].corners[1].x, routes[1].corners[1].x);
});

test('net labels at the thresholds: four destinations, a span over three layers, the plate’s area to free space (test 6)', () => {
  const boxes = [{ id: 's', x: 0, y: 0, w: 80, h: 200 }];
  const dest = (i, extra = {}) => ({ id: i, net: 's.out', from: 's', to: `d${i}`, a: { x: 80, y: 40 }, b: { x: 400, y: 40 + 60 * i }, span: 1, ...extra });
  const three = routeWires(boxes, [0, 1, 2].map((i) => dest(i)));
  assert.ok(three.routes.every((r) => !r.labelled), 'three destinations: wires');
  const four = routeWires(boxes, [0, 1, 2, 3].map((i) => dest(i)));
  assert.equal(NET_DESTS, 4);
  assert.ok(four.routes.every((r) => r.labelled), 'four: labels');
  assert.equal(four.labels.filter((l) => l.side === 'out').length, 1, 'one at the source');
  assert.equal(four.labels.filter((l) => l.side === 'in').length, 4, 'one per destination');
  assert.match(four.labels[0].text, /^◂ s\.out$/);
  assert.deepEqual(four.routes[0].corners, [{ x: 400 - LABEL_STUB, y: 40 }, { x: 400, y: 40 }], 'a stub into its pin');
  assert.ok(four.routes[0].whole.length >= 2, 'its whole route, for a trace');
  const long = routeWires(boxes, [dest(0, { span: NET_SPAN + 1 })]);
  assert.ok(long.routes[0].labelled, 'a span over three layers');
  assert.ok(!routeWires(boxes, [dest(0, { span: NET_SPAN })]).routes[0].labelled);
  assert.ok(routeWires(boxes, [dest(0, { crossesPlate: true })]).routes[0].labelled, 'from the plate’s area to free space');
  assert.equal(four.metrics.labels, 1);
});

test('a wire with no free way is drawn as labels, never under a box', () => {
  // The target's input faces a wall of boxes on every side.
  const boxes = [
    { id: 'a', x: 0, y: 0, w: 60, h: 60 },
    { id: 'b', x: 300, y: 0, w: 60, h: 60 },
    { id: 'wall', x: 240, y: -2000, w: 58, h: 4000 },
  ];
  const { routes, labels } = routeWires(boxes, [{ id: 0, net: 'a.out', from: 'a', to: 'b', a: { x: 60, y: 30 }, b: { x: 300, y: 30 }, span: 1 }]);
  assert.ok(routes[0].labelled);
  assert.ok(routes[0].failed);
  assert.equal(labels.length, 2);
});

test('feedback runs through its own lane below', () => {
  const boxes = [
    { id: 'a', x: 0, y: 0, w: 80, h: 60 },
    { id: 'b', x: 200, y: 0, w: 80, h: 60 },
  ];
  const { routes } = routeWires(boxes, [{ id: 0, net: 'b.out', from: 'b', to: 'a', a: { x: 280, y: 30 }, b: { x: 0, y: 30 }, feedback: true, under: 100, span: 1 }]);
  const c = routes[0].corners;
  assert.ok(c.some((p) => p.y === 100), 'along the lane');
  assert.equal(c.length, 6);
});

test('tracks in a channel: down legs right to left as they start lower, so parallel wires do not cross', () => {
  const wires = [
    { id: 0, net: 'p', a: { x: 100, y: 0 }, b: { x: 300, y: 100 } },
    { id: 1, net: 'q', a: { x: 100, y: 20 }, b: { x: 300, y: 120 } },
  ];
  const prefs = trackPreferences(wires);
  const xs = [...prefs.values()];
  assert.equal(xs.length, 2);
  const [x0, x1] = [prefs.get('100,0>300,100'), prefs.get('100,20>300,120')];
  assert.ok(x0 > x1, 'the higher start on the right');
  assert.equal(legCrossings([{ p: wires[0].a, q: wires[0].b, net: 'p', x: x0 }, { p: wires[1].a, q: wires[1].b, net: 'q', x: x1 }]), 0);
  assert.ok(legCrossings([{ p: wires[0].a, q: wires[0].b, net: 'p', x: x1 }, { p: wires[1].a, q: wires[1].b, net: 'q', x: x0 }]) > 0, 'the other way they cross');
});

test('geometry: corners simplified, turns rounded, a plain route for a drag', () => {
  assert.deepEqual(simplify([{ x: 0, y: 0 }, { x: 0, y: 0 }, { x: 5, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 5 }]), [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 5 }]);
  const g = orthGeometry([{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 40 }, { x: 100, y: 40 }]);
  assert.ok(g.curve.segs.some((s) => s.c1), 'a rounded turn');
  assert.deepEqual(g.box, { x0: 0, y0: 0, x1: 100, y1: 40 });
  assert.deepEqual(zCorners({ x: 0, y: 0 }, { x: 100, y: 40 }), [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 40 }, { x: 100, y: 40 }]);
  assert.equal(zCorners({ x: 100, y: 0 }, { x: 0, y: 0 }, { feedback: true, under: 80 })[2].y, 80);
});

test('the measures: crossings of different nets, bends, length; a curve’s too (rule 6: shown, never deciding)', () => {
  const wires = [{ net: 'a' }, { net: 'b' }];
  const routes = [
    { corners: [{ x: 0, y: 50 }, { x: 100, y: 50 }] },
    { corners: [{ x: 50, y: 0 }, { x: 50, y: 100 }] },
  ];
  assert.deepEqual(measure(wires, routes), { crossings: 1, bends: 0, length: 200, labels: 0 });
  assert.deepEqual(polylineMetrics([{ net: 'a', points: routes[0].corners }, { net: 'b', points: routes[1].corners }]), { crossings: 1, length: 200 });
  assert.equal(measure([{ net: 'a' }, { net: 'a' }], routes).crossings, 0, 'one net does not cross itself');
});

// ADR 0047's note "Layered placement and channel routing" (2026-10-31): a
// level laid out by flow (a die, a board without vias) is placed in layered
// columns and routed in channels: a forward wire between neighbouring
// columns at most two bends, feedback round the outside, a fan-out a tree
// with junctions, a stored chip exactly where it is with the others clear
// of it; a staircase left by the search straightened where a simpler shape
// fits. Deterministic.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { bandOrder, orderTracks, planChannels, siftBand } from './board-channels.js';
import { straighten } from './board-straighten.js';
import { junctions } from './board-net.js';
import { LEG_LANE, PARAMS, layout } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
const VOICES = ['moog_voice', 'kick909', 'snare909', 'clap909', 'hat909', 'tom909', 'rim909', 'cymbal909', 'tape_delay'];
const die = (name, positions = {}) => layout(STUDIO.composites[name], positions, { factsOf: (m) => exportFacts(STUDIO.composites?.[m] ?? null), board: { die: true } });
const intersects = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

test('the planner: H-V-H between neighbouring columns, one corridor across columns, feedback on an outer track, a net one trunk', () => {
  const spans = [[{ y0: 0, y1: 60 }], [{ y0: 0, y1: 60 }, { y0: 100, y1: 160 }], [{ y0: 0, y1: 60 }]];
  const colOf = (id) => ({ a: 0, b: 1, c: 1, d: 2 })[id] ?? null;
  const wires = [
    { id: 0, net: 'a.out', from: 'a', to: 'b', ya: 20, yb: 40, feedback: false, chain: [] },
    { id: 1, net: 'a.out', from: 'a', to: 'c', ya: 20, yb: 130, feedback: false, chain: [] },
    { id: 2, net: 'a.x', from: 'a', to: 'd', ya: 80, yb: 30, feedback: false, chain: [{ layer: 1, y: 80 }] },
    { id: 3, net: 'd.out', from: 'd', to: 'b', ya: 30, yb: 50, feedback: true, chain: [] },
  ];
  const { plans, channels, outer } = planChannels({ count: 3, colOf, spans, wires, side: () => 'below' });
  const corners = (id) => plans.get(id).corners;
  assert.equal(corners(0).length, 4, 'H-V-H');
  assert.equal(corners(1).length, 4, 'H-V-H');
  // The fan-out: both wires of a.out on one track of channel 0.
  assert.equal(corners(0)[1].x.v.track, corners(1)[1].x.v.track, 'one trunk');
  // The long wire runs along its source's height (free between the boxes of column 1): two bends.
  assert.equal(corners(2).length, 4);
  assert.equal(corners(2)[1].x.v.ch, 1, 'its vertical in the channel before its target');
  // Feedback: out to the channel after its source, along an outer track below, into its target from the channel before it.
  assert.equal(corners(3).length, 6);
  assert.equal(outer.below, 1);
  assert.ok(channels.get(0).n >= 1 && channels.get(1).n >= 1);
});

test('track order: two wires going down cross no more than they must (the one starting higher on the right)', () => {
  const g = (net, y0, y1, first) => ({ net, y0, y1, ends: [{ y: y0, dir: 'L' }, { y: y1, dir: 'R' }], verts: [], first });
  const high = g('high', 0, 100, 0);
  const low = g('low', 50, 150, 1);
  assert.deepEqual(orderTracks([high, low]).map((x) => x.net), ['low', 'high']);
  assert.deepEqual(orderTracks([low, high]).map((x) => x.net), ['low', 'high']);
});

test('every die by flow: a forward wire between neighbouring columns at most two bends, feedback outside the chips, deterministic', () => {
  for (const name of VOICES) {
    const d = die(name);
    const chips = d.nodes.filter((n) => n.kind === 'instance');
    const top = Math.min(...chips.map((n) => n.y));
    const bottom = Math.max(...chips.map((n) => n.y + n.h));
    const layer = new Map(d.nodes.map((n) => [n.id, n.layer]));
    for (const w of d.wires) {
      if (w.from.node === PARAMS) continue;
      if (!w.feedback && layer.get(w.to.node) - layer.get(w.from.node) === 1) assert.ok(w.corners.length - 2 <= 2, `${name}: ${w.label} has ${w.corners.length - 2} bends`);
      if (w.feedback) assert.ok(w.corners.some((p, k) => k > 0 && w.corners[k - 1].y === p.y && (p.y > bottom || p.y < top)), `${name}: ${w.label} round the outside`);
    }
    assert.deepEqual(die(name).wires.map((w) => w.corners), d.wires.map((w) => w.corners), `${name}: deterministic`);
  }
});

test('a chip moved by hand stays exactly where it was put; the others keep clear of it', () => {
  const d0 = die('moog_voice');
  const ladder = d0.nodes.find((n) => n.name === 'ladder');
  // Put the ladder over the oscillators' column.
  const osc = d0.nodes.find((n) => n.name === 'osc2');
  const at = { x: osc.x + 8, y: osc.y + 8 };
  const d = die('moog_voice', { ladder: at });
  const moved = d.nodes.find((n) => n.name === 'ladder');
  assert.deepEqual({ x: moved.x, y: moved.y }, at, 'exactly where it was put');
  assert.ok(moved.pinned);
  for (const n of d.nodes) if (n.kind === 'instance' && n !== moved) assert.ok(!intersects(n, moved), `${n.name} clear of it`);
  assert.ok(ladder);
  // Every wire still drawn, none under a chip but its own.
  for (const w of d.wires) assert.ok(w.corners.length >= 2 && !w.labelled, w.label);
});

test('junctions: where one net’s wires part, a dot; a corner of one wire is none', () => {
  const w = (id, corners) => ({ id, kind: 'connection', from: { node: 'a', port: 'out' }, to: { node: `t${id}`, port: 'in' }, corners });
  const wires = [w(0, [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 40 }, { x: 100, y: 40 }]), w(1, [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 80 }, { x: 100, y: 80 }])];
  assert.deepEqual(junctions(wires).map(({ x, y }) => [x, y]), [[50, 40]]);
  assert.deepEqual(junctions([wires[0]]), []);
});

test('straighten: a staircase between two pins becomes a Z where it is clear, and stays where a box is in the way', () => {
  const stairs = [{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 20 }, { x: 80, y: 20 }, { x: 80, y: 60 }, { x: 140, y: 60 }];
  const free = straighten([{ id: 0, net: 'n', corners: stairs.map((p) => ({ ...p })) }], []);
  assert.equal(free.get(0).length, 4, 'two bends');
  const blocked = straighten([{ id: 0, net: 'n', corners: stairs.map((p) => ({ ...p })) }], [{ id: 'box', x: 0, y: 30, w: 200, h: 10 }, { id: 'box2', x: -20, y: 5, w: 10, h: 100 }]);
  assert.ok(!blocked.has(0) || blocked.get(0).length < stairs.length);
});

/** The crossings among a die's parameter legs (different nets; a vertical through a horizontal, ends excluded). */
function legCrossings(d) {
  const legs = d.wires.filter((w) => w.from.node === PARAMS && !w.labelled);
  const segs = (c) => c.slice(1).map((q, i) => [c[i], q]);
  let n = 0;
  for (let i = 0; i < legs.length; i++) {
    for (let j = i + 1; j < legs.length; j++) {
      if (legs[i].from.port === legs[j].from.port) continue;
      for (const [a, b] of segs(legs[i].corners)) {
        for (const [c, e] of segs(legs[j].corners)) {
          const h1 = a.y === b.y;
          if (h1 === (c.y === e.y)) continue;
          const [H, V] = h1 ? [[a, b], [c, e]] : [[c, e], [a, b]];
          const x = V[0].x;
          const y = H[0].y;
          if (x > Math.min(H[0].x, H[1].x) && x < Math.max(H[0].x, H[1].x) && y > Math.min(V[0].y, V[1].y) && y < Math.max(V[0].y, V[1].y)) n++;
        }
      }
    }
  }
  return n;
}

test('siftBand: each net moved to its best lane (the Deck\'s nesting), kept on a tie, deterministic', () => {
  // Four legs going left from a strip, sources and drops in the same order: nested, none crosses (cost 0).
  const items = [
    { net: 'a', xs: 100, xe: [10] },
    { net: 'b', xs: 120, xe: [30] },
    { net: 'c', xs: 140, xe: [50] },
    { net: 'd', xs: 160, xe: [70] },
  ];
  const range = (it) => [Math.min(it.xs, ...it.xe), Math.max(it.xs, ...it.xe)];
  const inside = (x, r) => x > r[0] && x < r[1];
  const cost = (i, j) => (inside(j.xs, range(i)) ? 1 : 0) + i.xe.filter((x) => inside(x, range(j))).length;
  const total = (order) => order.reduce((s, it, k) => s + order.slice(k + 1).reduce((t, o) => t + cost(it, o), 0), 0);
  const worst = [...items].reverse();
  assert.ok(total(worst) > 0);
  const sifted = siftBand(worst, cost);
  assert.equal(total(sifted), 0, sifted.map((it) => it.net).join(''));
  assert.deepEqual(siftBand(sifted, cost), sifted, 'a nested order stays');
  for (const start of [items, worst, [items[2], items[0], items[3], items[1]]]) assert.equal(total(bandOrder(start).map((net) => items.find((it) => it.net === net))), 0, 'nested from any start');
  // A tie: two nets that never meet stay in the order given.
  const apart = [{ net: 'p', xs: 0, xe: [10] }, { net: 'q', xs: 100, xe: [110] }];
  assert.deepEqual(siftBand(apart, cost).map((it) => it.net), ['p', 'q']);
  assert.deepEqual(siftBand([...apart].reverse(), cost).map((it) => it.net), ['q', 'p'], 'kept on a tie');
});

test('a die\'s parameter legs: one bundle LEG_LANE apart under the strip, nested so that they cross each other rarely (the synth: 257 crossings before 2026-11-02)', () => {
  const d = die('moog_voice');
  const strip = d.nodes.find((n) => n.id === PARAMS);
  const legs = d.wires.filter((w) => w.from.node === PARAMS);
  assert.ok(legs.length >= 20 && legs.every((w) => !w.labelled));
  // Each net's run in the band: its first horizontal under the strip.
  const lanes = [...new Set(legs.map((w) => w.corners.find((p, k) => k > 0 && w.corners[k - 1].y === p.y && p.y > strip.y)?.y).filter(Number.isFinite))].sort((a, b) => a - b);
  const steps = lanes.slice(1).map((y, k) => y - lanes[k]);
  assert.ok(steps.filter((s) => s === LEG_LANE).length >= steps.length / 2, `lanes ${LEG_LANE} apart: ${steps.join(' ')}`);
  assert.ok(legCrossings(d) <= 40, `${legCrossings(d)} crossings among the legs`);
  for (const name of VOICES.filter((v) => v !== 'moog_voice')) {
    const v = die(name);
    const n = v.wires.filter((w) => w.from.node === PARAMS).length;
    assert.ok(legCrossings(v) <= Math.max(4, n), `${name}: ${legCrossings(v)} crossings among ${n} legs`);
  }
});

test('orderTracks: no group would cross fewer wires on another track of its channel (sifted, 2026-11-02)', () => {
  const cost = (a, b) => a.ends.filter((e) => e.dir === 'R' && e.y > b.y0 && e.y < b.y1).length + b.ends.filter((e) => e.dir === 'L' && e.y > a.y0 && e.y < a.y1).length;
  const total = (list) => list.reduce((s, g, i) => s + list.slice(i + 1).reduce((t, h) => t + cost(g, h), 0), 0);
  // A fixed pseudo-random channel: twelve groups, each attached left and right at heights in 0..400.
  let seed = 7;
  const next = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648) * 400;
  const groups = Array.from({ length: 12 }, (_, i) => {
    const a = Math.round(next());
    const b = Math.round(next());
    return { first: i, y0: Math.min(a, b), y1: Math.max(a, b), ends: [{ dir: 'L', y: a }, { dir: 'R', y: b }] };
  });
  const order = orderTracks(groups);
  const best = total(order);
  for (let i = 0; i < order.length; i++) {
    for (let j = 0; j < order.length; j++) {
      const moved = [...order];
      const [g] = moved.splice(i, 1);
      moved.splice(j, 0, g);
      assert.ok(total(moved) >= best, `moving ${g.first} to ${j}: ${total(moved)} < ${best}`);
    }
  }
  assert.deepEqual(orderTracks(groups).map((g) => g.first), order.map((g) => g.first), 'deterministic');
});

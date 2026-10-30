// ADR 0047 phase 3 (tests 8, 9): deep zoom. Each composite's inside is a
// die, laid out by the board's own engine (its interface pins at its edges,
// its chips by flow, the channel router and the maze), drawn in its package
// at the top level, which is widened towards it and never moves. The
// studio's synth (moog_voice) and its drum voices, the kick's drums: no
// wire under a chip, every net drawn, the same layout every time, each
// within the layout budget; the bond wires join each package pin to its
// die's pin; a die drawn moves nothing.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { INPUTS, OUTPUTS, WIDEN_MAX, dieArea, layout, widenPackage } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';
import { boundaryLinks, inside, scene } from './schematic-scene.js';
import { DIE_TEXT, FADE_FROM_PX } from './board-zoom.js';

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const STUDIO = read('./fixtures/studio-graph.json');
const KICK = read('./fixtures/board/kick.json');
const VOICES = ['moog_voice', 'kick909', 'snare909', 'clap909', 'hat909', 'tom909', 'rim909', 'cymbal909', 'tape_delay'];

/** A graph's levels as the page lays them out: the top level a board, each inside a die, each laid out once. */
function levels(graph) {
  const cache = new Map();
  const factsOf = (m) => exportFacts(graph.composites?.[m] ?? null);
  const of = (level) => {
    if (!cache.has(level)) {
      const source = level === '' ? graph.top : graph.composites?.[level];
      const dieOf = (node) => of(node.module)?.bounds ?? null;
      cache.set(level, source ? layout(source, {}, { factsOf, board: level === '' ? { dieOf } : { die: true, dieOf } }) : null);
    }
    return cache.get(level);
  };
  return of;
}

const dieOf = (graph, name) => layout(graph.composites[name], {}, { factsOf: (m) => exportFacts(graph.composites?.[m] ?? null), board: { die: true } });

const intersects = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** `value` with every object's keys in reverse order (arrays as they are). */
function reversedKeys(value) {
  if (Array.isArray(value)) return value.map(reversedKeys);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).reverse().map((k) => [k, reversedKeys(value[k])]));
  return value;
}

const snapshot = (b) => ({ nodes: b.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]), wires: b.wires.map((w) => [w.id, w.labelled, ...w.corners.flatMap((p) => [p.x, p.y])]), labels: b.board.labels.map((l) => [l.text, l.at.x, l.at.y, l.textAt.x, l.textAt.y]) });

test('a die: its interface at its edges, every net drawn, no wire under a chip (moog_voice, the drum voices, tape_delay, the kick’s)', () => {
  const report = [];
  for (const [graph, names] of [[STUDIO, VOICES], [KICK, ['drums', 'kick']]]) {
    for (const name of names) {
      const die = dieOf(graph, name);
      const source = graph.composites[name];
      const chips = die.nodes;
      // Its inputs' pins at its left edge, its outputs' at its right.
      const ins = chips.find((n) => n.id === INPUTS);
      const outs = chips.find((n) => n.id === OUTPUTS);
      if (ins) assert.equal(ins.x, Math.min(...chips.map((n) => n.x)), `${name}: inputs leftmost`);
      if (outs) assert.equal(outs.x + outs.w, Math.max(...chips.map((n) => n.x + n.w)), `${name}: outputs rightmost`);
      // Every connection and every export is a wire, drawn: routed, or labelled at its ends when no way was found.
      const exports = (source.inputs ?? []).reduce((s, e) => s + (e.to ?? []).length, 0) + (source.parameters ?? []).reduce((s, e) => s + (e.to ?? []).length, 0) + (source.outputs ?? []).filter((e) => e.from).length;
      assert.equal(die.wires.length, (source.connections ?? []).length + exports, `${name}: a wire per connection and export`);
      for (const w of die.wires) {
        assert.ok(w.corners.length >= 2, `${name}: ${w.label} drawn`);
        if (w.labelled) assert.ok(die.board.labels.some((l) => l.wire === w.id), `${name}: ${w.label} labelled`);
      }
      // No fan-out or global net is drawn as labels in a die: only a wire with no way (a few, recorded).
      assert.ok(die.board.metrics.labels <= 5, `${name}: ${die.board.metrics.labels} labelled nets`);
      for (const w of die.wires) {
        if (w.labelled) continue;
        for (let k = 1; k < w.corners.length; k++) {
          const [p, q] = [w.corners[k - 1], w.corners[k]];
          assert.ok(p.x === q.x || p.y === q.y, `${name}: ${w.label} orthogonal`);
          const seg = { x: Math.min(p.x, q.x), y: Math.min(p.y, q.y), w: Math.abs(q.x - p.x) || 0.001, h: Math.abs(q.y - p.y) || 0.001 };
          for (const n of chips) {
            if ((k === 1 && n.id === w.from.node) || (k === w.corners.length - 1 && n.id === w.to.node)) continue;
            assert.ok(!intersects(seg, n), `${name}: ${w.label} under ${n.id}`);
          }
        }
      }
      for (const a of chips) for (const b of chips) assert.ok(a === b || !intersects(a, b), `${name}: ${a.id} over ${b.id}`);
      report.push(`${name}: ${die.board.metrics.crossings} crossings, ${die.board.metrics.bends} bends, ${die.board.metrics.labels} labelled nets, ${die.wires.length} wires`);
    }
  }
  console.log(report.join('\n'));
});

test('a die is deterministic: the same inside twice, and with every object’s keys in another order, gives the same die', () => {
  for (const name of VOICES) {
    const a = snapshot(dieOf(STUDIO, name));
    assert.deepEqual(snapshot(dieOf(STUDIO, name)), a, name);
    assert.deepEqual(snapshot(dieOf(reversedKeys(STUDIO), name)), a, `${name}, keys reversed`);
  }
});

test('budget (test 9): each die laid out in under 50 ms', () => {
  const report = [];
  for (const name of VOICES) {
    dieOf(STUDIO, name);
    let best = Infinity;
    for (let i = 0; i < 3; i++) {
      const t = performance.now();
      dieOf(STUDIO, name);
      best = Math.min(best, performance.now() - t);
    }
    report.push(`${name}: ${best.toFixed(1)} ms`);
    assert.ok(best < 50, `${name}: ${best.toFixed(1)} ms`);
  }
  console.log(report.join('\n'));
});

test('a package is widened towards its die, at most WIDEN_MAX, in whole plate units; never narrower nor taller', () => {
  const node = { x: 0, y: 0, w: 104, h: 154 };
  const wide = widenPackage({ ...node }, { w: 1574, h: 600 });
  assert.equal(wide.w, Math.ceil((104 * WIDEN_MAX) / 8) * 8);
  assert.equal(wide.h, 154);
  assert.ok(wide.die);
  const tall = widenPackage({ ...node }, { w: 100, h: 1000 });
  assert.deepEqual([tall.w, tall.h], [104, 154], 'a tall die: as it was');
  assert.deepEqual(dieArea({ x: 10, y: 20, w: 104, h: 154 }), { x: 18, y: 54, w: 88, h: 112 });
});

test('the studio’s synth zoomed into: its die in its package, every package pin bonded to its die pin, nothing moved (test 8)', () => {
  const of = levels(STUDIO);
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  assert.ok(synth.die, 'a package');
  const die = of('moog_voice');
  // The bond wires: each package pin that the inside exports joins its frame pin.
  const links = boundaryLinks(synth, die);
  const framed = new Set([...(die.nodes.find((n) => n.id === INPUTS)?.outs ?? []), ...(die.nodes.find((n) => n.id === OUTPUTS)?.ins ?? [])].map((p) => p.name));
  const pins = [...synth.ins, ...synth.outs].filter((p) => framed.has(p.name));
  assert.equal(links.length, pins.length, 'one bond wire per pin');
  assert.ok(links.length >= 3, `${links.length} bond wires`);
  // The die lies in its package's body.
  const t = inside(synth, die.bounds);
  const area = dieArea(synth);
  assert.ok(die.bounds.x * t.k + t.x >= area.x - 1e-6 && (die.bounds.x + die.bounds.w) * t.k + t.x <= area.x + area.w + 1e-6);
  assert.ok(die.bounds.y * t.k + t.y >= area.y - 1e-6 && (die.bounds.y + die.bounds.h) * t.k + t.y <= area.y + area.h + 1e-6);
  // Zoomed into, at the scale its die's text is drawn: the top level's boxes are the stored ones, box for box.
  const viewport = { w: 1600, h: 1000 };
  const k = (FADE_FROM_PX + 1) / DIE_TEXT / t.k;
  const view = { k, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: 800, y: 500 };
  const closed = scene({ layoutOf: of, view, viewport });
  const open = scene({ layoutOf: of, view, viewport, pinned: new Set(['/synth']) });
  assert.deepEqual([...open.open], ['/synth']);
  assert.deepEqual(open.levels[0].layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]), closed.levels[0].layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]));
  assert.equal(open.levels[0].layout.opened, 'synth');
  // Every inner wire is in the scene at every zoom (a board culls none).
  assert.equal(open.levels[1].wires.length, die.wires.length);
});

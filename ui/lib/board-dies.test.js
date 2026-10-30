// ADR 0047 phase 3 (tests 8, 9): deep zoom, as the coordinator designed it
// after 63bbc55. Each composite's inside is a die laid out at its natural
// size by the board's own engine: its inputs on its left edge, its outputs
// on its right, its parameters on its top edge dropping into its chips' top
// pins, its chips in flow columns, the channel router and the maze. It
// opens over its package (`dieView`), growing from the package’s rectangle
// to its own, and nothing around it moves. The studio's synth (moog_voice)
// and its drum voices, the kick's drums: the die's rectangle, every
// interface pin on its edge, no wire under a chip, every net drawn, the
// same layout every time, each within the layout budget.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { DIE_MARGIN, INPUTS, OUTPUTS, PARAMS, layout } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';
import { boundaryLinks, drawnInside, scene } from './schematic-scene.js';
import { dieView, edgeStubs } from './board-die.js';
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
      // Its rectangle: its inputs' pins on its left edge, its outputs' on its right, its parameters' on its top; else
      // what it holds DIE_MARGIN out; everything inside it.
      const r = die.bounds;
      assert.deepEqual(die.board.die, r);
      const ins = chips.find((n) => n.id === INPUTS);
      const outs = chips.find((n) => n.id === OUTPUTS);
      const params = chips.find((n) => n.id === PARAMS);
      for (const p of ins?.outs ?? []) assert.equal(p.x, r.x, `${name}: ${p.name} on the left edge`);
      for (const p of outs?.ins ?? []) assert.equal(p.x, r.x + r.w, `${name}: ${p.name} on the right edge`);
      for (const p of params?.outs ?? []) assert.equal(p.y, r.y, `${name}: ${p.name} on the top edge`);
      const held = chips.filter((n) => n.kind === 'instance');
      const lowest = Math.max(...held.map((n) => n.y + n.h), ...die.wires.map((w) => w.box.y1));
      assert.equal(r.y + r.h, lowest + DIE_MARGIN, `${name}: its bottom what it holds and a margin`);
      // Else a margin out, on a whole grid step (16 units).
      const near = (v, edge) => v - DIE_MARGIN - edge >= 0 && v - DIE_MARGIN - edge < 16;
      if (!ins) assert.ok(near(Math.min(...held.map((n) => n.x), ...die.wires.map((w) => w.box.x0)), r.x), `${name}: left`);
      if (!params) assert.ok(near(Math.min(...held.map((n) => n.y), ...die.wires.map((w) => w.box.y0)), r.y), `${name}: top`);
      for (const n of held) assert.ok(n.x >= r.x && n.y >= r.y && n.x + n.w <= r.x + r.w && n.y + n.h <= r.y + r.h, `${name}: ${n.id} inside`);
      for (const w of die.wires) for (const q of w.corners) assert.ok(q.x >= r.x - 1e-6 && q.x <= r.x + r.w + 1e-6 && q.y >= r.y - 1e-6 && q.y <= r.y + r.h + 1e-6, `${name}: ${w.label} inside`);
      // Every connection and every export is a wire, drawn: routed, or labelled at its ends when no way was found.
      const exports = (source.inputs ?? []).reduce((s, e) => s + (e.to ?? []).length, 0) + (source.parameters ?? []).reduce((s, e) => s + (e.to ?? []).length, 0) + (source.outputs ?? []).filter((e) => e.from).length;
      assert.equal(die.wires.length, (source.connections ?? []).length + exports, `${name}: a wire per connection and export`);
      for (const w of die.wires) {
        assert.ok(w.corners.length >= 2, `${name}: ${w.label} drawn`);
        if (w.labelled) assert.ok(die.board.labels.some((l) => l.wire === w.id), `${name}: ${w.label} labelled`);
      }
      // No fan-out or global net is drawn as labels in a die: only a wire with no way (a few, recorded).
      assert.ok(die.board.metrics.labels <= 8, `${name}: ${die.board.metrics.labels} labelled nets`);
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

test('a die opens over its package: from the package’s rectangle to its own natural size, centred, kept within its level', () => {
  const of = levels(STUDIO);
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const die = of('moog_voice');
  const closed = dieView(synth, die, 0, top.bounds);
  assert.deepEqual(closed.rect, { x: synth.x, y: synth.y, w: synth.w, h: synth.h });
  const open = dieView(synth, die, 1, top.bounds);
  assert.equal(open.link.k, 1, 'one die unit a unit of its level');
  assert.equal(open.rect.w, die.bounds.w);
  assert.equal(open.rect.h, die.bounds.h);
  const b = top.bounds;
  // Within its level on each axis it fits on, else from its level's edge.
  if (open.rect.w <= b.w) assert.ok(open.rect.x >= b.x && open.rect.x + open.rect.w <= b.x + b.w + 1e-6, 'within its level across');
  else assert.equal(open.rect.x, b.x);
  if (open.rect.h <= b.h) assert.ok(open.rect.y >= b.y && open.rect.y + open.rect.h <= b.y + b.h + 1e-6, 'within its level down');
  else assert.equal(open.rect.y, b.y);
  // Unclamped, centred on the package.
  const free = dieView(synth, die, 1, null);
  assert.ok(Math.abs(free.rect.x + free.rect.w / 2 - (synth.x + synth.w / 2)) < 1e-9 && Math.abs(free.rect.y + free.rect.h / 2 - (synth.y + synth.h / 2)) < 1e-9);
  // Half open: between, the die filling its rectangle.
  const half = dieView(synth, die, 0.5, top.bounds);
  assert.ok(half.rect.w > synth.w && half.rect.w < die.bounds.w);
  // The stubs at its edge pins name what the board connects to the package.
  const stubs = edgeStubs(synth, top, die);
  assert.ok(stubs.some((s) => s.side === 'left') && stubs.some((s) => s.side === 'right'), JSON.stringify(stubs.map((s) => [s.side, s.text])));
  assert.ok(stubs.some((s) => s.side === 'right' && /echo/.test(s.text)), 'the synth’s output names the echo');
});

test('the studio’s synth zoomed into: its die over the board, no bond wire, nothing moved (test 8)', () => {
  const of = levels(STUDIO);
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  assert.ok(synth.die, 'a package');
  const die = of('moog_voice');
  assert.deepEqual(boundaryLinks(synth, die), [], 'no bond wire');
  const viewport = { w: 1600, h: 1000 };
  const view = { k: 0.6, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: 800, y: 500 };
  const closed = scene({ layoutOf: of, view, viewport });
  const open = scene({ layoutOf: of, view, viewport, pinned: new Set(['/synth']) });
  assert.deepEqual([...open.open], ['/synth']);
  assert.deepEqual(open.levels[0].layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]), closed.levels[0].layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]));
  assert.equal(open.levels[0].layout.opened, 'synth');
  assert.equal(open.levels[1].layout, die, 'the die as laid out');
  assert.ok(Math.abs(open.levels[1].transform.k - 0.6) < 1e-9, 'at its natural size: the level’s scale');
  // Half open, the die is drawn smaller.
  const half = scene({ layoutOf: of, view, viewport, pinned: new Set(['/synth']), opening: new Map([['/synth', 0.5]]) });
  assert.ok(half.levels[1].transform.k < 0.6 && half.levels[1].transform.k > 0);
  // Every inner wire is in the scene at every zoom (a board culls none).
  assert.equal(open.levels[1].wires.length, die.wires.length);
  void drawnInside;
});

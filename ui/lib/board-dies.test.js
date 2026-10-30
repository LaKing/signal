// ADR 0047 phase 3 (tests 8, 9): deep zoom as a microscope (the owner's
// verdict on 9e81dc8). Each composite's inside is a die laid out at its
// natural size by the board's own engine: its inputs on its left edge, its
// outputs on its right, its parameters on its top edge dropping into its
// chips' top pins, its chips in flow columns. Its package on the level
// around has the die's aspect and its pins where the die's interface pins
// fall when the die is scaled into it: one geometry. The camera zooms until
// the package fills the stage; nothing grows, nothing moves. The studio's
// synth (moog_voice) and its drum voices, the kick's drums: the die's
// rectangle, every interface pin on its edge and on its package's pin, no
// wire under a chip, every net drawn, the same layout every time, each
// within the layout budget.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { INPUTS, OUTPUTS, PARAMS, dieLink, layout } from './schematic-layout.js';
import { PU } from './board-layout.js';
import { exportFacts } from './schematic-groups.js';
import { boundaryLinks, drawnInside, scene } from './schematic-scene.js';
import { DIE_TEXT, FADE_FROM_PX } from './board-zoom.js';

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const STUDIO = read('./fixtures/studio-graph.json');
const KICK = read('./fixtures/board/kick.json');
const VOICES = ['moog_voice', 'kick909', 'snare909', 'clap909', 'hat909', 'tom909', 'rim909', 'cymbal909', 'tape_delay'];

/** A graph's levels as the page lays them out: the top level a board, each inside a die, each laid out once. */
function levels(graph) {
  const cache = new Map();
  const factsOf = (m) => exportFacts(graph.composites?.[m] ?? null);
  // As the page (`dieFor`): a die's parameters in the order of its package's top pins when the package asks first.
  const orders = new Map();
  const dieOf = (node) => {
    if (!graph.composites?.[node.module]) return null;
    if (!cache.has(node.module) && node.topOrder && !orders.has(node.module)) orders.set(node.module, node.topOrder);
    return of(node.module);
  };
  const of = (level) => {
    if (!cache.has(level)) {
      const source = level === '' ? graph.top : graph.composites?.[level];
      cache.set(level, source ? layout(source, {}, { factsOf, board: level === '' ? { dieOf } : { die: true, dieOf, paramOrder: orders.get(level) ?? null } }) : null);
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
      for (const v of [r.x, r.y, r.w, r.h]) assert.ok(Math.abs(v % 16) === 0, `${name}: on whole grid steps`);
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

test('a package has its die’s aspect and its pins where its die’s interface pins fall: one geometry (studio, kick)', () => {
  for (const [graph, names] of [[STUDIO, ['synth', 'echo', 'kick', 'snare', 'clap', 'hats', 'tom', 'rim', 'cymbal']], [KICK, ['drums']]]) {
    const of = levels(graph);
    const top = of('');
    for (const name of names) {
      const pkg = top.nodes.find((n) => n.name === name);
      assert.ok(pkg?.die, `${name}: a package`);
      const die = of(pkg.module);
      // The aspect within rounding (its width on the plate grid, its height rounded).
      assert.equal(pkg.w % PU, 0);
      assert.ok(Math.abs(pkg.w / pkg.h - die.bounds.w / die.bounds.h) <= (die.bounds.w / die.bounds.h) * (1 / pkg.h + 1e-9), `${name}: ${pkg.w}×${pkg.h} against ${die.bounds.w}×${die.bounds.h}`);
      // Scaled uniformly into the package, within it.
      const t = dieLink(pkg, die);
      const corner = { x: die.bounds.x * t.k + t.x, y: die.bounds.y * t.k + t.y };
      assert.ok(corner.x >= pkg.x - 0.5 && corner.y >= pkg.y - 0.5 && corner.x + die.bounds.w * t.k <= pkg.x + pkg.w + 0.5 && corner.y + die.bounds.h * t.k <= pkg.y + pkg.h + 0.5, `${name}: fits`);
      // Every interface pin of the die is a pin of the package, at the same point (whole units: within half a unit).
      const frame = (id) => die.nodes.find((n) => n.id === id);
      const pins = [...(frame(INPUTS)?.outs ?? []).map((p) => [p, false]), ...(frame(PARAMS)?.outs ?? []).map((p) => [p, false]), ...(frame(OUTPUTS)?.ins ?? []).map((p) => [p, true])];
      assert.ok(pins.length > 0, name);
      for (const [p, out] of pins) {
        const q = (out ? pkg.outs : pkg.ins).find((o) => o.name === p.name);
        assert.ok(q, `${name}: ${p.name} a package pin`);
        const at = { x: p.x * t.k + t.x, y: p.y * t.k + t.y };
        assert.ok(Math.abs(at.x - q.x) <= 0.5 && Math.abs(at.y - q.y) <= 0.5, `${name}: ${p.name} at ${q.x},${q.y}, its die's at ${at.x},${at.y}`);
      }
    }
  }
});

test('the studio’s synth zoomed into: its package as laid out, its die scaled into it, nothing moved, no bond wire (test 8)', () => {
  const of = levels(STUDIO);
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const die = of('moog_voice');
  assert.deepEqual(boundaryLinks(synth, die), [], 'no bond wire');
  const viewport = { w: 1600, h: 1000 };
  const view = { k: 2, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: 800, y: 500 };
  const closed = scene({ layoutOf: of, view, viewport });
  const open = scene({ layoutOf: of, view, viewport, pinned: new Set(['/synth']) });
  assert.deepEqual([...open.open], ['/synth']);
  const rects = (d) => d.levels[0].layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]);
  assert.deepEqual(rects(open), rects(closed), 'every box, the package too, as laid out');
  assert.equal(open.levels[0].layout.opened, 'synth');
  const t = drawnInside(synth, die).link;
  assert.ok(Math.abs(open.levels[1].transform.k - 2 * t.k) < 1e-9, 'the die at its package’s scale');
  // Every inner wire is in the scene at every zoom (a board culls none).
  assert.equal(open.levels[1].wires.length, die.wires.length);
});

// ADR 0047 phase 2 (tests 5–7, 9): the board as `layout` gives it for the
// example projects' top levels (ui/lib/schematic-layout.js `board`):
// deterministic, its snapshots, its routes, its anchoring, and the
// budgets (layout under 50 ms, at most 5 000 drawn elements). The
// projects' structures are `graph`'s (ui/lib/fixtures/board/*.json and
// studio-graph.json, checked against the projects by
// crates/signal-project/tests/structure.rs). The vias are a fixed
// stand-in for the plate as measured (no browser here): each control of
// the project's panel in a grid of knob cells. Snapshots are written again
// with SIGNAL_WRITE_BOARD_SNAPSHOTS=1, only with a recorded reason.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { layout } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';
import { panelControls } from './schematic-controls.js';
import { polylineMetrics } from './board-route.js';
import { centroid, near, CLEARANCE } from './board-layout.js';
import { shownWires } from './board-view.js';
import { bundleWires } from './schematic-bundle.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const PROJECTS = {
  kick: 'ui/lib/fixtures/board/kick.json',
  studio: 'ui/lib/fixtures/studio-graph.json',
  'studio-transport': 'ui/lib/fixtures/board/studio-transport.json',
  'physical-av': 'ui/lib/fixtures/board/physical-av.json',
  'library-demo': 'ui/lib/fixtures/board/library-demo.json',
};
const SNAPSHOTS = new URL('ui/lib/fixtures/board/snapshots.json', root);

function project(name) {
  const graph = JSON.parse(read(PROJECTS[name]));
  const panel = new URL(`projects/${name}/ui/main.html`, root);
  const controls = existsSync(panel) ? panelControls(readFileSync(panel, 'utf8')) : [];
  return { graph, controls, factsOf: (m) => exportFacts(graph.composites?.[m] ?? null) };
}

/**
 * The stand-in plate, as dense as a real one: the panel's controls in cards
 * by the instance they bind (in the panel's order), each card rows of up to
 * 7 knob cells of 88 × 128 px (the default panel's `CELLS`) under a 40 px
 * heading, the cards packed on shelves 1280 px wide, 16 px apart; every
 * rectangle in whole plate units.
 */
function standIn(controls) {
  const vias = new Map();
  const obstacles = [];
  const cards = new Map();
  for (const c of controls) {
    const host = String(c.binds[0] ?? '').replace(/^\//, '').split(/[./]/)[0];
    if (!cards.has(host)) cards.set(host, []);
    cards.get(host).push(c);
  }
  let x = 0;
  let y = 0;
  let shelf = 0;
  for (const list of cards.values()) {
    const across = Math.min(7, list.length);
    const w = across * 88 + 32;
    const h = 40 + Math.ceil(list.length / 7) * 128 + 16;
    if (x + w > 1280) {
      x = 0;
      y += shelf + 16;
      shelf = 0;
    }
    list.forEach((c, i) => {
      const r = { x: x + 16 + (i % 7) * 88 + 8, y: y + 40 + Math.floor(i / 7) * 128, w: 72, h: 112 };
      vias.set(c.index, r);
      obstacles.push(r);
    });
    x += w + 16;
    shelf = Math.max(shelf, h);
  }
  return { vias, obstacles, plate: controls.length ? { w: 1280, h: y + shelf } : null };
}

function boardOf(name, arrange, positions = {}) {
  const { graph, controls, factsOf } = project(name);
  const board = arrange === 'flow' && !controls.length ? { arrange } : { arrange, ...standIn(controls) };
  return layout(graph.top, positions, { factsOf, controls, board });
}

/** What a snapshot keeps of a board: boxes, routes' corners, labels. */
function snapshot(b) {
  return {
    nodes: b.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]),
    wires: b.wires.map((w) => [w.id, w.labelled ? 'L' : '', ...w.corners.flatMap((p) => [p.x, p.y])]),
    labels: b.board.labels.map((l) => [l.side, l.text, l.at.x, l.at.y]),
    metrics: b.board.metrics,
  };
}

/** `value` with every object's keys in reverse order (arrays as they are). */
function reversedKeys(value) {
  if (Array.isArray(value)) return value.map(reversedKeys);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).reverse().map((k) => [k, reversedKeys(value[k])]));
  return value;
}

test('determinism (test 5): the same level twice, and with every object’s keys in another order, gives the same board', () => {
  for (const name of Object.keys(PROJECTS)) {
    for (const arrange of ['controls', 'flow']) {
      const a = snapshot(boardOf(name, arrange));
      const b = snapshot(boardOf(name, arrange));
      assert.deepEqual(a, b, `${name} ${arrange}`);
      const { graph, controls, factsOf } = project(name);
      const shuffled = reversedKeys(graph);
      const board = arrange === 'flow' && !controls.length ? { arrange } : { arrange, ...standIn(controls) };
      const c = snapshot(layout(shuffled.top, {}, { factsOf: (m) => exportFacts(shuffled.composites?.[m] ?? null), controls, board }));
      assert.deepEqual(c, a, `${name} ${arrange}, keys reversed`);
    }
  }
});

test('snapshots (test 5): the example projects’ boards are as recorded', () => {
  const now = {};
  for (const name of Object.keys(PROJECTS)) now[name] = { controls: snapshot(boardOf(name, 'controls')), flow: snapshot(boardOf(name, 'flow')) };
  if (process.env.SIGNAL_WRITE_BOARD_SNAPSHOTS) writeFileSync(SNAPSHOTS, `${JSON.stringify(now)}\n`);
  const recorded = JSON.parse(readFileSync(SNAPSHOTS, 'utf8'));
  for (const name of Object.keys(PROJECTS)) {
    for (const arrange of ['controls', 'flow']) assert.deepEqual(now[name][arrange], recorded[name][arrange], `${name} by ${arrange}: changed; write again with SIGNAL_WRITE_BOARD_SNAPSHOTS=1 and record why`);
  }
});

test('routing (test 6): arranged by flow, no route passes under a box; the toggles move nothing', () => {
  for (const name of Object.keys(PROJECTS)) {
    const b = boardOf(name, 'flow');
    for (const w of b.wires) {
      if (w.labelled) continue;
      const c = w.corners;
      for (let i = 1; i < c.length; i++) {
        const [p, q] = [c[i - 1], c[i]];
        assert.ok(p.x === q.x || p.y === q.y, 'orthogonal');
        for (const n of b.nodes) {
          if ((i === 1 && n.id === w.from.node) || (i === c.length - 1 && n.id === w.to.node)) continue;
          if (n.via && w.kind === 'binding') continue;
          const inside = Math.min(p.x, q.x) < n.x + n.w && Math.max(p.x, q.x) > n.x && Math.min(p.y, q.y) < n.y + n.h && Math.max(p.y, q.y) > n.y;
          assert.ok(!inside, `${name}: ${w.label} under ${n.id}`);
        }
      }
    }
    // A kind hidden: its wires are not drawn, and nothing else changes (the layout never reads the toggles).
    const solo = new Set();
    const shown = shownWires(b.wires, ['events'], solo, (w) => w.id);
    assert.ok(shown.every((w) => w.shape !== 'events'));
    assert.deepEqual(snapshot(boardOf(name, 'flow')), snapshot(b));
  }
});

test('anchoring (test 7): an anchored chip covers no via and lies near its vias; a stored position wins', () => {
  const b = boardOf('studio', 'controls');
  const vias = b.nodes.filter((n) => n.kind === 'control');
  assert.ok(vias.length > 20, 'the studio’s panel controls');
  assert.ok(b.board.anchored.length >= 5, `anchored: ${b.board.anchored}`);
  for (const id of b.board.anchored) {
    const chip = b.nodes.find((n) => n.id === id);
    for (const v of vias) assert.ok(!near(chip, v.control.rect, CLEARANCE - 1), `${id} over a control of the plate`);
    const own = vias.filter((v) => v.host === id).map((v) => v.control.rect);
    const c = centroid(own);
    const d = Math.hypot(chip.x + chip.w / 2 - c.x, chip.y + chip.h / 2 - c.y);
    assert.ok(d < 1000, `${id} ${Math.round(d)} from its vias`);
  }
  const pinned = boardOf('studio', 'controls', { synth: { x: 2000, y: 40 } });
  const synth = pinned.nodes.find((n) => n.id === 'synth');
  assert.deepEqual([synth.x, synth.y, synth.pinned], [2000, 40, true]);
  const flow = boardOf('studio', 'flow', { synth: { x: 2000, y: 40 } });
  assert.deepEqual([flow.nodes.find((n) => n.id === 'synth').x, flow.board.anchored.length], [2000, 0]);
  // Every via is where its control is on the plate.
  for (const v of vias) assert.ok(v.via && v.pinned);
});

/** About how many SVG elements a board draws (a model of `nodeElement`, `controlElement` and the wires: an estimate, not a count of a DOM). */
function elements(b) {
  let n = 0;
  for (const node of b.nodes) {
    const pins = node.ins.length + node.outs.length;
    if (node.kind === 'control') n += 1 + 1 + 1 + 4 + 1 + pins * 2;
    else n += 1 + 1 + 1 + 2 + pins * 3;
  }
  n += b.wires.length + b.board.labels.length * 2;
  return n;
}

test('budgets (test 9): each board laid out in under 50 ms, at most 5 000 elements drawn', () => {
  const report = [];
  for (const name of Object.keys(PROJECTS)) {
    for (const arrange of ['controls', 'flow']) {
      boardOf(name, arrange);
      let best = Infinity;
      for (let i = 0; i < 5; i++) {
        const t = performance.now();
        boardOf(name, arrange);
        best = Math.min(best, performance.now() - t);
      }
      const b = boardOf(name, arrange);
      const count = elements(b);
      report.push(`${name} ${arrange}: ${best.toFixed(1)} ms, ~${count} elements`);
      assert.ok(best < 50, `${name} ${arrange}: ${best.toFixed(1)} ms`);
      assert.ok(count <= 5000, `${name} ${arrange}: ${count} elements`);
    }
  }
  // Measured, never deciding (rule 6): the curved layout's against the board's, as `node --test` prints them.
  for (const name of ['studio', 'library-demo']) {
    const { graph, factsOf } = project(name);
    const old = layout(graph.top, {}, { factsOf });
    const was = polylineMetrics(old.wires.map((w) => ({ points: w.points, net: `${w.from.node}.${w.from.port}` })));
    const now = layout(graph.top, {}, { factsOf, board: { arrange: 'flow' } }).board.metrics;
    report.push(`${name}: curves ${was.crossings} crossings, length ${was.length}; board by flow (no plate) ${now.crossings} crossings, ${now.bends} bends, length ${now.length}, ${now.labels} labelled nets`);
  }
  console.log(report.join('\n'));
});

test('buses (§6.4): zoomed out, a board’s pair of wires is one bus with its count along its trunk; a labelled net is never in one', () => {
  const { graph, factsOf } = project('studio');
  const b = layout(graph.top, {}, { factsOf, board: { arrange: 'flow' } });
  const items = bundleWires(b.wires, 0.05);
  const buses = items.filter((i) => i.kind === 'bundle');
  assert.ok(buses.length > 0, 'the studio has pairs');
  for (const bus of buses) {
    assert.equal(bus.count, bus.wires.length);
    assert.ok(bus.wires.every((w) => !w.labelled));
    const turns = bus.curve.segs.filter((s) => s.c1).length;
    assert.ok(turns <= 4, 'straight runs and a few rounded turns: no patch-cord curve');
  }
  for (const w of b.wires.filter((x) => x.labelled)) assert.ok(items.includes(w), 'a labelled wire on its own');
});

/**
 * A plate as tall as the studio's measured one: MONO SYNTH's knobs in rows,
 * its piano roll (a control of no instance: an obstacle, no via) under
 * them, the whole width; RHYTHM COMPOSER and the rest below it, densely.
 */
function tallPlate(controls) {
  const vias = new Map();
  const obstacles = [];
  const synth = controls.filter((c) => /^\/synth[./]/.test(c.binds[0] ?? ''));
  const rest = controls.filter((c) => !synth.includes(c));
  synth.forEach((c, i) => {
    const r = { x: 24 + (i % 14) * 88, y: 48 + Math.floor(i / 14) * 112, w: 64, h: 104 };
    vias.set(c.index, r);
    obstacles.push(r);
  });
  const roll = { x: 24, y: 48 + Math.ceil(synth.length / 14) * 112 + 8, w: 1232, h: 320 };
  obstacles.push(roll);
  const top = roll.y + roll.h + 56;
  rest.forEach((c, i) => {
    const r = { x: 24 + (i % 13) * 96, y: top + Math.floor(i / 13) * 120, w: 72, h: 104 };
    vias.set(c.index, r);
    obstacles.push(r);
  });
  return { vias, obstacles, plate: { w: 1280, h: top + Math.ceil(rest.length / 13) * 120 + 24 } };
}

test('no chip lies over a control as measured (§5), on a plate as tall as the studio’s, a stored position from before the board included', () => {
  const { graph, controls, factsOf } = project('studio');
  const plate = tallPlate(controls);
  // A stored position over the drum knobs (written by the curved layout) is moved off them on the board.
  const over = plate.vias.get(controls.find((c) => /^\/kick\./.test(c.binds[0] ?? '')).index);
  for (const positions of [{}, { kick: { x: over.x, y: over.y } }]) {
    for (const arrange of ['controls', 'flow']) {
      const b = layout(graph.top, positions, { factsOf, controls, board: { arrange, ...plate } });
      const chips = b.nodes.filter((n) => n.kind !== 'control');
      for (const chip of chips) {
        for (const o of plate.obstacles) {
          const hit = chip.x < o.x + o.w && o.x < chip.x + chip.w && chip.y < o.y + o.h && o.y < chip.y + chip.h;
          assert.ok(!hit, `${arrange}: ${chip.id} at ${chip.x},${chip.y} over a control at ${o.x},${o.y}`);
        }
        for (const other of chips) {
          if (other === chip) continue;
          const hit = chip.x < other.x + other.w && other.x < chip.x + chip.w && chip.y < other.y + other.h && other.y < chip.y + chip.h;
          assert.ok(!hit, `${arrange}: ${chip.id} over ${other.id}`);
        }
      }
    }
  }
});

test('vias (§5): a small pad at its control’s middle; a chip’s legs drawn all or none; the stand-in plate’s board crosses little', () => {
  const b = boardOf('studio', 'controls');
  const vias = b.nodes.filter((n) => n.kind === 'control');
  for (const v of vias) {
    const r = v.control.rect;
    assert.ok(v.w <= 16 && v.h < r.h, 'a pad, not the control');
    assert.ok(Math.abs(v.x + v.w / 2 - (r.x + r.w / 2)) <= 1 && Math.abs(v.y + v.h / 2 - (r.y + r.h / 2)) <= 1, 'at its middle');
  }
  const legs = new Map();
  for (const w of b.wires.filter((x) => x.kind === 'binding')) {
    const chip = w.from.node.startsWith('@control:') ? w.to.node : w.from.node;
    if (!legs.has(chip)) legs.set(chip, new Set());
    legs.get(chip).add(w.labelled);
  }
  for (const [chip, kinds] of legs) assert.equal(kinds.size, 1, `${chip}: all or none`);
  // A long leg is labelled at its chip only: its via names it on the plate.
  const viaIds = new Set(vias.map((v) => v.id));
  for (const l of b.board.labels) {
    const w = b.wires.find((x) => x.id === l.wire);
    if (w.kind === 'binding') assert.ok(!viaIds.has(l.side === 'out' ? w.from.node : w.to.node), 'no label at a via');
  }
  assert.ok(b.board.metrics.crossings <= 12, `few crossings: ${b.board.metrics.crossings}`);
});

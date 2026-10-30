// 2026-10-30 (the owner, after ADR 0046 phase 1): an instance with many
// host-panel controls (the studio's synth, ~27 knobs) lays them out in a
// compact grid beside its box, not one tall column; the marquee starts
// with Shift- or ⌘-drag (Ctrl-drag off a Mac) and takes the panel's control
// nodes inside it too, added to the selection; a mixed selection drags as
// one group and gives Group into slot the controls it moves.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from './schematic-layout.js';
import { CONTROL_GAP, GRID_FROM, GRID_MAX_COLUMNS, gridColumns, panelControls } from './schematic-controls.js';
import { dragGroup, groupControls, inside, marqueeStarts, toggled, union } from './schematic-select.js';
import { instanceHit } from './schematic-hit.js';

const PARAMS = Array.from({ length: 27 }, (_, i) => `p${i}`);

const top = {
  instances: [
    { name: 'lfo', module: 'std.sine_osc', ports: [{ name: 'out', direction: 'out', shape: 'signal' }], parameters: [] },
    { name: 'synth', module: 'moog_voice', ports: [{ name: 'gate', direction: 'in', shape: 'signal' }, { name: 'out', direction: 'out', shape: 'signal' }], parameters: PARAMS.map((name) => ({ name })) },
    { name: 'echo', module: 'std.gain', ports: [{ name: 'in', direction: 'in', shape: 'signal' }], parameters: [{ name: 'db' }] },
  ],
  connections: [
    { from: 'lfo.out', to: 'synth.p0' },
    { from: 'synth.out', to: 'echo.in' },
  ],
};

/** The studio-like main panel: a card of 27 knobs on the synth, a slot card (counted by `panel.check`, not drawn), the echo's level. */
const PANEL = `<signal-panel><div class="signal-cards">
<section class="signal-card"><h2>synth</h2><div class="row">${PARAMS.map((p) => `<signal-knob bind="/synth.${p}" label="${p}"></signal-knob>`).join('')}</div></section>
<signal-slot bind="/deck"></signal-slot>
<section class="signal-card"><signal-fader bind="/echo.db" label="level"></signal-fader><signal-meter bind="/synth.out" label="out"></signal-meter></section>
</div></signal-panel>`;

const controls = panelControls(PANEL);
const drawn = layout(top, {}, { controls });
const entry = { path: '', level: '', transform: { k: 1, x: 0, y: 0 }, layout: drawn, nodes: drawn.nodes, wires: drawn.wires };
const knobs = drawn.nodes.filter((n) => n.kind === 'control' && n.host === 'synth' && n.side === 'left');
const box = drawn.nodes.find((n) => n.kind === 'instance' && n.name === 'synth');

test("many host-panel controls stand in a compact grid beside their box, in the panel's order, never overlapping", () => {
  assert.equal(knobs.length, 27);
  const columns = new Set(knobs.map((n) => n.x));
  assert.ok(columns.size >= 2 && columns.size <= GRID_MAX_COLUMNS, `${columns.size} columns`);
  const height = Math.max(...knobs.map((n) => n.y + n.h)) - Math.min(...knobs.map((n) => n.y));
  const column = knobs.reduce((s, n) => s + n.h, 0);
  assert.ok(height < column / 2, 'far shorter than one column');
  assert.ok(height <= Math.max(box.h, 200) * 1.5, 'about as tall as its box');
  // Next to the box: its nearest column CONTROL_GAP from its left edge, none over it.
  assert.equal(Math.max(...knobs.map((n) => n.x + n.w)) <= box.x - CONTROL_GAP + 1, true);
  assert.ok(Math.max(...knobs.map((n) => n.x + n.w)) >= box.x - CONTROL_GAP - 60, 'close to the box');
  // Reading order: row by row.
  for (let i = 1; i < knobs.length; i++) {
    const a = knobs[i - 1];
    const b = knobs[i];
    assert.ok(b.y > a.y || (b.y === a.y && b.x > a.x), `${b.name} after ${a.name}`);
  }
  for (let i = 0; i < knobs.length; i++) {
    for (let j = i + 1; j < knobs.length; j++) {
      const a = knobs[i];
      const b = knobs[j];
      assert.ok(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, `${a.name} and ${b.name} apart`);
    }
  }
  // Their binding wires still reach the box's pins; the lfo's wire goes into p0's knob (an automated knob).
  assert.ok(drawn.wires.some((w) => w.kind === 'binding' && w.to.node === 'synth' && w.to.port === 'p5'));
  const lfo = drawn.wires.find((w) => w.written.from === 'lfo.out');
  assert.equal(lfo.to.node, knobs[0].id);
  // No other box overlaps the grid.
  for (const other of drawn.nodes.filter((n) => n.kind === 'instance' && n.name !== 'synth')) {
    for (const k of knobs) assert.ok(other.x + other.w <= k.x || k.x + k.w <= other.x || other.y + other.h <= k.y || k.y + k.h <= other.y, `${other.name} clear of ${k.name}`);
  }
});

test('a few controls keep their column beside their pins; the grid only from GRID_FROM', () => {
  const few = layout(top, {}, { controls: panelControls('<signal-knob bind="/synth.p1"></signal-knob><signal-knob bind="/synth.p2"></signal-knob>') });
  const two = few.nodes.filter((n) => n.kind === 'control');
  assert.equal(new Set(two.map((n) => n.x)).size, 1, 'one column');
  assert.ok(GRID_FROM > 2);
  assert.equal(gridColumns(1, 48, 100), 1);
  assert.equal(gridColumns(27, 48, 34 + 16 * 30), 3);
  assert.equal(gridColumns(27, 48, 60), GRID_MAX_COLUMNS);
  assert.ok(gridColumns(6, 48, 1000) >= 2, 'never one tall column');
});

test("the main panel's controls carry their index as panel.check counts them (a slot card counts, it is no node)", () => {
  assert.equal(controls[0].check, 0);
  assert.equal(controls[26].check, 26);
  const fader = controls.find((c) => c.binds[0] === '/echo.db');
  assert.equal(fader.check, 28, 'after the 27 knobs and the slot card');
});

test('Shift- or ⌘-drag (Ctrl-drag off a Mac) on the background starts the marquee; a plain drag or one on a box does not', () => {
  const background = { kind: 'background' };
  assert.ok(marqueeStarts(background, { shiftKey: true }, true));
  assert.ok(marqueeStarts(background, { metaKey: true }, true));
  assert.ok(marqueeStarts(background, { ctrlKey: true }, false), 'Ctrl off a Mac');
  assert.ok(!marqueeStarts(background, { ctrlKey: true }, true), 'Ctrl on a Mac is the right click');
  assert.ok(!marqueeStarts(background, {}, true), 'a plain drag pans');
  assert.ok(!marqueeStarts(instanceHit(entry, box), { metaKey: true }, true), 'on a box: a ⌘-click toggles it');
});

test('the marquee takes the control nodes inside it, with the boxes; added to what is selected', () => {
  const drawnScene = { levels: [entry] };
  // Around the synth's grid only: its knobs, not the box.
  const x0 = Math.min(...knobs.map((n) => n.x)) - 1;
  const y0 = Math.min(...knobs.map((n) => n.y)) - 1;
  const x1 = Math.max(...knobs.map((n) => n.x + n.w)) + 1;
  const y1 = Math.max(...knobs.slice(0, 6).map((n) => n.y + n.h)) + 1;
  const took = inside(drawnScene, { x: x0, y: y0, w: x1 - x0, h: y1 - y0 });
  assert.ok(took.length >= 3 && took.every((h) => h.kind === 'control' && h.node.host === 'synth'), 'the knobs of the first rows');
  // The whole drawing: every box, every control, the connections between boxes.
  const b = drawn.bounds;
  const all = inside(drawnScene, { x: b.x - 1, y: b.y - 1, w: b.w + 2, h: b.h + 2 });
  assert.equal(all.filter((h) => h.kind === 'control').length, drawn.nodes.filter((n) => n.kind === 'control').length);
  assert.equal(all.filter((h) => h.kind === 'instance').length, 3);
  // Additive: a second marquee adds to the first, each part once; a ⌘-click takes one out again.
  const first = took;
  const both = union(first, all);
  assert.equal(both.length, all.length);
  const less = toggled(both, first[0]);
  assert.equal(less.length, both.length - 1);
});

test('a mixed selection: its boxes and its controls’ hosts drag together; Group into slot moves the selected controls of the box', () => {
  const synthHit = instanceHit(entry, box);
  const echo = instanceHit(entry, drawn.nodes.find((n) => n.name === 'echo'));
  const knobHits = knobs.slice(0, 3).map((n) => ({ kind: 'control', path: '', level: '', node: n, address: n.binds[0].address, addresses: [n.binds[0].address] }));
  const fader = drawn.nodes.find((n) => n.kind === 'control' && n.host === 'echo');
  const faderHit = { kind: 'control', path: '', level: '', node: fader, address: '/echo.db', addresses: ['/echo.db'] };
  // The synth with three of its knobs and the echo's fader: dragging the synth takes the echo along (the fader's host).
  const selected = [synthHit, ...knobHits, faderHit];
  assert.deepEqual(dragGroup(selected, synthHit), ['synth', 'echo']);
  // A box not selected but whose control is: a member of the group too.
  assert.deepEqual(dragGroup([faderHit, synthHit], echo), ['echo', 'synth']);
  assert.deepEqual(dragGroup([], echo), ['echo']);
  // Group into slot on the synth: the three selected knobs, by panel.check's index.
  assert.deepEqual(groupControls(selected, synthHit), [0, 1, 2]);
  // Nothing of the box's selected (only the echo's fader): every control, the op's default.
  assert.equal(groupControls([synthHit, faderHit], synthHit), null);
  assert.equal(groupControls([synthHit], synthHit), null);
  assert.equal(groupControls(selected, null), null);
});

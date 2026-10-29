// A panel's controls as nodes of the schematic (the owner's, 2026-10-29:
// "make it, as in NI Reaktor, a small square element of its own … keep
// that it has an input and an output, as an automated fader").

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  CONTROL_GAP,
  attributeValue,
  controlFraction,
  controlHit,
  controlValue,
  controlValueText,
  editableAttributes,
  findInTree,
  glyphParts,
  panelControls,
  splitBind,
} from './schematic-controls.js';
import { layout, movedWires } from './schematic-layout.js';
import { openedLayout } from './schematic-open.js';
import { hitTest } from './schematic-hit.js';
import { hitKey, reselectAll } from './schematic-select.js';
import { connectionsOf, sidebar } from './schematic-sidebar.js';
import { schematicMenu } from './schematic-menu.js';
import { startOf } from './schematic-trace.js';
import { DragFollow } from './schematic-drag.js';
import { bake, bakedMatrix, bakedPath } from './schematic-draw.js';
import { fit, toScreen } from './schematic-view.js';
import { parseContent, serialize } from './panel-tree.js';
import { setAttributes } from './panel-edit.js';
import { subjectOf } from '../app/lib/show-in.js';
import { KICK } from './schematic-scene.test.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));
const WHITELIST = JSON.parse(read('ui/lib/panel-whitelist.json'));

/** KICK's top level, its out box with a gain parameter as the kick project's. */
const KICK_PANEL = `
<signal-panel>
  <!-- <signal-knob bind="/lfo.freq_hz"></signal-knob> is a comment -->
  <div class="signal-cards">
    <section class="signal-card">
      <signal-knob bind="/drums.kick_decay" taper="log" min="100" max="1500" label="decay"></signal-knob>
      <signal-fader bind="/out.gain" taper="db" label="gain"></signal-fader>
      <signal-meter bind="/out.l" label="L"></signal-meter>
      <signal-scope bind="/drums/kick/dc.out" label="kick"></signal-scope>
      <signal-slot bind="/drums"></signal-slot>
      <signal-knob bind="/nowhere.x" label="gone"></signal-knob>
    </section>
  </div>
</signal-panel>`;

const kickLayout = (positions = {}) => layout(KICK.top, positions, { controls: panelControls(KICK_PANEL) });
const controlsOf = (drawn) => drawn.nodes.filter((n) => n.kind === 'control');
const byName = (drawn, name) => drawn.nodes.find((n) => n.name === name);

test('a panel file’s controls: in order, with what they bind, comments, slots and pattern editors left out', () => {
  const controls = panelControls(KICK_PANEL);
  assert.deepEqual(
    controls.map((c) => [c.type, c.binds, c.label]),
    [
      ['knob', ['/drums.kick_decay'], 'decay'],
      ['fader', ['/out.gain'], 'gain'],
      ['meter', ['/out.l'], 'L'],
      ['scope', ['/drums/kick/dc.out'], 'kick'],
      ['knob', ['/nowhere.x'], 'gone'],
    ],
  );
  assert.equal(controls[0].attributes.min, '100');
  // The studio's panel: every bound control but its piano roll, step grid and slot.
  const studio = panelControls(read('projects/studio/ui/main.html'));
  assert.ok(studio.length > 50);
  assert.ok(studio.every((c) => !['piano-roll', 'step-grid', 'slot'].includes(c.type)));
  // Several of the same tag and bind are told apart by `occurrence` (the panel editor's tree finds them so).
  const twice = panelControls('<signal-value bind="/a.x"></signal-value><signal-value bind="/a.x" digits="2"></signal-value>');
  assert.deepEqual(twice.map((c) => c.occurrence), [0, 1]);
  assert.deepEqual(splitBind('/drums/kick/dc.out'), { instance: 'drums/kick/dc', port: 'out', top: 'drums', nested: true });
  assert.equal(splitBind('/deck_a'), null);
});

test('each control is a node of its own beside its host: writers left, readers of an output right, nothing overlapping', () => {
  const drawn = kickLayout();
  const controls = controlsOf(drawn);
  // The control of an instance the level does not have is left out.
  assert.deepEqual(controls.map((c) => c.name), ['decay', 'gain', 'L', 'kick']);
  const drums = byName(drawn, 'drums');
  const out = byName(drawn, 'out');
  const decay = byName(drawn, 'decay');
  assert.equal(decay.host, 'drums');
  assert.equal(decay.x + decay.w + CONTROL_GAP, drums.x, 'left of its host');
  const kick = byName(drawn, 'kick');
  assert.equal(kick.x, drums.x + drums.w + CONTROL_GAP, 'a reader of a port inside it: right of it');
  assert.equal(byName(drawn, 'L').x + byName(drawn, 'L').w + CONTROL_GAP, out.x);
  // Square-ish, smaller than a module's box.
  for (const c of controls) assert.ok(c.w <= 64 && c.h <= 64 && c.h >= 40, `${c.name} ${c.w}×${c.h}`);
  const boxes = drawn.nodes;
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const [a, b] = [boxes[i], boxes[j]];
      assert.ok(!(a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h), `${a.id} overlaps ${b.id}`);
    }
  }
  // The whole studio, its 60-odd controls too.
  const studio = layout(STUDIO.top, {}, { controls: panelControls(read('projects/studio/ui/main.html')) });
  const all = studio.nodes;
  let overlaps = 0;
  for (let i = 0; i < all.length; i++) for (let j = i + 1; j < all.length; j++) if (all[i].x < all[j].x + all[j].w && all[j].x < all[i].x + all[i].w && all[i].y < all[j].y + all[j].h && all[j].y < all[i].y + all[i].h) overlaps++;
  assert.equal(overlaps, 0);
  assert.ok(controlsOf(studio).length > 50);
});

test('an automated fader: the wires into what it sets come into its input, its output sets it by a binding wire', () => {
  const drawn = kickLayout();
  const decay = byName(drawn, 'decay');
  assert.deepEqual(decay.ins.map((p) => [p.name, p.direction, p.connected]), [['drums.kick_decay', 'in', true]]);
  assert.deepEqual(decay.outs.map((p) => [p.name, p.direction, p.connected]), [['drums.kick_decay', 'out', true]]);
  // The lfo's wire, as written, drawn into the control.
  const lfo = drawn.wires.find((w) => w.written.from === '/lfo.out');
  assert.deepEqual(lfo.to, { node: decay.id, port: 'drums.kick_decay' });
  assert.deepEqual(lfo.target, { node: 'drums', port: 'kick_decay' });
  assert.deepEqual(lfo.written, { from: '/lfo.out', to: '/drums.kick_decay' });
  const binding = drawn.wires.find((w) => w.kind === 'binding' && w.from.node === decay.id);
  assert.deepEqual(binding.to, { node: 'drums', port: 'kick_decay' });
  // Its end at the control's pins.
  assert.deepEqual(lfo.curve.segs.at(-1).to, { x: decay.ins[0].x, y: decay.ins[0].y });
  assert.equal(decay.ins[0].x, decay.x);
  assert.equal(decay.outs[0].x, decay.x + decay.w);
  // The host's pin is set: connected, and the wire still listed as reaching it.
  const pin = byName(drawn, 'drums').ins.find((p) => p.name === 'kick_decay');
  assert.equal(pin.connected, true);
  const entry = { path: '', level: '', layout: drawn };
  assert.deepEqual(connectionsOf(entry, byName(drawn, 'drums'), pin).map((c) => c.wire.hit.wire.written.from), ['/lfo.out']);
  // A fader nothing else drives: its input open.
  const gain = byName(drawn, 'gain');
  assert.equal(gain.ins[0].connected, false);
});

test('a meter of an input is fed what reaches it; a control of a port inside a composite has its pin unwired', () => {
  const drawn = kickLayout();
  const meter = byName(drawn, 'L');
  assert.equal(meter.outs.length, 0, 'a reader has no output');
  const taps = drawn.wires.filter((w) => w.kind === 'binding' && w.to.node === meter.id);
  assert.deepEqual(taps.map((w) => w.from), [{ node: 'drums', port: 'out' }]);
  const kick = byName(drawn, 'kick');
  assert.equal(kick.ins[0].role, null);
  assert.equal(drawn.wires.filter((w) => w.from.node === kick.id || w.to.node === kick.id).length, 0);
  // Several addresses: a pin each, on each side a writer uses.
  const xy = layout(KICK.top, {}, { controls: panelControls('<signal-xy bind="/drums.kick_decay /out.gain"></signal-xy>') });
  const pad = controlsOf(xy)[0];
  assert.equal(pad.ins.length, 2);
  assert.equal(pad.outs.length, 2);
  assert.notEqual(pad.ins[0].y, pad.ins[1].y);
  assert.equal(xy.wires.filter((w) => w.kind === 'binding').length, 2);
});

test('a control rides with its host: a stored position, and a drag of it', () => {
  const drawn = kickLayout({ drums: { x: 600, y: 300 } });
  const drums = byName(drawn, 'drums');
  const decay = byName(drawn, 'decay');
  assert.deepEqual([drums.x, drums.y], [600, 300]);
  assert.equal(decay.x + decay.w + CONTROL_GAP, 600);
  assert.ok(decay.y >= 300);
  // A drag of drums moves its controls' wires with it.
  const routes = movedWires(drawn, new Map([['drums', { x: 640, y: 300 }], [decay.id, { x: decay.x + 40, y: decay.y }]]));
  assert.ok(routes.some((r) => r.wire.written.from === '/lfo.out'));
  assert.ok(routes.some((r) => r.wire.kind === 'binding' && r.wire.from.node === decay.id));
  // DragFollow takes the host's controls with it.
  const transform = { k: 1, x: 0, y: 0 };
  const baked = bake(transform, { w: 1000, h: 600 });
  const element = () => {
    const attributes = {};
    return { getAttribute: (n) => attributes[n] ?? null, setAttribute: (n, v) => (attributes[n] = String(v)), removeAttribute: (n) => delete attributes[n], attributes };
  };
  const group = {
    baked,
    placed: bakedMatrix(baked, transform),
    wireCache: new Map(drawn.wires.map((w) => [w.id, { el: Object.assign(element(), {}) }])),
    nodeCache: new Map(drawn.nodes.map((n) => [n.id, { el: element() }])),
  };
  for (const w of drawn.wires) group.wireCache.get(w.id).el.setAttribute('d', bakedPath(baked, w.points, w.curve));
  const follow = new DragFollow('', 'drums');
  follow.apply({ layout: drawn, levels: [{ path: '', level: '', transform }], groups: new Map([['', group]]), to: { x: 640, y: 320 } });
  assert.equal(group.nodeCache.get(decay.id).el.attributes.transform, 'translate(40 20)');
  assert.equal(group.nodeCache.get('lfo').el.attributes.transform, undefined);
});

test('a control is hit whole, selected by its place in the panel, found again; its binding wire is never hit', () => {
  const drawn = kickLayout();
  const viewport = { w: 1200, h: 800 };
  const transform = fit(drawn.bounds, viewport);
  const scene = { levels: [{ path: '', level: '', transform, nodes: drawn.nodes, wires: drawn.wires, layout: drawn }] };
  const decay = byName(drawn, 'decay');
  for (const at of [{ x: decay.x + decay.w / 2, y: decay.y + decay.h / 2 }, decay.ins[0], decay.outs[0]]) {
    const hit = hitTest(scene, toScreen(transform, at));
    assert.equal(hit.kind, 'control');
    assert.equal(hit.node.id, decay.id);
    assert.equal(hit.address, '/drums.kick_decay');
  }
  const binding = drawn.wires.find((w) => w.kind === 'binding' && w.from.node === decay.id);
  const mid = binding.points[Math.floor(binding.points.length / 2)];
  assert.notEqual(hitTest(scene, toScreen(transform, mid)).kind, 'wire');
  const hit = controlHit({ path: '', level: '' }, decay);
  assert.equal(hitKey(hit), `control  ${decay.id}`);
  assert.notEqual(hitKey(hit), hitKey(controlHit({ path: '', level: '' }, byName(drawn, 'gain'))));
  // A new graph keeps it selected; a panel without it lets it go.
  const again = reselectAll([hit], () => kickLayout());
  assert.equal(again.length, 1);
  assert.notEqual(again[0].node, decay);
  assert.equal(again[0].node.id, decay.id);
  assert.deepEqual(reselectAll([hit], () => layout(KICK.top)), []);
});

test('its sidebar: what it is, its attributes, what it binds and what drives it, the value of its parameter; its menu and Show in', () => {
  const drawn = kickLayout();
  const decay = byName(drawn, 'decay');
  const hit = controlHit({ path: '', level: '' }, decay);
  const description = { controls: [{ instance: 'drums', module: 'drums', parameter: 'kick_decay', address: '/drums.kick_decay', unit: 'ms', min: 0.1, max: 60000, default: 400, base: 420 }] };
  const model = sidebar([hit], { graph: KICK, description, layoutOf: () => drawn });
  assert.equal(model.kind, 'control');
  assert.equal(model.title, 'decay');
  assert.deepEqual(
    model.fields.map(([k]) => k),
    ['type', 'label', 'binds', 'host', 'taper', 'min', 'max'],
  );
  assert.equal(model.fields[0][1], 'Knob (<signal-knob>)');
  assert.deepEqual(model.binds.map((b) => [b.label, b.role, b.hit?.kind, b.hit?.address]), [['/drums.kick_decay', 'sets', 'port', '/drums.kick_decay']]);
  assert.deepEqual(model.connections.map((c) => c.wire.label), ['/lfo.out →']);
  assert.deepEqual(model.parameters.map((p) => [p.name, p.address, p.editable, p.fallback]), [['drums.kick_decay', '/drums.kick_decay', true, 420]]);
  assert.deepEqual(model.actions.map((a) => [a.id, a.disabled]), [['trace', false], ['bound', false], ['copy-address', false]]);
  assert.deepEqual(schematicMenu(hit, {}).filter((i) => i.id !== 'separator').map((i) => i.id), ['inspect', 'trace', 'bound', 'copy-address']);
  assert.deepEqual(startOf(hit), { kind: 'pins', addresses: ['/drums.kick_decay'], label: 'decay' });
  // Show in Panel and Parameters take its address, as its control on the panel.
  assert.deepEqual(subjectOf({ hit }), { kind: 'pin', address: '/drums.kick_decay' });
  // The value on its node, live from the stream or `describe`.
  const shown = controlValue(decay, description, new Map([['/drums.kick_decay', { base: 800 }]]));
  assert.equal(shown.value, 800);
  assert.equal(controlValueText(controlValue(decay, description)), '420.0 ms');
  assert.equal(controlFraction(decay, shown), (800 - 100) / 1400, 'in its own min and max');
  assert.equal(controlValue(byName(drawn, 'L'), description), null, 'a meter has none');
  // A wire drawn into it ends, as written, at what it binds.
  const lfo = drawn.wires.find((w) => w.written.from === '/lfo.out');
  const wire = sidebar([{ kind: 'wire', path: '', level: '', wire: lfo, from: '/lfo.out', to: '/drums.kick_decay' }], { graph: KICK, layoutOf: () => drawn });
  assert.equal(wire.links[1].hit.node.name, 'drums');
});

test('a glyph per type, in its area; unknown values drawn as such', () => {
  const area = { x: 10, y: 20, w: 36, h: 20 };
  for (const type of ['fader', 'knob', 'toggle', 'button', 'xy', 'meter', 'scope', 'canvas', 'keyboard', 'value', 'whatever']) {
    const parts = glyphParts(type, area, 0.25);
    assert.ok(parts.length >= 2, type);
    for (const part of parts) assert.ok(['rect', 'circle', 'line', 'path'].includes(part.tag));
  }
  const low = glyphParts('fader', area, 0)[1].attributes.y;
  const high = glyphParts('fader', area, 1)[1].attributes.y;
  assert.ok(high < low, 'a fader’s cap goes up');
  assert.match(glyphParts('knob', area, NaN)[1].attributes.class, /unknown/);
});

test('its properties are written as the panel editor writes the panel: the one attribute of that one control', () => {
  const content = `
  <div class="signal-cards">
    <section class="signal-card">
      <signal-knob bind="/a.x" label="one" min="0" max="1"></signal-knob>
      <signal-knob bind="/a.x" label="two"></signal-knob>
    </section>
  </div>
`;
  const [, second] = panelControls(content);
  const tree = parseContent(content, WHITELIST, { mode: 'read' }).tree;
  const found = findInTree(tree, second);
  assert.equal(found.attributes.label, 'two');
  const changed = setAttributes(tree, found.id, { label: 'second', max: attributeValue('max', ' 2.5 ').value });
  const written = panelControls(serialize(changed.tree, WHITELIST));
  assert.deepEqual(written.map((c) => [c.label, c.attributes.max ?? null]), [['one', '1'], ['second', '2.5']]);
  assert.deepEqual(attributeValue('min', 'low'), { error: 'min is a number' });
  assert.deepEqual(attributeValue('unit', ''), { value: null });
  assert.deepEqual(editableAttributes('signal-knob', WHITELIST), ['label', 'min', 'max', 'default', 'unit']);
  assert.deepEqual(editableAttributes('signal-meter', WHITELIST), ['label']);
});

test('a host opened in place: its controls go out beside its frame, the boxes around make room for theirs', () => {
  const drawn = kickLayout();
  const inner = layout(KICK.composites.drums);
  const opened = openedLayout(drawn, 'drums', inner.bounds);
  const frame = opened.nodes.find((n) => n.name === 'drums' && n.kind === 'instance');
  assert.ok(frame.grown, 'the frame grew');
  const decay = opened.nodes.find((n) => n.name === 'decay');
  const kick = opened.nodes.find((n) => n.name === 'kick' && n.kind === 'control');
  assert.ok(decay.x + decay.w < frame.x, 'left of the frame');
  assert.ok(kick.x > frame.x + frame.w, 'right of the frame');
  // The out box's controls moved with it.
  const host = (l) => l.nodes.find((n) => n.name === 'out');
  for (const name of ['gain', 'L']) {
    const before = drawn.nodes.find((n) => n.name === name);
    const after = opened.nodes.find((n) => n.name === name);
    assert.equal(after.x - before.x, host(opened).x - host(drawn).x, name);
  }
  // Nothing overlaps the frame, and the lfo's wire still ends at the control's input.
  for (const n of opened.nodes) {
    if (n === frame) continue;
    assert.ok(!(n.x < frame.x + frame.w && frame.x < n.x + n.w && n.y < frame.y + frame.h && frame.y < n.y + n.h), `${n.id} under the frame`);
  }
  const lfo = opened.wires.find((w) => w.written.from === '/lfo.out');
  assert.deepEqual(lfo.curve.segs.at(-1).to, { x: decay.x, y: decay.ins[0].y });
  // `moved` names instances only.
  assert.ok([...opened.moved.keys()].every((k) => ['lfo', 'out'].includes(k)));
});

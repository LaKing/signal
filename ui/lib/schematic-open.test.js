// A composite opened in place (M2.13-3, ADR 0030 §2): its box grown into a
// frame, the wires continuing across its edge, one open per level; opened
// and closed by the person only, never by the zoom (2026-10-22).

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { HEADER, PAD, ROW, layout } from './schematic-layout.js';
import {
  FRAME_K, GROW_MAX, READ_K, aroundReason, chooseOpen, makeRoom, dragAction, drawOrder, editLevel, editable, grownNode, linkScale, openFrameHit, openedLayout, pinOne, unpin,
} from './schematic-open.js';
import { PatchSession } from './schematic-edit.js';
import { SchematicEditor } from '../controls/schematic-editor.js';
import { boundaryLinks, breadcrumbs, chain, inside, levelRect, scene } from './schematic-scene.js';
import { hitTest } from './schematic-hit.js';
import { fit, focus, pan, toScreen, zoomAt } from './schematic-view.js';
import { exportFacts } from './schematic-groups.js';
import { KICK } from './schematic-scene.test.js';
import { estimateWidth } from './schematic-labels.js';
import { crosses, curveBox, flatten } from './schematic-wire.js';
import { bundleWires } from './schematic-bundle.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
const viewport = { w: 1000, h: 600 };
const centre = { x: viewport.w / 2, y: viewport.h / 2 };

function layoutsOf(graph) {
  const layouts = new Map();
  const factsOf = (module) => exportFacts(graph.composites?.[module] ?? null);
  return (level) => {
    if (!layouts.has(level)) {
      const source = level === '' ? graph.top : graph.composites?.[level];
      layouts.set(level, source ? layout(source, graph.layout?.positions?.[level] ?? {}, { factsOf }) : null);
    }
    return layouts.get(level);
  };
}
const kickOf = layoutsOf(KICK);
const studioOf = layoutsOf(STUDIO);
const nodeOf = (layoutOf, level, name) => layoutOf(level).nodes.find((n) => n.name === name);
const near = (a, b, message) => assert.ok(Math.abs(a.x - b.x) < 1e-9 && Math.abs(a.y - b.y) < 1e-9, `${message}: ${a.x},${a.y} vs ${b.x},${b.y}`);
/** A point of an inside (`t`: inside → parent) in its parent's units. */
const out = (t, p) => ({ x: p.x * t.k + t.x, y: p.y * t.k + t.y });
/** The scale of a level on screen at which `node`'s inside (of layout `bounds`), open, is drawn at READ_K. */
const readableAt = (node, bounds) => READ_K / linkScale(node, bounds);
const SYNTH = new Set(['/synth']);

test('a box grows into a frame where it is: its inside at FRAME_K, its pins on its edges in order', () => {
  const box = nodeOf(kickOf, '', 'drums');
  const bounds = kickOf('drums').bounds;
  const frame = grownNode(box, bounds);
  assert.ok(frame.grown && frame !== box);
  assert.ok(frame.x <= box.x && frame.y <= box.y && frame.x + frame.w >= box.x + box.w && frame.y + frame.h >= box.y + box.h, 'it holds the box');
  const natural = inside(box, bounds).k;
  assert.ok(natural < FRAME_K && natural * GROW_MAX > FRAME_K, `drums holds its inside at ${natural}`);
  assert.ok(Math.abs(linkScale(box, bounds) - FRAME_K) < 1e-12, `${linkScale(box, bounds)}`);
  // The inside is centred on the box, as far as the frame still covers it
  // (the header takes HEADER − PAD more at the top than the bottom pad).
  const middleOf = (frame, bounds) => out(inside(frame, bounds), { x: bounds.x + bounds.w / 2, y: bounds.y + bounds.h / 2 });
  const middle = middleOf(frame, bounds);
  assert.ok(Math.abs(middle.x - (box.x + box.w / 2)) < 1e-9 && Math.abs(middle.y - (box.y + box.h / 2)) <= (HEADER - PAD) / 2 + 1e-9, `${middle.y}`);
  const tall = { x: 0, y: 0, w: 2000, h: 4000 };
  near(middleOf(grownNode({ ...box }, tall), tall), { x: box.x + box.w / 2, y: box.y + box.h / 2 }, 'a tall frame’s inside centred');
  for (const [side, x] of [['ins', frame.x], ['outs', frame.x + frame.w]]) {
    assert.deepEqual(frame[side].map((p) => p.name), box[side].map((p) => p.name), side);
    frame[side].forEach((p, i) => {
      assert.equal(p.x, x);
      if (i) assert.ok(p.y - frame[side][i - 1].y >= ROW - 1e-9, `${side} ${p.name}: a row apart at least`);
    });
  }
  assert.equal(grownNode(frame, bounds), frame, 'a frame does not grow again');
  assert.equal(grownNode(box, bounds), frame, 'the same frame each time');
});

test('the studio’s synth opens with its inside as readable as Enter draws it, its frame at most GROW_MAX times its box', () => {
  const synth = nodeOf(studioOf, '', 'synth');
  const bounds = studioOf('moog_voice').bounds;
  const natural = inside(synth, bounds).k;
  const k = linkScale(synth, bounds);
  assert.ok(Math.abs(k - natural * GROW_MAX) < 1e-12, `capped: ${k} = ${natural} × ${GROW_MAX}`);
  assert.ok(Math.abs(k - 0.363) < 0.001, `measured 0.363: ${k}`);
  // Opened (pinned) at the scale Enter draws it at, its inside is drawn at READ_K.
  const view = { k: readableAt(synth, bounds), ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  const drawn = scene({ layoutOf: studioOf, view, viewport, pinned: SYNTH });
  assert.deepEqual(drawn.levels.map((l) => l.path), ['', '/synth']);
  assert.ok(Math.abs(drawn.levels[1].transform.k - READ_K) < 1e-9, `inside at ${drawn.levels[1].transform.k}`);
  // The surrounding level is still drawn around it (its boxes moved out of the frame's way: off this small stage, on a larger one).
  assert.ok(!drawn.levels[0].behind);
  const wide = scene({ layoutOf: studioOf, view: { ...view, x: 1000, y: 600 }, viewport: { w: 2000, h: 1200 }, pinned: SYNTH });
  assert.ok(wide.levels[0].nodes.length > 1, `${wide.levels[0].nodes.map((n) => n.name)}`);
});

test('a small inside is not grown, a huge one at most GROW_MAX times', () => {
  const box = { id: 'c', name: 'c', kind: 'instance', composite: true, module: 'm', x: 0, y: 0, w: 400, h: 300, ins: [], outs: [] };
  assert.equal(grownNode(box, { x: 0, y: 0, w: 100, h: 50 }), box, 'it fits its box larger than FRAME_K');
  const huge = { x: 0, y: 0, w: 1e6, h: 1e6 };
  assert.ok(Math.abs(linkScale(box, huge) / inside(box, huge).k - GROW_MAX) < 1e-9);
});

// Wire continuation across the edge (AC2): the outer wire ends at the grown
// box's pin, the link starts there and ends at the frame's pin, where the
// inner wire starts; and again one level further in.
test('outer wires continue across the frame edge to the inner wires, nested too', () => {
  const top = kickOf('');
  const drumsInside = kickOf('drums');
  const opened = openedLayout(top, 'drums', drumsInside.bounds);
  const frame = opened.nodes.find((n) => n.name === 'drums');
  assert.equal(frame, grownNode(nodeOf(kickOf, '', 'drums'), drumsInside.bounds));
  const t = inside(frame, drumsInside.bounds);
  const links = boundaryLinks(frame, drumsInside);
  const check = (parent, frameNode, inner, t, links, wireTo) => {
    let crossed = 0;
    for (const wire of parent.wires) {
      const into = wire.to.node === frameNode.id;
      const from = wire.from.node === frameNode.id;
      if (!into && !from) continue;
      crossed++;
      assert.ok(wire.crossing, wire.label);
      const end = into ? wire.curve.segs.at(-1).to : wire.curve.start;
      const port = into ? wire.to.port : wire.from.port;
      const pin = (into ? frameNode.ins : frameNode.outs).find((p) => p.name === port);
      near(end, pin, `${wire.label}: the outer wire reaches the frame's pin`);
      const link = links.find((l) => l.name === port);
      near(out(t, into ? link.from : link.to), pin, `${wire.label}: the link starts there`);
      // The link's other end is the frame's pin inside, where the inner wire starts (or ends).
      const other = into ? link.to : link.from;
      const innerWire = inner.wires.find((w) => (into ? w.from.node === '@inputs' && w.from.port === port : w.to.node === '@outputs' && w.to.port === port));
      if (innerWire) near(into ? innerWire.curve.start : innerWire.curve.segs.at(-1).to, other, `${wire.label}: the inner wire continues it`);
      wireTo?.(wire, into);
    }
    return crossed;
  };
  assert.equal(check(opened, frame, drumsInside, t, links), 2, 'lfo → kick_decay and out → out.l cross drums’ edge');
  // Nested: kick open inside drums.
  const kickInside = kickOf('kick');
  const drumsOpened = openedLayout(drumsInside, 'kick', kickInside.bounds);
  const kickFrame = drumsOpened.nodes.find((n) => n.name === 'kick');
  const tk = inside(kickFrame, kickInside.bounds);
  assert.equal(check(drumsOpened, kickFrame, kickInside, tk, boundaryLinks(kickFrame, kickInside)), 3, 'trigger, decay and out cross kick’s edge');
  // In the scene the two links compose: drums' inside is drawn with kick open.
  const drawn = scene({ layoutOf: kickOf, view: { k: readableAt(kickFrame, kickInside.bounds) / t.k, ox: out(t, { x: kickFrame.x + kickFrame.w / 2, y: kickFrame.y + kickFrame.h / 2 }).x, oy: out(t, { x: kickFrame.x + kickFrame.w / 2, y: kickFrame.y + kickFrame.h / 2 }).y, x: centre.x, y: centre.y }, viewport, pinned: new Set(['/drums', '/drums/kick']) });
  assert.deepEqual(drawn.levels.map((l) => l.path), ['', '/drums', '/drums/kick']);
  assert.equal(drawn.levels[1].layout, drumsOpened, 'drums is drawn with kick grown');
});

test('opening changes no stored position: every other box is the same, or moved out of the way as `moved` says', () => {
  const top = kickOf('');
  const before = JSON.stringify(top);
  const opened = openedLayout(top, 'drums', kickOf('drums').bounds);
  assert.equal(JSON.stringify(top), before, 'the stored layout is untouched');
  assert.deepEqual(opened.bounds, top.bounds);
  assert.equal(opened.opened, 'drums');
  for (const [i, n] of top.nodes.entries()) {
    if (n.name === 'drums') continue;
    const m = opened.moved.get(n.name);
    if (!m) assert.equal(opened.nodes[i], n, n.name);
    else assert.deepEqual([opened.nodes[i].x, opened.nodes[i].y], [n.x + m.dx, n.y + m.dy], n.name);
  }
  for (const [i, w] of top.wires.entries()) {
    const moved = opened.moved.has(w.from.node) || opened.moved.has(w.to.node) || w.from.node === 'drums' || w.to.node === 'drums';
    if (!moved && !w.feedback) assert.equal(opened.wires[i], w, w.label);
  }
  assert.equal(openedLayout(top, 'drums', kickOf('drums').bounds), opened, 'cached');
  assert.equal(openedLayout(top, 'nope', kickOf('drums').bounds), top);
});

/** A level of two composites side by side (each inside two boxes), and its insides. */
function pair() {
  const box = (name, x, module = 'inner') => ({ name, module, composite: true, ports: [], parameters: [] });
  const top = layout({ instances: [box('a', 0), box('b', 0), { name: 'lib', module: 'lib.voice', composite: true, ports: [], parameters: [] }], connections: [] });
  const inner = layout({ instances: [{ name: 'x', module: 'std.x', composite: false, ports: [], parameters: [] }, { name: 'y', module: 'std.y', composite: false, ports: [], parameters: [] }], connections: [] });
  return { top, inner, layoutOf: (level) => (level === '' ? top : level === 'inner' ? inner : null) };
}

test('only a pinned composite opens, at any scale: never one the view would pick (2026-10-22)', () => {
  const { top, layoutOf } = pair();
  const pick = (pinned) => chooseOpen({ path: '', layout: top, layoutOf, pinned })?.node.name ?? null;
  assert.equal(pick(new Set()), null, 'nothing pinned: nothing open, however near or large');
  assert.equal(pick(new Set(['/a'])), 'a');
  assert.equal(pick(new Set(['/b', '/b/x'])), 'b');
  assert.equal(pick(new Set(['/c'])), null, 'no such instance');
  // AC6: a library composite (its inside not in `graph`) never opens, not even pinned.
  assert.equal(pick(new Set(['/lib'])), null);
  // A scene, zoomed far into a, opens nothing that is not pinned.
  const a = top.nodes.find((n) => n.name === 'a');
  for (const k of [0.01, 1, 100, 1e6]) {
    const view = { k, ox: a.x + a.w / 2, oy: a.y + a.h / 2, x: centre.x, y: centre.y };
    assert.deepEqual([...scene({ layoutOf, view, viewport }).open], [], `at ${k}: closed`);
    assert.deepEqual([...scene({ layoutOf, view, viewport, pinned: new Set(['/a']) }).open], ['/a'], `at ${k}: pinned, open`);
  }
});

// Slice 2: the person's navigation and the level edited.

/** The studio's synth opened (pinned, as a click opens it) and drawn readable about its centre, at a 1000 px stage: its scene. */
function synthOpen(factor = 1.2) {
  const synth = nodeOf(studioOf, '', 'synth');
  const view = { k: readableAt(synth, studioOf('moog_voice').bounds) * factor, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  return scene({ layoutOf: studioOf, view, viewport, pinned: SYNTH });
}
/** The studio's top level fitted, everything closed (Fit). */
const fitted = () => scene({ layoutOf: studioOf, view: fit(studioOf('').bounds, viewport), viewport });


test('a zoom never opens nor closes: Enter, then steps in and out, keeps it open; unpinned it stays closed (2026-10-22)', () => {
  const pinned = pinOne(new Set(), '/synth');
  let drawn = scene({ layoutOf: studioOf, view: synthOpen().view, viewport, pinned });
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.path), ['', '/synth']);
  // Far out (its inside a few hundredths of a pixel per unit) and far in: open throughout.
  for (const factor of [...Array(20).fill(0.7), ...Array(40).fill(1.3)]) {
    drawn = scene({ layoutOf: studioOf, view: zoomAt(drawn.view, factor, centre), viewport, pinned });
    assert.ok(drawn.open.has('/synth'), `open at ${drawn.view.k}`);
  }
  // Closed, the same zooms open nothing, about the synth itself.
  const synth = nodeOf(studioOf, '', 'synth');
  let view = { k: 0.05, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  for (let i = 0; i < 40; i++) {
    view = zoomAt(view, 1.3, centre);
    const closed = scene({ layoutOf: studioOf, view, viewport });
    assert.deepEqual([...closed.open], [], `closed at ${view.k}`);
    view = closed.view;
  }
});

test('a pan never opens nor closes (2026-10-22)', () => {
  const drawn = synthOpen();
  for (const [dx, dy] of [[5000, 0], [0, -5000], [-1e5, 1e5]]) {
    assert.deepEqual([...scene({ layoutOf: studioOf, view: pan(drawn.view, dx, dy), viewport, pinned: SYNTH }).open], ['/synth'], `panned ${dx},${dy}: open`);
    assert.deepEqual([...scene({ layoutOf: studioOf, view: pan(drawn.view, dx, dy), viewport }).open], [], `panned ${dx},${dy} unpinned: closed`);
  }
});

test('one pin per level, the frame drawn last, editing at the innermost open level only (AC3)', () => {
  assert.deepEqual([...pinOne(new Set(['/a', '/a/x', '/b/y']), '/b')].sort(), ['/b', '/b/y']);
  assert.deepEqual([...pinOne(new Set(['/a', '/a/x']), '/a/z')].sort(), ['/a', '/a/z']);
  const drawn = synthOpen();
  const top = drawn.levels[0];
  const order = drawOrder(top.nodes, top.layout.opened);
  assert.equal(order.at(-1).name, 'synth');
  assert.deepEqual(order.slice(0, -1), top.nodes.filter((n) => n.name !== 'synth'));
  assert.equal(drawOrder(top.nodes, undefined), top.nodes);
  assert.deepEqual(editLevel(drawn), { path: '/synth', level: 'moog_voice' });
  // The open frame (its box, of the top level) is not edited: a press on it starts no drag, so a
  // drop writes no position (auditor-2-drag-grown-frame); what is inside it is edited.
  const frame = top.layout.nodes.find((n) => n.name === 'synth');
  const onFrame = hitTest(drawn, toScreen(top.transform, { x: frame.x + frame.w / 2, y: frame.y + 4 }));
  assert.deepEqual([onFrame.kind, onFrame.address], ['instance', '/synth']);
  assert.equal(editable(drawn, onFrame), false);
  const neighbour = top.nodes.find((n) => n.name !== 'synth' && n.kind === 'instance');
  assert.equal(editable(drawn, { kind: 'instance', path: '', node: neighbour }), false, 'nor its neighbours');
  const inner = drawn.levels[1];
  const box = inner.nodes.find((n) => n.kind === 'instance');
  const within = hitTest(drawn, toScreen(inner.transform, { x: box.x + box.w / 2, y: box.y + box.h / 2 }));
  assert.equal(within.path, '/synth');
  assert.equal(editable(drawn, within), true);
  const closed = fitted();
  assert.equal(editable(closed, onFrame), true, 'nothing open: the top level is edited');
});

// 2026-10-29, the owner: "on a composite you can't move the view".
test('a drag pans on empty space, an open frame’s body included; moves a box of the level edited; holds a box around it only in Edit patch', () => {
  const drawn = synthOpen();
  const [top, inner] = drawn.levels;
  const frame = top.layout.nodes.find((n) => n.name === 'synth');
  // The open frame's empty inside hits as the frame instance (its level's), which is not edited, yet is empty space.
  const empty = { x: frame.x + 2, y: frame.y + frame.h - 2 };
  const onEmpty = hitTest(drawn, toScreen(top.transform, empty));
  assert.deepEqual([onEmpty.kind, onEmpty.address], ['instance', '/synth']);
  assert.equal(openFrameHit(drawn, onEmpty), true);
  // A box beside the frame, at the top level (around the level edited).
  const onNeighbour = { kind: 'instance', path: '', level: top.level, node: { kind: 'instance', name: 'beside' }, address: '/beside' };
  assert.equal(openFrameHit(drawn, onNeighbour), false);
  assert.equal(editable(drawn, onNeighbour), false);
  const box = inner.nodes.find((n) => n.kind === 'instance');
  const onBox = hitTest(drawn, toScreen(inner.transform, { x: box.x + box.w / 2, y: box.y + box.h / 2 }));
  assert.equal(openFrameHit(drawn, onBox), false);
  const press = (hit, editing, extra = {}) => dragAction({ hit, own: editable(drawn, hit), movable: editable(drawn, hit), frame: openFrameHit(drawn, hit), editing, ...extra });
  for (const editing of [false, true]) {
    assert.equal(press({ kind: 'background', path: '', level: '' }, editing), 'pan', 'the background');
    assert.equal(press(onEmpty, editing), 'pan', 'an open frame’s body');
    assert.equal(press(onBox, editing), 'move', 'a box of the level edited');
    assert.equal(press({ kind: 'wire', path: '/synth' }, editing), 'pan', 'a wire');
  }
  assert.equal(press(onNeighbour, false), 'pan', 'a box around, outside Edit patch');
  assert.equal(press(onNeighbour, true), 'hold', 'a box around, in Edit patch: not moved, said so');
  assert.equal(press(onNeighbour, true, { additive: true }), 'pan');
  assert.equal(press(onBox, false, { movable: false }), 'pan', 'a box of the level edited not drawn as a group');
  assert.equal(press(onBox, true, { movable: false }), 'pan');
  // A closed composite's box at the top level, nothing open: moved.
  const closed = fitted();
  const synthBox = closed.levels[0].nodes.find((n) => n.name === 'synth');
  const onSynth = hitTest(closed, toScreen(closed.levels[0].transform, { x: synthBox.x + synthBox.w / 2, y: synthBox.y + synthBox.h / 2 }));
  assert.equal(onSynth.address, '/synth');
  assert.equal(openFrameHit(closed, onSynth), false);
  assert.equal(dragAction({ hit: onSynth, own: true, movable: true, frame: false, editing: false }), 'move');
});

// auditor-1-editing-level-ungated (round 2): undo and redo, and a drag whose level stops being the innermost.
test('undo and redo of another level are refused while a composite is open; the entry stays', async () => {
  const drawn = synthOpen();
  const said = [];
  const sent = [];
  const editor = Object.create(SchematicEditor.prototype);
  const session = new PatchSession();
  session.setEditing(true);
  session.graphSeen({ projectId: 'p', files: { '': 'base-top', moog_voice: 'base-voice' } });
  session.history.push({ level: '', forward: [{ op: 'x' }], backward: [{ op: 'y' }] });
  Object.assign(editor, { session, updateButtons: () => {}, notice: { style: {} }, s: { drawn, say: (m) => said.push(m), connection: { request: (m) => (sent.push(m), Promise.resolve({ base: 'b2', inverse: [] })) } } });
  assert.equal(await editor.write(-1), false);
  assert.deepEqual(sent, []);
  assert.match(said.at(-1), /Not undone/);
  assert.ok(session.history.canUndo, 'kept for when the top level is the innermost again');
  assert.equal(await editor.write(0, '', [{ op: 'z' }]), false, 'a new patch of the top level too');
  assert.equal(sent.length, 0);
  // At the innermost level a patch is written; with nothing open, the undo is.
  assert.equal(await editor.write(0, 'moog_voice', [{ op: 'z' }]), true);
  assert.equal(sent.length, 1);
  editor.s.drawn = fitted();
  session.history.at = 1;
  session.history.entries = [{ level: '', forward: [], backward: [{ op: 'y' }] }];
  assert.equal(await editor.write(-1), true);
  assert.equal(sent.length, 2);
});

test('a drag begun at the top level is not editable once the synth opened (what pointerUp checks before a drop)', () => {
  const closed = fitted();
  const melody = closed.levels[0].layout.nodes.find((n) => n.name === 'melody');
  const press = { kind: 'instance', path: '', level: '', node: melody, address: '/melody' };
  assert.equal(editable(closed, press), true, 'pressed: a drag starts');
  assert.equal(editable(synthOpen(), press), false, 'released after the synth opened: no move written');
});

// The operator's review of M2.13-3 (2026-10-20): room made for the frame,
// the wires across its edge in their direction, the pointer's point kept
// (a click's since 2026-10-22), outer edits off.

const overlap = (a, b, gap = 0) => a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap;

test('the boxes around an open composite are moved out of its frame, each side in its order (makeRoom)', () => {
  const top = studioOf('');
  const synth = nodeOf(studioOf, '', 'synth');
  const bounds = studioOf('moog_voice').bounds;
  const opened = openedLayout(top, 'synth', bounds);
  const frame = opened.nodes.find((n) => n.name === 'synth');
  assert.ok(frame.grown && frame.w > synth.w * 4, `${frame.w}`);
  const before = new Map(top.nodes.map((n) => [n.name, n]));
  const after = opened.nodes.filter((n) => n.kind === 'instance' && n.name !== 'synth');
  // The stored layout grown in place covered melody and echo.
  assert.ok(overlap(grownNode(synth, bounds), before.get('melody')) && overlap(grownNode(synth, bounds), before.get('echo')));
  for (const n of after) {
    assert.ok(!overlap(n, frame), `${n.name} clear of the frame`);
    const was = before.get(n.name);
    if (was.x + was.w <= synth.x) assert.ok(n.x + n.w <= frame.x, `${n.name} stays left of it`);
    if (was.x >= synth.x + synth.w) assert.ok(n.x >= frame.x + frame.w, `${n.name} stays right of it`);
    if (was.y >= synth.y + synth.h && was.x < synth.x + synth.w && was.x + was.w > synth.x) assert.ok(n.y >= frame.y + frame.h, `${n.name} stays below it`);
  }
  // Room for the frame's pin names: melody ends well left of its left edge.
  const melody = after.find((n) => n.name === 'melody');
  assert.ok(frame.x - (melody.x + melody.w) >= 'osc1_octave'.length * 6, `${frame.x - (melody.x + melody.w)}`);
  // No two boxes overlap, and the order along each axis is kept.
  for (const [i, a] of after.entries()) {
    for (const b of after.slice(i + 1)) {
      assert.ok(!overlap(a, b), `${a.name} and ${b.name}`);
      const [pa, pb] = [before.get(a.name), before.get(b.name)];
      if (pa.x + pa.w <= pb.x) assert.ok(a.x + a.w <= b.x, `${a.name} left of ${b.name}`);
      if (pa.y + pa.h <= pb.y) assert.ok(a.y + a.h <= b.y, `${a.name} above ${b.name}`);
    }
  }
  assert.ok(opened.moved.has('melody') && opened.moved.has('echo') && !opened.moved.has('synth'));
  assert.equal(JSON.stringify(studioOf('')), JSON.stringify(top), 'the stored layout is untouched');
});

test('makeRoom moves nothing out of the way of a frame that covers nothing', () => {
  const box = (name, x, y) => ({ id: name, name, kind: 'instance', x, y, w: 100, h: 100, ins: [], outs: [] });
  const c = box('c', 500, 500);
  const far = [box('l', 0, 500), box('r', 1000, 500), box('u', 500, 0), box('d', 500, 1000), c];
  assert.equal(makeRoom(far, c, { ...c, x: 450, y: 450, w: 200, h: 200 }, { left: 10, right: 10 }).size, 0);
  // A corner box in the frame's rows moves sideways (only as far as the frame's left edge); none ends in the frame.
  const corner = box('ul', 300, 300);
  const all = [...far, corner];
  const moves = makeRoom(all, c, { ...c, x: 200, y: 200, w: 700, h: 700 }, { left: 10, right: 10 });
  const at = (n) => ({ ...n, x: n.x + (moves.get(n)?.dx ?? 0), y: n.y + (moves.get(n)?.dy ?? 0) });
  for (const n of all) if (n !== c) assert.ok(!overlap(at(n), { x: 200, y: 200, w: 700, h: 700 }), n.name);
  assert.ok(at(corner).x + corner.w <= 190 && at(far[0]).x + 100 <= at(corner).x, 'left of the frame, and still right of `l`');
  assert.ok(moves.get(far[0]).dx < 0 && !moves.get(far[0]).dy, 'left: leftward only');
  assert.ok(!moves.has(far[3]), 'below, clear of it already: not moved');
  const taller = makeRoom(all, c, { ...c, x: 200, y: 200, w: 700, h: 850 }, { left: 10, right: 10 });
  assert.ok(taller.get(far[3]).dy === 1050 + 24 - 1000 && !taller.get(far[3]).dx, 'below: down only, GAP_Y under the frame');
});

test('a wire across the frame’s edge runs in the direction it ran to the box: melody → synth into its left pins, synth → echo on to the right', () => {
  const top = studioOf('');
  const opened = openedLayout(top, 'synth', studioOf('moog_voice').bounds);
  const crossing = opened.wires.filter((w) => w.crossing);
  assert.deepEqual(crossing.map((w) => `${w.from.node}.${w.from.port}>${w.to.node}.${w.to.port}`).sort(), ['melody.gate>synth.gate', 'melody.hz>synth.hz', 'melody.velocity>synth.velocity', 'synth.out>echo.in']);
  for (const w of crossing) {
    const was = top.wires.find((v) => v.from.node === w.from.node && v.from.port === w.from.port && v.to.node === w.to.node && v.to.port === w.to.port);
    assert.equal(w.feedback, Boolean(was.feedback), `${w.from.node}.${w.from.port}: no loop because the frame grew`);
    assert.ok(w.curve.segs.at(-1).to.x > w.curve.start.x, `${w.from.node}.${w.from.port} runs left to right`);
    assert.ok(w.box.y1 <= opened.bottom, 'not routed below the level');
  }
  // No wire of the level became a loop because of the frame.
  assert.equal(opened.wires.filter((w) => w.feedback).length, top.wires.filter((w) => w.feedback).length);
});

test('around the open composite, edits are off with the reason (aroundReason)', () => {
  const drawn = synthOpen();
  assert.equal(aroundReason(drawn, { path: '/synth' }), null);
  assert.match(aroundReason(drawn, { path: '' }), /^the top level is around the open \/synth: close it \(×\)/);
  assert.equal(aroundReason(scene({ layoutOf: studioOf, view: { k: 0.1, x: 0, y: 0 }, viewport }), { path: '' }), null, 'nothing open: the top level is edited');
});

// 2026-10-22, the owner: "it would be better if it opened on a click, and
// there were an × in the top-right corner to close it".

test('a click opens by pinning: one per level, a sibling closing with what was open inside it (pinOne); unpin closes with what is inside', () => {
  assert.deepEqual([...pinOne(new Set(['/drums', '/drums/kick']), '/other')].sort(), ['/other']);
  assert.deepEqual([...pinOne(new Set(['/drums']), '/drums/kick')].sort(), ['/drums', '/drums/kick'], 'inside the open one: both open');
  assert.deepEqual([...unpin(new Set(['/drums', '/drums/kick', '/drumsx']), '/drums')], ['/drumsx'], 'what is inside goes, a name sharing its prefix stays');
  assert.deepEqual([...unpin(new Set(['/drums', '/drums/kick']), '/drums/kick')], ['/drums']);
  const same = new Set(['/a']);
  assert.equal(unpin(same, '/b'), same, 'nothing to close: the same set');
  const drawn = scene({ layoutOf: kickOf, view: fit(kickOf('').bounds, viewport), viewport, pinned: pinOne(new Set(), '/drums') });
  assert.deepEqual([...drawn.open], ['/drums']);
});

// 2026-10-22, the operator's review: "the close × is about 12 px at fit zoom".
// 2026-10-22 (the operator's review): the drums' wires ran under an open
// voice's frame corner (the studio's synth or kick open). What passes the
// frame goes around it, wire by wire and bundled alike.
test('no wire passes under an open frame: the drums’ wires go around the synth’s, the kick’s and every voice’s', () => {
  const top = studioOf('');
  const ids = new Map(top.nodes.map((n) => [n.id, n.name]));
  for (const name of ['synth', 'kick', 'snare', 'hats', 'echo']) {
    const module = nodeOf(studioOf, '', name).module;
    const opened = openedLayout(top, name, studioOf(module).bounds);
    const frame = opened.nodes.find((n) => n.name === name);
    const rect = { x: frame.x, y: frame.y, w: frame.w, h: frame.h };
    assert.deepEqual(opened.avoid.rect, rect);
    // A wire whose boxes stay where they were and that passes clear of the frame is the stored one (in the studio every box moves for frames this large; `opening changes no stored position` checks such wires on KICK).
    top.wires.forEach((w, i) => {
      if (w.feedback || [w.from.node, w.to.node].some((id) => ids.get(id) === name || opened.moved.has(ids.get(id))) || crosses(w.curve, rect)) return;
      assert.equal(opened.wires[i], w, w.label);
    });
    for (const w of opened.wires) {
      if (w.crossing || w.feedback) continue;
      assert.ok(!crosses(w.curve, rect), `${name} open: ${w.label} under its frame`);
      // Its geometry is its curve's.
      assert.deepEqual(w.box, curveBox(w.curve), w.label);
    }
    // Bundled at small scale (and fanned half-way), the same.
    for (const k of [0.1, 0.3]) {
      for (const item of bundleWires(opened.wires, k, { bottom: opened.bottom, avoid: opened.avoid })) {
        if (item.kind !== 'bundle' || item.feedback || item.wires.some((w) => w.crossing)) continue;
        assert.ok(!crosses(item.curve, rect), `${name} open, k ${k}: bundle ${item.key} under its frame`);
        for (const m of item.members) assert.ok(!crosses(m.curve, rect), `${name} open, k ${k}: ${m.wire.label}`);
      }
    }
  }
  // The synth open: the drums (left of it, moved aside) reach the voices below it along its left lane, between the boxes and its pins' names.
  const opened = openedLayout(top, 'synth', studioOf('moog_voice').bounds);
  const frame = opened.nodes.find((n) => n.name === 'synth');
  const drums = opened.nodes.find((n) => n.name === 'drums');
  const t2 = opened.wires.find((w) => w.written.from === '/drums.t2' && w.written.to === '/snare.trigger');
  const xs = flatten(t2.curve).map((p) => p.x);
  assert.ok(Math.min(...xs) >= drums.x + drums.w - 1e-9 && Math.min(...xs) < frame.x, 'down the left lane, right of the drums');
  const lane = opened.avoid.lanes.left;
  assert.ok(lane > drums.x + drums.w && lane < frame.x, 'the lane between the drums and the frame');
  assert.ok(flatten(t2.curve).some((p) => p.x === lane && p.y > frame.y && p.y < frame.y + frame.h), 'along it beside the frame');
});

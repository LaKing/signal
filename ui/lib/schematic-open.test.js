// A composite opened in place (M2.13-3, ADR 0030 §2): its box grown into a
// frame, the wires continuing across its edge, one open per level.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { HEADER, PAD, ROW, layout } from './schematic-layout.js';
import {
  CLOSE_K, FRAME_K, GROW_MAX, OPEN_K, READ_K, aroundReason, chooseOpen, makeRoom, closedFit, closedScale, crumbView, drawOrder, editLevel, editable, enterView, grownNode, linkScale, openedLayout, opensAt, pinOne, unpinned, zoomSince,
} from './schematic-open.js';
import { PatchSession } from './schematic-edit.js';
import { SchematicEditor } from '../controls/schematic-editor.js';
import { boundaryLinks, breadcrumbs, inside, levelRect, openAnchor, scene } from './schematic-scene.js';
import { hitTest } from './schematic-hit.js';
import { fit, focus, pan, toScreen, wheelFactor, zoomAt } from './schematic-view.js';
import { exportFacts } from './schematic-groups.js';
import { KICK } from './schematic-scene.test.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
const viewport = { w: 1000, h: 600 };
const screen = { x: 0, y: 0, w: viewport.w, h: viewport.h };
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
  // Zoomed to its threshold about its centre, it opens, its inside drawn at OPEN_K.
  const at = opensAt(synth, bounds) * 1.001;
  const view = { k: at, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  const drawn = scene({ layoutOf: studioOf, view, viewport });
  assert.deepEqual(drawn.levels.map((l) => l.path), ['', '/synth']);
  assert.ok(drawn.levels[1].transform.k >= OPEN_K && drawn.levels[1].transform.k < OPEN_K * 1.01, `inside at ${drawn.levels[1].transform.k}`);
  // The surrounding level is still drawn around it (its boxes moved out of the frame's way: off this small stage, on a larger one).
  assert.ok(!drawn.levels[0].behind);
  const wide = scene({ layoutOf: studioOf, view: { ...view, x: 1000, y: 600 }, viewport: { w: 2000, h: 1200 } });
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
  const drawn = scene({ layoutOf: kickOf, view: { k: opensAt(kickFrame, kickInside.bounds) / t.k * 1.01, ox: out(t, { x: kickFrame.x + kickFrame.w / 2, y: kickFrame.y + kickFrame.h / 2 }).x, oy: out(t, { x: kickFrame.x + kickFrame.w / 2, y: kickFrame.y + kickFrame.h / 2 }).y, x: centre.x, y: centre.y }, viewport, open: new Set(['/drums']) });
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

test('one composite open per level: pinned, else held, else under the zoom point, else nearest', () => {
  const { top, inner, layoutOf } = pair();
  const a = top.nodes.find((n) => n.name === 'a');
  const b = top.nodes.find((n) => n.name === 'b');
  const k = opensAt(a, inner.bounds) * 1.2;
  const transform = { k, x: 0, y: 0 };
  const pick = (options) => chooseOpen({ path: '', layout: top, transform, screen: { x: -1e9, y: -1e9, w: 2e9, h: 2e9 }, layoutOf, ...options })?.node.name ?? null;
  const middle = (n) => ({ x: (n.x + n.w / 2) * k, y: (n.y + n.h / 2) * k });
  assert.equal(pick({ at: middle(a) }), 'a');
  assert.equal(pick({ at: middle(b) }), 'b');
  // auditor-2-focus-containment: beside both, the nearest.
  assert.equal(pick({ at: { x: middle(b).x + 1e4, y: middle(b).y } }), 'b');
  assert.equal(pick({ at: middle(b), open: new Set(['/a']) }), 'a', 'the one open stays open');
  assert.equal(pick({ at: middle(a), pinned: new Set(['/b']) }), 'b', 'a pinned one first');
  // Below OPEN_K nothing opens; between CLOSE_K and OPEN_K only what was open.
  const lower = (f) => chooseOpen({ path: '', layout: top, transform: { k: opensAt(a, inner.bounds) * f, x: 0, y: 0 }, screen: { x: -1e9, y: -1e9, w: 2e9, h: 2e9 }, layoutOf, open: new Set(['/a']), at: { x: 0, y: 0 } })?.node.name ?? null;
  assert.equal(lower((1 + CLOSE_K / OPEN_K) / 2), 'a');
  assert.equal(lower((CLOSE_K / OPEN_K) * 0.99), null);
  // AC6: a library composite (its inside not in `graph`) never opens, not even pinned.
  assert.notEqual(pick({ pinned: new Set(['/lib']), at: middle(top.nodes.find((n) => n.name === 'lib')) }), 'lib');
  const libOnly = { ...top, nodes: top.nodes.filter((n) => n.name === 'lib') };
  assert.equal(chooseOpen({ path: '', layout: libOnly, transform: { k: 1e6, x: 0, y: 0 }, screen, layoutOf, pinned: new Set(['/lib']), at: centre }), null);
  // Fit keeps every composite closed.
  const fitted = closedFit(top, layoutOf, viewport);
  assert.ok(fitted.k <= closedScale(top, layoutOf) && fitted.k * linkScale(a, inner.bounds) < CLOSE_K);
});

// Slice 2: the person's navigation and the level edited.

/** The studio's synth opened by zooming (no pin), at a 1000 px stage: its scene. */
function synthZoomed(factor = 1.2) {
  const synth = nodeOf(studioOf, '', 'synth');
  const view = { k: opensAt(synth, studioOf('moog_voice').bounds) * factor, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  return scene({ layoutOf: studioOf, view, viewport });
}

test('Enter fits the synth readable at a 1000 px stage, overflowing rather than below OPEN_K (auditor-2-focus-below-close)', () => {
  const bounds = studioOf('moog_voice').bounds;
  const middle = { x: bounds.x + bounds.w / 2, y: bounds.y + bounds.h / 2 };
  const plain = fit(bounds, viewport, 24, 40, 120);
  assert.ok(plain.k < OPEN_K, `a plain fit is ${plain.k}`);
  const view = enterView(bounds, viewport, 24, 40, 120);
  assert.ok(Math.abs(view.k - READ_K) < 1e-12);
  near(toScreen(view, middle), toScreen(plain, middle), 'about the fit’s centre');
  const wide = { w: 2400, h: 1200 };
  assert.deepEqual(enterView(bounds, wide, 24, 40, 120), fit(bounds, wide, 24, 40, 120), 'readable already: the plain fit');
});

test('a breadcrumb to the open synth keeps it open and readable (auditor-2-focus-below-close)', () => {
  const drawn = synthZoomed();
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.path), ['', '/synth']);
  const rect = levelRect(drawn, '/synth');
  const plain = focus(drawn.view, rect, viewport, 24, 40, 120);
  const plainK = drawn.levels[1].transform.k * (plain.k / drawn.view.k);
  assert.ok(plainK < OPEN_K, `a plain focus draws it at ${plainK}, less readable than OPEN_K`);
  const view = crumbView(drawn.view, drawn.levels[1], rect, viewport, studioOf, 24, 40, 120);
  const after = scene({ layoutOf: studioOf, view, viewport, open: drawn.open });
  assert.deepEqual(breadcrumbs(after).map((c) => c.path), ['', '/synth'], 'still open');
  assert.ok(after.levels[1].transform.k >= READ_K - 1e-9, `at ${after.levels[1].transform.k}`);
});

test('only a zoom out below CLOSE_K unpins: Enter, then a step in, keeps it open (auditor-1-pinned-never-closes, auditor-2-unpin-direction)', () => {
  const bounds = studioOf('moog_voice').bounds;
  let pinned = pinOne(new Set(), '/synth');
  let view = { ...enterView(bounds, viewport, 24, 40, 120), path: '/synth' };
  let drawn = scene({ layoutOf: studioOf, view, viewport, pinned });
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.path), ['', '/synth']);
  assert.equal(unpinned(pinned, drawn, 1.25), pinned, 'a step in keeps it');
  drawn = scene({ layoutOf: studioOf, view: zoomAt(drawn.view, 1.25, centre), viewport, pinned, open: drawn.open });
  assert.ok(drawn.open.has('/synth'));
  // Steps out: kept while its inside stays at CLOSE_K or more, then unpinned and closed.
  let steps = 0;
  while (pinned.has('/synth') && steps++ < 100) {
    const k = drawn.levels.find((l) => l.path === '/synth').transform.k;
    pinned = unpinned(pinned, drawn, 0.8);
    if (pinned.has('/synth')) assert.ok(k * 0.8 >= CLOSE_K, `kept at ${k * 0.8}`);
    drawn = scene({ layoutOf: studioOf, view: zoomAt(drawn.view, 0.8, centre), viewport, pinned, open: drawn.open });
  }
  assert.ok(!pinned.has('/synth'), 'unpinned by zooming out');
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.path), [''], 'and closed');
});

test('one pin per level, the frame drawn last, editing at the innermost open level only (AC3)', () => {
  assert.deepEqual([...pinOne(new Set(['/a', '/a/x', '/b/y']), '/b')].sort(), ['/b', '/b/y']);
  assert.deepEqual([...pinOne(new Set(['/a', '/a/x']), '/a/z')].sort(), ['/a', '/a/z']);
  const drawn = synthZoomed();
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
  const closed = scene({ layoutOf: studioOf, view: closedFit(studioOf(''), studioOf, viewport), viewport });
  assert.equal(editable(closed, onFrame), true, 'nothing open: the top level is edited');
});

// auditor-1-pinned-never-closes (round 2): several zoom events before a frame.
test('zoom-outs that arrive before a render count together: the pinned synth unpins and closes', () => {
  const bounds = studioOf('moog_voice').bounds;
  let pinned = pinOne(new Set(), '/synth');
  const drawn = scene({ layoutOf: studioOf, view: { ...enterView(bounds, viewport, 24, 40, 120), path: '/synth' }, viewport, pinned });
  const inside = drawn.levels.find((l) => l.path === '/synth').transform.k;
  assert.ok(Math.abs(inside - READ_K) < 1e-9);
  // Two steps of 0.75, as zoomBy takes them, no scene between: each alone keeps it (0.354), together 0.266, below CLOSE_K.
  let view = drawn.view;
  for (let i = 0; i < 2; i++) {
    view = zoomAt(view, 0.75, centre);
    const since = zoomSince(drawn, view);
    assert.ok(Math.abs(since - 0.75 ** (i + 1)) < 1e-12, `the zoom since the last scene: ${since}`);
    pinned = unpinned(pinned, drawn, since);
    if (i === 0) assert.ok(pinned.has('/synth'), 'the first step alone keeps it');
    // The old reading, the last scene's scale times this one step, would keep it after the second as well.
    if (i === 1) assert.ok(unpinned(pinOne(new Set(), '/synth'), drawn, 0.75).has('/synth'));
  }
  assert.ok(!pinned.has('/synth'), 'unpinned');
  const after = scene({ layoutOf: studioOf, view, viewport, pinned, open: drawn.open });
  assert.deepEqual(breadcrumbs(after).map((c) => c.path), [''], 'and closed');
  assert.equal(zoomSince(drawn, { ...view, path: '/other' }), null, 'a view of another level is no zoom');
});

// auditor-1-editing-level-ungated (round 2): undo and redo, and a drag whose level stops being the innermost.
test('undo and redo of another level are refused while a composite is open; the entry stays', async () => {
  const drawn = synthZoomed();
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
  editor.s.drawn = scene({ layoutOf: studioOf, view: closedFit(studioOf(''), studioOf, viewport), viewport });
  session.history.at = 1;
  session.history.entries = [{ level: '', forward: [], backward: [{ op: 'y' }] }];
  assert.equal(await editor.write(-1), true);
  assert.equal(sent.length, 2);
});

test('a drag begun at the top level is not editable once a zoom opened the synth (what pointerUp checks before a drop)', () => {
  const closed = scene({ layoutOf: studioOf, view: closedFit(studioOf(''), studioOf, viewport), viewport });
  const melody = closed.levels[0].layout.nodes.find((n) => n.name === 'melody');
  const press = { kind: 'instance', path: '', level: '', node: melody, address: '/melody' };
  assert.equal(editable(closed, press), true, 'pressed: a drag starts');
  assert.equal(editable(synthZoomed(), press), false, 'released after the synth opened: no move written');
});

// The operator's review of M2.13-3 (2026-10-20): room made for the frame,
// the wires across its edge in their direction, hysteresis, the zoom's
// point kept, nothing opened by a pan, outer edits off.

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

test('hysteresis: a wheel tick out of a composite at its threshold, or of Enter’s view, keeps it open', () => {
  const tick = wheelFactor(100);
  assert.ok(tick < 1 && OPEN_K * tick >= CLOSE_K, `a tick (${tick}) from OPEN_K stays above CLOSE_K`);
  assert.ok(OPEN_K * tick * tick >= CLOSE_K, 'two ticks too');
  assert.ok(Math.abs(OPEN_K / CLOSE_K - 1.6) < 1e-12);
  assert.ok(READ_K > OPEN_K);
  const bounds = studioOf('moog_voice').bounds;
  const view = enterView(bounds, { w: 1300, h: 1000 }, 24, 40, 120);
  assert.ok(view.k >= READ_K - 1e-12 && view.k * tick >= CLOSE_K, `${view.k}`);
});

test('a composite opened by a zoom keeps the box’s point under the pointer there in its frame, its header below the bar (openAnchor)', () => {
  const synth = nodeOf(studioOf, '', 'synth');
  const bounds = studioOf('moog_voice').bounds;
  const k = opensAt(synth, bounds) * 1.01;
  const big = { w: 1300, h: 1000 };
  const at = { x: 650, y: 500 };
  for (const [fx, fy] of [[0.5, 0.5], [0.1, 0.3], [0.9, 0.8]]) {
    const view = { k, ox: synth.x + synth.w * fx, oy: synth.y + synth.h * fy, x: at.x, y: at.y };
    const drawn = scene({ layoutOf: studioOf, view, viewport: big, at });
    assert.deepEqual([...drawn.open], ['/synth']);
    const anchor = openAnchor(drawn, new Set(), at, big, 0);
    const after = scene({ layoutOf: studioOf, view: pan(drawn.view, anchor.dx, anchor.dy), viewport: big, open: drawn.open, opening: false, at });
    assert.deepEqual([...after.open], ['/synth'], `${fx},${fy}: still open`);
    const top = after.levels[0];
    const frame = top.layout.nodes.find((n) => n.name === 'synth');
    const p = toScreen(top.transform, { x: frame.x + fx * frame.w, y: frame.y + fy * frame.h });
    const head = toScreen(top.transform, frame).y;
    // Under the pointer, unless that would take its header above the screen's top (the frame fits: it is kept below).
    assert.ok(Math.abs(p.x - at.x) < 1e-9 && (Math.abs(p.y - at.y) < 1e-9 || Math.abs(head) < 1e-9), `the frame's point ${fx},${fy} under the pointer: ${p.x},${p.y}, header at ${head}`);
    assert.equal(openAnchor(drawn, drawn.open, at, big, 0), null, 'already open: no anchor');
  }
  // With a bar: the frame's header kept below it when the frame fits.
  const view = { k, ox: synth.x + synth.w / 2, oy: synth.y, x: 650, y: 10 };
  const drawn = scene({ layoutOf: studioOf, view, viewport: big, at: { x: 650, y: 10 } });
  const anchor = openAnchor(drawn, new Set(), { x: 650, y: 10 }, big, 40);
  const top = drawn.levels[0];
  const frame = top.layout.nodes.find((n) => n.name === 'synth');
  assert.ok(toScreen(top.transform, frame).y + anchor.dy >= 40 - 1e-9);
});

test('a pan opens nothing new: only a zoom, Enter or a breadcrumb do (opening)', () => {
  const synth = nodeOf(studioOf, '', 'synth');
  const view = { k: opensAt(synth, studioOf('moog_voice').bounds) * 1.2, ox: synth.x + synth.w / 2, oy: synth.y + synth.h / 2, x: centre.x, y: centre.y };
  assert.deepEqual([...scene({ layoutOf: studioOf, view, viewport }).open], ['/synth']);
  assert.deepEqual([...scene({ layoutOf: studioOf, view, viewport, opening: false }).open], [], 'panned onto it: stays closed');
  assert.deepEqual([...scene({ layoutOf: studioOf, view, viewport, opening: false, open: new Set(['/synth']) }).open], ['/synth'], 'open: stays open');
  assert.deepEqual([...scene({ layoutOf: studioOf, view, viewport, opening: false, pinned: new Set(['/synth']) }).open], ['/synth'], 'pinned: open');
});

test('around the open composite, edits are off with the reason (aroundReason)', () => {
  const drawn = synthZoomed();
  assert.equal(aroundReason(drawn, { path: '/synth' }), null);
  assert.match(aroundReason(drawn, { path: '' }), /^the top level is around the open \/synth/);
  assert.equal(aroundReason(scene({ layoutOf: studioOf, view: { k: 0.1, x: 0, y: 0 }, viewport }), { path: '' }), null, 'nothing open: the top level is edited');
});

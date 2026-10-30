// ADR 0047 phase 4 (the owner's feedback on 0.3.16, the board): every drawn
// element hit, one colour per net across a package's edge, corners a few px
// on screen at every zoom, ribbons instead of net labels, and a chip moved
// rerouting only its own wires (nothing else jumps, during the drag or
// after). Measured on the studio's board on its measured plate
// (ui/lib/fixtures/board/studio-plate.json) and its synth's die.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { CONNECTOR_PITCH, INPUTS, PARAMS, layout, mazeNet, movedWires, routed } from './schematic-layout.js';
import { laneWires, overlaps } from './board-lanes.js';
import { exportFacts } from './schematic-groups.js';
import { approximatePlate } from './fixtures/board/plate-approx.js';
import { scene } from './schematic-scene.js';
import { toScreen } from './schematic-view.js';
import { MIN_PORT_PX, hitTest } from './schematic-hit.js';
import { CORNER } from './board-route.js';
import { CORNER_PX, cornerAt, viaRect } from './board-view.js';
import { RIBBON_CLEAR, RIBBON_MIN, formRibbons, ribbonLabelRect, ribbonName } from './board-ribbon.js';
import { rerouteMoved } from './board-reroute.js';
import { stretchCorners } from './board-stretch.js';
import { relayout, structureKey } from './board-memo.js';
import { packageTones, sceneTones, wireTones } from './board-tone.js';
import { portNet } from './board-net.js';
import { rounded } from './schematic-wire.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

/** The studio as the page lays it out: the board on its measured plate, each inside a die (once, in its package's order). */
function studio(graph = STUDIO) {
  const panel = new URL('projects/studio/ui/main.html', root);
  const approx = existsSync(panel) ? approximatePlate(readFileSync(panel, 'utf8')) : { controls: [] };
  const fx = JSON.parse(read('ui/lib/fixtures/board/studio-plate.json'));
  const factsOf = (m) => exportFacts(graph.composites?.[m] ?? null);
  const cache = new Map();
  const orders = new Map();
  const dieOf = (node) => {
    if (!graph.composites?.[node.module]) return null;
    if (!cache.has(node.module) && node.topOrder && !orders.has(node.module)) orders.set(node.module, node.topOrder);
    return of(node.module);
  };
  const of = (level) => {
    if (!cache.has(level)) {
      const source = level === '' ? graph.top : graph.composites?.[level];
      const board = level === '' ? { vias: new Map(fx.vias), obstacles: fx.obstacles, plate: fx.size, cards: fx.cards, dieOf } : { die: true, dieOf, paramOrder: orders.get(level) ?? null };
      cache.set(level, source ? layout(source, {}, { factsOf, controls: level === '' ? approx.controls : null, board }) : null);
    }
    return cache.get(level);
  };
  return of;
}

const viewport = { w: 1600, h: 1000 };
/** A view showing level point `p` at the stage's middle at scale `k`. */
const viewAt = (p, k) => ({ k, x: viewport.w / 2 - p.x * k, y: viewport.h / 2 - p.y * k });

test('every drawn element is hit at every zoom: a chip’s side and top pins, a via and its pins, a wire, a leg (as its chip’s pin)', () => {
  const of = studio();
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const topPin = synth.ins.find((p) => p.side === 'top');
  const sidePin = synth.outs[0];
  const via = top.nodes.find((n) => n.via && n.ins.length + n.outs.length > 0);
  const viaPin = [...via.ins, ...via.outs][0];
  const wire = top.wires.find((w) => w.kind !== 'binding' && w.corners.length >= 2 && Math.abs(w.corners[1].x - w.corners[0].x) + Math.abs(w.corners[1].y - w.corners[0].y) > 64);
  const leg = top.wires.find((w) => w.kind === 'binding' && w.to.node === 'synth');
  for (const k of [0.15, 0.5, 1, 3, 12]) {
    for (const [what, p, expect] of [
      ['a top pin', topPin, (h) => h.kind === 'port' && h.port.name === topPin.name],
      ['a side pin', sidePin, (h) => h.kind === 'port' && h.port.name === sidePin.name],
      ['a via pin', viaPin, (h) => h.kind === 'control' && h.node.id === via.id],
    ]) {
      const drawn = scene({ layoutOf: of, view: viewAt(p, k), viewport });
      const at = toScreen(drawn.levels[0].transform, p);
      // Just off the pin, by less than MIN_PORT_PX on screen, at any zoom; a pin of a connector (ADR 0047 phase 5) packed closer
      // than that on screen may give its neighbour in the connector.
      const hit = hitTest(drawn, { x: at.x + (MIN_PORT_PX - 1) * 0.7, y: at.y + (MIN_PORT_PX - 1) * 0.7 });
      const neighbour = what === 'a top pin' && synth.connector && hit.kind === 'port' && hit.port.side === 'top' && Math.abs(hit.port.x - p.x) * k < 2 * MIN_PORT_PX;
      assert.ok(expect(hit) || neighbour, `${what} at k ${k}: ${hit.kind} ${hit.address ?? ''}`);
    }
    // A wire 3 px off its middle, at any zoom (its tolerance is screen px).
    const mid = { x: (wire.corners[0].x + wire.corners[1].x) / 2, y: (wire.corners[0].y + wire.corners[1].y) / 2 };
    const drawn = scene({ layoutOf: of, view: viewAt(mid, k), viewport });
    const at = toScreen(drawn.levels[0].transform, mid);
    const horizontal = wire.corners[0].y === wire.corners[1].y;
    const hit = hitTest(drawn, horizontal ? { x: at.x, y: at.y + 3 } : { x: at.x + 3, y: at.y });
    assert.ok(hit.kind === 'wire' || hit.kind === 'port', `a wire at k ${k}: ${hit.kind}`);
    if (hit.kind === 'wire') assert.equal(hit.wire.id, wire.id);
    // A leg is hit as the pin it sets on its chip (whose net it is).
    const lc = leg.display ?? leg.corners;
    const lm = lc[Math.floor(lc.length / 2)];
    const ld = scene({ layoutOf: of, view: viewAt(lm, k), viewport });
    const lh = hitTest(ld, toScreen(ld.levels[0].transform, lm));
    if (k >= 1) assert.ok(lh.kind === 'port' || lh.kind === 'control', `a leg at k ${k}: ${lh.kind}`);
  }
});

test('zoomed into the synth, its package’s pins and the die’s interface pins at the same points (its zero-high strip, its zero-wide columns) are drawn and hit, and a pin’s net is both sides of its edge', () => {
  const of = studio();
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  for (const k of [1.5, 3, 8]) {
    for (const pin of [synth.ins.find((p) => p.side === 'top'), synth.ins.find((p) => p.side !== 'top'), synth.outs[0]].filter(Boolean)) {
      const drawn = scene({ layoutOf: of, view: viewAt(pin, k), viewport, pinned: new Set(['/synth']) });
      const inner = drawn.levels.at(-1);
      assert.equal(inner.path, '/synth');
      // The die's interface node at this pin is drawn (never culled for having no width or height).
      assert.ok(inner.nodes.some((n) => n.edge && [...n.ins, ...n.outs].some((p) => p.name === pin.name)), `${pin.name} at k ${k}: its interface pin drawn`);
      const at = toScreen(drawn.levels[0].transform, pin);
      const hit = hitTest(drawn, { x: at.x + 2, y: at.y + 2 });
      assert.equal(hit.kind, 'port', `${pin.name} at k ${k}`);
      // A connector's pins (ADR 0047 phase 5) closer than MIN_PORT_PX on screen: its neighbour in the connector is as good a hit.
      const packed = pin.side === 'top' && synth.connector;
      const tops = new Set(synth.ins.filter((p) => p.side === 'top').map((p) => `/synth.${p.name}`));
      const near = packed && tops.has(hit.address);
      if (!near) assert.equal(hit.address, `/synth.${pin.name}`, `${pin.name} at k ${k}: the composite’s port, outside or inside`);
      const net = portNet(drawn.levels, hit);
      const paths = new Set(net.map((n) => n.path));
      assert.ok(paths.has('') && paths.has('/synth'), `${pin.name}: the outer wire and its continuation (${[...paths]})`);
    }
  }
  // A strip's pin a little off, zoomed in: within PORT_PX it is the pin, not the wire leaving it.
  const die = of('moog_voice');
  const strip = die.nodes.find((n) => n.id === PARAMS);
  assert.ok(strip && strip.h === 0);
  const pin = synth.ins.find((p) => p.side === 'top');
  const drawn = scene({ layoutOf: of, view: viewAt(pin, 6), viewport, pinned: new Set(['/synth']) });
  const at = toScreen(drawn.levels[0].transform, pin);
  const hit = hitTest(drawn, { x: at.x, y: at.y + 6 });
  assert.equal(hit.kind, 'port');
  assert.equal(hit.port.name, pin.name);
});

test('a net keeps one colour across a package’s edge: a knob’s leg amber outside, its continuation inside the die amber, at every depth', () => {
  const of = studio();
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const drawn = scene({ layoutOf: of, view: viewAt({ x: synth.x + synth.w / 2, y: synth.y + synth.h / 2 }, 2), viewport, pinned: new Set(['/synth']) });
  sceneTones(drawn.levels);
  const [outer, inner] = drawn.levels;
  let checked = 0;
  for (const w of inner.layout.wires) {
    if (w.from.node !== PARAMS && w.from.node !== INPUTS) continue;
    const reaching = outer.layout.wires.filter((o) => o.to.node === synth.id && o.to.port === w.from.port).sort((a, b) => a.id - b.id)[0];
    if (!reaching) continue;
    assert.equal(inner.tones.get(w.id), outer.tones.get(reaching.id), `${w.from.port}: the same tone inside as outside`);
    assert.equal(inner.pinTones.get(`${w.from.node}\nout\n${w.from.port}`), outer.pinTones.get(`${synth.id}\nin\n${w.from.port}`), `${w.from.port}: its pins too`);
    checked++;
  }
  assert.ok(checked >= 5, `checked ${checked}`);
  assert.ok([...inner.tones.values()].includes('binding'), 'a leg’s amber carried in');
  // Pure: a die wire with nothing outside keeps its own tone; the inheritance is by port name.
  const tones = wireTones(inner.layout, new Map([['x', 'events']]));
  for (const w of inner.layout.wires) if (w.kind !== 'binding') assert.equal(tones.get(w.id), w.shape);
  assert.deepEqual([...packageTones(outer.layout, outer.tones, { id: 'nothing' })], []);
});

test('corners are CORNER_PX on screen at every zoom (half-octave steps), never rounder than the route', () => {
  for (const k of [0.1, 0.5, 1, 2, 3, 8, 40, 1000]) {
    const r = cornerAt(k);
    assert.ok(r <= CORNER + 1e-9);
    const px = r * k;
    if (k >= 1) assert.ok(px >= CORNER_PX / Math.SQRT2 - 1e-9 && px <= CORNER_PX * Math.SQRT2 + 1e-9, `k ${k}: ${px} px`);
    else assert.ok(px <= CORNER_PX + 1e-9, `k ${k}: ${px} px`);
    // The arc's handles end r from its corner.
    const curve = rounded([{ x: 0, y: 0 }, { x: 100, y: 0 }, { x: 100, y: 100 }], r);
    const arc = curve.segs.find((s) => s.c1);
    assert.ok(Math.abs(100 - curve.segs[0].to.x - r) < 1e-9 && Math.abs(arc.to.y - r) < 1e-9);
  }
  assert.equal(cornerAt(1), CORNER);
});

test('ribbons: deterministic, every member continuous from its pins, packed in its band at 45°, the band clear of every box; the studio’s triggers one ribbon', () => {
  const of = studio();
  const top = of('');
  const ribbons = top.board.ribbons;
  assert.ok(ribbons.length >= 1);
  const trig = ribbons.find((r) => r.label === 't ×7');
  assert.ok(trig, `the drum triggers (${ribbons.map((r) => r.label)})`);
  for (const r of ribbons) {
    assert.ok(r.members.length >= RIBBON_MIN);
    for (const n of top.nodes) {
      if (!(n.w > 0 && n.h > 0)) continue;
      const b = r.band;
      assert.ok(!(b.x < n.x + n.w && n.x < b.x + b.w && b.y < n.y + n.h && n.y < b.y + b.h), `${r.label} over ${n.id}`);
    }
  }
  for (const w of top.wires) {
    if (!w.display) continue;
    assert.deepEqual(w.display[0], w.corners[0]);
    assert.deepEqual(w.display.at(-1), w.corners.at(-1));
    for (let k = 1; k < w.display.length; k++) {
      const dx = Math.abs(w.display[k].x - w.display[k - 1].x);
      const dy = Math.abs(w.display[k].y - w.display[k - 1].y);
      // A lane's short step across (ADR 0047 phase 5) may be steeper than 45° where its stretches are short.
      assert.ok(dx === 0 || dy === 0 || Math.abs(dx - dy) < 1e-9 || Math.min(dx, dy) <= 16, `${w.id}: orthogonal, 45° or a short step`);
    }
  }
  // The same wires twice, in reverse order, with every object's keys reversed: the same ribbons.
  const input = top.wires.map((w) => ({ id: w.id, corners: w.corners, net: `${w.from.node}\n${w.from.port}` }));
  const a = formRibbons(input, top.nodes);
  const b = formRibbons([...input].reverse().map((w) => ({ net: w.net, corners: w.corners, id: w.id })), top.nodes);
  assert.deepEqual(b.ribbons.map((r) => [r.axis, r.band, r.members.map((m) => [m.net, m.track, m.slot])]).sort(), a.ribbons.map((r) => [r.axis, r.band, r.members.map((m) => [m.net, m.track, m.slot])]).sort());
  assert.deepEqual([...b.display].sort(), [...a.display].sort());
  assert.equal(ribbonName(['trig1', 'trig2', 'trig3']), 'trig');
  assert.equal(ribbonName(['t2', 't3']), 't');
  assert.equal(ribbonName(['out', 'in']), '');
});

test('no net labels where a wire can go: the studio’s board has none', () => {
  const top = studio()('');
  assert.equal(top.board.labels.length, 0);
  assert.ok(top.wires.every((w) => !w.labelled));
});

test('dragged on the board, a chip’s wires keep their routes stretched at the moved end; every other wire stays exactly', () => {
  const top = studio()('');
  const chip = top.nodes.find((n) => n.name === 'bus');
  const moved = movedWires(top, chip.id, { x: chip.x + 24, y: chip.y + 40 });
  const ids = new Set(moved.map((m) => m.wire.id));
  for (const w of top.wires) assert.equal(ids.has(w.id), w.from.node === chip.id || w.to.node === chip.id, `${w.id}: only the chip’s own`);
  for (const m of moved) {
    const c = m.corners;
    for (let k = 1; k < c.length; k++) assert.ok(c[k].x === c[k - 1].x || c[k].y === c[k - 1].y, 'orthogonal');
    const pin = m.wire.from.node === chip.id ? c[0] : c.at(-1);
    const was = m.wire.from.node === chip.id ? m.wire.corners[0] : m.wire.corners.at(-1);
    assert.deepEqual(pin, { x: was.x + 24, y: was.y + 40 }, 'at the moved pin');
    const far = m.wire.from.node === chip.id ? c.at(-1) : c[0];
    assert.deepEqual(far, m.wire.from.node === chip.id ? m.wire.corners.at(-1) : m.wire.corners[0], 'its other end where it was');
    assert.ok(routed(m, 1).points.length > 1);
  }
  assert.deepEqual(stretchCorners([{ x: 0, y: 0 }, { x: 100, y: 0 }], null, { dx: 0, dy: 30 }), [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 30 }, { x: 100, y: 30 }]);
});

test('let go, a moved chip reroutes only its own wires: every other route identical, every other box in place (on the board and in a die)', () => {
  const of = studio();
  for (const [level, name, d] of [['', 'bus', { x: 24, y: 48 }], ['moog_voice', 'glide', { x: 32, y: 32 }]]) {
    const before = of(level);
    const node = before.nodes.find((n) => n.name === name);
    const after = rerouteMoved(before, new Map([[node.id, { x: node.x + d.x, y: node.y + d.y }]]));
    for (const [i, n] of after.nodes.entries()) {
      if (n.id === node.id) assert.deepEqual([n.x, n.y], [node.x + d.x, node.y + d.y]);
      else assert.deepEqual([n.x, n.y, n.w, n.h], [before.nodes[i].x, before.nodes[i].y, before.nodes[i].w, before.nodes[i].h], `${level} ${n.id} stays`);
    }
    const redo = new Set(after.rerouted);
    let same = 0;
    for (const w of before.wires) {
      const touches = w.from.node === node.id || w.to.node === node.id;
      if (touches) assert.ok(redo.has(w.id), `${level} ${w.id}: its own, rerouted`);
      if (redo.has(w.id)) continue;
      assert.equal(JSON.stringify(after.wires.find((x) => x.id === w.id).corners), JSON.stringify(w.corners), `${level} ${w.id}: byte-identical`);
      same++;
    }
    assert.ok(same > 0);
    // Clean: orthogonal, and no rerouted wire under a box but its own ends.
    for (const id of redo) {
      const w = after.wires.find((x) => x.id === id);
      for (let k = 1; k < w.corners.length; k++) assert.ok(w.corners[k].x === w.corners[k - 1].x || w.corners[k].y === w.corners[k - 1].y);
    }
    assert.equal(after.board.die ?? null, before.board.die ?? null, 'a die keeps its rectangle');
    // Deterministic.
    assert.equal(JSON.stringify(rerouteMoved(before, new Map([[node.id, { x: node.x + d.x, y: node.y + d.y }]])).wires.map((w) => w.corners)), JSON.stringify(after.wires.map((w) => w.corners)));
  }
});

test('the same structure laid out again (a parameter’s value, the selection) keeps the very layout; a drop reroutes only what moved', () => {
  const of = studio();
  const before = of('');
  const source = STUDIO.top;
  const key = structureKey(source);
  // A value is not structure.
  const changed = JSON.parse(JSON.stringify(source));
  changed.instances[0].parameters[0].default = 999;
  assert.equal(structureKey(changed), key);
  let calls = 0;
  const full = () => {
    calls++;
    return before;
  };
  const prev = { key, stored: {}, layout: before, dies: new Map() };
  const kept = relayout(prev, structureKey(changed), {}, full);
  assert.equal(kept.how, 'kept');
  assert.equal(kept.layout, before, 'the very object: nothing drawn again');
  assert.equal(calls, 0, 'no layout');
  // A drop: only the moved chip's wires change.
  const bus = before.nodes.find((n) => n.name === 'bus');
  const moved = relayout(prev, key, { bus: { x: bus.x + 16, y: bus.y + 32 } }, full);
  assert.equal(moved.how, 'rerouted');
  assert.equal(calls, 0);
  for (const w of before.wires) if (w.from.node !== bus.id && w.to.node !== bus.id && !moved.layout.rerouted.includes(w.id)) assert.deepEqual(moved.layout.wires.find((x) => x.id === w.id).corners, w.corners);
  // A changed structure, or a position reset: a full layout.
  assert.equal(relayout(prev, `${key}x`, {}, full).how, 'full');
  assert.equal(relayout({ ...prev, stored: { bus: { x: 0, y: 0 } } }, key, {}, full).how, 'full');
  assert.equal(relayout(prev, key, {}, full, () => false).how, 'full', 'a die it was laid around changed');
});

test('a via measured a fraction off a half unit snaps the same way (a plate measured again gives the same board)', () => {
  assert.deepEqual(viaRect({ x: 3.99999, y: 12.00001, w: 88.00002, h: 119.99998 }), viaRect({ x: 4.00001, y: 11.99999, w: 87.99998, h: 120.00002 }));
});

test('nothing a wire is drawn with fills it: no rule of the schematic’s or the board’s CSS fills a class a wire’s path carries (the coordinator’s check of 3f32ab2: a `leg` tone took the pins’ legs’ fill)', () => {
  const css = `${read('ui/controls/schematic.js')}\n${read('ui/controls/schematic-board.js')}`;
  const wireClasses = new Set(['wire', 'signal', 'events', 'vector', 'binding', 'tone-leg', 'dashed', 'crossing', 'raised', 'hover', 'selected', 'traced', 'link']);
  // Every class a wire's tone can give, at every level of a scene zoomed into the synth.
  const of = studio();
  const synth = of('').nodes.find((n) => n.name === 'synth');
  const drawn = scene({ layoutOf: of, view: viewAt({ x: synth.x + synth.w / 2, y: synth.y + synth.h / 2 }, 2), viewport, pinned: new Set(['/synth']) });
  sceneTones(drawn.levels);
  for (const entry of drawn.levels) {
    for (const w of entry.layout.wires) {
      const tone = entry.tones.get(w.id);
      const cls = tone === 'binding' && w.kind !== 'binding' ? 'tone-leg' : tone;
      assert.ok(wireClasses.has(cls), `${entry.path} ${w.id}: ${cls}`);
    }
  }
  for (const m of css.matchAll(/(^|\n)([^{}\n]+)\{([^{}]*)\}/g)) {
    const fill = /(^|;)\s*fill:\s*([^;]+)/.exec(m[3])?.[2]?.trim();
    if (!fill || fill === 'none') continue;
    for (const sel of m[2].split(',').map((x) => x.trim())) {
      const bare = /^(?:path)?\.([a-z-]+)$/.exec(sel);
      assert.ok(!bare || !wireClasses.has(bare[1]), `“${sel} { fill: ${fill} }” would fill a wire`);
    }
  }
});

test('a ribbon’s label overlaps no box, no via’s name, no net label and no other ribbon’s label; one label per set of members; no band within RIBBON_CLEAR of a box (the coordinator’s check of 3f32ab2)', () => {
  const of = studio();
  const levels = ['', ...Object.keys(STUDIO.composites)].map((l) => [l, of(l)]);
  let labelled = 0;
  for (const [name, lay] of levels) {
    const boxes = lay.nodes.filter((n) => n.w > 0 && n.h > 0).map((b) => ({ x: b.x, y: b.y - (b.via ? 8 : 0), w: b.w, h: b.h + (b.via ? 8 : 0) }));
    const texts = lay.board.labels.map((l) => ({ x: l.side === 'in' ? l.at.x - 5.4 * l.text.length : l.at.x, y: l.at.y - 9, w: 5.4 * l.text.length, h: 11 }));
    const placed = [];
    const sets = [];
    const meets = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    for (const r of lay.board.ribbons) {
      // Side by side (phase 4) a band keeps RIBBON_CLEAR from every box; in lanes (phase 5: a chip's legs, a connector's ribbon) it lies over none.
      const clear = r.lanes ? 0 : RIBBON_CLEAR - 1;
      for (const b of r.lanes ? boxes.filter((b) => !b.plate) : boxes) {
        const grown = { x: b.x - clear, y: b.y - clear, w: b.w + 2 * clear, h: b.h + 2 * clear };
        assert.ok(!meets(r.band, grown), `${name} ${r.label}: its band near a box`);
      }
      if (!r.text) continue;
      labelled++;
      const rect = ribbonLabelRect(r.label, r.text);
      for (const o of [...boxes, ...texts, ...placed]) assert.ok(!meets(rect, o), `${name} ${r.label}: over ${JSON.stringify(o)}`);
      const members = new Set(r.members.map((m) => m.net));
      assert.ok(!sets.some((s) => [...members].every((n) => s.has(n))), `${name} ${r.label}: labelled twice`);
      sets.push(members);
      placed.push(rect);
    }
  }
  assert.ok(labelled >= 1);
});

// ADR 0047 phase 5 (the owner, 2026-10-30): wires on every level, the synth's legs in ribbons, ribbons into the chips.

test('wires on every level: no net is a label on any board or die of the example projects, and the board draws no label', async () => {
  const { SchematicBoard } = await import('../controls/schematic-board.js');
  assert.equal(SchematicBoard.prototype.drawLabels, undefined, 'no label drawing mode');
  const of = studio();
  for (const level of ['', ...Object.keys(STUDIO.composites)]) {
    const lay = of(level);
    assert.equal(lay.board.labels.length, 0, `${level || 'top'}: no label`);
    assert.ok(lay.wires.every((w) => !w.labelled), `${level || 'top'}: every wire drawn`);
  }
});

test('the synth’s legs gather into ribbons (per row into the channel, down to its connector; one label, its connector’s), never drawn on top of each other', () => {
  const top = studio()('');
  const synthLegs = top.wires.filter((w) => w.kind === 'binding' && w.to.node === 'synth');
  assert.ok(synthLegs.length >= 20);
  const ribbons = top.board.ribbons.filter((r) => r.lanes && r.group === 'legs:synth');
  assert.ok(ribbons.length >= 1, 'in ribbons');
  assert.ok(Math.max(...ribbons.map((r) => r.members.length)) >= 6, 'a wide one');
  // One label, consistent: its connector's (`params ×n` on the package); its ribbons say nothing of their own.
  assert.equal(ribbons.filter((r) => r.text).length, 0, 'labelled by its connector');
  const synth = top.nodes.find((n) => n.id === 'synth');
  assert.match(synth.connector.label, /^params ×\d+$/);
  // Laid out as ribbons (ADR 0047 phase 5, the coordinator's check): each leg down from its via to its row's line, along it to the
  // channel, down the channel, into the connector; the channel between the plate's controls, not through them.
  const plan = synth.legPlan;
  assert.ok(plan && plan.rows.length >= 2, 'rows of vias');
  for (const w of synthLegs) {
    const c = w.corners;
    assert.ok(c.some((p) => p.x === plan.x), `${w.id}: down the channel`);
    assert.ok(plan.rows.some((r) => c.some((p) => p.y === r.y)), `${w.id}: along its row's line`);
  }
  const { plate } = { plate: JSON.parse(readFileSync(new URL('ui/lib/fixtures/board/studio-plate.json', root), 'utf8')) };
  const topRow = Math.min(...plan.rows.map((r) => r.y));
  for (const r of plate.obstacles) assert.ok(!(r.x < plan.x && plan.x < r.x + r.w && r.y < plan.floor && r.y + r.h > topRow), 'the channel crosses no control');
  // Their drawn ways: no two of them on one line over a stretch (routes they share are drawn in lanes).
  const drawn = synthLegs.map((w) => ({ net: mazeNet(w).net, points: w.display ?? w.corners }));
  const shared = synthLegs.map((w) => ({ net: mazeNet(w).net, points: w.corners }));
  assert.ok(overlaps(shared) > 20, 'their routes share their way (a bus)');
  // A lane's corner may still touch the next stretch's lane by a unit or two (measured: one such place on the studio).
  assert.ok(overlaps(drawn) <= 1, `drawn apart (${overlaps(drawn)})`);
  for (const w of synthLegs) {
    const c = w.display ?? w.corners;
    assert.deepEqual(c[0], w.corners[0], 'from its via');
    assert.deepEqual(c.at(-1), w.corners.at(-1), 'to its pin');
  }
});

test('a ribbon goes into its chip: the synth’s parameters are one connector on its package, in its vias’ order, continued inside the die as one ribbon', () => {
  const of = studio();
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const die = of('moog_voice');
  const strip = die.nodes.find((n) => n.id === PARAMS);
  assert.ok(strip.connector && synth.connector, 'a connector on the strip and on the package');
  const tops = synth.ins.filter((p) => p.side === 'top').sort((a, b) => a.x - b.x);
  assert.equal(synth.connector.count, tops.length);
  assert.equal(synth.connector.label, `params ×${tops.length}`);
  // Compact: its pins within the connector, CONNECTOR_PITCH apart in the die (a few units on the package), in the package's order.
  for (const p of tops) assert.ok(p.x >= synth.x + synth.connector.dx0 && p.x <= synth.x + synth.connector.dx1);
  assert.ok(synth.connector.dx1 - synth.connector.dx0 < synth.w / 2, 'a compact group, not spread over the edge');
  assert.deepEqual(tops.map((p) => p.name), [...synth.topOrder.keys()].sort((a, b) => synth.topOrder.get(a) - synth.topOrder.get(b)), 'the vias’ order');
  const xs = strip.outs.map((p) => p.x).sort((a, b) => a - b);
  for (let i = 1; i < xs.length; i++) assert.equal(xs[i] - xs[i - 1], CONNECTOR_PITCH);
  // Inside: the strip's wires are one bus, drawn in lanes as a ribbon labelled once.
  const inner = die.wires.filter((w) => w.from.node === PARAMS);
  assert.ok(inner.every((w) => w.bus === 'bus:params'));
  // Drawn in lanes (they share their way), a band only where they run packed clear of every chip, never a label of its own.
  assert.ok(inner.filter((w) => w.display).length >= inner.length / 2, 'in lanes');
  const ribbons = die.board.ribbons.filter((r) => r.lanes && r.group === 'params');
  assert.equal(ribbons.filter((r) => r.text).length, 0);
  assert.ok(overlaps(die.wires.map((w) => ({ net: mazeNet(w).net, points: w.display ?? w.corners }))) <= 1);
});

test('one member traced through: a leg hit at the plate, its connector pin, and its continuation inside the die, in one colour', () => {
  const of = studio();
  const top = of('');
  const synth = top.nodes.find((n) => n.name === 'synth');
  const leg = top.wires.find((w) => w.kind === 'binding' && w.to.node === 'synth');
  const pin = synth.ins.find((p) => p.name === leg.to.port);
  const drawn = scene({ layoutOf: of, view: viewAt(pin, 6), viewport, pinned: new Set(['/synth']) });
  sceneTones(drawn.levels);
  const at = toScreen(drawn.levels[0].transform, pin);
  const hit = hitTest(drawn, at);
  assert.equal(hit.kind, 'port');
  assert.equal(hit.port.name, leg.to.port, 'its own pin in the connector');
  const net = portNet(drawn.levels, hit);
  const outer = net.filter((n) => n.path === '');
  const inner = net.filter((n) => n.path === '/synth');
  assert.ok(outer.some((n) => n.wire.id === leg.id), 'the leg outside');
  assert.ok(inner.length >= 1 && inner.every((n) => n.wire.from.port === leg.to.port), 'its continuation inside');
  for (const n of inner) assert.equal(drawn.levels.at(-1).tones.get(n.wire.id), 'binding', 'the leg’s colour inside');
});

test('lanes: three nets sharing a way each get a lane, keep it along the run, peel off where they turn without crossing, deterministic', () => {
  // A bus coming down x = 0 and turning right along y = 100, each member dropping into its own pin further right.
  const wires = [0, 1, 2].map((i) => ({ id: i, net: `n${i}`, corners: [{ x: 0, y: 0 }, { x: 0, y: 100 }, { x: 100 + 40 * i, y: 100 }, { x: 100 + 40 * i, y: 140 }] }));
  const a = laneWires(wires, { isLeg: () => true });
  const b = laneWires([...wires].reverse(), { isLeg: () => true });
  assert.deepEqual([...b.display].sort(), [...a.display].sort(), 'deterministic');
  assert.equal(a.laned.size, 3);
  const shown = wires.map((w) => ({ net: w.net, points: a.display.get(w.id) }));
  assert.equal(overlaps(shown), 0, 'never on top of each other');
  assert.ok(overlaps(wires.map((w) => ({ net: w.net, points: w.corners }))) > 0, 'their routes share');
  // Along the horizontal run each keeps one y; the first to leave (n0, at x 100) runs nearest its drop (the bottom).
  const yAt = (id) => a.display.get(id).find((p, k, c) => k > 0 && c[k - 1].y === p.y && p.x > 50)?.y;
  assert.ok(yAt(0) > yAt(1) && yAt(1) > yAt(2), `peeling off without crossing (${[0, 1, 2].map(yAt)})`);
  for (const w of wires) {
    assert.deepEqual(a.display.get(w.id)[0], w.corners[0]);
    assert.deepEqual(a.display.get(w.id).at(-1), w.corners.at(-1));
  }
  assert.equal(a.ribbons.length >= 1, true);
  assert.match(a.ribbons[0].label, /^params ×3$/);
});

test('a ribbon looks like one (the coordinator’s check of ead3baa): its band only where its members run packed, as wide as their lanes, each member along it inside it, clear of every chip', () => {
  const of = studio();
  for (const level of ['', 'moog_voice']) {
    const lay = of(level);
    const chips = lay.nodes.filter((n) => n.w > 0 && n.h > 0);
    for (const r of lay.board.ribbons.filter((x) => x.lanes)) {
      const thick = r.axis === 'h' ? r.band.h : r.band.w;
      const long = r.axis === 'h' ? r.band.w : r.band.h;
      assert.ok(thick <= r.members.length * 3 + 6, `${level || 'top'} ${r.label}: ${thick} thick for ${r.members.length}`);
      for (const c of chips) {
        const g = { x: c.x - RIBBON_CLEAR, y: c.y - RIBBON_CLEAR, w: c.w + 2 * RIBBON_CLEAR, h: c.h + 2 * RIBBON_CLEAR };
        assert.ok(!(r.band.x < g.x + g.w && g.x < r.band.x + r.band.w && r.band.y < g.y + g.h && g.y < r.band.y + r.band.h), `${level || 'top'} ${r.label}: clear of ${c.id}`);
      }
      // Each member runs along the band, inside it, over most of its length.
      for (const id of r.wires) {
        const w = lay.wires.find((x) => x.id === id);
        const c = w.display ?? w.corners;
        let along = 0;
        for (let k = 1; k < c.length; k++) {
          const [p, q] = [c[k - 1], c[k]];
          const inside = (pt) => pt.x >= r.band.x - 0.01 && pt.x <= r.band.x + r.band.w + 0.01 && pt.y >= r.band.y - 0.01 && pt.y <= r.band.y + r.band.h + 0.01;
          if (r.axis === 'h' && p.y === q.y && inside({ x: r.band.x + r.band.w / 2, y: p.y })) along = Math.max(along, Math.min(Math.max(p.x, q.x), r.band.x + r.band.w) - Math.max(Math.min(p.x, q.x), r.band.x));
          if (r.axis === 'v' && p.x === q.x && inside({ x: p.x, y: r.band.y + r.band.h / 2 })) along = Math.max(along, Math.min(Math.max(p.y, q.y), r.band.y + r.band.h) - Math.max(Math.min(p.y, q.y), r.band.y));
        }
        // (but where it turns in or out at the band's ends, by as much as the band is thick)
        if (r.members.some((m) => m.net === mazeNet(w).net)) assert.ok(along >= long - 2 * thick - 1, `${level || 'top'} ${r.label}: wire ${id} along it (${along} of ${long})`);
      }
    }
  }
});

test('no chip lies across a card’s edge: inside its card, or beside it wholly (the coordinator’s check of ead3baa)', () => {
  const top = studio()('');
  const fx = JSON.parse(read('ui/lib/fixtures/board/studio-plate.json'));
  for (const n of top.nodes.filter((x) => x.kind === 'instance')) {
    for (const c of fx.cards) {
      const meets = n.x < c.x + c.w && c.x < n.x + n.w && n.y < c.y + c.h && c.y < n.y + n.h;
      const inside = n.x >= c.x && n.y >= c.y && n.x + n.w <= c.x + c.w && n.y + n.h <= c.y + c.h;
      assert.ok(!meets || inside, `${n.id} across ${c.title}`);
    }
  }
});

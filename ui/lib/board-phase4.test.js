// ADR 0047 phase 4 (the owner's feedback on 0.3.16, the board): every drawn
// element hit, one colour per net across a package's edge, corners a few px
// on screen at every zoom, ribbons instead of net labels, and a chip moved
// rerouting only its own wires (nothing else jumps, during the drag or
// after). Measured on the studio's board on its measured plate
// (ui/lib/fixtures/board/studio-plate.json) and its synth's die.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { INPUTS, PARAMS, layout, movedWires, routed } from './schematic-layout.js';
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
      // Just off the pin, by less than MIN_PORT_PX on screen, at any zoom.
      const hit = hitTest(drawn, { x: at.x + (MIN_PORT_PX - 1) * 0.7, y: at.y + (MIN_PORT_PX - 1) * 0.7 });
      assert.ok(expect(hit), `${what} at k ${k}: ${hit.kind} ${hit.address ?? ''}`);
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
    const lm = leg.corners[Math.floor(leg.corners.length / 2)];
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
      assert.equal(hit.address, `/synth.${pin.name}`, `${pin.name} at k ${k}: the composite’s port, outside or inside`);
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
      assert.ok(dx === 0 || dy === 0 || Math.abs(dx - dy) < 1e-9, `${w.id}: orthogonal or 45°`);
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
      for (const b of boxes) {
        const grown = { x: b.x - RIBBON_CLEAR + 1, y: b.y - RIBBON_CLEAR + 1, w: b.w + 2 * RIBBON_CLEAR - 2, h: b.h + 2 * RIBBON_CLEAR - 2 };
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

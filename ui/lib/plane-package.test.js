// The engine package (the owner's decision of 2026-11-03): one object, at
// the same place with the same pins on the Wiring, Control and Engine
// layers. `packageOf` lays it out once; the Wiring's ports are its signal
// pins and its cables end on them, the Control layer's cube is it, and the
// Engine's inside lies in its body with its edges at its pins. It never
// covers a control while any size fits (smaller where the room is small;
// with no room at all, cut to its instrument at its bottom right), and the same
// arguments give the same package.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent } from './panel-tree.js';
import { panelCards, parsePlane } from './plane-file.js';
import { assignInstruments, edgePins, stackMembers } from './plane-instruments.js';
import { pinPlaces, stackLayout } from './plane-layout.js';
import { wiringLayout, wiringModel } from './plane-wiring.js';
import { automations, controlLayout, engineOf, innerAddress } from './plane-control.js';
import { REVEAL, chipPath, engineLayout, insideOf, packageReveal, revealShare } from './plane-engine.js';
import { CLEARANCE, PACKAGE_PU, overlaps, packageOf, pinOf } from './plane-package.js';
import { PU } from './plate.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

function studioCards() {
  const text = read('projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  return panelCards(parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' }).tree);
}

/** Knobs in rows across an instrument's rectangle, as its front panel measures them (plane units). */
function knobs(rect, binds, { perRow = 10, top = 48, pitch = 70, w = 56, h = 100, gap = 140 } = {}) {
  return binds.map((address, i) => ({ id: `c${i}`, tag: 'signal-knob', binds: [address], writes: true, label: address, rect: { x: rect.x + 40 + (i % perRow) * pitch, y: rect.y + top + Math.floor(i / perRow) * gap, w, h } }));
}

/** The studio's primary Stack: its Wiring model, each instrument's package (controls as `controlsOf` gives them), the Wiring, Control and Engine laid out. */
function studio(controlsOf = () => []) {
  const { instances, connections } = STUDIO.top;
  const cards = studioCards();
  const owner = assignInstruments({ instances, connections, cards });
  const stack = parsePlane(read('projects/studio/ui/plane.toml')).plane.stacks[0];
  const { stackOf } = stackMembers({ stacks: [stack] }, owner, instances);
  const pins = edgePins(stack, owner, stackOf, connections);
  const heights = { 'card:transport': 120, 'card:mono-synth': 520, 'card:rhythm-composer': 400, 'card:tape-echo': 200, 'card:master': 260 };
  const layout = stackLayout(stack, stack.instruments.map((entry) => ({ entry, w: stack.width * PU, h: heights[entry], x: null })));
  const model = wiringModel(stack, owner, stackOf, instances, connections, pins);
  const fronts = new Map();
  const packages = new Map();
  for (const r of layout.rects) {
    const card = cards.find((c) => `card:${c.id}` === r.entry);
    const controls = controlsOf(r, card?.binds ?? []);
    const binds = controls.filter((c) => c.writes).flatMap((c) => c.binds);
    const autos = automations({ entry: r.entry, binds, owner, graph: STUDIO });
    const inner = new Map(binds.map((b) => [b, innerAddress(b, STUDIO)]).filter(([, a]) => a));
    const ports = model.ports.get(r.entry);
    const args = { rect: { x: r.x, y: r.y, w: r.w, h: r.h }, controls, engine: engineOf(r.entry, owner), autos, inner, title: r.entry };
    fronts.set(r.entry, args);
    packages.set(r.entry, packageOf({ ...args, ins: ports.ins, outs: ports.outs }));
  }
  const pinYs = new Map(pinPlaces(pins, layout.rects).map((p) => [`${p.side} ${p.address}`, p.y]));
  const wiring = wiringLayout(model, layout, pinYs, packages);
  const control = new Map(layout.rects.map((r) => [r.entry, controlLayout({ ...fronts.get(r.entry), pkg: packages.get(r.entry) })]));
  const engines = layout.rects.map((r) => engineLayout({ entry: r.entry, rect: r, pkg: packages.get(r.entry), names: engineOf(r.entry, owner), graph: STUDIO, title: r.entry }));
  return { owner, layout, model, packages, fronts, wiring, control, engines };
}

const withKnobs = (r, binds) => knobs(r, binds.filter((b) => b !== '/melody.position'));

test('one package, the same rectangle and signal pins at the Wiring, Control and Engine layers', () => {
  const s = studio(withKnobs);
  for (const r of s.layout.rects) {
    const pkg = s.packages.get(r.entry);
    // Control's cube is the package itself.
    assert.equal(s.control.get(r.entry).cube, pkg, `${r.entry}: Control draws the package`);
    // The Engine's frame is the package's rectangle, its inside in the package's body.
    const e = s.engines.find((x) => x.entry === r.entry);
    assert.deepEqual(e.frame, { x: pkg.x, y: pkg.y, w: pkg.w, h: pkg.h }, `${r.entry}: the Engine’s package`);
    // The inside in the package's body (the control signal's lanes kept in it by the layout).
    assert.deepEqual(e.area, pkg.body);
    // The Wiring's ports are the package's signal pins, x and y.
    for (const port of s.wiring.ports.filter((p) => p.entry === r.entry)) {
      const pin = pinOf(pkg, port.side, port.address);
      assert.ok(pin, `${port.id}: a pin of the package`);
      assert.deepEqual([port.x, port.y], [pin.x, pin.y], `${port.id} at its pin`);
      assert.equal(pin.x, port.side === 'in' ? pkg.x : pkg.x + pkg.w, 'inputs on the left edge, outputs on the right');
    }
    // The Control layer's automation pins are the package's, under its outputs on its right edge.
    for (const a of s.control.get(r.entry).autoPins) {
      const pin = pkg.autos.find((q) => q.address === a.address);
      assert.deepEqual([a.x, a.y], [pin.x, pin.y]);
      assert.ok(pkg.outs.every((o) => o.y < pin.y), 'under the signal outputs');
    }
  }
  assert.deepEqual(s.packages.get('card:mono-synth').ins.map((p) => p.address), ['/melody.tick']);
  assert.deepEqual(s.packages.get('card:mono-synth').outs.map((p) => p.address), ['/synth.out']);
});

test('the package never covers a control: clear of every one by CLEARANCE, its pins too; within the instrument; deterministic', () => {
  const crowded = (r, binds) => {
    const list = binds.filter((b) => b !== '/melody.position');
    // As the tape echo's six knobs spread across its whole width, and a scope taking the master's right side.
    return r.entry === 'card:master'
      ? [...knobs(r, list.slice(0, 2), { pitch: 100 }), { id: 'scope', tag: 'signal-scope', binds: ['/limiter.out'], writes: false, label: 'scope', rect: { x: r.x + 380, y: r.y + 120, w: r.w - 400, h: r.h - 160 } }]
      : knobs(r, list, { perRow: 6, pitch: Math.max(70, (r.w - 120) / Math.max(1, Math.min(6, list.length))), top: 40 });
  };
  for (const controlsOf of [() => [], withKnobs, crowded]) {
    const s = studio(controlsOf);
    for (const r of s.layout.rects) {
      const pkg = s.packages.get(r.entry);
      const controls = s.fronts.get(r.entry).controls;
      assert.ok(pkg.x >= r.x && pkg.y >= r.y && pkg.x + pkg.w <= r.x + r.w + 1e-6 && pkg.y + pkg.h <= r.y + r.h + 1e-6, `${r.entry}: inside its rectangle`);
      for (const c of controls) {
        assert.ok(!overlaps(pkg, c.rect, CLEARANCE - 1e-6), `${r.entry}: the package clears ${c.id}`);
        for (const p of [...pkg.ins, ...pkg.outs, ...pkg.params, ...pkg.autos]) {
          const inside = p.x > c.rect.x && p.x < c.rect.x + c.rect.w && p.y > c.rect.y && p.y < c.rect.y + c.rect.h;
          assert.ok(!inside, `${r.entry}: pin ${p.address} not over ${c.id}`);
        }
      }
      // Its pins within its edges, in order down its sides.
      for (const list of [pkg.ins, [...pkg.outs, ...pkg.autos]]) {
        for (let i = 1; i < list.length; i++) assert.ok(list[i].y > list[i - 1].y);
        for (const p of list) assert.ok(p.y > pkg.y + pkg.head.h - 1e-6 && p.y < pkg.y + pkg.h, `${p.address} on the side under the title`);
      }
    }
    assert.deepEqual(studio(controlsOf).packages, s.packages, 'the same arguments, the same packages');
  }
});

test('room for it: as large as the free room allows, smaller where the room is small, its parts in proportion', () => {
  const rect = { x: 0, y: 0, w: 1280, h: 600 };
  const ins = [{ id: 'port in /a.in', address: '/a.in', label: 'in', kind: 'audio' }];
  const outs = [{ id: 'port out /a.out', address: '/a.out', label: 'out', kind: 'audio' }];
  const roomy = packageOf({ rect, engine: ['a'], ins, outs });
  assert.ok(roomy.w >= 40 * PU, `a large package where there is room (${roomy.w})`);
  // A row of knobs filling the instrument but a gap of 120 between two of them and a strip above: a smaller package there.
  const tight = { x: 0, y: 0, w: 1280, h: 220 };
  const controls = [
    { id: 'l', binds: ['/a.x'], writes: true, rect: { x: 16, y: 16, w: 560, h: 190 } },
    { id: 'r', binds: ['/a.y'], writes: true, rect: { x: 696, y: 16, w: 568, h: 190 } },
  ];
  const small = packageOf({ rect: tight, controls, engine: ['a'], ins, outs });
  for (const c of controls) assert.ok(!overlaps(small, c.rect, CLEARANCE - 1e-6), `clear of ${c.id}`);
  assert.ok(small.w < PACKAGE_PU.w * PU, `shrunk to the gap (${small.w})`);
  assert.ok(small.ins[0].y > small.y + small.head.h && small.outs[0].y < small.y + small.h, 'its pins under its title, inside it');
  // No room at all (a strip shorter than its pins need, a sliver): cut to the instrument, never spilling onto the next one.
  const many = Array.from({ length: 12 }, (_, i) => ({ id: `port in /a.i${i}`, address: `/a.i${i}`, label: `i${i}`, kind: 'audio' }));
  for (const strip of [{ x: 0, y: 500, w: 1280, h: 120 }, { x: 0, y: 500, w: 60, h: 30 }]) {
    const cut = packageOf({ rect: strip, engine: ['a'], ins: many, outs });
    assert.ok(cut.x >= strip.x && cut.y >= strip.y && cut.x + cut.w <= strip.x + strip.w + 1e-6 && cut.y + cut.h <= strip.y + strip.h + 1e-6, `inside ${JSON.stringify(strip)}: ${JSON.stringify([cut.x, cut.y, cut.w, cut.h])}`);
    for (const p of cut.ins) assert.ok(p.y >= cut.y && p.y <= cut.y + cut.h, 'its pins on it');
  }
});

test('the Wiring’s cables end on the package’s pins, run from them across their box’s edge, and never pass over a package or another box', () => {
  const s = studio(withKnobs);
  const boxOf = new Map(s.wiring.boxes.map((b) => [b.entry, b]));
  for (const c of s.wiring.cables) {
    const first = c.points[0];
    const last = c.points.at(-1);
    if (!c.a.edge) {
      const pin = pinOf(s.packages.get(c.a.entry), 'out', c.a.address);
      assert.deepEqual([first.x, first.y], [pin.x, pin.y], `${c.id} leaves its output pin`);
      assert.equal(c.points[1].y, pin.y, 'level with it');
    }
    if (!c.b.edge) {
      const pin = pinOf(s.packages.get(c.b.entry), 'in', c.b.address);
      assert.deepEqual([last.x, last.y], [pin.x, pin.y], `${c.id} ends on its input pin`);
      assert.equal(c.points.at(-2).y, pin.y, 'level with it');
    }
    for (let i = 1; i < c.points.length; i++) {
      const [a, b] = [c.points[i - 1], c.points[i]];
      const seg = { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(a.x - b.x), h: Math.abs(a.y - b.y) };
      const inner = (r) => seg.x < r.x + r.w - 1e-6 && seg.x + seg.w > r.x + 1e-6 && seg.y < r.y + r.h - 1e-6 && seg.y + seg.h > r.y + 1e-6;
      for (const [entry, pkg] of s.packages) assert.ok(!inner(pkg), `${c.id} passes over ${entry}’s package`);
      for (const [entry, box] of boxOf) if (entry !== c.a.entry && entry !== c.b.entry) assert.ok(!inner(box), `${c.id} passes over ${entry}`);
    }
  }
  assert.equal(s.wiring.cables.length, 7);
});

test('the Engine’s inside lies in the package’s body, its edges at the package’s pins; the package is the top-level chip (Layer 1), a composite in it Layer 2', () => {
  const s = studio(withKnobs);
  for (const e of s.engines) {
    const pkg = s.packages.get(e.entry);
    for (const c of e.cards) assert.ok(c.x >= pkg.body.x - 1e-6 && c.x + c.w <= pkg.body.x + pkg.body.w + 1e-6 && c.y >= pkg.body.y - 1e-6 && c.y + c.h <= pkg.body.y + pkg.body.h + 1e-6, `${c.name} in the body`);
    for (const edge of e.edges) {
      // An automation source's tap ends at the package's automation pin.
      const pin = pinOf(pkg, edge.side, edge.address) ?? pkg.autos.find((a) => a.address === edge.address && edge.side === 'out');
      assert.ok(pin, `${edge.id}: the package has its pin`);
      assert.deepEqual([edge.x, edge.y], [pin.x, pin.y], `${edge.id} at its pin`);
    }
    assert.equal(e.card.id, `package ${e.entry}`);
    assert.ok(e.card.package && e.card.composite);
  }
  // Closed at the Stack's fit (about one screen px a plane unit), whole once its cards are seen at their size.
  const synth = s.engines.find((e) => e.entry === 'card:mono-synth');
  const [a, b] = synth.card.reveal;
  assert.deepEqual(synth.card.reveal, packageReveal(synth.area, synth.scale));
  assert.ok(a >= 2 * REVEAL[0] && b >= 2 * REVEAL[1]);
  assert.equal(revealShare(synth.card.body.w * 1.2, synth.card.reveal), 0, 'closed at the fit');
  assert.equal(revealShare(b, synth.card.reveal), 1);
  // Layer 1 inside the package; Layer 2 inside the synth composite in it, its id as the instrument's level names it.
  const chip = synth.cards.find((c) => c.name === 'synth');
  const inside = (card) => (card.package ? synth : insideOf({ graph: STUDIO, card, pins: synth.pins, scale: synth.scale }));
  const centre = { x: chip.body.x + chip.body.w / 2, y: chip.body.y + chip.body.h / 2 };
  const cards = s.engines.map((e) => e.card);
  assert.deepEqual(chipPath(cards, centre, (b - 1) / synth.card.body.w, inside), [], 'not yet inside the package');
  assert.equal(chipPath(cards, centre, (b + 1) / synth.card.body.w, inside)[0]?.id, 'package card:mono-synth', 'inside the package once it is whole');
  const both = Math.max((b + 1) / synth.card.body.w, (REVEAL[1] + 1) / chip.body.w);
  assert.deepEqual(chipPath(cards, centre, both, inside).map((x) => x.id), ['package card:mono-synth', 'card synth']);
});

/**
 * Every drawn way at a level as segments with its net: a wire by its source (a fan-out one net), a control's leg by the
 * parameter it comes from; `[{net, a, b}]`, and the dots `[{net, x, y}]`.
 */
function waysOf(laid) {
  const segs = [];
  const dots = [];
  const add = (net, points, junctions = []) => {
    for (let i = 1; i < points.length; i++) segs.push({ net, a: points[i - 1], b: points[i] });
    for (const j of junctions) dots.push({ net, ...j });
  };
  for (const w of laid.wires) add(`wire ${String(w.from).replace(/^\//, '')}`, w.points, w.junctions);
  for (const b of laid.binds ?? []) add(`bind ${b.export ?? b.address}`, b.points, b.junctions);
  return { segs, dots };
}

/** The length two axis-parallel segments run along one another (0 when they only touch or cross). */
function alongLength(s, t) {
  const eps = 1e-6;
  const vertical = (q) => Math.abs(q.a.x - q.b.x) < eps;
  const horizontal = (q) => Math.abs(q.a.y - q.b.y) < eps;
  const span = (lo1, hi1, lo2, hi2) => Math.min(hi1, hi2) - Math.max(lo1, lo2);
  if (vertical(s) && vertical(t) && Math.abs(s.a.x - t.a.x) < eps && !(horizontal(s) && horizontal(t)))
    return span(Math.min(s.a.y, s.b.y), Math.max(s.a.y, s.b.y), Math.min(t.a.y, t.b.y), Math.max(t.a.y, t.b.y));
  if (horizontal(s) && horizontal(t) && Math.abs(s.a.y - t.a.y) < eps)
    return span(Math.min(s.a.x, s.b.x), Math.max(s.a.x, s.b.x), Math.min(t.a.x, t.b.x), Math.max(t.a.x, t.b.x));
  return 0;
}

/** Whether point `p` lies inside segment `s` (not at its ends). */
function midSegment(p, s) {
  const eps = 1e-6;
  if (Math.abs(s.a.x - s.b.x) < eps && Math.abs(p.x - s.a.x) < eps) return p.y > Math.min(s.a.y, s.b.y) + eps && p.y < Math.max(s.a.y, s.b.y) - eps;
  if (Math.abs(s.a.y - s.b.y) < eps && Math.abs(p.y - s.a.y) < eps) return p.x > Math.min(s.a.x, s.b.x) + eps && p.x < Math.max(s.a.x, s.b.x) - eps;
  return false;
}

/** The package and every chip inside it, laid as they are drawn: `[{name, laid}]`. */
function levelsOf(e) {
  const out = [{ name: e.entry, laid: e }];
  const walk = (laid, path, depth) => {
    for (const card of laid.cards) {
      if (!card.composite || depth > 4) continue;
      const inside = insideOf({ graph: STUDIO, card, pins: laid.pins, scale: laid.scale });
      if (!inside) continue;
      out.push({ name: `${path}/${card.name}`, laid: inside });
      walk(inside, `${path}/${card.name}`, depth + 1);
    }
  };
  walk(e, e.entry, 0);
  return out;
}

test('no way runs along another’s at any level (they may cross), and a dot only where a way branches off its own net', () => {
  const s = studio(withKnobs);
  const problems = [];
  for (const e of s.engines) {
    for (const { name, laid } of levelsOf(e)) {
      const { segs, dots } = waysOf(laid);
      const tol = 0.05 * laid.scale;
      for (let i = 0; i < segs.length; i++) for (let j = i + 1; j < segs.length; j++) {
        if (segs[i].net === segs[j].net) continue;
        const l = alongLength(segs[i], segs[j]);
        if (l > tol) problems.push(`${name}: ${segs[i].net} along ${segs[j].net} (${l.toFixed(2)})`);
      }
      for (const d of dots) if (!segs.some((q) => q.net === d.net && midSegment(d, q))) problems.push(`${name}: a dot of ${d.net} at ${d.x.toFixed(1)},${d.y.toFixed(1)} on no run of its net`);
    }
  }
  assert.deepEqual(problems, []);
});

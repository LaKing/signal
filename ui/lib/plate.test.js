// ADR 0047 phase 1: the fixed front plate's numbers (ui/lib/plate.js), the
// editor's operations on places (ui/lib/panel-edit.js) and the version a
// write carries its plate in (ui/lib/panel-tree.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  GUTTER,
  MIN_SCALE,
  PU,
  REFERENCE_PU,
  blocked,
  cardSize,
  defaultPlate,
  drawnBox,
  fitScale,
  freeze,
  freezeBox,
  moved,
  overlap,
  pack,
  parsePlace,
  parsePlate,
  placeText,
  resized,
  snap,
  union,
  zoomed,
} from './plate.js';
import { defaultPanel } from './panel.js';
import { addCard, plateCards, setPlace, setPlaces } from './panel-edit.js';
import { EditSession } from './panel-edit.js';
import { generatedTree, parseContent, serialize, splitVersion, versionText } from './panel-tree.js';

const whitelist = JSON.parse(readFileSync(new URL('./panel-whitelist.json', import.meta.url), 'utf8'));

const contains = (outer, inner) => outer.x <= inner.x + 1e-9 && outer.y <= inner.y + 1e-9 && outer.x + outer.w >= inner.x + inner.w - 1e-9 && outer.y + outer.h >= inner.y + inner.h - 1e-9;

test('plates and places are whole plate units; anything else is refused', () => {
  assert.equal(PU, 8);
  assert.equal(REFERENCE_PU * PU, 1280);
  assert.deepEqual(parsePlate('160 90'), { w: 160, h: 90 });
  assert.deepEqual(parsePlace(' 0 12  160 38 '), { x: 0, y: 12, w: 160, h: 38 });
  for (const bad of ['160', '0 90', '1.5 2', '-1 2', 'a b', '1001 1', '']) assert.equal(parsePlate(bad), null, bad);
  for (const bad of ['0 0 0 4', '0 0 4', '0 0 4 4 4', '-1 0 4 4']) assert.equal(parsePlace(bad), null, bad);
  assert.equal(placeText({ x: 1, y: 2, w: 3, h: 4 }), '1 2 3 4');
  assert.deepEqual(drawnBox({ x: 1, y: 2, w: 3, h: 4 }), { x: 12, y: 20, w: 16, h: 24 });
});

// Test 1 (its pure half; the headless-browser half is the operator's): the
// window changes only the scale. A plate's places are text, so a window of
// any width and any device scale factor draws each card at place × scale.
test('a fixed plate: the window changes the scale only, never a place', () => {
  const places = [
    { x: 0, y: 0, w: 160, h: 12 },
    { x: 0, y: 12, w: 160, h: 90 },
    { x: 0, y: 102, w: 80, h: 40 },
  ];
  const plate = { w: 160 * PU, h: 142 * PU };
  const rects = (width) => {
    const s = fitScale({ kind: 'width' }, { w: width, h: 900 }, plate);
    return { s, rects: places.map((p) => ({ x: p.x * PU * s, y: p.y * PU * s, w: p.w * PU * s, h: p.h * PU * s })) };
  };
  const at = Object.fromEntries([700, 1280, 2560].map((w) => [w, rects(w)]));
  assert.equal(at[1280].s, 1);
  assert.equal(at[2560].s, 2);
  assert.equal(at[700].s, 700 / 1280);
  for (const w of [700, 2560]) {
    at[w].rects.forEach((r, i) => {
      const ref = at[1280].rects[i];
      for (const k of ['x', 'y', 'w', 'h']) assert.ok(Math.abs(r[k] / at[w].s - ref[k]) < 1e-9, `${w}: card ${i} ${k} is the same in plate units`);
    });
  }
  // Never under MIN_SCALE: then the plate pans sideways as well.
  assert.equal(fitScale({ kind: 'width' }, { w: 300, h: 900 }, plate), MIN_SCALE);
  // ⌘0: the whole plate in the view; ⌘+ and ⌘−: a manual scale.
  assert.equal(fitScale({ kind: 'whole' }, { w: 1280, h: 568 }, plate), 0.5);
  assert.equal(zoomed(1, 1).scale, 1.25);
  assert.equal(fitScale(zoomed(1, -1), { w: 10, h: 10 }, plate), 0.8);
});

/** The boxes of `cards` (`{span, h}`) as the 1280 px implicit plate's grid lays them out: `columns` columns, the theme's gap, the plate's padding. */
function gridBoxes(cards, columns = 4, gap = 16) {
  const inner = REFERENCE_PU * PU - 2 * GUTTER;
  const col = (inner - gap * (columns - 1)) / columns;
  const boxes = [];
  let c = 0;
  let y = GUTTER;
  let rowH = 0;
  for (const card of cards) {
    const span = Math.min(columns, card.span ?? 1);
    if (c + span > columns) {
      y += rowH + gap;
      c = 0;
      rowH = 0;
    }
    boxes.push({ x: GUTTER + c * (col + gap), y, w: span * col + (span - 1) * gap, h: card.h });
    c += span;
    rowH = Math.max(rowH, card.h);
  }
  return boxes;
}

// Test 2: Freeze layout's places contain the 1280 px boxes, never overlap,
// and keep the arrangement (each card's row and column order).
test('Freeze layout rounds each card outward to whole units: its box contained, no overlap, the arrangement kept', () => {
  const layouts = [
    // The studio: wide cards one under another.
    [{ span: 4, h: 97.5 }, { span: 4, h: 745.3 }, { span: 4, h: 410.2 }, { span: 4, h: 180 }, { span: 4, h: 260.7 }],
    // kick: three cards, then a wide scope.
    [{ h: 100.4 }, { h: 180.2 }, { h: 300.1 }, { span: 4, h: 210 }],
    // Uneven heights over several rows.
    Array.from({ length: 11 }, (_, i) => ({ h: 90 + ((i * 37.3) % 120), span: i % 5 === 4 ? 2 : 1 })),
  ];
  for (const cards of layouts) {
    const boxes = gridBoxes(cards);
    const { plate, places } = freeze(boxes);
    assert.equal(plate.w, REFERENCE_PU);
    places.forEach((p, i) => {
      assert.ok(contains(drawnBox(p), boxes[i]), `card ${i}: ${placeText(p)} holds its box`);
      assert.ok(p.x + p.w <= plate.w && p.y + p.h <= plate.h, 'on the plate');
      for (const q of places.slice(i + 1)) assert.ok(!overlap(p, q), `${placeText(p)} and ${placeText(q)}`);
    });
    const order = (list) => list.map((b, i) => [b.y, b.x, i]).sort((a, b) => a[0] - b[0] || a[1] - b[1]).map((e) => e[2]);
    assert.deepEqual(order(places), order(boxes), 'the same arrangement');
    assert.deepEqual(freeze(boxes), freeze(boxes), 'a function of the boxes');
  }
  assert.deepEqual(freezeBox({ x: GUTTER, y: GUTTER, w: 1280 - 2 * GUTTER, h: 92 }), { x: 0, y: 0, w: 160, h: 13 }, 'a wide card spans the plate');
});

// Test 4: the generated default panel's packing is a pure function.
test('the default panel is packed on shelves 160 units wide: the same describe, the same plate, no overlap', () => {
  const describe = {
    controls: [
      { instance: 'osc', module: 'std.sine_osc', address: '/osc.freq', parameter: 'freq', unit: 'Hz', min: 20, max: 2000, ports: [] },
      { instance: 'osc', module: 'std.sine_osc', address: '/osc.amp', parameter: 'amp', unit: null, min: 0, max: 1, ports: [] },
      ...Array.from({ length: 20 }, (_, i) => ({ instance: 'bank', module: 'lib.bank', address: `/bank.k${i}`, parameter: `k${i}`, unit: null, min: 0, max: 1, ports: [] })),
      { instance: 'out', module: 'std.audio_out', address: '/out.gain', parameter: 'gain', unit: 'dB', min: -60, max: 6, ports: [] },
    ],
    instances: [{ address: '/out', ports: [{ name: 'l', address: '/out.l', direction: 'in', shape: 'signal' }, { name: 'r', address: '/out.r', direction: 'in', shape: 'signal' }] }],
    sinks: [{ address: '/out', kind: 'audio' }],
  };
  const spec = defaultPanel(describe);
  const a = defaultPlate(spec);
  // The same description with every object's keys in another order.
  const shuffled = JSON.parse(JSON.stringify(describe), (k, v) => (v && typeof v === 'object' && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).reverse()) : v));
  assert.deepEqual(defaultPlate(defaultPanel(shuffled)), a);
  assert.equal(a.places.length, 4, 'osc, bank, out and the output card');
  assert.equal(a.plate.w, 160);
  a.places.forEach((p, i) => {
    assert.ok(p.x + p.w <= 160 && p.y + p.h <= a.plate.h);
    for (const q of a.places.slice(i + 1)) assert.ok(!overlap(p, q));
  });
  assert.ok(a.places[1].w >= 150, 'twenty knobs take a shelf of their own, wrapped');
  assert.ok(a.places[1].h > cardSize(['signal-knob']).h, 'in two rows');
  assert.deepEqual(a.places[3], { x: 0, y: a.places[3].y, w: 160, h: a.places[3].h }, 'the output card is wide');
  // Cards of a shelf share its height.
  assert.equal(a.places[2].y, a.places[1].y + a.places[1].h);
  assert.deepEqual(pack([{ w: 40, h: 10 }, { w: 40, h: 20 }, { w: 100, h: 5 }]).places, [
    { x: 0, y: 0, w: 40, h: 20 },
    { x: 40, y: 0, w: 40, h: 20 },
    { x: 0, y: 20, w: 100, h: 5 },
  ]);
  // The first write writes the places with it: the generated tree carries them.
  const tree = generatedTree(spec);
  assert.match(tree.plate, /^160 \d+$/);
  assert.ok(plateCards(tree).every((c) => c.place));
});

test('the editor moves and resizes on the grid: snapped, refused over another card or beyond the plate', () => {
  const plate = { w: 160, h: 60 };
  const a = { x: 0, y: 0, w: 80, h: 20 };
  const b = { x: 80, y: 0, w: 80, h: 20 };
  assert.equal(snap(13, 1), 2);
  assert.equal(snap(13, 0.5), 3);
  assert.deepEqual(moved(a, 3, 50, plate), { x: 3, y: 40, w: 80, h: 20 }, 'kept on the plate');
  assert.equal(blocked(moved(a, 3, 0, plate), [b], plate), 'it would overlap the card at 80 0 80 20');
  assert.equal(blocked({ x: 100, y: 0, w: 80, h: 4 }, [], plate), 'it would reach beyond the plate');
  assert.equal(blocked(moved(a, 0, 20, plate), [b], plate), null);
  assert.deepEqual(resized(a, 'se', 4, 2, plate), { x: 0, y: 0, w: 84, h: 22 });
  assert.deepEqual(resized(a, 'nw', 100, 100, plate), { x: 72, y: 16, w: 8, h: 4 }, 'never under the smallest card');
  assert.deepEqual(resized(b, 'ne', 10, -5, plate), { x: 80, y: 0, w: 80, h: 20 }, 'kept on the plate');
  assert.deepEqual(union([{ x: 8, y: 8, w: 10, h: 10 }, { x: 30, y: 0, w: 5, h: 5 }]), { x: 8, y: 0, w: 27, h: 18 });
  assert.equal(union([]), null);
});

const placedPanel = `
  <div class="signal-cards">
    <section class="signal-card" place="0 0 80 20">
      <h2>a</h2>
    </section>
    <signal-slot bind="/synth" place="80 0 80 20"></signal-slot>
  </div>
`;

test('places are written in the content, the plate beside it; Freeze, Add card and undo on a plate', () => {
  const parsed = parseContent(placedPanel, whitelist);
  assert.equal(parsed.error, undefined, parsed.error);
  const tree = { ...parsed.tree, plate: '160 20' };
  assert.equal(serialize(tree, whitelist), placedPanel, 'canonical: place after class');
  const version = versionText(tree, whitelist);
  assert.equal(version, `plate="160 20"${placedPanel}`);
  assert.deepEqual(splitVersion(version), { plate: '160 20', html: placedPanel });
  assert.deepEqual(splitVersion(placedPanel), { plate: null, html: placedPanel });
  const cards = plateCards(tree);
  assert.deepEqual(cards.map((c) => c.place), [{ x: 0, y: 0, w: 80, h: 20 }, { x: 80, y: 0, w: 80, h: 20 }], 'the open slot card is a card of the plate');
  // A new card goes below the others, the plate grown to hold it.
  const added = addCard(tree, { title: 'new' });
  assert.equal(added.tree.plate, '160 32');
  assert.deepEqual(plateCards(added.tree).at(-1).place, { x: 0, y: 20, w: 40, h: 12 });
  // A move is a place.
  const slot = cards[1].node.id;
  const movedTree = setPlace(tree, slot, { x: 80, y: 4, w: 80, h: 16 }).tree;
  assert.match(serialize(movedTree, whitelist), /<signal-slot bind="\/synth" place="80 4 80 16"><\/signal-slot>/);
  // Freeze of an unplaced panel: its places and plate; the version before has neither.
  const bare = parseContent(placedPanel.replace(/ place="[^"]*"/g, ''), whitelist).tree;
  bare.plate = null;
  const before = versionText(bare, whitelist);
  const frozen = setPlaces(bare, { w: 160, h: 20 }, plateCards(bare).map((c, i) => [c.node.id, cards[i].place])).tree;
  assert.equal(versionText(frozen, whitelist), version);
  assert.equal(splitVersion(before).plate, null);
  // The session sends the plate with every write (null removes it), so an undo writes the file back.
  const session = new EditSession(() => ({ error: 'unused' }));
  session.enter({ file: 'main.html', base: 'b0' }, before);
  assert.deepEqual(session.request(version).message, { type: 'panel.write', file: 'main.html', html: placedPanel, plate: '160 20', base: 'b0' });
  session.written({ base: 'b1' }, version, 0);
  const undone = session.history.undo();
  assert.equal(undone, before);
  assert.deepEqual(session.request(undone).message.plate, null);
});

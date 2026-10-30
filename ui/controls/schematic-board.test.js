// ADR 0047 §5 (phase 2): the device view's controller in <signal-schematic> (ui/controls/schematic-board.js), without a DOM.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { BOARD_CSS, PLATE_FACE_CSS, SchematicBoard } from './schematic-board.js';
import { LAYER_OPACITY, fadeSteps } from '../lib/board-view.js';

function host() {
  const h = { events: [], relaid: 0, redrawn: 0, plateClass: null };
  h.dispatchEvent = (e) => h.events.push(e);
  h.relayoutTop = () => h.relaid++;
  h.redrawAll = () => h.redrawn++;
  h.faces = { setPlateClass: (c) => (h.plateClass = c) };
  return h;
}

test('a layer or a kind switched: drawn, laid out never, told to the window', () => {
  const h = host();
  const board = new SchematicBoard(h);
  assert.equal(board.state.layer, 'board');
  assert.equal(board.covered, false);
  board.set({ layer: 'plate' });
  assert.equal(board.covered, true, 'the plate covers the board: a press pans');
  assert.equal(h.relaid, 0);
  assert.equal(h.plateClass, ' plate layer-plate', 'no animation class: the layer fades by a transition');
  assert.equal(h.events.at(-1).type, 'signal-view-change');
  assert.deepEqual(h.events.at(-1).detail, { layer: 'plate', hidden: [] });
  assert.equal(board.option().arrange, undefined, 'one arrangement');
  const key = board.key;
  board.set({ hidden: ['events'] });
  assert.equal(h.relaid, 0, 'a kind moves nothing');
  assert.notEqual(board.key, key, 'but draws again');
  assert.ok(board.hiddenKind('events'));
  const told = h.events.length;
  board.set({ hidden: ['events'] });
  assert.equal(h.events.length, told, 'no change, nothing said');
  board.restore({ layer: 'xray' });
  assert.equal(board.state.layer, 'xray');
  assert.equal(h.events.length, told, 'the window’s kept state is not said back');
  board.stepLayer();
  assert.equal(board.state.layer, 'board');
});

test('the plate face keeps only its controls in X-ray and Board; its slot cards go with the cards', () => {
  assert.match(PLATE_FACE_CSS, /:host\(\.plate\.layer-xray\) \.face-panel \[bind\]/);
  assert.match(PLATE_FACE_CSS, /prefers-reduced-motion/);
});

test('a layer step fades monotonically (the owner: "from X-ray to Board the plate flashes in, then vanishes"): transitions only, no keyframes', () => {
  assert.doesNotMatch(PLATE_FACE_CSS + BOARD_CSS, /@keyframes|animation:/);
  assert.match(PLATE_FACE_CSS, /:host\(\.plate\) \{ transition: opacity/);
  assert.match(PLATE_FACE_CSS, /:host\(\.plate\.layer-board\) \{ opacity: 0;/);
  // Every step between layers, for every part: from the opacity shown to the target, never past either.
  for (const from of Object.keys(LAYER_OPACITY)) {
    for (const to of Object.keys(LAYER_OPACITY)) {
      for (const part of ['controls', 'cards', 'ghosts', 'board']) {
        const steps = fadeSteps(from, to, part);
        const up = steps.at(-1) >= steps[0];
        for (let i = 1; i < steps.length; i++) assert.ok(up ? steps[i] >= steps[i - 1] : steps[i] <= steps[i - 1], `${from} → ${to}, ${part}`);
      }
    }
  }
  assert.deepEqual(fadeSteps('xray', 'board', 'controls'), [1, 0.75, 0.5, 0.25, 0]);
  assert.deepEqual(fadeSteps('xray', 'board', 'cards'), [0, 0, 0, 0, 0], 'the cards stay hidden from X-ray to Board');
});

test('from X-ray to Board nothing moves and nothing shows that X-ray hid (the owner, 2026-10-30: "a jump"): the cards hidden in both, no layout, no measure', () => {
  // The CSS: the face panel's cards are hidden on X-ray and on Board alike, only the controls visible (they fade with the plate).
  for (const layer of ['xray', 'board']) {
    assert.match(PLATE_FACE_CSS, new RegExp(`:host\\(\\.plate\\.layer-${layer}\\) \\.face-panel[,{ ][^}]*visibility: hidden`), `${layer}: the cards hidden`);
    assert.match(PLATE_FACE_CSS, new RegExp(`:host\\(\\.plate\\.layer-${layer}\\) \\.face-panel signal-slot\\[bind\\]`), `${layer}: a slot's card hidden`);
  }
  // The controller: a layer step draws again; it never lays out, measures the plate or moves the view.
  const h = host();
  let measured = 0;
  h.faces.faces = new Map();
  const board = new SchematicBoard(h);
  board.measurePlate = () => measured++;
  board.restore({ layer: 'xray' });
  const option = JSON.stringify([...board.option().vias]);
  const key = board.plate.key;
  board.stepLayer();
  assert.equal(board.state.layer, 'board');
  assert.equal(h.relaid, 0, 'no layout');
  assert.equal(measured, 0, 'no measure');
  assert.equal(board.plate.key, key);
  assert.equal(JSON.stringify([...board.option().vias]), option, 'the vias as they were');
  assert.equal(h.redrawn, 2, 'drawn again (restore and the step), opacity only');
});

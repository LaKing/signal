// ADR 0047 §5 (phase 2): the device view's controller in <signal-schematic> (ui/controls/schematic-board.js), without a DOM.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { PLATE_FACE_CSS, SchematicBoard } from './schematic-board.js';

function host() {
  const h = { events: [], relaid: 0, redrawn: 0, plateClass: null };
  h.dispatchEvent = (e) => h.events.push(e);
  h.relayoutTop = () => h.relaid++;
  h.redrawAll = () => h.redrawn++;
  h.faces = { setPlateClass: (c) => (h.plateClass = c) };
  return h;
}

test('a layer, an arrangement or a kind switched: drawn, laid out only for the arrangement, told to the window', () => {
  const h = host();
  const board = new SchematicBoard(h);
  assert.equal(board.state.layer, 'board');
  assert.equal(board.covered, false);
  board.set({ layer: 'plate' });
  assert.equal(board.covered, true, 'the plate covers the board: a press pans');
  assert.equal(h.relaid, 0);
  assert.match(h.plateClass, / plate layer-plate lifting/);
  assert.equal(h.events.at(-1).type, 'signal-view-change');
  assert.deepEqual(h.events.at(-1).detail, { layer: 'plate', arrange: 'controls', hidden: [] });
  board.set({ arrange: 'flow' });
  assert.equal(h.relaid, 1, 'the arrangement is the layout’s only view input');
  assert.equal(board.option().arrange, 'flow');
  const key = board.key;
  board.set({ hidden: ['events'] });
  assert.equal(h.relaid, 1, 'a kind moves nothing');
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

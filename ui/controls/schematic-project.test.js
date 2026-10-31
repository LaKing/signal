// <signal-schematic> switching projects (review 2026-10-31, P2): another
// project is told by `graph.projectId` (its folder, opaque), never by its
// name; its switch lets go of everything the last one's view held (the
// open chips, a move still being written, the selection, the trace, the
// cached layouts and the board's memo, the faces and the plate's measured
// geometry), and a reply of the last project's that arrives late (its
// panel read) is ignored. A project without a panel has an empty plate.

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSchematic } = await import('./schematic.js');
const { SchematicBoard } = await import('./schematic-board.js');

const top = { instances: [{ name: 'osc', module: 'osc', ports: [], parameters: [] }], connections: [] };

/** A schematic without a DOM: `setGraph` and `readPanel` real, the drawing left out; each panel read answered by hand. */
function schematic() {
  const s = Object.create(SignalSchematic.prototype);
  const reads = [];
  Object.assign(s, {
    graph: null,
    staleBox: { style: {} },
    svg: { classList: { remove() {}, toggle() {} } },
    layouts: new Map(),
    moved: new Map(),
    open: new Set(),
    pinned: new Set(),
    selected: [],
    tracing: null,
    zooming: null,
    view: { x: 0, y: 0, k: 1 },
    fitted: true,
    panelText: null,
    panelControls: [],
    editor: { graphSeen() {} },
    connection: {
      request: (message) =>
        new Promise((resolve) => {
          reads.push({ message, resolve });
        }),
    },
  });
  s.faces = { cleared: 0, clear() { this.cleared++; }, pageSeen() {}, faces: new Map() };
  s.board = new SchematicBoard(s);
  for (const name of ['say', 'showSide', 'schedule', 'closeMenu', 'retrace']) s[name] = () => {};
  s.layoutOf = () => null;
  return { s, reads };
}

test('another project of the same name (another projectId) lets go of the last one’s view, plate geometry and memo; its late panel read is ignored', async () => {
  const { s, reads } = schematic();
  s.setGraph({ project: 'demo', projectId: 'A', top, layout: { positions: {} } });
  // Project A's view: a chip open, a move being written, a selection, its plate measured, its board memo.
  s.open = new Set(['/osc']);
  s.pinned = new Set(['/osc']);
  s.moved.set('', new Map([['osc', { x: 40, y: 40, pending: true }]]));
  s.selected = [{ kind: 'instance', path: '', address: '/osc' }];
  s.board.plate = { size: { w: 1280, h: 800 }, vias: new Map([['/osc.freq', {}]]), obstacles: new Array(63).fill({ x: 0, y: 0, w: 8, h: 8 }), cards: new Array(5).fill({ x: 0, y: 0, w: 100, h: 100 }), key: 'A-plate' };
  s.lastLaid = new Map([['', { key: 'A' }]]);
  s.dieOrders = new Map([['voice', ['a']]]);
  s.panelText = '<signal-knob bind="/osc.freq"></signal-knob>';
  const clears = s.faces.cleared;
  const lateA = reads.at(-1);
  // Project B: the same name, another folder, no panel file.
  s.setGraph({ project: 'demo', projectId: 'B', top, layout: { positions: {} } });
  assert.equal(s.open.size, 0, 'no chip open');
  assert.equal(s.pinned.size, 0);
  assert.equal(s.moved.size, 0, 'no pending move');
  assert.deepEqual(s.selected, [], 'nothing selected');
  assert.equal(s.fitted, false, 'fitted again');
  assert.equal(s.faces.cleared, clears + 1, 'the faces let go');
  assert.equal(s.lastLaid.size, 0, 'the board memo let go');
  assert.equal(s.dieOrders.size, 0);
  assert.equal(s.board.plate.obstacles.length, 0, 'no obstacle of the last plate');
  assert.equal(s.board.plate.cards.length, 0, 'no card of the last plate');
  assert.equal(s.board.plate.size, null, 'no plate size');
  assert.equal(s.board.plate.vias.size, 0);
  assert.equal(s.panelText, null);
  // A's panel read answers late: ignored. B's says it has no panel: an empty plate.
  lateA.resolve({ exists: true, html: '<signal-knob bind="/osc.freq"></signal-knob>' });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(s.panelText, null, 'the last project’s late read is ignored');
  reads.at(-1).resolve({ exists: false });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(s.panelText, '');
  assert.equal(s.board.plate.obstacles.length, 0);
  assert.deepEqual(s.board.option(), { vias: new Map(), obstacles: [], plate: null, cards: [] }, 'the top level laid out on an empty plate');
});

test('the same project again (same projectId) keeps its view; a renamed project is the same project', () => {
  const { s } = schematic();
  s.setGraph({ project: 'demo', projectId: 'A', top, layout: { positions: {} } });
  s.open = new Set(['/osc']);
  s.board.plate = { size: { w: 1280, h: 800 }, vias: new Map(), obstacles: [{ x: 0, y: 0, w: 8, h: 8 }], cards: [], key: 'k' };
  s.setGraph({ project: 'demo renamed', projectId: 'A', top, layout: { positions: {} } });
  assert.equal(s.open.size, 1);
  assert.equal(s.board.plate.obstacles.length, 1);
});

test('the panel file removed within a project: the plate goes empty', () => {
  const { s } = schematic();
  s.graph = { project: 'demo', projectId: 'A', top };
  s.panelText = '<signal-knob bind="/osc.freq"></signal-knob>';
  s.board.plate = { size: { w: 1280, h: 800 }, vias: new Map(), obstacles: [{ x: 0, y: 0, w: 8, h: 8 }], cards: [{ x: 0, y: 0, w: 1, h: 1 }], key: 'k' };
  s.panelSeen('');
  assert.equal(s.board.plate.obstacles.length, 0);
  assert.equal(s.board.plate.cards.length, 0);
});

// ADR 0047 §2: a grid of cards laid out on its plate (ui/controls/plate.js),
// with the least of a DOM it needs: custom properties and data attributes
// only, a card without a place below the placed ones.
import test from 'node:test';
import assert from 'node:assert/strict';
import { layOutCards, overflowing } from './plate.js';

function element(attributes = {}, { offsetHeight = 0, scrollHeight = 0, clientHeight = 0 } = {}) {
  const props = new Map();
  const attrs = new Map(Object.entries(attributes));
  const dataset = {};
  return {
    children: [],
    dataset,
    props,
    offsetHeight,
    scrollHeight,
    clientHeight,
    style: { setProperty: (n, v) => props.set(n, v), removeProperty: (n) => props.delete(n) },
    getAttribute: (n) => attrs.get(n) ?? null,
    hasAttribute: (n) => attrs.has(n) || (n.startsWith('data-') && n.slice(5).replace(/-./g, (m) => m[1].toUpperCase()) in dataset),
  };
}

test('the cards go to their places, an unplaced one below; without a plate the grid flows', () => {
  const grid = element();
  const a = element({ place: '0 0 80 20' });
  const b = element({ place: '80 0 80 30' });
  const loose = element({}, { offsetHeight: 100 });
  grid.children.push(a, b, loose);
  const size = layOutCards(grid, '160 30');
  assert.equal('plate' in grid.dataset, true);
  assert.equal(grid.props.get('--signal-plate-w'), '160');
  assert.deepEqual(['x', 'y', 'w', 'h'].map((k) => b.props.get(`--signal-place-${k}`)), ['80', '0', '80', '30']);
  assert.equal('placed' in a.dataset, true);
  // 100 px tall and the gutter: 14 units, below the bottom (30); its height is its content's.
  assert.deepEqual(['x', 'y', 'w', 'h'].map((k) => loose.props.get(`--signal-place-${k}`)), ['0', '30', '160', undefined]);
  assert.equal('unplaced' in loose.dataset, true);
  assert.deepEqual(size, { w: 1280, h: 44 * 8, unplaced: [loose] });
  // The plate gone: everything as written again.
  assert.equal(layOutCards(grid, null), null);
  assert.equal('plate' in grid.dataset, false);
  assert.equal(a.props.size + b.props.size + loose.props.size, 0);
  assert.equal('placed' in a.dataset, false);
});

test('a placed card whose content is taller than its card is found, with the height it needs', () => {
  const grid = element();
  const fits = element({ place: '0 0 80 20' }, { scrollHeight: 152, clientHeight: 152 });
  const tall = element({ place: '80 0 80 20' }, { scrollHeight: 300, clientHeight: 152 });
  grid.children.push(fits, tall);
  layOutCards(grid, '160 20');
  const found = overflowing({ ...grid, hasAttribute: (n) => n === 'data-plate' });
  assert.deepEqual(found.map((f) => [f.card, f.need]), [[tall, 39]]);
});

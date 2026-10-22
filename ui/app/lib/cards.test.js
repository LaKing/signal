import test from 'node:test';
import assert from 'node:assert/strict';
import { appChannel, applyCards, arrangement, cardIds, dropBefore, isCardId, slug } from './cards.js';

// ADR 0032: a card's id is its `id`, else its heading's slug, made unique.
test('cards get stable ids from their id or heading', () => {
  assert.equal(slug('Kick drum'), 'kick-drum');
  assert.equal(slug('  Tape échő!  '), 'tape-echo');
  assert.equal(slug(''), 'card');
  assert.equal(slug('—'), 'card');
  assert.deepEqual(
    cardIds([
      { id: '', title: 'kick' },
      { id: '', title: 'kick' },
      { id: 'out', title: 'output' },
      { id: 'out', title: 'output' },
      { id: 'kick-2', title: 'x' },
      { id: 'bad id', title: '' },
    ]),
    ['kick', 'kick-3', 'out', 'output', 'kick-2', 'card'],
  );
  assert.ok(isCardId('kick-2') && isCardId('a:b.c_d'));
  assert.ok(!isCardId('a b') && !isCardId('') && !isCardId(7) && !isCardId('x'.repeat(129)));
});

test('a panel window shows its cards in its order; the main window the rest', () => {
  const ids = ['intro', 'kick', 'output', 'scope'];
  assert.deepEqual(arrangement(ids, { view: 'panel', cards: ['scope', 'kick'] }), [
    { id: 'intro', shown: false, order: 2 },
    { id: 'kick', shown: true, order: 1 },
    { id: 'output', shown: false, order: 4 },
    { id: 'scope', shown: true, order: 0 },
  ]);
  const main = arrangement(ids, { view: 'main', held: ['scope', 'kick'], order: ['output'] });
  assert.deepEqual(main.filter((c) => c.shown).map((c) => c.id), ['intro', 'output']);
  assert.equal(main.find((c) => c.id === 'output').order, 0, 'its order first');
  assert.deepEqual(arrangement(ids, null).map((c) => c.shown), [true, true, true, true], 'untold: all');
  assert.deepEqual(arrangement(ids, { view: 'panel' }).map((c) => c.shown), [false, false, false, false], 'an empty panel window');
});

test('a dropped card goes before the card it is dropped on, or the next', () => {
  const boxes = [
    { id: 'a', left: 0, top: 0, right: 100, bottom: 50 },
    { id: 'b', left: 110, top: 0, right: 210, bottom: 50 },
    { id: 'c', left: 0, top: 60, right: 100, bottom: 110 },
  ];
  assert.equal(dropBefore(boxes, { x: 10, y: 10 }), 'a');
  assert.equal(dropBefore(boxes, { x: 90, y: 10 }), 'b', 'its second half: after it');
  assert.equal(dropBefore(boxes, { x: 200, y: 10 }), 'c');
  assert.equal(dropBefore(boxes, { x: 90, y: 100 }), null, 'after the last');
  assert.equal(dropBefore(boxes, { x: 300, y: 55 }), 'c', 'between rows: the next row');
  assert.equal(dropBefore(boxes, { x: 10, y: 500 }), null, 'below them all: the end');
  assert.equal(dropBefore([], { x: 0, y: 0 }), null);
});

function fakeCard(id, title) {
  const h = { tagName: 'H2', textContent: title };
  return { id, children: [h], style: { display: '', order: '' }, parentElement: { closest: () => null } };
}

test('cards are shown and ordered with CSS only', () => {
  const cards = [fakeCard('', 'kick'), fakeCard('', 'output'), fakeCard('scope', 'scope')];
  const doc = { querySelectorAll: (q) => (q === '.signal-card' ? cards : []) };
  applyCards(doc, { view: 'panel', cards: ['scope', 'kick'] });
  assert.deepEqual(cards.map((c) => c.style.display), ['', 'none', '']);
  assert.deepEqual(cards.map((c) => c.style.order), ['1', '3', '0']);
  applyCards(doc, null);
  assert.deepEqual(cards.map((c) => c.style.display), ['', '', '']);
  assert.deepEqual(cards.map((c) => c.style.order), ['', '', '']);
});

test('the app channel is wry\'s window.ipc, absent in a browser', () => {
  assert.equal(appChannel({}), null);
  const sent = [];
  const post = appChannel({ ipc: { postMessage: (text) => sent.push(text) } });
  post({ type: 'card.drag', card: 'kick' });
  assert.deepEqual(sent, ['{"type":"card.drag","card":"kick"}']);
});

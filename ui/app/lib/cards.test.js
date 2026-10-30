import test from 'node:test';
import assert from 'node:assert/strict';
import { EMPTY_PANEL, HELD_CSS, HELD_MARK, HIDDEN_MARK, appChannel, applyCards, arrangement, cardClick, cardIds, cardMenu, cardsOf, isCardId, menuMessage, slug } from './cards.js';
import { contextItems } from './context.js';

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

test('a panel window shows the cards it holds; the main window all, the held ones as placeholders; none is reordered (ADR 0047 §3)', () => {
  const ids = ['intro', 'kick', 'output', 'scope'];
  assert.deepEqual(arrangement(ids, { view: 'panel', cards: ['scope', 'kick'] }), [
    { id: 'intro', shown: false, held: null },
    { id: 'kick', shown: true, held: null },
    { id: 'output', shown: false, held: null },
    { id: 'scope', shown: true, held: null },
  ]);
  const main = arrangement(ids, { view: 'main', held: [{ card: 'scope', where: 'Panel window 1 (screen 2)' }, 'kick'] });
  assert.deepEqual(main.map((c) => c.shown), [true, true, true, true], 'nothing disappears');
  assert.deepEqual(main.map((c) => c.held), [null, '', null, 'Panel window 1 (screen 2)']);
  assert.deepEqual(arrangement(ids, null).map((c) => c.shown), [true, true, true, true], 'untold main: all');
  // Owner, 2026-10-22: "the new window is filled too": empty means empty.
  assert.deepEqual(arrangement(ids, EMPTY_PANEL).map((c) => c.shown), [false, false, false, false], 'a new panel window');
  assert.deepEqual(arrangement(ids, { view: 'panel' }).map((c) => c.shown), [false, false, false, false]);
});

// Owner, 2026-10-22: "not drag and drop — the panel elements should be movable from the context menu".
test('the card menu moves a card to each other window, a new one, back to main; no Move up or down (ADR 0047 §3)', () => {
  const targets = [{ index: 0, label: 'Main window' }, { index: 2, label: 'Panel window 2 (screen 3)' }];
  const two = arrangement(['synth', 'rhythm', 'echo'], { view: 'panel', cards: ['rhythm', 'synth'] });
  assert.deepEqual(cardMenu({ view: 'panel', targets }, two, 'rhythm'), [
    { id: 'move:0', label: 'Move to Main window' },
    { id: 'move:2', label: 'Move to Panel window 2 (screen 3)' },
    { id: 'new', label: 'Move to new panel window' },
    { id: 'main', label: 'Bring back to main window' },
  ]);
  const main = arrangement(['title', 'synth', 'rhythm', 'echo'], { view: 'main', held: [{ card: 'synth', where: 'Panel window 1 (screen 2)' }] });
  assert.deepEqual(cardMenu({ view: 'main', targets: [] }, main, 'rhythm').map((i) => i.id), ['new']);
  // What each item sends.
  const size = { width: 1000, height: 400 };
  assert.deepEqual(menuMessage('move:2', 'rhythm', two, size), { type: 'card.move', card: 'rhythm', to: 2 });
  assert.deepEqual(menuMessage('new', 'rhythm', two, size), { type: 'card.move', card: 'rhythm', to: 'new', size });
  assert.deepEqual(menuMessage('main', 'rhythm', two, size), { type: 'card.move', card: 'rhythm', to: 'main', size });
  assert.equal(menuMessage('down', 'rhythm', two, size), null, 'card.order is gone');
  assert.equal(menuMessage('move:x', 'rhythm', two, size), null);
  // In a main window the items come first in the app's menu, as a part.
  const items = contextItems({ part: cardMenu({ view: 'main', targets }, main, 'rhythm') });
  assert.deepEqual(items.slice(0, 3).map((i) => i.id), ['part:move:0', 'part:move:2', 'part:new']);
});

test('a right click is the card\'s on its background or heading, not on a bound control or a text field, nor while edited', () => {
  const card = { localName: 'section' };
  const h2 = { localName: 'h2', tagName: 'H2' };
  assert.equal(cardClick([h2, card], card), true);
  assert.equal(cardClick([card], card), true);
  const knob = { localName: 'signal-knob', binding: { address: '/a.x' } };
  assert.equal(cardClick([knob, card], card), false, 'the control keeps its menu');
  assert.equal(cardClick([{ tagName: 'INPUT', type: 'text' }, card], card), false, 'a text field keeps the system\'s');
  const editing = { localName: 'signal-panel', hasAttribute: (a) => a === 'editing' };
  assert.equal(cardClick([h2, editing, card], card), false);
  assert.equal(cardClick([h2], card), false, 'not in this card');
});

function fakeCard(id, title, rect = { x: 0, y: 0, w: 10, h: 10 }) {
  const h = { tagName: 'H2', textContent: title };
  const props = new Map();
  const attrs = new Set();
  const style = {
    display: '',
    order: '',
    setProperty: (name, value, priority) => (value ? props.set(name, [value, priority]) : props.delete(name)),
    get: (name) => props.get(name),
  };
  return { id, rect, children: [h], style, props, attrs, setAttribute: (a) => attrs.add(a), removeAttribute: (a) => attrs.delete(a), hasAttribute: (a) => attrs.has(a), parentElement: { closest: () => null } };
}

/** A panel document of `cards`, its <signal-panel> recording the crop it is given. */
function fakeDoc(cards) {
  const panel = { crops: [], setCrop: (r) => panel.crops.push(r), plateRect: (e) => e.rect };
  return { panel, querySelector: (q) => (q === 'signal-panel' ? panel : null), querySelectorAll: (q) => (q.startsWith('.signal-card') ? cards : []) };
}

test('a panel window hides the cards it does not hold, their places kept, and crops the plate to its own (ADR 0047 §3)', () => {
  const cards = [fakeCard('', 'kick', { x: 8, y: 8, w: 300, h: 100 }), fakeCard('', 'output', { x: 320, y: 8, w: 300, h: 100 }), fakeCard('scope', 'scope', { x: 8, y: 120, w: 1264, h: 200 })];
  const doc = fakeDoc(cards);
  const [, placed] = applyCards(doc, { view: 'panel', cards: ['scope', 'kick'] });
  assert.deepEqual(placed.map((p) => p.shown), [true, false, true]);
  assert.deepEqual(cards.map((c) => c.style.display), ['', '', ''], 'no card leaves the flow: nothing on the plate moves');
  assert.deepEqual(cards[1].style.get('visibility'), ['hidden', 'important']);
  assert.ok(cards[1].attrs.has(HIDDEN_MARK) && !cards[0].attrs.has(HIDDEN_MARK));
  assert.deepEqual(cards.map((c) => c.style.order), ['', '', ''], 'never reordered');
  assert.equal(cards[0].props.size, 0);
  assert.deepEqual(doc.panel.crops.at(-1), { x: 8, y: 8, w: 1264, h: 312 }, 'the bounding box of its cards');
  applyCards(doc, { view: 'panel', cards: [] });
  assert.deepEqual(doc.panel.crops.at(-1), { x: 0, y: 0, w: 0, h: 0 }, 'holding nothing: nothing shown, never the whole plate');
  applyCards(doc, { view: 'main', held: [] });
  assert.equal(doc.panel.crops.at(-1), null, 'a main window: the whole plate');
  assert.ok(!cards[1].attrs.has(HIDDEN_MARK));
});

// Owner, 2026-10-22: "after moving the mono synth, some remnant stayed in
// the first window" (its header: the collapsed card's top showed). A held
// card keeps only its place, wholly hidden; its placeholder is drawn over it.
test('a card held elsewhere is wholly hidden in the main window, its place kept for the placeholder', () => {
  const cards = [fakeCard('', 'kick'), fakeCard('', 'MONO SYNTH'), fakeCard('scope', 'scope')];
  const doc = fakeDoc(cards);
  applyCards(doc, { view: 'main', held: [{ card: 'mono-synth', where: 'Panel window 1 (screen 1)' }] });
  assert.deepEqual(cards.map((c) => c.style.display), ['', '', ''], 'its place stays');
  assert.deepEqual(cards[1].style.get('visibility'), ['hidden', 'important']);
  assert.equal(cards[1].style.get('max-height'), undefined, 'ADR 0047: its whole place, not collapsed');
  assert.ok(cards[1].attrs.has(HELD_MARK));
  assert.match(HELD_CSS, /\[data-signal-app-held\] \*::before/, 'its decorations too');
  assert.match(HELD_CSS, /visibility: hidden !important/);
  assert.equal(cards[0].props.size, 0);
  // Brought back: all of it again.
  applyCards(doc, { view: 'main', held: [] });
  assert.equal(cards[1].props.size, 0);
  assert.ok(!cards[1].attrs.has(HELD_MARK));
  applyCards(doc, null);
  assert.deepEqual(cards.map((c) => c.style.display), ['', '', '']);
  assert.deepEqual(cards.map((c) => c.style.order), ['', '', '']);
});

test('the app channel is wry\'s window.ipc, absent in a browser', () => {
  assert.equal(appChannel({}), null);
  const sent = [];
  const post = appChannel({ ipc: { postMessage: (text) => sent.push(text) } });
  post({ type: 'card.move', card: 'kick', to: 0 });
  assert.deepEqual(sent, ['{"type":"card.move","card":"kick","to":0}']);
});

// ADR 0046 §5: an open slot card is a card titled by its slot, laid out by
// its own box; its part's cards are cards of their own, `<slot>:<id>`.
test("an open slot card and its part's cards get their own ids", () => {
  const slot = { localName: 'signal-slot', id: '', children: [], getAttribute: (a) => (a === 'bind' ? '/synth' : null), parentElement: { closest: () => null }, box: { style: {} } };
  slot.cardBox = () => slot.box;
  const inPart = (card) => ({ ...card, parentElement: { closest: (q) => (q === 'signal-slot' ? slot : null) } });
  const elements = [fakeCard('', 'kick'), slot, inPart(fakeCard('', 'MONO SYNTH')), inPart(fakeCard('', 'MONO SYNTH')), fakeCard('', 'synth')];
  const doc = { querySelectorAll: (q) => (q.includes('signal-slot') ? elements : []) };
  const cards = cardsOf(doc);
  assert.deepEqual(cards.map((c) => c.id), ['kick', 'slot.synth', 'synth:mono-synth', 'synth:mono-synth-2', 'synth'], 'the slot card’s id shifts no heading’s');
  assert.equal(cards[1].element, slot.box, 'laid out by its own box');
  assert.equal(cards[1].title, 'synth');
  assert.ok(cards.every((c) => isCardId(c.id)));
});

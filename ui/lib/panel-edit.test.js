// M2.9-2 (docs/ui-controls.md "Editing a panel", ADR 0026 §4): every editor
// operation as a pure function on a tree, the two-step Remove card, the
// palette from `describe`, the history of written versions, drops and keys.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent, serialize, shape } from './panel-tree.js';
import {
  EditSession,
  History,
  addCard,
  addControl,
  cardIsEmpty,
  describeCard,
  editKey,
  editLabels,
  find,
  insertionIndex,
  makeGroup,
  moveCard,
  moveControl,
  newControl,
  paletteEntries,
  removeCard,
  removeCardStep,
  removeControl,
  renameCard,
  searchPalette,
  setAttributes,
  setScopeWide,
  setWide,
  ungroup,
} from './panel-edit.js';

const whitelist = JSON.parse(readFileSync(new URL('./panel-whitelist.json', import.meta.url), 'utf8'));

const PANEL = `
<div class="signal-cards">
  <section class="signal-card"><h1>title</h1><p>about</p></section>
  <section class="signal-card"><h2>a</h2><div class="row"><signal-knob bind="/a.x" label="x"></signal-knob><signal-knob bind="/a.y" label="y"></signal-knob><signal-knob bind="/a.z" label="z"></signal-knob></div></section>
  <section class="signal-card"><h2>out</h2><div class="row"><signal-fader bind="/out.gain" label="gain"></signal-fader><div class="signal-group"><signal-meter bind="/out.l" label="L"></signal-meter><signal-meter bind="/out.r" label="R"></signal-meter></div></div></section>
  <section class="signal-card"><h2>scope</h2><signal-scope bind="/a.out" label="a"></signal-scope></section>
</div>`;

const tree = () => parseContent(PANEL, whitelist).tree;
const card = (inner) => `\n  <div class="signal-cards">\n    <section class="signal-card">${inner}</section>\n  </div>\n`;
/** The id of the node matching `test` (by document order). */
function id(t, test) {
  const all = [];
  const walk = (n) => {
    for (const c of n.children ?? []) {
      all.push(c);
      walk(c);
    }
  };
  walk(t);
  return all.find(test).id;
}
const control = (t, bind) => id(t, (n) => n.kind === 'control' && n.attributes.bind === bind);
const cardNamed = (t, name) => id(t, (n) => n.kind === 'card' && n.children.some((c) => c.text === name));
const binds = (node) => (node.children ?? []).map((c) => (c.kind === 'control' ? c.attributes.bind : c.kind));
const text = (t) => serialize(t, whitelist);
const ok = (result) => {
  assert.equal(result.error, undefined, result.error);
  return result.tree;
};

test('operations never change their argument', () => {
  const t = tree();
  const before = JSON.stringify(t);
  ok(removeCard(t, cardNamed(t, 'a')));
  ok(moveControl(t, control(t, '/a.x'), { into: cardNamed(t, 'out') }));
  ok(setWide(t, cardNamed(t, 'a'), true));
  assert.equal(JSON.stringify(t), before);
});

test('cards: move in the grid order, wide, rename, describe, add, remove', () => {
  const t = tree();
  const grid = t.children[0].id;
  const names = (x) => x.children[0].children.map((c) => c.children.find((h) => h.kind !== 'p')?.text ?? c.children[0]?.kind);
  // Drag the scope card to the front, then the title card to the end.
  let m = ok(moveCard(t, cardNamed(t, 'scope'), { cards: grid, index: 0 }));
  assert.deepEqual(names(m), ['scope', 'title', 'a', 'out']);
  m = ok(moveCard(m, cardNamed(m, 'title'), { cards: grid, index: 4 }));
  assert.deepEqual(names(m), ['scope', 'a', 'out', 'title']);
  // Moving a card before the one after it changes nothing.
  assert.deepEqual(names(ok(moveCard(t, cardNamed(t, 'a'), { cards: grid, index: 2 }))), ['title', 'a', 'out', 'scope']);
  assert.match(text(ok(moveCard(t, cardNamed(t, 'a'), { cards: grid, index: 3 }))), /<h2>out<\/h2>[\s\S]*<h2>a<\/h2>[\s\S]*<h2>scope<\/h2>/);

  const wide = ok(setWide(t, cardNamed(t, 'a'), true));
  assert.match(text(wide), /<section class="signal-card wide">\n\s+<h2>a<\/h2>/);
  assert.doesNotMatch(text(ok(setWide(wide, cardNamed(wide, 'a'), false))), /wide/);

  assert.match(text(ok(renameCard(t, cardNamed(t, 'a'), 'drums'))), /<h2>drums<\/h2>/);
  assert.match(text(ok(renameCard(t, cardNamed(t, 'title'), 'kick'))), /<h1>kick<\/h1>/, 'a title card keeps its h1');

  assert.match(text(ok(describeCard(t, cardNamed(t, 'a'), 'the kick'))), /<h2>a<\/h2>\n\s+<p>the kick<\/p>/);
  assert.match(text(ok(describeCard(t, cardNamed(t, 'title'), 'new'))), /<p>new<\/p>/);
  assert.doesNotMatch(text(ok(describeCard(t, cardNamed(t, 'title'), ''))), /<p>/);

  const added = addCard(t, { cards: grid, index: 1, title: 'new' });
  assert.equal(added.error, undefined);
  assert.deepEqual(names(added.tree), ['title', 'new', 'a', 'out', 'scope']);
  assert.match(text(added.tree), /<section class="signal-card">\n\s+<h2>new<\/h2>\n\s+<div class="row"><\/div>\n\s+<\/section>/);
  assert.ok(cardIsEmpty(added.tree, added.id));
  // A panel without a grid gets one.
  assert.match(text(addCard({ kind: 'panel', next: 1, children: [] }).tree), /signal-cards/);

  assert.deepEqual(names(ok(removeCard(t, cardNamed(t, 'out')))), ['title', 'a', 'scope']);
  assert.ok(moveCard(t, 'nope', { cards: grid, index: 0 }).error);
});

test('removing a card that is not empty asks first, in two steps', () => {
  const t = tree();
  const empty = addCard(t, { title: 'blank' });
  // An empty card goes at once.
  assert.deepEqual(removeCardStep(null, empty.tree, empty.id), { action: 'remove', pending: null });
  // A card with controls asks, then goes on the second click.
  const a = cardNamed(t, 'a');
  const first = removeCardStep(null, t, a);
  assert.deepEqual(first, { action: 'confirm', pending: a });
  assert.deepEqual(removeCardStep(first.pending, t, a), { action: 'remove', pending: null });
  assert.ok(!find(ok(removeCard(t, a)), a));
  // Asked about card a, a click for card out asks again.
  const out = cardNamed(t, 'out');
  assert.deepEqual(removeCardStep(first.pending, t, out), { action: 'confirm', pending: out });
  // After another operation the editor sets pending back to null: asked again.
  assert.equal(removeCardStep(null, t, a).action, 'confirm');
});

test('controls: within a row, into another card, into and out of a group, group, ungroup, remove', () => {
  const t = tree();
  const rowA = find(t, control(t, '/a.x')).parent.id;
  // Within a row: x to the end, z to the front.
  let m = ok(moveControl(t, control(t, '/a.x'), { into: rowA }));
  assert.deepEqual(binds(find(m, rowA).node), ['/a.y', '/a.z', '/a.x']);
  m = ok(moveControl(t, control(t, '/a.z'), { into: rowA, index: 0 }));
  assert.deepEqual(binds(find(m, rowA).node), ['/a.z', '/a.x', '/a.y']);
  assert.deepEqual(binds(find(ok(moveControl(t, control(t, '/a.x'), { into: rowA, index: 1 })), rowA).node), ['/a.x', '/a.y', '/a.z'], 'before itself: unmoved');
  // Into another card's row, and into a card without a row (one is made).
  const rowOut = find(t, control(t, '/out.gain')).parent.id;
  m = ok(moveControl(t, control(t, '/a.y'), { into: rowOut, index: 1 }));
  assert.deepEqual(binds(find(m, rowOut).node), ['/out.gain', '/a.y', 'group']);
  m = ok(moveControl(t, control(t, '/a.y'), { into: cardNamed(t, 'scope') }));
  assert.match(text(m), /<signal-scope bind="\/a\.out" label="a"><\/signal-scope>\n\s+<div class="row">\n\s+<signal-knob bind="\/a\.y" label="y">/);
  // Into a group and out of it.
  const group = find(t, control(t, '/out.l')).parent.id;
  m = ok(moveControl(t, control(t, '/out.gain'), { into: group, index: 0 }));
  assert.deepEqual(binds(find(m, group).node), ['/out.gain', '/out.l', '/out.r']);
  m = ok(moveControl(t, control(t, '/out.r'), { into: rowOut, index: 0 }));
  assert.deepEqual(binds(find(m, rowOut).node), ['/out.r', '/out.gain', 'group']);
  assert.deepEqual(binds(find(m, group).node), ['/out.l']);
  // A group left empty goes.
  m = ok(moveControl(m, control(m, '/out.l'), { into: rowA }));
  assert.equal(find(m, group), null);
  // A whole group moves into a row, never into a group.
  m = ok(moveControl(t, group, { into: rowA, index: 0 }));
  assert.deepEqual(binds(find(m, rowA).node), ['group', '/a.x', '/a.y', '/a.z']);
  assert.ok(moveControl(t, group, { into: group }).error);

  // Group adjacent controls, in their order; not others.
  const grouped = makeGroup(t, [control(t, '/a.z'), control(t, '/a.y')]);
  assert.equal(grouped.error, undefined);
  assert.deepEqual(binds(find(grouped.tree, rowA).node), ['/a.x', 'group']);
  assert.deepEqual(binds(find(grouped.tree, grouped.id).node), ['/a.y', '/a.z']);
  assert.match(makeGroup(t, [control(t, '/a.x'), control(t, '/a.z')]).error, /next to each other/);
  assert.match(makeGroup(t, [control(t, '/a.x'), control(t, '/out.gain')]).error, /one row/);
  assert.match(makeGroup(t, [control(t, '/a.x')]).error, /two/);
  assert.match(makeGroup(t, [control(t, '/out.l'), control(t, '/out.r')]).error, /one row/, 'already grouped');
  // Ungroup puts them back in place.
  assert.deepEqual(binds(find(ok(ungroup(t, group)), rowOut).node), ['/out.gain', '/out.l', '/out.r']);
  assert.ok(ungroup(t, rowOut).error);

  // Remove.
  assert.deepEqual(binds(find(ok(removeControl(t, control(t, '/a.y'))), rowA).node), ['/a.x', '/a.z']);
  m = ok(removeControl(ok(removeControl(t, control(t, '/out.l'))), control(t, '/out.r')));
  assert.equal(find(m, group), null, 'an emptied group goes');
});

test('inspector edits: attributes, and wide for a scope', () => {
  const t = tree();
  const knob = control(t, '/a.x');
  const m = ok(setAttributes(t, knob, { label: 'cutoff', taper: 'log', min: 20, max: '2000', skin: 'svg:assets/knob.svg' }));
  assert.match(text(m), /<signal-knob bind="\/a\.x" taper="log" min="20" max="2000" label="cutoff" skin="svg:assets\/knob\.svg"><\/signal-knob>/);
  assert.match(text(ok(setAttributes(m, knob, { taper: null, skin: undefined }))), /<signal-knob bind="\/a\.x" min="20" max="2000" label="cutoff"><\/signal-knob>/);
  const scope = control(t, '/a.out');
  const wide = ok(setScopeWide(t, scope, true));
  assert.match(text(wide), /<section class="signal-card wide scope">\n\s+<h2>scope<\/h2>/);
  assert.match(text(ok(setScopeWide(wide, scope, false))), /<section class="signal-card">\n\s+<h2>scope<\/h2>/);
});

const DESCRIBE = {
  controls: [
    { address: '/osc.freq', instance: 'osc', parameter: 'freq', ports: ['/osc.freq'], unit: 'Hz', min: 20, max: 20000 },
    { address: '/osc.q', instance: 'osc', parameter: 'q', ports: ['/osc.q'], unit: null, min: 0.1, max: 10 },
    { address: '/out.gain', instance: 'out', parameter: 'gain', ports: ['/out.gain'], unit: null, min: 0, max: 4 },
  ],
  instances: [
    {
      address: '/osc',
      parameters: [
        { address: '/osc.freq', kind: 'frequency', unit: 'Hz', range: { min: 20, max: 20000 } },
        { address: '/osc.q', kind: 'q', unit: null, range: { min: 0.1, max: 10 } },
      ],
      ports: [
        { name: 'out', address: '/osc.out', direction: 'out', shape: 'signal' },
        { name: 'gate', address: '/osc.gate', direction: 'in', shape: 'events' },
      ],
    },
    { address: '/drums/kick/env', parameters: [{ address: '/drums/kick/env.decay_ms', kind: 'time', unit: 'ms', range: { min: 1, max: 2000 } }], ports: [] },
    {
      address: '/out',
      parameters: [{ address: '/out.gain', kind: 'gain', unit: null, range: { min: 0, max: 4 } }],
      ports: [
        { name: 'l', address: '/out.l', direction: 'in', shape: 'signal' },
        { name: 'bodies', address: '/out.v', direction: 'out', shape: 'vector' },
      ],
    },
  ],
  sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 1 }],
};

test('the palette: every parameter and port of describe, searchable, with the kinds that bind it', () => {
  const entries = paletteEntries(DESCRIBE);
  const by = Object.fromEntries(entries.map((e) => [e.address, e]));
  assert.deepEqual(Object.keys(by).sort(), ['/drums/kick/env.decay_ms', '/osc.freq', '/osc.gate', '/osc.out', '/osc.q', '/out.gain', '/out.l', '/out.v']);
  assert.equal(entries.filter((e) => e.address === '/osc.freq').length, 1, 'a control is listed once');
  assert.deepEqual(by['/osc.freq'].kinds, ['knob', 'fader', 'toggle', 'xy', 'value']);
  assert.deepEqual(by['/drums/kick/env.decay_ms'].kinds, ['value']);
  assert.deepEqual(by['/osc.gate'].kinds, ['button']);
  assert.deepEqual(by['/out.l'].kinds, ['meter', 'scope']);
  assert.deepEqual(by['/osc.out'].kinds, ['scope']);
  assert.deepEqual(by['/out.v'].kinds, []);
  assert.deepEqual(searchPalette(entries, 'OSC f').shown.map((e) => e.address), ['/osc.freq']);
  const few = searchPalette(entries, '', 3);
  assert.equal(few.shown.length, 3);
  assert.equal(few.more, entries.length - 3);
});

test('a new control takes its defaults from the parameter, as the generated panel does', () => {
  const entries = paletteEntries(DESCRIBE);
  const by = Object.fromEntries(entries.map((e) => [e.address, e]));
  assert.deepEqual(newControl('knob', [by['/osc.freq']], DESCRIBE), { tag: 'signal-knob', attributes: { bind: '/osc.freq', taper: 'log', min: '20', max: '20000', label: 'freq' } });
  assert.deepEqual(newControl('fader', [by['/out.gain']], DESCRIBE), { tag: 'signal-fader', attributes: { bind: '/out.gain', taper: 'db', min: '0', max: '4', label: 'gain' } });
  assert.deepEqual(newControl('toggle', [by['/osc.q']], DESCRIBE), { tag: 'signal-toggle', attributes: { bind: '/osc.q', min: '0.1', max: '10', label: 'q' } });
  assert.deepEqual(newControl('xy', [by['/osc.freq'], by['/osc.q']], DESCRIBE), { tag: 'signal-xy', attributes: { bind: '/osc.freq /osc.q', taper: 'log', label: 'freq · q' } });
  assert.deepEqual(newControl('value', [by['/drums/kick/env.decay_ms']], DESCRIBE), { tag: 'signal-value', attributes: { bind: '/drums/kick/env.decay_ms', label: 'decay_ms' } });
  assert.deepEqual(newControl('button', [by['/osc.gate']], DESCRIBE), { tag: 'signal-button', attributes: { bind: '/osc.gate', label: 'gate' } });
  assert.deepEqual(newControl('meter', [by['/out.l']], DESCRIBE), { tag: 'signal-meter', attributes: { bind: '/out.l', label: 'l' } });
  assert.deepEqual(newControl('scope', [by['/osc.out']], DESCRIBE), { tag: 'signal-scope', attributes: { bind: '/osc.out', label: 'out' } });
  assert.match(newControl('knob', [by['/osc.out']], DESCRIBE).error, /cannot/);
  assert.match(newControl('xy', [by['/osc.freq']], DESCRIBE).error, /cannot/);
  // Added into a card's row, within the whitelist.
  const t = tree();
  const added = addControl(t, newControl('knob', [by['/osc.freq']], DESCRIBE), { into: cardNamed(t, 'a'), index: 0 });
  assert.equal(added.error, undefined);
  assert.equal(find(added.tree, added.id).node.attributes.bind, '/osc.freq');
  assert.match(text(added.tree), /<div class="row">\n\s+<signal-knob bind="\/osc\.freq" taper="log" min="20" max="20000" label="freq"><\/signal-knob>\n\s+<signal-knob bind="\/a\.x"/);
  assert.equal(parseContent(text(added.tree), whitelist).error, undefined);
});

test('undo and redo go through the written versions', () => {
  const h = new History('v0');
  assert.equal(h.undo(), null);
  h.push('v1');
  h.push('v2');
  assert.equal(h.current, 'v2');
  assert.equal(h.undo(), 'v1');
  h.moved(-1);
  assert.equal(h.undo(), 'v0');
  h.moved(-1);
  assert.equal(h.current, 'v0');
  assert.ok(!h.canUndo);
  assert.equal(h.redo(), 'v1');
  h.moved(1);
  assert.equal(h.current, 'v1');
  // A new write drops what redo would have written.
  h.push('v3');
  assert.ok(!h.canRedo);
  assert.deepEqual(h.versions, ['v0', 'v1', 'v3']);
  assert.equal(h.undo(), 'v1');
});

test('drops land in reading order, and keys', () => {
  const box = (left, top) => ({ left, top, right: left + 10, bottom: top + 10, width: 10, height: 10 });
  const rects = [box(0, 0), box(20, 0), box(0, 20)];
  assert.equal(insertionIndex(rects, { x: 2, y: 5 }), 0);
  assert.equal(insertionIndex(rects, { x: 8, y: 5 }), 1);
  assert.equal(insertionIndex(rects, { x: 40, y: 5 }), 2, 'right of a line: before the next line');
  assert.equal(insertionIndex(rects, { x: 40, y: 25 }), 3);
  assert.equal(insertionIndex([], { x: 0, y: 0 }), 0);
  const key = (key, more = {}) => editKey({ key, target: { localName: 'div' }, ...more });
  assert.equal(key('z', { metaKey: true }), 'undo');
  assert.equal(key('Z', { metaKey: true, shiftKey: true }), 'redo');
  assert.equal(key('Escape'), 'leave');
  assert.equal(key('z'), null);
  assert.equal(key('z', { ctrlKey: true }), null);
  assert.equal(editKey({ key: 'z', metaKey: true, target: { localName: 'input' } }), null, 'a text field keeps its own undo');
  // auditor-1-006 (AC1): Escape leaves from an editor field too, as the
  // page sees it inside the shadow root (composedPath).
  for (const localName of ['input', 'textarea', 'select']) {
    assert.equal(editKey({ key: 'Escape', target: { localName: 'signal-panel' }, composedPath: () => [{ localName }] }), 'leave', localName);
  }
});

test('card numbers follow the grid order after a move', () => {
  const t = tree();
  const labels = (x) => [...editLabels(x).values()].filter((l) => l.startsWith('card '));
  assert.deepEqual(labels(t), ['card 1', 'card 2', 'card 3', 'card 4']);
  const moved = ok(moveCard(t, cardNamed(t, 'scope'), { cards: t.children[0].id, index: 0 }));
  assert.equal(editLabels(moved).get(cardNamed(moved, 'scope')), 'card 1');
  assert.equal(editLabels(moved).get(cardNamed(moved, 'title')), 'card 2');
  assert.equal(editLabels(t).get(control(t, '/out.l')), '/out.l');
  assert.equal(editLabels(t).get(find(t, control(t, '/out.l')).parent.id), 'group');
});

/** A session reading content as the editor does: canonical text, or why not editable. */
function session() {
  return new EditSession((html) => {
    const parsed = parseContent(html, whitelist, { mode: 'read' });
    return parsed.error ? { error: parsed.error } : { tree: parsed.tree, text: serialize(parsed.tree, whitelist) };
  });
}

test('the session history survives leaving and re-entering edit mode (auditor-1-005, AC7)', () => {
  const s = session();
  assert.equal(s.enter({ file: 'main.html', base: 'b0' }, 'v0'), false, 'a first entry starts the history');
  const write = s.request('v1');
  assert.deepEqual(write.message, { type: 'panel.write', file: 'main.html', html: 'v1', base: 'b0' });
  s.written({ base: 'b1' }, 'v1', 0);
  assert.ok(s.history.canUndo);
  // Left and entered again on the file it wrote: the history goes on.
  assert.equal(s.enter({ file: 'main.html', base: 'b1' }, 'v1'), true);
  assert.ok(s.history.canUndo);
  assert.equal(s.history.undo(), 'v0');
  // Undo is written as a step of the same history.
  assert.equal(s.request('v0').message.base, 'b1');
  s.written({ base: 'b2' }, 'v0', -1);
  assert.equal(s.history.current, 'v0');
  assert.ok(s.history.canRedo);
  // Changed by someone else meanwhile: the history starts again.
  assert.equal(s.enter({ file: 'main.html', base: 'other' }, 'v9'), false);
  assert.ok(!s.history.canUndo);
  assert.deepEqual(s.history.versions, ['v9']);
  // One write at a time.
  s.request('v10');
  assert.match(s.request('v11').refused, /last change/);
});

test('a conflict that is not editable stops writing and keeps the base (auditor-1-004)', () => {
  const s = session();
  s.enter({ file: 'main.html', base: 'b0' }, 'v0');
  s.request('v1');
  const outcome = s.refused({ code: 'conflict', reply: { base: 'external', html: card('<h2>x</h2><script>keep()</script>') } });
  assert.match(outcome.blocked, /not editable here/);
  assert.equal(s.base, 'b0', 'the external base is not taken');
  // No later write is sent: every request is refused, undo included.
  for (const text of ['v2', 'v0']) {
    const later = s.request(text);
    assert.equal(later.message, undefined);
    assert.match(later.refused, /not editable here/);
  }
  // Reading the panel again (re-entering) on editable content writes again, with a new history.
  assert.equal(s.enter({ file: 'main.html', base: 'fixed' }, 'v5'), false);
  assert.equal(s.request('v6').message.base, 'fixed');
  // A conflict whose content has no one panel stops writing too.
  const t = session();
  t.enter({ file: 'main.html', base: 'b0' }, 'v0');
  t.request('v1');
  assert.ok(t.refused({ code: 'conflict', reply: { base: 'x', html: null } }).blocked);
  assert.ok(t.request('v2').refused);
});

test('an editable conflict is adopted; another refusal shows the last version again', () => {
  const s = session();
  s.enter({ file: 'main.html', base: 'b0' }, 'v0');
  s.request('v1');
  const html = card('<h2>by hand</h2>');
  const outcome = s.refused({ code: 'conflict', reply: { base: 'external', html } });
  assert.equal(outcome.adopt.children[0].children[0].children[0].text, 'by hand');
  assert.equal(s.base, 'external');
  assert.deepEqual(s.history.versions, [serialize(outcome.adopt, whitelist)]);
  assert.equal(s.request('v2').message.base, 'external');
  assert.deepEqual(s.refused({ code: 'bad-binding', message: 'not written: wrong kind' }), { restore: 'not written: wrong kind' });
  assert.equal(s.base, 'external');
  assert.ok(s.request('v3').message, 'writable again after a refusal');
  // Another client's push is the same: editable content adopted.
  s.busy = false;
  assert.ok(s.changed({ base: 'pushed', html }).adopt);
  assert.equal(s.base, 'pushed');
});

test('keys (continued)', () => {
  assert.deepEqual(shape(tree()), shape(tree()));
});

test('a control dropped just before the group of one it leaves keeps that place', () => {
  // Row [A, G[B], C]: B before G, its own group of one: [A, B, C], not appended.
  const t = parseContent(card('<h2>m</h2><div class="row"><signal-knob bind="/a.x" label="a"></signal-knob><div class="signal-group"><signal-meter bind="/out.l" label="b"></signal-meter></div><signal-knob bind="/a.z" label="c"></signal-knob></div>'), whitelist).tree;
  const row = find(t, control(t, '/a.x')).parent.id;
  const m = ok(moveControl(t, control(t, '/out.l'), { into: row, index: 1 }));
  assert.deepEqual(binds(find(m, row).node), ['/a.x', '/out.l', '/a.z']);
});

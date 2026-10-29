// ADR 0046 (test 7, the library half): a part's relative addresses rooted
// and made relative again, a part tree rendered rooted, the editor's palette
// for a part, the tree a part starts from, a part's controls for the
// schematic; and the canonical text the server writes when it moves
// controls into a part (crates/signal-ui/src/parts.rs) is the page's own
// (ui/lib/fixtures/studio-canonical.html, also read by the Rust test).

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent, serialize } from './panel-tree.js';
import { emptyPartTree, partControls, partFile, partPalette, partSlot, relativeAddress, rootAddress, rootTree, rootedAttribute } from './parts.js';

const ui = new URL('../', import.meta.url);
const read = (path) => readFileSync(new URL(path, ui), 'utf8');
const whitelist = JSON.parse(read('lib/panel-whitelist.json'));

test('relative addresses are rooted at the slot, and nothing outside it', () => {
  assert.equal(rootAddress('.cutoff', '/synth'), '/synth.cutoff');
  assert.equal(rootAddress('/vcf.cutoff', '/synth'), '/synth/vcf.cutoff');
  assert.equal(rootAddress('/a/b.out', '/rack1/fx'), '/rack1/fx/a/b.out');
  for (const outside of ['echo.in', '../echo.in', '/vcf', '.Cut', '.a.b', '', '/a/.b']) assert.equal(rootAddress(outside, '/synth'), null, outside);
  assert.equal(relativeAddress('/synth.cutoff', '/synth'), '.cutoff');
  assert.equal(relativeAddress('/synth/vcf.cutoff', '/synth'), '/vcf.cutoff');
  assert.equal(relativeAddress('/synthx.cutoff', '/synth'), null);
  assert.equal(relativeAddress('/echo.in', '/synth'), null);
  assert.equal(partFile('synth'), 'parts/synth.html');
  assert.equal(partSlot('parts/synth.html'), 'synth');
  assert.equal(partSlot('main.html'), null);
});

test('a part renders rooted: its address attributes, nothing else', () => {
  assert.equal(rootedAttribute('bind', '.x .y', '/xy'), '/xy.x /xy.y');
  assert.equal(rootedAttribute('label', '.x', '/s'), '.x');
  assert.equal(rootedAttribute('audition', 'none', '/s'), 'none');
  assert.equal(rootedAttribute('position-bind', '/seq.position', '/s'), '/s/seq.position');
  assert.equal(rootedAttribute('bind', 'echo.in', '/s'), 'echo.in', 'kept, so the check names it');
  const parsed = parseContent('<div class="signal-cards"><section class="signal-card"><h2>s</h2><div class="row"><signal-knob bind=".cutoff" label="cutoff"></signal-knob></div></section></div>', whitelist);
  const rooted = rootTree(parsed.tree, '/synth');
  const knob = rooted.children[0].children[0].children[1].children[0];
  assert.equal(knob.attributes.bind, '/synth.cutoff');
  assert.equal(knob.attributes.label, 'cutoff');
  // The tree as written is untouched.
  assert.equal(parsed.tree.children[0].children[0].children[1].children[0].attributes.bind, '.cutoff');
});

test("a part's palette is its slot's addresses, relative", () => {
  const entries = [
    { address: '/synth.cutoff', label: 'synth.cutoff', kinds: ['knob'] },
    { address: '/synth/vcf.out', label: '/synth/vcf.out', kinds: ['scope'] },
    { address: '/echo.mix', label: 'echo.mix', kinds: ['knob'] },
  ];
  assert.deepEqual(
    partPalette(entries, '/synth').map((e) => [e.address, e.absolute]),
    [
      ['.cutoff', '/synth.cutoff'],
      ['/vcf.out', '/synth/vcf.out'],
    ],
  );
});

test('a part starts as one card of the interface parameters, within the whitelist', () => {
  const tree = emptyPartTree({ name: 'synth', parameters: [{ name: 'cutoff' }, { name: 'drive' }] });
  const text = serialize(tree, whitelist);
  assert.ok(text.includes('<signal-knob bind=".cutoff" label="cutoff"></signal-knob>'), text);
  assert.equal(parseContent(text, whitelist).error, undefined);
  assert.ok(serialize(emptyPartTree({ name: 'x', parameters: [] }), whitelist).includes('<h2>x</h2>'));
});

test("a part's controls for the schematic carry their slot and their binds as written", () => {
  const part = {
    controls: [
      { tag: 'signal-knob', label: 'cutoff', bind: '.cutoff', attributes: { bind: '.cutoff', label: 'cutoff' } },
      { tag: 'signal-scope', label: null, bind: '/vcf.out', attributes: { bind: '/vcf.out' } },
      { tag: 'signal-knob', label: null, bind: 'echo.mix', attributes: {} },
      { tag: 'signal-piano-roll', label: null, bind: '', attributes: {} },
    ],
  };
  const controls = partControls(part, 'synth', 10);
  assert.equal(controls.length, 2);
  assert.deepEqual(controls[0].binds, ['/synth.cutoff']);
  assert.deepEqual(controls[0].relative, ['.cutoff']);
  assert.equal(controls[0].part, 'synth');
  assert.equal(controls[0].index, 10);
  assert.equal(controls[1].type, 'scope');
  assert.deepEqual(controls[1].binds, ['/synth/vcf.out']);
});

test("the server's canonical panel text is the page's", () => {
  const text = read('../projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  const parsed = parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' });
  assert.equal(parsed.error, undefined);
  assert.equal(serialize(parsed.tree, whitelist), read('lib/fixtures/studio-canonical.html'));
  // ADR 0046 §5: the open slot card stands among the cards; no other control does.
  const card = '\n  <div class="signal-cards">\n    <signal-slot bind="/synth"></signal-slot>\n  </div>\n';
  assert.equal(serialize(parseContent(card, whitelist).tree, whitelist), card);
  assert.match(parseContent('<div class="signal-cards"><signal-knob bind="/a.b"></signal-knob></div>', whitelist).error, /not allowed/);
});

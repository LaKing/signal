// M2.9-2 (docs/ui-controls.md "Editing a panel", ADR 0026 §5): the
// canonical serialiser, the whitelist (operator decision of 2026-10-16: h1
// headings, lowercase class tokens), comments dropped on reading, and the
// round trip of the example panels (read from their files, never written)
// and the generated default panel.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { checkTree, generatedTree, parseContent, serialize, shape } from './panel-tree.js';
import { defaultPanel } from './panel.js';
import { textOutsideCards } from './cards.test.js';

const ui = new URL('../', import.meta.url);
const read = (path) => readFileSync(new URL(path, ui), 'utf8');
const whitelist = JSON.parse(read('lib/panel-whitelist.json'));
const EXAMPLES = ['../projects/kick/ui/main.html', '../projects/pulse/ui/main.html', '../projects/physical-av/ui/main.html', '../projects/studio/ui/main.html'];

/** The content of a file's <signal-panel> (the examples have one, without comments in it). */
function content(text) {
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  return text.slice(start, text.indexOf('</signal-panel>'));
}

const parse = (html, mode) => {
  const parsed = parseContent(html, whitelist, { mode });
  assert.equal(parsed.error, undefined, parsed.error);
  return parsed.tree;
};
const refused = (html, pattern, mode = 'submit') => {
  const parsed = parseContent(html, whitelist, { mode });
  assert.ok(parsed.error, `refused: ${html}`);
  if (pattern) assert.match(parsed.error, pattern);
};
const card = (inner, classes = 'signal-card') => `<div class="signal-cards"><section class="${classes}">${inner}</section></div>`;
const bindings = (text) => [...text.matchAll(/<(signal-[a-z]+)[^>]*?\sbind="([^"]*)"/g)].map((m) => `${m[1]} ${m[2]}`);

test('the canonical form: a line per element, two spaces per level, the fixed attribute order', () => {
  const tree = parse(card('<h2>out</h2><div class="row"><signal-knob label="g" max="2" bind="/out.gain" min="0" bipolar=""></signal-knob><div class="signal-group"><signal-meter label="L" bind="/out.l"></signal-meter></div></div>', 'signal-card x wide'));
  const text = serialize(tree, whitelist);
  assert.equal(
    text,
    `
  <div class="signal-cards">
    <section class="signal-card wide x">
      <h2>out</h2>
      <div class="row">
        <signal-knob bind="/out.gain" min="0" max="2" bipolar label="g"></signal-knob>
        <div class="signal-group">
          <signal-meter bind="/out.l" label="L"></signal-meter>
        </div>
      </div>
    </section>
  </div>
`,
  );
  assert.equal(serialize(tree, whitelist), text, 'the same tree, the same text');
  assert.equal(serialize(parse(text), whitelist), text, 'canonical text reads back to itself');
  // `bipolar` is written bare: bare and empty read the same.
  const bare = parse(card('<div class="row"><signal-knob bind="/a.b" bipolar></signal-knob></div>'));
  const empty = parse(card('<div class="row"><signal-knob bind="/a.b" bipolar=""></signal-knob></div>'));
  assert.deepEqual(shape(bare), shape(empty));
  assert.equal(serialize(bare, whitelist), serialize(empty, whitelist));
  assert.match(serialize(bare, whitelist), /<signal-knob bind="\/a\.b" bipolar><\/signal-knob>/);
  // Text and values are escaped; an empty container is one line.
  const escaped = parse(card('<h2>a &amp; b &lt;c&gt;</h2><div class="row"></div><signal-value bind="/a.b" label="&quot;x&quot; &#38; y"></signal-value>'));
  assert.match(serialize(escaped, whitelist), /<h2>a &amp; b &lt;c&gt;<\/h2>/);
  assert.match(serialize(escaped, whitelist), /<div class="row"><\/div>/);
  assert.match(serialize(escaped, whitelist), /label="&quot;x&quot; &amp; y"/);
});

test('the whitelist: h1 headings and lowercase class tokens (operator decision of 2026-10-16)', () => {
  parse(card('<h1>kick</h1><p>about</p>', 'signal-card wide steps'));
  parse(card('<h2 class="note">x</h2><p class="small-print">y</p><div class="row tight"><div class="signal-group pair"><signal-meter class="a b" bind="/out.l"></signal-meter></div></div>', 'signal-card view'));
  parse('<div class="signal-cards dense"></div>');
  refused(card('<h1 class="x">kick</h1>'), /class/);
  refused(card('', 'signal-card Bad'), /lowercase/);
  refused(card('', 'signal-card a_b'), /lowercase/);
  refused(card('', 'signal-card x x'), /twice/);
  refused(card('<script>alert(1)</script>'), /<script>/);
  refused(card('<style>p{}</style>'), /<style>/);
  refused(card('<h2 style="color: red">x</h2>'), /style is not allowed/);
  refused(card('<div class="row"><signal-knob bind="/a.b" onclick="x()"></signal-knob></div>'), /event handlers/);
  refused(card('<img src="x.png">'), /<img>/);
  refused(card('<h2>x<small>y</small></h2>'), /<small>/);
  refused(card('<div class="row"><signal-knob bind="/a.b" foo="1"></signal-knob></div>'), /foo/);
  refused(card('<div class="row"><signal-knob bind=/a.b></signal-knob></div>'), /double quotes/);
  refused(card("<div class=\"row\"><signal-knob bind='/a.b'></signal-knob></div>"), /double quotes/);
  refused(card('<div class="row"><signal-knob bind="/a.b" bind="/c.d"></signal-knob></div>'), /twice/);
  refused(card('<h2>a &mdash; b</h2>'), /reference/);
  // The server reads exactly these spellings too (panel_write.rs).
  refused(card('<h2>&#X41;</h2>'), /reference/);
  refused(card('<h2>&#x+41;</h2>'), /reference/);
  assert.equal(parse(card('<h2>&#x41;&#65;</h2>')).children[0].children[0].children[0].text, 'AA');
  refused(card('<div class="row"><div class="row"></div></div>'), /not allowed/);
  refused(card('<div class="row">text</div>'), /outside a heading/);
  refused(card('<signal-schematic></signal-schematic>'), /not an element/);
  refused('<section class="signal-card"></section>', /not allowed in <signal-panel>/);
  refused(card('<div class="row"><signal-knob bind="/a.b"></div>'), /does not close/);
  refused(card('<!DOCTYPE html>'));
});

test('the whitelist names each control’s observed attributes, plus class', () => {
  const observed = (file, name) => {
    const source = read(`controls/${file}`);
    const own = new RegExp(`class ${name} extends (\\w+) \\{[\\s\\S]*?static observedAttributes = \\[([^\\]]*)\\]`).exec(source);
    const list = (text) => [...text.matchAll(/'([a-z-]+)'/g)].map((m) => m[1]);
    if (own) return [...(own[2].includes('...SignalValueElement') ? list(read('controls/base.js').match(/class SignalValueElement[\s\S]*?observedAttributes = \[([^\]]*)\]/)[1]) : []), ...list(own[2])];
    // Inherits SignalValueElement's (the toggle).
    return list(read('controls/base.js').match(/class SignalValueElement[\s\S]*?observedAttributes = \[([^\]]*)\]/)[1]);
  };
  const classes = {
    'signal-fader': ['fader.js', 'SignalFader'],
    'signal-knob': ['knob.js', 'SignalKnob'],
    'signal-xy': ['xy.js', 'SignalXY'],
    'signal-toggle': ['button.js', 'SignalToggle'],
    'signal-value': ['value.js', 'SignalValue'],
    'signal-button': ['button.js', 'SignalButton'],
    'signal-meter': ['meter.js', 'SignalMeter'],
    'signal-scope': ['scope.js', 'SignalScope'],
    'signal-canvas': ['canvas.js', 'SignalCanvas'],
    'signal-piano-roll': ['piano-roll.js', 'SignalPianoRoll'],
    'signal-step-grid': ['step-grid.js', 'SignalStepGrid'],
    'signal-slot': ['slot.js', 'SignalSlot'],
  };
  assert.deepEqual(Object.keys(whitelist.controls).sort(), Object.keys(classes).sort());
  for (const [tag, [file, name]] of Object.entries(classes)) {
    const listed = whitelist.controls[tag].filter((a) => a !== 'class');
    assert.deepEqual([...listed].sort(), [...new Set(observed(file, name))].sort(), tag);
    for (const a of whitelist.controls[tag]) assert.ok(whitelist.order.includes(a), `${a} has a place in the order`);
  }
});

test('comments in an existing panel are dropped on reading; the editor never sends one', () => {
  const plain = card('<h2>x</h2><div class="row"><div class="signal-group"><signal-meter bind="/out.l"></signal-meter></div></div>');
  const commented = `<!-- before -->${card('<!-- a note --><h2>x</h2><div class="row"><!-- in a row --><div class="signal-group"><!-- in a group --><signal-meter bind="/out.l"></signal-meter></div></div>')}<!-- <script>alert(1)</script> </signal-panel> -->`;
  const tree = parse(commented, 'read');
  assert.deepEqual(shape(tree), shape(parse(plain)));
  assert.doesNotMatch(serialize(tree, whitelist), /<!--/);
  refused(commented, /comment/, 'submit');
  refused(card('<script>alert(1)</script>'), /<script>/, 'read');
  refused(card('<!-- never closed'), /unterminated/, 'read');
});

test('the example panels round-trip, h1 and their classes kept', () => {
  for (const file of EXAMPLES) {
    const original = content(read(file));
    const tree = parse(original, 'read');
    const text = serialize(tree, whitelist);
    assert.deepEqual(shape(parse(text)), shape(tree), `${file}: the written text reads back to the same tree`);
    assert.equal(serialize(parse(text), whitelist), text, `${file}: canonical`);
    assert.deepEqual(bindings(text), bindings(original), `${file}: every control and binding, in order`);
    assert.deepEqual(textOutsideCards(`<body><signal-panel>${text}</signal-panel></body>`), [], file);
    assert.match(text, /<h1>[^<]+<\/h1>/, `${file}: the title card's h1`);
    assert.ok(checkTree(tree, whitelist).ok);
  }
  assert.match(serialize(parse(content(read(EXAMPLES[1])), 'read'), whitelist), /class="signal-card wide steps"/);
  assert.match(serialize(parse(content(read(EXAMPLES[2])), 'read'), whitelist), /class="signal-card view"/);
  assert.match(serialize(parse(content(read(EXAMPLES[0])), 'read'), whitelist), /class="signal-card wide scope"/);
});

const DESCRIBE = {
  controls: [
    { address: '/osc.freq', instance: 'osc', parameter: 'freq', module: 'std.sine_osc', ports: ['/osc.freq'], unit: 'Hz', min: 20, max: 20000 },
    { address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', ports: ['/out.gain'], unit: null, min: 0, max: 4 },
  ],
  instances: [
    { address: '/osc', parameters: [{ address: '/osc.freq', kind: 'frequency' }], ports: [{ name: 'out', address: '/osc.out', direction: 'out', shape: 'signal' }] },
    {
      address: '/out',
      parameters: [{ address: '/out.gain', kind: 'gain' }],
      ports: [
        { name: 'l', address: '/out.l', direction: 'in', shape: 'signal' },
        { name: 'r', address: '/out.r', direction: 'in', shape: 'signal' },
      ],
    },
  ],
  sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 2 }],
};

test('the generated default panel round-trips: a card per instance, the output card wide with the meter pair', () => {
  const tree = generatedTree(defaultPanel(DESCRIBE));
  const text = serialize(tree, whitelist);
  assert.ok(checkTree(tree, whitelist).ok, checkTree(tree, whitelist).error);
  assert.deepEqual(shape(parse(text)), shape(tree));
  assert.equal(serialize(parse(text), whitelist), text);
  assert.deepEqual(textOutsideCards(`<body><signal-panel>${text}</signal-panel></body>`), []);
  assert.match(text, /<h2>osc<\/h2>\n\s+<p>std\.sine_osc<\/p>/, 'the module is the description, not a <small>');
  assert.match(text, /<signal-knob bind="\/osc\.freq" taper="log" label="freq"><\/signal-knob>/);
  assert.match(text, /<signal-fader bind="\/out\.gain" taper="db" label="gain"><\/signal-fader>/);
  assert.match(text, /<section class="signal-card wide">\n\s+<h2>output<\/h2>\n\s+<div class="row">\n\s+<div class="signal-group">\n\s+<signal-meter bind="\/out\.l" label="l"><\/signal-meter>\n\s+<signal-meter bind="\/out\.r" label="r"><\/signal-meter>/);
  assert.match(text, /<signal-scope bind="\/out\.l" label="\/out\.l"><\/signal-scope>/);
});

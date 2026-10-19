// The port labels of composites' boxes and frames (M2.13-2, ADR 0030 §1,
// docs/ui-controls.md "Schematic"): always at least one, never on each
// other within a box or between boxes, every pin in exactly one label, at
// every zoom; the grouping texts; the cap; the steps' bound; the notes
// beside a boundary's pins, a trace's summary after the note.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { MAX_LABELS, PITCH_PX, boxLabels, estimateWidth, fitText, shownText, insideNote, minWidth, outsideNote, overlaps, pinText, placeCounts, placeLabels, prefixOf, setMeasure, textWidth, wrapText } from './schematic-labels.js';
import { GAP_X, layout } from './schematic-layout.js';
import { badgeText, behind } from './schematic-trace.js';

/** The studio's `graph` structure (`top`, `composites`), as `Project::structure` gives it (crates/signal-project/tests/structure.rs keeps it equal). */
const STUDIO =JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

/** The moog voice's 31 inputs (3 ports, 28 parameters) in export order, and its output. */
const VOICE_INS = ['hz', 'gate', 'velocity', 'glide', 'osc1_octave', 'osc1_detune', 'osc1_wave', 'osc2_octave', 'osc2_detune', 'osc2_wave', 'osc3_octave', 'osc3_detune', 'osc3_wave', 'osc1_level', 'osc2_level', 'osc3_level', 'noise_level', 'cutoff', 'emphasis', 'drive', 'env_amount', 'key_track', 'f_attack', 'f_decay', 'f_sustain', 'f_release', 'a_attack', 'a_decay', 'a_sustain', 'a_release', 'volume'];

/** A box on screen: `ins` on its left edge, `outs` on its right, `pitch` px apart from `y + 30`. */
function box(name, { x = 0, y = 0, w = 120, ins = VOICE_INS, outs = ['out'], pitch = 16, kind = 'instance', texts } = {}) {
  const h = 34 + pitch * Math.max(ins.length, outs.length) + 8;
  const at = (side, names) => names.map((n, i) => ({ name: n, x: side ? x + w : x, y: y + 34 + pitch * (i + 0.5) }));
  return { kind, name, rect: { x, y, w, h }, ins: at(false, ins), outs: at(true, outs), texts };
}

/** No two labels overlap, and every pin of `boxes` is in exactly one label. */
function sound(boxes, labels, what) {
  for (let i = 0; i < labels.length; i++) {
    for (let j = i + 1; j < labels.length; j++) assert.ok(!overlaps(labels[i], labels[j]), `${what}: "${labels[i].text}" overlaps "${labels[j].text}"`);
  }
  const seen = new Map();
  for (const label of labels) for (const p of label.pins) seen.set(`${p.box} ${p.name}`, (seen.get(`${p.box} ${p.name}`) ?? 0) + 1);
  boxes.forEach((b, i) => {
    for (const p of [...b.ins, ...b.outs]) assert.equal(seen.get(`${i} ${p.name}`), 1, `${what}: pin ${i}.${p.name}`);
  });
  assert.ok(labels.length >= 1, `${what}: at least one label`);
  for (const label of labels) assert.ok(label.y1 - label.y0 >= PITCH_PX - 1e-9, `${what}: a label keeps its size`);
}

test('prefixes: up to the first underscore, else without trailing digits, else none', () => {
  assert.equal(prefixOf('osc1_octave'), 'osc1_');
  assert.equal(prefixOf('f_attack'), 'f_');
  assert.equal(prefixOf('t1'), 't');
  assert.equal(prefixOf('hz'), null);
  assert.equal(prefixOf('_x'), null);
});

test('a box shows every pin while its rows are far enough apart', () => {
  const b = box('synth', { pitch: 16 });
  const { level, labels } = boxLabels(b, 0);
  assert.equal(level, 1);
  assert.deepEqual(labels.filter((l) => l.align === 'left').map((l) => l.text), VOICE_INS);
  assert.deepEqual(labels.filter((l) => l.align === 'right').map((l) => l.text), ['out']);
});

test('closer rows group by prefix, then merge the groups, then count', () => {
  const runs = boxLabels(box('env', { ins: ['osc1_a', 'osc1_b', 'osc1_c', 'f_a', 'f_b', 'f_c', 'f_d'], pitch: 5 }), 0).labels.filter((l) => l.align === 'left');
  assert.deepEqual(
    runs.map((l) => l.text),
    ['osc1_* (3)', 'f_* (4)'],
  );
  // The closest neighbours merge first: groups far enough apart keep their own labels.
  const two = boxLabels(box('synth', { pitch: 7 }), 0, 2).labels.filter((l) => l.align === 'left').map((l) => l.text);
  assert.ok(two.includes('hz, gate (2)') && two.includes('osc1_* (3)') && two.includes('f_* (4)') && two.includes('a_* (4)'), two.join(' | '));
  const three = boxLabels(box('synth', { pitch: 2 }), 0, 3).labels.filter((l) => l.align === 'left').map((l) => l.text);
  assert.ok(three.some((t) => /^\S+, \S+ \(\d+\)$/.test(t) || /^\S+ … \S+ \(\d+\)$/.test(t)), three.join(' | '));
  const four = boxLabels(box('synth', { pitch: 0.5 }), 0, 4).labels.map((l) => l.text);
  assert.ok(four.includes('31 inputs') && four.includes('out'), four.join(' | '));
  const five = boxLabels(box('synth', { pitch: 0.1 }), 0, 5).labels.map((l) => l.text);
  assert.deepEqual(five, ['synth: 31 in · 1 out']);
});

test('a box whose two sides collide shows one summary', () => {
  // Pins on both sides, rows apart, but a box too narrow for its two labels side by side.
  const b = box('thin', { w: 30, ins: ['alpha_long_name'], outs: ['beta_long_name'] });
  assert.deepEqual(boxLabels(b, 0).labels.map((l) => l.text), ['thin: 1 in · 1 out']);
});

test('one box at every zoom: always labelled, never overlapping, every pin once', () => {
  for (let pitch = 20; pitch >= 0.05; pitch *= 0.8) {
    const b = box('synth', { pitch, w: Math.max(1, 120 * (pitch / 16)) });
    const { labels } = placeLabels([b]);
    sound([b], labels, `pitch ${pitch.toFixed(2)}`);
  }
  // A box 2 px high (the smallest the scene draws) still has its label.
  const tiny = { kind: 'instance', name: 'synth', rect: { x: 0, y: 0, w: 3, h: 2 }, ins: VOICE_INS.map((n, i) => ({ name: n, x: 0, y: i * 0.05 })), outs: [{ name: 'out', x: 3, y: 0 }] };
  const { labels } = placeLabels([tiny]);
  sound([tiny], labels, 'tiny');
  assert.ok(labels.length <= 2, labels.map((l) => l.text).join(' | '));
  assert.ok(labels.some((l) => l.text === '31 inputs' || l.text === 'synth: 31 in · 1 out'), labels.map((l) => l.text).join(' | '));
});

/** Two moog-like composites wired together, laid out as the schematic lays them out (GAP_X apart). */
function neighbours() {
  const ports = (names, direction) => names.map((name) => ({ name, direction, shape: 'signal', kind: 'generic', rate: 1 }));
  const voice = (name) => ({ name, module: 'voice', composite: true, ports: [...ports(VOICE_INS.slice(0, 3), 'in'), ...ports(['out'], 'out')], parameters: VOICE_INS.slice(3).map((p) => ({ name: p, kind: 'control' })) });
  return layout({ instances: [voice('a'), voice('b')], connections: [{ from: '/a.out', to: '/b.hz', converters: [] }] });
}

/** The boxes of layout `drawn`'s instances at scale `k`. */
function boxesAt(drawn, k) {
  const at = (p) => ({ name: p.name, x: p.x * k, y: p.y * k });
  return drawn.nodes.map((n) => ({ kind: 'instance', name: n.name, rect: { x: n.x * k, y: n.y * k, w: n.w * k, h: n.h * k }, ins: n.ins.map(at), outs: n.outs.map(at) }));
}

test('two neighbouring composites, GAP_X apart, never overlap at any mid zoom', () => {
  const drawn = neighbours();
  const [a, b] = drawn.nodes;
  assert.equal(b.x - (a.x + a.w), GAP_X);
  for (let k = 0.1; k <= 1.5 + 1e-9; k += 0.05) {
    const boxes = boxesAt(drawn, k);
    const { labels } = placeLabels(boxes);
    sound(boxes, labels, `k ${k.toFixed(2)}`);
  }
  // Somewhere in that range a box had to give way to its neighbour, and was grouped rather than overlapped.
  const mid = placeLabels(boxesAt(drawn, 0.3)).labels;
  assert.ok(mid.some((l) => l.pins.length > 1), mid.map((l) => l.text).join(' | '));
});

test('a box whose label leaves it gives way before a neighbour whose labels fit', () => {
  // A large readable box, and a tiny one whose summary reaches into it.
  const big = box('big', { x: 0, y: 0, w: 300, ins: ['in'], outs: ['out'], pitch: 40 });
  const small = box('s', { x: 200, y: 40, w: 4, ins: ['x'], outs: [], pitch: 1 });
  const { labels } = placeLabels([big, small]);
  sound([big, small], labels, 'intruder');
  assert.ok(labels.some((l) => l.text === 'in') && labels.some((l) => l.text === 'out'), labels.map((l) => l.text).join(' | '));
});

test('boxes stacked on each other end as one cluster, within 6 steps per box', () => {
  const boxes = Array.from({ length: 50 }, (_, i) => box(`v${i}`, { x: i * 0.1, y: i * 0.1, w: 3, pitch: 0.05 }));
  const { labels, iterations } = placeLabels(boxes);
  sound(boxes, labels, 'stack');
  // Almost all of them share one label; a box left beside it keeps its own, on no other.
  assert.ok(labels.length <= 3, labels.map((l) => l.text).join(' | '));
  assert.match(labels[0].text, /^v0, v1 \+\d+: \d+ ports$/);
  assert.ok(iterations <= 6 * boxes.length, `${iterations} steps`);
});

test('the cap: more labels than allowed are grouped, every box and pin still labelled', () => {
  // 20 separate boxes of 31 rows each: 640 labels at level 1.
  const boxes = Array.from({ length: 20 }, (_, i) => box(`v${i}`, { x: (i % 5) * 400, y: Math.floor(i / 5) * 600 }));
  assert.ok(boxes.reduce((n, b) => n + boxLabels(b, 0).labels.length, 0) > MAX_LABELS);
  const { labels } = placeLabels(boxes);
  assert.ok(labels.length <= MAX_LABELS);
  sound(boxes, labels, 'cap');
  // More separate boxes than the cap: forced to share, deterministically.
  const many = Array.from({ length: 450 }, (_, i) => box(`m${i}`, { x: (i % 30) * 500, y: Math.floor(i / 30) * 500, ins: ['a'], outs: [] }));
  const capped = placeLabels(many, { cap: MAX_LABELS });
  assert.ok(capped.labels.length <= MAX_LABELS);
  sound(many, capped.labels, 'many');
  assert.deepEqual(placeLabels(many).labels.map((l) => l.text), capped.labels.map((l) => l.text));
});

test('a frame pin says where it connects outside; a box pin where it goes inside (the studio)', () => {
  assert.equal(pinText('hz', outsideNote(STUDIO, '/synth.hz')), 'hz ← /melody.hz');
  assert.equal(pinText('out', outsideNote(STUDIO, '/synth.out')), 'out → /echo.in');
  assert.equal(pinText('hz', insideNote(STUDIO, '/synth.hz')), 'hz → 2: glide.in, hz_prev.in');
  assert.equal(pinText('gate', insideNote(STUDIO, '/synth.gate')), 'gate → 2: fenv.gate, aenv.gate');
  assert.equal(pinText('out', insideNote(STUDIO, '/synth.out')), 'out ← volume.out');
  // A port fed by several: a count, at most three names.
  assert.match(outsideNote(STUDIO, '/kick.trigger'), /^← \/drums\.t1$/);
});

/** /a (a composite whose out comes from inner.out) → /b (a composite whose in goes to sink.in). */
const NESTED = {
  top: {
    instances: [
      { name: 'a', module: 'ca', composite: true, ports: [{ name: 'out', direction: 'out', shape: 'signal' }], parameters: [] },
      { name: 'b', module: 'cb', composite: true, ports: [{ name: 'in', direction: 'in', shape: 'signal' }], parameters: [] },
    ],
    connections: [{ from: '/a.out', to: '/b.in', converters: [] }],
  },
  composites: {
    ca: { inputs: [], outputs: [{ name: 'out', from: 'inner.out' }], parameters: [], instances: [{ name: 'inner', module: 'std.osc', composite: false, ports: [{ name: 'out', direction: 'out' }], parameters: [] }], connections: [] },
    cb: { inputs: [{ name: 'in', to: ['sink.in'] }], outputs: [], parameters: [], instances: [{ name: 'sink', module: 'std.gain', composite: false, ports: [{ name: 'in', direction: 'in' }], parameters: [] }], connections: [] },
  },
};

test('a traced frame pin keeps where it connects outside, and the trace summary follows it', () => {
  const note = outsideNote(NESTED, '/b.in');
  assert.equal(note, '← /a.out');
  const summary = badgeText(behind(NESTED, '/b.in', 'outside'), '/b.in', 'outside');
  assert.equal(summary, '← from /a/inner.out');
  const label = pinText('in', note, summary);
  assert.equal(label, 'in ← /a.out · trace: ← from /a/inner.out');
  // A closed box's traced pin the same way: inside note, then the summary.
  const inside = pinText('out', insideNote(NESTED, '/a.out'), badgeText(behind(NESTED, '/a.out', 'inside'), '/a.out', 'inside'));
  assert.equal(inside, 'out ← inner.out · trace: ← inside from inner.out');
});

test('labels carry the notes: a longer text groups sooner rather than overlapping', () => {
  const texts = new Map(VOICE_INS.map((n) => [n, `${n} ← /a/very/long/address.of_some_port`]));
  const boxes = [box('a', { texts, pitch: 12 }), box('b', { x: 200, texts, pitch: 12 })];
  const { labels } = placeLabels(boxes);
  sound(boxes, labels, 'noted');
});

test('a closed box’s note stays within the box, cut short, never narrower than the pin’s name', () => {
  const texts = new Map([['hz', 'hz → 2: glide.in, hz_prev.in']]);
  const b = box('synth', { w: 100, ins: ['hz'], outs: [], texts });
  const { labels } = placeLabels([b]);
  assert.equal(labels.length, 1);
  const [l] = labels;
  assert.equal(l.text, 'hz → 2: glide.in, hz_prev.in', 'the whole text is kept (the title and the tip)');
  assert.ok(l.x0 >= b.rect.x && l.x1 <= b.rect.x + b.rect.w, `${l.x0}..${l.x1} within 0..100`);
  // A name longer than the half keeps its whole width, as far as the box allows.
  const wide = box('synth', { w: 100, ins: ['noise_level'], outs: ['out'], texts: new Map([['noise_level', 'noise_level → mixer.gain_d']]) });
  const n = placeLabels([wide]).labels.find((l) => l.pins[0].name === 'noise_level');
  assert.ok(n.x1 - n.x0 >= textWidth('noise_level') - 1e-9, `${n.x1 - n.x0} px`);
  // Narrower than that: cut short inside the box (never spilling over it), else the summary.
  const long = box('synth', { w: 40, ins: ['noise_level'], outs: [], texts: new Map([['noise_level', 'noise_level → mixer.gain_d']]) });
  for (const l of placeLabels([long]).labels) assert.ok(l.x0 >= long.rect.x - 1e-9 && l.x1 <= long.rect.x + long.rect.w + 1e-9, `${l.text}: ${l.x0}..${l.x1}`);
});

test('the studio synth hovered or traced at zoom 1 keeps its 32 labels, its notes shown on its edge', () => {
  const drawn = layout(STUDIO.top);
  const synth = drawn.nodes.find((n) => n.name === 'synth');
  const at = (p) => ({ name: p.name, x: p.x, y: p.y });
  const plain = { kind: 'instance', name: 'synth', rect: { x: synth.x, y: synth.y, w: synth.w, h: synth.h }, ins: synth.ins.map(at), outs: synth.outs.map(at) };
  assert.equal(placeLabels([plain]).labels.length, 32);
  const texts = new Map([...synth.ins, ...synth.outs].map((p) => [p.name, pinText(p.name, insideNote(STUDIO, `/synth.${p.name}`))]));
  const hovered = { ...plain, texts };
  const { labels } = placeLabels([hovered]);
  sound([hovered], labels, 'hovered synth');
  assert.equal(labels.length, 32);
  assert.ok(labels.find((l) => l.text === 'hz → 2: glide.in, hz_prev.in'));
  assert.ok(labels.find((l) => l.text === 'out ← volume.out'));
  // Traced: the summary after the note, still 32 labels on the box's edge.
  texts.set('hz', pinText('hz', insideNote(STUDIO, '/synth.hz'), badgeText(behind(STUDIO, '/synth.hz', 'inside'), '/synth.hz', 'inside')));
  const traced = placeLabels([{ ...plain, texts }]).labels;
  assert.equal(traced.length, 32);
  assert.match(traced.find((l) => l.pins[0].name === 'hz').text, /^hz → 2: glide\.in, hz_prev\.in · trace: /);
});

test('bundle counts at one place both stay, on no other label', () => {
  const port = boxLabels(box('a', { ins: ['x'], outs: [] }), 0).labels;
  const mid = { x: 60, y: 46 };
  const counts = placeCounts(port, [
    { text: '2', at: [mid, { x: 60, y: 46 }] },
    { text: '3', at: [mid] },
    { text: '4', at: [{ x: port[0].x0 + 2, y: port[0].y0 + 2 }] },
  ]);
  assert.deepEqual(
    counts.map((c) => c.text),
    ['2', '3', '4'],
  );
  const all = [...port, ...counts];
  for (let i = 0; i < all.length; i++) for (let j = i + 1; j < all.length; j++) assert.ok(!overlaps(all[i], all[j]), `${all[i].text} on ${all[j].text}`);
});

test('a count displaced by a strip along the top edge stays inside the viewport', () => {
  // auditor-1's case: the top row taken, the count's places beneath it at the edge.
  const strip = { text: 'strip', x0: 0, y0: 0, x1: 500, y1: 11, align: 'left', pins: [] };
  const viewport = { w: 500, h: 300 };
  const [count] = placeCounts([strip], [{ text: '3', at: [{ x: 100, y: 5 }, { x: 150, y: 5 }, { x: 200, y: 5 }] }], viewport);
  assert.ok(count.y0 >= 0 && count.x0 >= 0 && count.x1 <= viewport.w && count.y1 <= viewport.h, `${count.x0},${count.y0}..${count.x1},${count.y1}`);
  assert.ok(!overlaps(count, strip));
  assert.equal(count.y0, 11, 'the next row down');
  // A place past an edge is moved in.
  const [edge] = placeCounts([], [{ text: '12', at: [{ x: 499, y: 299 }] }], viewport);
  assert.ok(edge.x1 <= 500 && edge.y1 <= 300);
  // A viewport with no room left: the count is still placed, inside it.
  const full = { text: 'all', x0: 0, y0: 0, x1: 500, y1: 300, align: 'left', pins: [] };
  const [kept] = placeCounts([full], [{ text: '2', at: [{ x: 250, y: 150 }] }], viewport);
  assert.ok(kept.x0 >= 0 && kept.y0 >= 0 && kept.x1 <= 500 && kept.y1 <= 300);
  // Many counts in a small viewport: every one inside, none on another.
  const small = { w: 60, h: 40 };
  const many = placeCounts([], Array.from({ length: 6 }, () => ({ text: '2', at: [{ x: 30, y: 20 }] })), small);
  assert.equal(many.length, 6);
  for (const c of many) assert.ok(c.x0 >= 0 && c.y0 >= 0 && c.x1 <= small.w && c.y1 <= small.h);
  for (let i = 0; i < many.length; i++) for (let j = i + 1; j < many.length; j++) assert.ok(!overlaps(many[i], many[j]));
});

test('widths are measured: a label holds its whole text, padding included, whatever the measure', () => {
  // The estimate is never narrower than the labels' font at 9 px (-apple-system, measured by canvas measureText in Chrome on macOS).
  const measured = { hz: 10.48, decay: 26.22, 2: 5.6, 12: 9.94, 'osc1_* (4)': 44.94, 'f_* (4)': 28.28, 'gate ← /melody.gate': 89.54, 'synth: 31 in · 1 out': 79.93, WWW: 26.63, '…': 7.41 };
  for (const [t, w] of Object.entries(measured)) assert.ok(estimateWidth(t) >= w, `${t}: ${estimateWidth(t)} < ${w}`);
  // .plabel: 2 px padding and a 1 px border each side.
  for (const t of ['hz', 'decay', '2', 'osc1_* (4)', 'gate ← /melody.gate']) assert.ok(textWidth(t) >= estimateWidth(t) + 6, t);
  // A measure set by the control replaces the estimate (and is used once per text).
  let calls = 0;
  setMeasure((t) => {
    calls += 1;
    return 10 * t.length;
  });
  try {
    assert.equal(textWidth('hz'), 27);
    assert.equal(textWidth('hz'), 27);
    assert.equal(calls, 1, 'measured once');
    const [l] = boxLabels(box('a', { ins: ['decay'], outs: [] }), 0).labels;
    assert.equal(l.x1 - l.x0, textWidth('decay'));
    assert.equal(l.shown, 'decay', 'shown whole');
    // A count badge is never cut.
    const [c] = placeCounts([], [{ text: '12', at: [{ x: 50, y: 50 }] }], { w: 200, h: 200 });
    assert.equal(c.x1 - c.x0, textWidth('12'));
    assert.equal(c.shown, '12');
  } finally {
    setMeasure(null);
  }
});

test('text cut and wrapped to a width: never wider, ending in … when cut', () => {
  assert.equal(fitText('hz', textWidth('hz')), 'hz');
  const cut = fitText('osc1_octave', textWidth('osc1_') + 2);
  assert.match(cut, /…$/);
  assert.ok(textWidth(cut) <= textWidth('osc1_') + 2);
  assert.equal(minWidth('hz'), textWidth('hz'));
  const lines = wrapText('synth: 31 in · 1 out', textWidth('31 in ·') + 1, 3);
  assert.deepEqual(lines, ['synth:', '31 in ·', '1 out']);
  for (const line of wrapText('synth: 31 in · 1 out', 30, 2)) assert.ok(textWidth(line) <= 30 || line === '…', line);
});

test('a summary stays inside its box: wrapped onto lines, cut short, never over a neighbour', () => {
  // The studio synth at k 0.3: a box 37 px wide, 161 px high.
  const thin = box('synth', { w: 37, pitch: 16 * 0.3 });
  const { labels } = placeLabels([thin]);
  assert.equal(labels.length, 1);
  const [s] = labels;
  assert.equal(s.text, 'synth: 31 in · 1 out');
  assert.ok(s.x0 >= thin.rect.x && s.x1 <= thin.rect.x + thin.rect.w, `${s.x0}..${s.x1}`);
  assert.ok(s.lines >= 2 || s.shown === 'synth', s.shown);
  for (const line of s.shown.split('\n')) assert.ok(textWidth(line) <= s.x1 - s.x0 + 1e-9, line);
  // A cluster of summaries stays within the boxes it names.
  const stack = [box('a', { w: 20, pitch: 0.2 }), box('b', { x: 5, w: 20, pitch: 0.2 })];
  for (const l of placeLabels(stack).labels) assert.ok(l.x0 >= -1e-9 && l.x1 <= 25 + 1e-9, `${l.text} ${l.x0}..${l.x1}`);
});

test('a closed box keeps every label inside it, a long one cut beside its neighbour on the same row', () => {
  // The synth collapsed at k 0.6: 74 px wide; its first row has a label on both sides.
  const b = box('synth', { w: 74, pitch: 9.6, ins: ['hz', 'osc1_octave_and_more'], outs: ['out'] });
  const { labels } = placeLabels([b]);
  sound([b], labels, 'narrow');
  for (const l of labels) {
    assert.ok(l.x0 >= b.rect.x - 1e-9 && l.x1 <= b.rect.x + b.rect.w + 1e-9, `${l.text} ${l.x0}..${l.x1}`);
    assert.ok(textWidth(l.shown) <= l.x1 - l.x0 + 1e-9, `${l.shown} fits`);
  }
});

test('a frame’s labels leaving the viewport go on their pins’ other side, inside it', () => {
  // The frame's inputs (their pins on its right edge, labels to the left) at the viewport's left edge.
  const frame = { kind: 'input', name: 'inputs', rect: { x: -20, y: 0, w: 30, h: 200 }, ins: [], outs: ['gate', 'hz'].map((n, i) => ({ name: n, x: 10, y: 20 + 16 * i })), texts: new Map([['gate', 'gate ← /melody.gate']]) };
  const viewport = { w: 400, h: 300 };
  const out = placeLabels([frame]).labels;
  assert.ok(out.some((l) => l.x0 < 0), 'without a viewport: outward');
  const kept = placeLabels([frame], { viewport }).labels;
  for (const l of kept) {
    assert.equal(l.align, 'left');
    assert.ok(l.x0 >= 10 && l.x1 <= viewport.w, `${l.text} ${l.x0}..${l.x1}`);
  }
});

test('a pin’s name is never cut to a guess: a narrow two-sided box shows its summary instead', () => {
  // library-demo's lfo at k 0.53: 60 px wide, a label on both sides of each row.
  const lfo = box('lfo', { w: 60, ins: ['reset', 'rate'], outs: ['out', 'ramp', 'sine', 'square'], pitch: 13 });
  const { labels } = placeLabels([lfo]);
  for (const l of labels) if (l.pins.length === 1) assert.ok(l.shown === l.pins[0].name || l.shown.length >= 5, `${l.shown} of ${l.pins[0].name}`);
  assert.ok(labels.length === 1 || labels.every((l) => !/^\S{1,3}…$/.test(l.shown)), labels.map((l) => l.shown).join(' | '));
});

test('a summary too wide even wrapped shows a shorter form, else nothing: never a row of cut words', () => {
  // The studio synth at k 0.33: 41 px wide; at k 0.24: 30 px.
  const at = (w) => boxLabels(box('synth', { w, pitch: 4 }), 0, 5).labels[0];
  const roomy = at(120);
  assert.equal(roomy.shown, 'synth: 31 in · 1 out');
  const mid = at(41);
  assert.ok(['synth:\n31 in ·\n1 out', 'synth'].includes(mid.shown), mid.shown);
  const small = at(30);
  assert.ok(small.shown === '' || (small.shown.endsWith('…') && small.shown.length >= 4) || small.shown === 'synth', JSON.stringify(small.shown));
  assert.equal(small.text, 'synth: 31 in · 1 out', 'the whole text is still its title');
  assert.ok(small.x1 - small.x0 <= 30);
});

test('a noted pin cut short keeps its name whole: `out…`, never `ou…`', () => {
  const text = 'out → /echo.in · trace: → 4 ends';
  assert.equal(shownText(text, textWidth('out…'), 'out'), 'out…');
  assert.equal(shownText(text, textWidth('out'), 'out'), 'out');
  assert.match(shownText(text, textWidth('out → /ec…'), 'out'), /^out → \/e/);
  assert.equal(shownText('osc1_* (4)', textWidth('osc1_*')), 'osc1_*');
});

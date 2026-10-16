import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { card, cardStream, converterText, dragMessage, releaseMessage, resetMessage, rowValues, staleNotice } from './schematic-inspector.js';
import { layout } from './schematic-layout.js';

const GRAPH = {
  top: {
    instances: [
      { name: 'drums', module: 'drums', composite: true, ports: [], parameters: [{ name: 'kick_decay', rate: 1, range: { min: 0.1, max: 60000 }, unit: 'ms', default: 400 }] },
      { name: 'out', module: 'std.audio_out', composite: false, ports: [{ name: 'l', direction: 'in', shape: 'signal', kind: 'audio', rate: 1 }], parameters: [{ name: 'gain', rate: 1, range: { min: 0, max: 4 }, default: 1 }] },
    ],
    connections: [],
  },
  composites: {
    drums: {
      instances: [
        { name: 'kick', module: 'kick', composite: true, ports: [], parameters: [{ name: 'decay', rate: 1, range: { min: 0.1, max: 60000 }, unit: 'ms', default: 400 }] },
        { name: 'env', module: 'std.exp_env', composite: false, ports: [], parameters: [{ name: 'decay_ms', rate: 1, range: { min: 0.1, max: 60000 }, unit: 'ms', default: 400 }] },
      ],
      connections: [],
      inputs: [],
      outputs: [],
      parameters: [{ name: 'kick_decay', to: ['kick.decay'] }],
    },
    kick: {
      instances: [{ name: 'amp_env', module: 'std.exp_env', composite: false, ports: [], parameters: [{ name: 'decay_ms', rate: 1 }] }],
      connections: [],
      inputs: [],
      outputs: [],
      parameters: [{ name: 'decay', to: ['amp_env.decay_ms'] }],
    },
  },
};

const DESCRIBE = {
  playing: true,
  controls: [
    { address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', unit: null, min: 0, max: 4, default: 1, base: 0.75, inFile: true },
    { address: '/drums.kick_decay', instance: 'drums', parameter: 'kick_decay', module: 'drums', unit: 'ms', min: 0.1, max: 60000, default: 400, base: 420, inFile: false },
  ],
  instances: [
    { address: '/drums/env', parameters: [{ name: 'decay_ms', base: 250 }] },
    { address: '/drums/kick/amp_env', parameters: [{ name: 'decay_ms', base: 420 }] },
  ],
};

const node = (level, name) => layout(level === '' ? GRAPH.top : GRAPH.composites[level]).nodes.find((n) => n.name === name);

test('a top-level instance shows its controls, editable as in the Parameters tab', () => {
  const model = card({ kind: 'instance', path: '', node: node('', 'out'), address: '/out' }, { graph: GRAPH, description: DESCRIBE });
  assert.equal(model.title, '/out');
  assert.equal(model.editable, true);
  assert.deepEqual(model.parameters.map((p) => [p.name, p.address, p.stream, p.editable, p.fallback]), [['gain', '/out.gain', '/out.gain', true, 0.75]]);
  // The same messages as the Parameters tab's slider.
  assert.deepEqual(dragMessage('/out.gain', 0.5), { type: 'ramp', address: '/out.gain', to: 0.5, ms: 5 });
  assert.deepEqual(releaseMessage('/out.gain', 0.5), { type: 'commit', address: '/out.gain', value: 0.5 });
  assert.deepEqual(resetMessage('/out.gain'), { type: 'commit', address: '/out.gain', value: null });
  const composite = card({ kind: 'instance', path: '', node: node('', 'drums'), address: '/drums' }, { graph: GRAPH, description: DESCRIBE });
  assert.deepEqual(composite.parameters.map((p) => p.address), ['/drums.kick_decay'], 'its exported parameters are controls');
});

test('inside a composite, parameters are shown and never written; a composite’s lead to the plan', () => {
  const env = card({ kind: 'instance', path: '/drums', node: node('drums', 'env'), address: '/drums/env' }, { graph: GRAPH, description: DESCRIBE });
  assert.equal(env.editable, false);
  assert.deepEqual(env.parameters.map((p) => [p.address, p.stream, p.editable, p.fallback, p.unit, p.max]), [['/drums/env.decay_ms', '/drums/env.decay_ms', false, 250, 'ms', 60000]]);
  const kick = card({ kind: 'instance', path: '/drums', node: node('drums', 'kick'), address: '/drums/kick' }, { graph: GRAPH, description: DESCRIBE });
  assert.deepEqual(kick.parameters.map((p) => [p.address, p.stream, p.fallback]), [['/drums/kick.decay', '/drums/kick/amp_env.decay_ms', 420]]);
  assert.deepEqual(cardStream(kick), { stream: 'parameters', addresses: ['/drums/kick/amp_env.decay_ms'], rate: 15 });
  assert.equal(cardStream({ parameters: [] }), null);
});

test('base and effective come from the parameters stream; describe only until it reports (auditor-1-002)', () => {
  const env = card({ kind: 'instance', path: '/drums', node: node('drums', 'env'), address: '/drums/env' }, { graph: GRAPH, description: DESCRIBE }).parameters[0];
  // Not reported yet (NaN): describe's base, no effective value.
  let values = new Map([['/drums/env.decay_ms', { base: NaN, effective: NaN }]]);
  assert.deepEqual(rowValues(env, values, null), { base: 250, effective: NaN, unsaved: null });
  assert.deepEqual(rowValues(env, new Map(), null).base, 250);
  // Another client set it live to 900: describe still says 250, the card says 900.
  values = new Map([['/drums/env.decay_ms', { base: 900, effective: 910 }]]);
  assert.deepEqual(rowValues(env, values, null), { base: 900, effective: 910, unsaved: null });
});

test('the unsaved mark is the host’s, as on the Parameters tab', () => {
  const gain = card({ kind: 'instance', path: '', node: node('', 'out'), address: '/out' }, { graph: GRAPH, description: DESCRIBE }).parameters[0];
  const unsaved = { count: 1, parameters: [{ address: '/out.gain', file: 0.75, inFile: true, control: true, applied: true }] };
  assert.equal(rowValues(gain, new Map(), unsaved).unsaved.file, 0.75);
  assert.equal(rowValues(gain, new Map(), { count: 0, parameters: [] }).unsaved, null);
});

test('a port shows its address, shape, kind and rate; a connection its ends and converter', () => {
  const port = card({ kind: 'port', address: '/sparks.pos', port: { name: 'pos', direction: 'out', shape: 'vector', kind: 'control', rate: 800 } });
  assert.deepEqual(port.fields, [['address', '/sparks.pos'], ['shape', 'vector'], ['kind', 'control'], ['rate', 'divisor 800']]);
  const converter = { name: 'out_d800', module: 'core.decimate.d1.d800.signal.control', conversion: 'decimate', fromRate: 1, toRate: 800 };
  const wire = card({ kind: 'wire', from: '/env.out', to: '/sparks.rate', wire: { kind: 'connection', scale: 4000, converters: [converter] } });
  assert.deepEqual(wire.fields, [
    ['from', '/env.out'],
    ['to', '/sparks.rate'],
    ['scale', '4000'],
    ['converter', 'core.decimate.d1.d800.signal.control (decimate, divisor 1 → 800)'],
  ]);
  assert.equal(converterText(converter), wire.fields[3][1]);
  const plain = card({ kind: 'wire', from: '/a.out', to: '/b.in', wire: { kind: 'connection', scale: null, converters: [] } });
  assert.deepEqual(plain.fields.slice(2), [['scale', '1'], ['converter', 'none']]);
  assert.equal(card({ kind: 'background' }), null);
});

// auditor-1-004: the fallback follows the latest describe (stopped, or before
// the stream reports), not the one the card was built with.
test('without a stream value, the base is the latest describe\u2019s', () => {
  const model = card({ kind: 'instance', path: '', node: node('', 'out'), address: '/out' }, { graph: GRAPH, description: DESCRIBE });
  const gain = model.parameters[0];
  assert.equal(rowValues(gain, new Map(), null, DESCRIBE).base, 0.75);
  const later = { ...DESCRIBE, controls: DESCRIBE.controls.map((c) => (c.address === '/out.gain' ? { ...c, base: 0.9 } : c)) };
  assert.equal(rowValues(gain, new Map(), null, later).base, 0.9);
  const env = card({ kind: 'instance', path: '/drums', node: node('drums', 'env'), address: '/drums/env' }, { graph: GRAPH, description: DESCRIBE }).parameters[0];
  const inner = { ...DESCRIBE, instances: [{ address: '/drums/env', parameters: [{ name: 'decay_ms', base: 333 }] }] };
  assert.equal(rowValues(env, new Map(), null, inner).base, 333);
  // A stream value still wins.
  assert.equal(rowValues(gain, new Map([['/out.gain', { base: 0.2, effective: 0.2 }]]), null, later).base, 0.2);
});

// M2.9-1b: `graph.error` shows its diagnostics over the last drawing, marked stale.
test('a project that stops loading: its diagnostics, the drawing marked stale', () => {
  const error = { type: 'graph.error', code: 'does-not-load', message: 'project.toml:3: expected `]`\nproject.toml:9: unknown module' };
  assert.deepEqual(staleNotice(error, true), {
    title: 'The project does not load; the drawing is stale',
    text: 'project.toml:3: expected `]`\nproject.toml:9: unknown module',
  });
  assert.equal(staleNotice(error, false).title, 'The project does not load', 'nothing drawn yet: nothing stale');
  assert.equal(staleNotice({ code: 'no-project', message: 'open a project folder first' }, true).title, 'No project is open; the drawing is stale');
});

// auditor-1-stale-svg-hidden: the SVG marked stale stays visible, dimmed;
// only the diagnostics box is hidden until it has something to say.
test('the stale drawing is dimmed, not hidden, by the schematic’s styles', () => {
  const source = readFileSync(new URL('../controls/schematic.js', import.meta.url), 'utf8');
  const css = source.slice(source.indexOf('const CSS = `'), source.indexOf('${ENTRY_CSS}'));
  const rules = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].map(([, selector, body]) => ({ selector: selector.trim(), body }));
  const onStale = rules.filter((r) => r.selector.split(',').some((s) => /(^|[\s>])(svg)?\.stale\b/.test(s.trim())));
  assert.deepEqual(onStale.map((r) => r.selector), ['svg.stale'], 'only one rule matches the stale SVG');
  assert.doesNotMatch(onStale[0].body, /visibility|display|position/);
  assert.match(onStale[0].body, /opacity/);
  assert.match(source, /html\('section', \{ className: 'diagnostics' \}\)/, 'the box over it has a class of its own');
});

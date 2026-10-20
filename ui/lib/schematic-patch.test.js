// M2.9-3 (docs/ui-controls.md "Editing the patch"): the ops the page sends,
// the default name of an added instance, the drag-to-connect target filter,
// the count of instances a composite edit reaches, and the palette entries.

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addConnection,
  addInstance,
  connectTargets,
  defaultName,
  instanceCount,
  nameError,
  namesOf,
  paletteEntries,
  paletteLevel,
  patchMessage,
  removeConnection,
  removeInstance,
  scaleConnection,
} from './schematic-patch.js';
import { searchPalette } from './panel-edit.js';
import { layout } from './schematic-layout.js';
import { nodeRect, scene } from './schematic-scene.js';
import { hitTest } from './schematic-hit.js';
import { fit, toLocal } from './schematic-view.js';
import { closedFit } from './schematic-open.js';

const port = (name, direction, shape = 'signal') => ({ name, direction, shape, kind: 'audio', rate: 1 });
const TOP = {
  instances: [
    { name: 'transport', module: 'std.transport', composite: false, ports: [port('beat', 'out', 'events')], parameters: [] },
    { name: 'lfo', module: 'std.lfo', composite: false, ports: [port('out', 'out')], parameters: [{ name: 'freq_hz' }] },
    { name: 'drums', module: 'drums', composite: true, ports: [port('trigger', 'in', 'events'), port('out', 'out')], parameters: [{ name: 'kick_decay' }] },
    { name: 'scope', module: 'std.particles', composite: false, ports: [port('pos', 'out', { vector: 8 }), port('feed', 'in', { vector: 8 })], parameters: [] },
    { name: 'out', module: 'std.audio_out', composite: false, ports: [port('l', 'in'), port('r', 'in')], parameters: [{ name: 'gain' }] },
  ],
  connections: [{ from: '/transport.beat', to: '/drums.trigger' }, { from: '/lfo.out', to: '/drums.kick_decay', scale: 120 }],
};

test('the ops are written as the protocol names them, endpoints without the leading /', () => {
  assert.deepEqual(addInstance('svf', 'std.svf', { x: 10.4, y: -3.6 }), { op: 'instance.add', name: 'svf', module: 'std.svf', x: 10, y: -4 });
  assert.deepEqual(addInstance('svf', 'std.svf', {}, { q: 2 }), { op: 'instance.add', name: 'svf', module: 'std.svf', params: { q: 2 } });
  assert.deepEqual(removeInstance('lfo'), { op: 'instance.remove', name: 'lfo' });
  assert.deepEqual(addConnection('/lfo.out', '/out.l'), { op: 'connection.add', from: 'lfo.out', to: 'out.l' });
  assert.deepEqual(removeConnection('osc.out', 'vca.a'), { op: 'connection.remove', from: 'osc.out', to: 'vca.a' });
  assert.deepEqual(scaleConnection('/lfo.out', '/drums.kick_decay', 60), { op: 'connection.scale', from: 'lfo.out', to: 'drums.kick_decay', scale: 60 });
  assert.deepEqual(patchMessage('kick', 'abc', [removeInstance('dc')]), { type: 'patch', level: 'kick', base: 'abc', ops: [{ op: 'instance.remove', name: 'dc' }] });
});

test('the default name is the type’s last segment with the lowest free _n suffix', () => {
  assert.equal(defaultName('std.svf', new Set()), 'svf');
  assert.equal(defaultName('std.svf', new Set(['svf'])), 'svf_2');
  assert.equal(defaultName('std.svf', new Set(['svf', 'svf_2', 'svf_4'])), 'svf_3');
  assert.equal(defaultName('kick', namesOf(TOP)), 'kick');
  assert.equal(defaultName('std.audio_out', namesOf(TOP)), 'audio_out');
  assert.equal(defaultName('lab.2d', new Set()), 'm_2d', 'made a valid name');
});

test('names match [a-z][a-z0-9_]* and are unique at the level', () => {
  const taken = namesOf(TOP);
  assert.equal(nameError('svf_2', taken), null);
  assert.match(nameError('Svf', taken), /lower-case/);
  assert.match(nameError('2svf', taken), /lower-case/);
  assert.match(nameError('', taken), /lower-case/);
  assert.match(nameError('lfo', taken), /already/);
});

test('a dragged output may go to inputs of its shape and, a signal, to parameters', () => {
  const targets = (instance, portName, shape) => connectTargets(TOP, { instance, port: portName, shape }).map((t) => `${t.instance}.${t.port}${t.parameter ? ' (parameter)' : ''}`);
  // Signal: every signal input and every parameter, but not lfo.out → drums.kick_decay again.
  assert.deepEqual(targets('lfo', 'out', 'signal'), ['lfo.freq_hz (parameter)', 'out.l', 'out.r', 'out.gain (parameter)']);
  // Events: event inputs only, without the existing beat → trigger.
  assert.deepEqual(targets('transport', 'beat', 'events'), []);
  assert.deepEqual(connectTargets({ ...TOP, connections: [] }, { instance: 'transport', port: 'beat', shape: 'events' }).map((t) => `${t.instance}.${t.port}`), ['drums.trigger']);
  // A vector: vector inputs of its shape only, never a parameter.
  assert.deepEqual(targets('scope', 'pos', { vector: 8 }), ['scope.feed']);
  // Inside a composite, endpoints have no leading /.
  const inside = { instances: [{ name: 'osc', ports: [port('out', 'out')], parameters: [] }, { name: 'vca', ports: [port('a', 'in'), port('b', 'in')], parameters: [] }], connections: [{ from: 'osc.out', to: 'vca.a' }] };
  assert.deepEqual(connectTargets(inside, { instance: 'osc', port: 'out', shape: 'signal' }).map((t) => `${t.instance}.${t.port}`), ['vca.b']);
});

test('a dragged pin as the layout gives it: a vector goes to vector inputs only', () => {
  // The shapes as graph gives them; the pin's shape as the layout names it.
  const level = {
    instances: [
      { name: 'mesh', module: 'std.mesh2d', composite: false, ports: [port('z', 'out', { vector: 256 }), port('out', 'out')], parameters: [] },
      { name: 'view', module: 'std.particles', composite: false, ports: [port('feed', 'in', { vector: 256 })], parameters: [] },
      { name: 'sig', module: 'std.gain', composite: false, ports: [port('in', 'in')], parameters: [{ name: 'gain' }] },
    ],
    connections: [],
  };
  const pins = (name) => layout(level).nodes.find((n) => n.name === name).outs;
  const vector = pins('mesh').find((p) => p.name === 'z');
  assert.equal(vector.shape, 'vector', 'the layout names the shape');
  const targets = connectTargets(level, { instance: 'mesh', port: vector.name, shape: vector.shape }).map((t) => `${t.instance}.${t.port}`);
  assert.deepEqual(targets, ['view.feed']);
  const signal = pins('mesh').find((p) => p.name === 'out');
  const fromSignal = connectTargets(level, { instance: 'mesh', port: signal.name, shape: signal.shape }).map((t) => `${t.instance}.${t.port}`);
  assert.deepEqual(fromSignal, ['sig.in', 'sig.gain']);
});

test('a double-click inside an open composite adds to that composite, where it was clicked', () => {
  const graph = {
    top: {
      instances: [
        { name: 'drums', module: 'drums', composite: true, ports: [port('out', 'out')], parameters: [] },
        { name: 'out', module: 'std.audio_out', composite: false, ports: [port('l', 'in')], parameters: [] },
      ],
      connections: [{ from: '/drums.out', to: '/out.l' }],
    },
    composites: {
      drums: {
        instances: [{ name: 'dc', module: 'std.dc_block', composite: false, ports: [port('in', 'in'), port('out', 'out')], parameters: [] }],
        connections: [],
        inputs: [],
        outputs: [{ name: 'out', from: 'dc.out', shape: 'signal' }],
        parameters: [],
      },
    },
  };
  const layouts = new Map();
  const layoutOf = (name) => {
    if (!layouts.has(name)) layouts.set(name, layout(name === '' ? graph.top : graph.composites[name]));
    return layouts.get(name);
  };
  const viewport = { w: 1000, h: 600 };
  const drawn = scene({ layoutOf, view: fit(layoutOf('').bounds, viewport), viewport, pinned: new Set(['/drums']) });
  assert.ok(drawn.open.has('/drums'), 'drums is open');
  const box = nodeRect(drawn, '/drums');
  // Inside drums' box, away from the instances drawn in it.
  const point = { x: box.x + box.w - 2, y: box.y + box.h - 2 };
  const hit = hitTest(drawn, point);
  assert.equal(hit.kind, 'instance', 'the inside of an open box hit-tests as the box');
  const level = paletteLevel(hit, drawn, point);
  assert.equal(level.level, 'drums');
  assert.equal(level.path, '/drums');
  const at = toLocal(level.transform, point);
  assert.ok(Number.isFinite(at.x) && Number.isFinite(at.y));
  // M2.13-3 (AC3): while drums is open, the top level's background around
  // it adds nowhere (it added to the top level before): only the innermost
  // open level is edited.
  const outside = { x: 2, y: 2 };
  assert.equal(hitTest(drawn, outside).kind, 'background');
  assert.equal(paletteLevel(hitTest(drawn, outside), drawn, outside), null);
  // Fitted as Fit does (M2.13-3, `closedFit`): a plain fit would now open drums by its readability.
  const closed = scene({ layoutOf, view: closedFit(layoutOf(''), layoutOf, viewport), viewport });
  const r = nodeRect(closed, '/drums');
  const center = { x: r.x + r.w / 2, y: r.y + r.h / 2 };
  assert.equal(paletteLevel(hitTest(closed, center), closed, center), null);
  // Nothing open, the background adds to the top level.
  assert.equal(paletteLevel(hitTest(closed, outside), closed, outside).level, '');
});

test('a composite level’s edit reaches every instance of it, through the nesting', () => {
  const graph = {
    top: { instances: [{ name: 'a', module: 'drums', composite: true }, { name: 'b', module: 'drums', composite: true }, { name: 'k', module: 'kick', composite: true }] },
    composites: { drums: { instances: [{ name: 'kick', module: 'kick', composite: true }] }, kick: { instances: [{ name: 'osc', module: 'std.sine_osc', composite: false }] } },
  };
  assert.equal(instanceCount(graph, ''), 1);
  assert.equal(instanceCount(graph, 'drums'), 2);
  assert.equal(instanceCount(graph, 'kick'), 3, 'one at the top, one in each drums');
});

test('the palette lists primitives then composites, searched by name and description', () => {
  const entries = paletteEntries({
    primitives: [{ module: 'std.svf', description: 'State-variable filter' }, { module: 'std.lfo', description: 'Low-frequency oscillator' }],
    composites: [{ module: 'kick', description: null, composite: true }],
  });
  assert.deepEqual(entries.map((e) => e.module), ['std.svf', 'std.lfo', 'kick']);
  assert.deepEqual(searchPalette(entries, 'filter').shown.map((e) => e.module), ['std.svf']);
  assert.deepEqual(searchPalette(entries, 'composite').shown.map((e) => e.module), ['kick']);
  // ADR 0027 §6: each with its palette group (a composite's the project's) and description.
  const grouped = paletteEntries({ primitives: [{ module: 'std.svf', category: 'filter', description: 'State-variable filter' }], composites: [{ module: 'kick', description: null, composite: true }] });
  assert.deepEqual(grouped.map((e) => [e.module, e.group, e.description]), [['std.svf', 'filter', 'State-variable filter'], ['kick', 'project', null]]);
});

test('a library composite from `modules` is added as any type: lib.<name>, named by its last segment', () => {
  const modules = {
    primitives: [],
    composites: [
      { module: 'drums', composite: true, description: null },
      { module: 'lib.tape_echo', composite: true, library: true, category: 'delay', description: 'A tape echo.' },
      { module: 'lib.svf_core', composite: true, library: true, category: 'filter', description: 'std.svf rebuilt.', reference_for: 'std.svf' },
    ],
  };
  const entries = paletteEntries(modules);
  assert.deepEqual(entries.map((e) => [e.module, e.group]), [['drums', 'project'], ['lib.tape_echo', 'library'], ['lib.svf_core', 'reference']]);
  assert.equal(entries[1].label, 'A tape echo.');
  const name = defaultName('lib.tape_echo', new Set(['tape_echo']));
  assert.equal(name, 'tape_echo_2');
  assert.deepEqual(addInstance(name, 'lib.tape_echo', { x: 1, y: 2 }), { op: 'instance.add', name: 'tape_echo_2', module: 'lib.tape_echo', x: 1, y: 2 });
});

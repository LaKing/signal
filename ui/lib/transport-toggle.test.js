import test from 'node:test';
import assert from 'node:assert/strict';

import { playheadRuns, toggleTransport, transportTargets } from './transport-toggle.js';

const playhead = (address) => ({
  address,
  module: 'std.playhead',
  parameters: [
    { name: 'bpm', address: `${address}.bpm` },
    { name: 'run', address: `${address}.run` },
    { name: 'rate', address: `${address}.rate` },
  ],
});
const control = (address, ports, base, more = {}) => ({ address, ports, base, min: 0, max: 1, default: 1, modulated: false, ...more });

/** studio-transport's: `lib.transport` exports its playhead's run as `/transport.run`. */
const studio = {
  playing: true,
  instances: [playhead('/transport/playhead'), { address: '/melody', module: 'std.note_seq', parameters: [{ name: 'ppq', address: '/melody.ppq' }] }],
  controls: [
    control('/transport.bpm', ['/transport/playhead.bpm'], 120, { min: 20, max: 400 }),
    control('/transport.run', ['/transport/playhead.run'], 1),
    control('/melody.ppq', ['/melody.ppq'], 96, { min: 1, max: 960 }),
  ],
};

test('the targets are the controls that reach a playhead\'s run', () => {
  assert.deepEqual([...playheadRuns(studio)], ['/transport/playhead.run']);
  assert.deepEqual(transportTargets(studio), [{ address: '/transport.run', base: 1, min: 0, max: 1 }]);
  assert.deepEqual(transportTargets(null), []);
  assert.deepEqual(transportTargets({ instances: [], controls: studio.controls }), [], 'no playhead, nothing');
  // A playhead whose run no control reaches (a parameter inside a composite, not exported).
  assert.deepEqual(transportTargets({ instances: studio.instances, controls: [studio.controls[0]] }), []);
});

test('Space stops a running transport and plays a stopped one', () => {
  assert.deepEqual(toggleTransport(transportTargets(studio)), { value: 0, sets: [{ address: '/transport.run', value: 0 }] });
  const stopped = { ...studio, controls: studio.controls.map((c) => (c.address === '/transport.run' ? { ...c, base: 0 } : c)) };
  assert.deepEqual(toggleTransport(transportTargets(stopped)), { value: 1, sets: [{ address: '/transport.run', value: 1 }] });
  assert.equal(toggleTransport([]), null, 'nothing to toggle');
  // A press before the next describe: what this page sent counts.
  const sent = new Map([['/transport.run', 0]]);
  assert.equal(toggleTransport(transportTargets(studio), sent).value, 1);
});

test('several transports toggle together; a wired run is skipped; no base is the default', () => {
  // two-studios: each include's transport is a control of its own.
  const two = {
    instances: [playhead('/a/transport/playhead'), playhead('/b/transport/playhead'), playhead('/c/playhead')],
    controls: [
      control('/a/transport.run', ['/a/transport/playhead.run'], 0),
      control('/b/transport.run', ['/b/transport/playhead.run'], 1),
      control('/c.run', ['/c/playhead.run'], 0, { modulated: true }),
    ],
  };
  const targets = transportTargets(two);
  assert.deepEqual(targets.map((t) => t.address), ['/a/transport.run', '/b/transport.run']);
  assert.deepEqual(toggleTransport(targets), { value: 0, sets: [{ address: '/a/transport.run', value: 0 }, { address: '/b/transport.run', value: 0 }] }, 'one runs: all stop');
  const none = { ...two, controls: two.controls.map((c) => ({ ...c, base: 0 })) };
  assert.equal(toggleTransport(transportTargets(none)).value, 1, 'none runs: all play');
  const fresh = { instances: [playhead('/p')], controls: [control('/p.run', ['/p.run'], undefined, { default: 1 })] };
  assert.equal(transportTargets(fresh)[0].base, 1);
  // Within the control's range.
  const narrow = { instances: [playhead('/p')], controls: [control('/p.run', ['/p.run'], 0.25, { min: 0.25, max: 0.75 })] };
  assert.deepEqual(toggleTransport(transportTargets(narrow)).sets, [{ address: '/p.run', value: 0.75 }]);
});

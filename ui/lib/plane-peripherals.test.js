// ADR 0051 §2–3, tests 2 and 3 (phase 1's part): the peripherals from the
// Setup and the inventory, with a scripted SSL 2+, a laptop output, no
// device and an absent device; the device's names; our signal direction
// (its playback channels are inputs on its box, its capture channels
// outputs); the role binding and the Setup-level cable it makes with the
// project's connection.

import test from 'node:test';
import assert from 'node:assert/strict';
import { MAX_PORT_ROWS, boundChannels, cableDrop, capturePorts, channelSummary, deviceBoxes, deviceEntry, endpointLabel, landingChannels, landingPorts, mainBinding, newRoleName, peripherals, playRoles, playbackPortOf, playbackPorts, setupCables, splitBoxes } from './plane-peripherals.js';

const config = (channels) => ({ channels, min_rate: 44100, max_rate: 192000, buffer: [15, 4096], sample_format: 'f32' });
const SSL_ID = 'coreaudio:AppleUSBAudioEngine:Solid State Logic:SSL 2+ MKII';
const ssl = (index, direction) => ({ index, name: 'SSL 2+ MKII', id: SSL_ID, direction, default: direction === 'output', rate: 48000, configs: [config(direction === 'output' ? 4 : 2)] });
const speakers = (index, isDefault = false) => ({ index, name: 'MacBook Pro Speakers', id: 'coreaudio:BuiltInSpeakerDevice', direction: 'output', default: isDefault, rate: 48000, configs: [config(2)] });
const mic = (index) => ({ index, name: 'MacBook Pro Microphone', id: 'coreaudio:BuiltInMicrophoneDevice', direction: 'input', default: true, rate: 48000, configs: [config(1)] });
const MAIN = [{ name: 'out', role: 'main', layout: 'stereo', ports: ['/out.l', '/out.r'] }];

test('ports are named as the device names them: playback in stereo pairs, capture per channel', () => {
  assert.deepEqual(playbackPorts(4).map((p) => [p.name, p.channels]), [
    ['Out 1–2', [1, 2]],
    ['Out 3–4', [3, 4]],
  ]);
  assert.deepEqual(playbackPorts(3).map((p) => p.name), ['Out 1–2', 'Out 3']);
  assert.deepEqual(capturePorts(2).map((p) => [p.name, p.channels]), [
    ['In 1', [1]],
    ['In 2', [2]],
  ]);
});

test('one box per device, in our direction: the SSL 2+’s Out 1–2 and Out 3–4 are inputs on its box, its In 1 and In 2 outputs', () => {
  const boxes = deviceBoxes([speakers(0), ssl(1, 'output'), ssl(2, 'input'), mic(3)]);
  assert.deepEqual(
    boxes.map((b) => ({ name: b.name, inputs: b.inputs.map((p) => p.name), outputs: b.outputs.map((p) => p.name), side: b.side })),
    [
      { name: 'MacBook Pro Speakers', inputs: ['Out 1–2'], outputs: [], side: 'right' },
      { name: 'SSL 2+ MKII', inputs: ['Out 1–2', 'Out 3–4'], outputs: ['In 1', 'In 2'], side: 'right' },
      { name: 'MacBook Pro Microphone', inputs: [], outputs: ['In 1'], side: 'left' },
    ],
  );
  assert.deepEqual(boxes[1].indexes, [1, 2]);
});

test('the scripted SSL 2+: main · stereo on its Out 1–2, cabled from the Stack’s edge pin to channels 1 and 2', () => {
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID, signature: '4 ch' }, outputChannels: null } };
  const devices = { devices: [speakers(0), ssl(1, 'output'), ssl(2, 'input')], output: { found: 1 } };
  const { boxes, bindings } = peripherals({ setup, devices, endpoints: MAIN });
  assert.deepEqual(bindings, [{ endpoint: 'out', role: 'main', layout: 'stereo', ports: ['/out.l', '/out.r'], device: `id:${SSL_ID}`, channels: [1, 2], heard: true, note: null }]);
  assert.equal(endpointLabel(bindings[0]), 'main · stereo');
  const box = boxes.find((b) => b.key === bindings[0].device);
  assert.equal(playbackPortOf(box, 1).name, 'Out 1–2');
  const pins = new Map([['primary', [{ side: 'out', instrument: 'card:master', address: '/limiter.out', label: 'out', outside: ['/out.l', '/out.r'] }]]]);
  assert.deepEqual(setupCables(pins, bindings), [
    { stack: 'primary', pin: '/limiter.out', endpoint: 'out', port: '/out.l', device: `id:${SSL_ID}`, channel: 1, dashed: false },
    { stack: 'primary', pin: '/limiter.out', endpoint: 'out', port: '/out.r', device: `id:${SSL_ID}`, channel: 2, dashed: false },
  ]);
});

test('the binding follows the Setup’s output_channels: main on Out 3–4 moves only the cable’s device end', () => {
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: [3, 4] } };
  const devices = { devices: [ssl(0, 'output')], output: { found: 0 } };
  const { boxes, bindings } = peripherals({ setup, devices, endpoints: MAIN });
  assert.deepEqual(bindings[0].channels, [3, 4]);
  assert.equal(playbackPortOf(boxes[0], bindings[0].channels[0]).name, 'Out 3–4');
  // A map the device cannot play is the identity, as the player takes it.
  assert.deepEqual(boundChannels([5, 6], 2, 4), [1, 2]);
  assert.deepEqual(boundChannels([1, 1], 2, 4), [1, 2]);
  assert.deepEqual(boundChannels([2], 2, 4), [1, 2]);
});

test('a laptop: no [output] in the Setup is the system’s default output', () => {
  const setup = { setup: { output: null, outputChannels: null } };
  const devices = { devices: [speakers(0, true), mic(1)], output: null };
  assert.deepEqual(mainBinding(setup, devices), { key: 'id:coreaudio:BuiltInSpeakerDevice', present: true, name: 'MacBook Pro Speakers', reason: null });
  const { bindings } = peripherals({ setup, devices, endpoints: MAIN });
  assert.equal(bindings[0].device, 'id:coreaudio:BuiltInSpeakerDevice');
  assert.equal(bindings[0].heard, true);
});

test('bound but absent: a hollow box with the Setup’s name, "not present", dashed cables', () => {
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: null } };
  const devices = { devices: [speakers(0, true)], output: { unresolved: 'the saved device SSL 2+ MKII is not present' } };
  const { boxes, bindings } = peripherals({ setup, devices, endpoints: MAIN });
  const absent = boxes.find((b) => !b.present);
  assert.equal(absent.name, 'SSL 2+ MKII');
  assert.match(absent.note, /^not present/);
  assert.deepEqual(absent.inputs.map((p) => p.name), ['Out 1–2']);
  assert.equal(bindings[0].device, absent.key);
  assert.equal(bindings[0].heard, false);
  const pins = new Map([['primary', [{ side: 'out', address: '/limiter.out', outside: ['/out.l', '/out.r'] }]]]);
  assert.ok(setupCables(pins, bindings).every((c) => c.dashed));
});

test('no audio device, or headless: role endpoints only, not heard on this machine, no cable to a device', () => {
  const { boxes, bindings } = peripherals({ setup: null, devices: null, endpoints: MAIN });
  assert.deepEqual(boxes, []);
  assert.equal(bindings[0].device, null);
  assert.equal(bindings[0].note, 'not heard on this machine');
  assert.deepEqual(setupCables(new Map([['primary', [{ side: 'out', address: '/limiter.out', outside: ['/out.l'] }]]]), bindings), []);
});

test('a device of many channels stays a box: past MAX_PORT_ROWS one row holds the rest, and a role on it still finds its port', () => {
  assert.deepEqual(playbackPorts(64).map((p) => p.name), ['Out 1–2', 'Out 3–4', 'Out 5–6', 'Out 7–8', 'Out 9–10', 'Out 11–64']);
  assert.deepEqual(capturePorts(64).map((p) => p.name), ['In 1', 'In 2', 'In 3', 'In 4', 'In 5', 'In 6–64']);
  assert.equal(playbackPorts(64).length, MAX_PORT_ROWS);
  assert.equal(playbackPorts(12).length, MAX_PORT_ROWS, 'six pairs fit');
  const [box] = deviceBoxes([{ index: 0, name: 'Loopback', id: 'l', direction: 'output', configs: [config(64)] }]);
  assert.equal(playbackPortOf(box, 33).name, 'Out 11–64');
});

// The operator's review of phase 1: whole boxes only for what is bound or cabled, the rest in one list.
const device = (index, name, id, direction, channels, isDefault = false) => ({ index, name, id, direction, default: isDefault, rate: 48000, configs: [config(channels)] });
const STUDIO_MAC = [
  device(0, 'DELL U3219Q', 'dell-1', 'output', 2),
  device(1, 'DELL U3219Q', 'dell-2', 'output', 2),
  ssl(2, 'output'),
  ssl(3, 'input'),
  device(4, 'BlackHole 64ch', 'blackhole', 'output', 64),
  device(5, 'BlackHole 64ch', 'blackhole', 'input', 64),
  device(6, 'Mac Studio Speakers', 'speakers', 'output', 2),
  device(7, 'Microsoft Teams Audio', 'teams', 'output', 1),
  device(8, 'AT2020USB+', 'mic', 'input', 2),
];

test('whole boxes only for the devices bound or cabled; the rest in one list, in inventory order', () => {
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: null } };
  const devices = { devices: STUDIO_MAC, output: { found: 2 } };
  const { boxes, bindings, main } = peripherals({ setup, devices, endpoints: MAIN });
  const pins = new Map([['primary', [{ side: 'out', instrument: 'card:master', address: '/limiter.out', label: 'out', outside: ['/out.l', '/out.r'] }]]]);
  const cables = setupCables(pins, bindings);
  const { full, others } = splitBoxes(boxes, { bindings, cables, main });
  assert.deepEqual(full.map((b) => b.name), ['SSL 2+ MKII']);
  assert.deepEqual(others.map((b) => b.name), ['DELL U3219Q', 'DELL U3219Q', 'BlackHole 64ch', 'Mac Studio Speakers', 'Microsoft Teams Audio', 'AT2020USB+']);
  assert.equal(full.length + others.length, boxes.length, 'every device drawn once');
  assert.deepEqual(others.map(channelSummary), ['2 out', '2 out', '64 out · 64 in', '2 out', '1 out', '2 in']);
});

test('the Setup’s output device is whole even when the project plays nothing; an absent bound device is whole', () => {
  // No role in the project: the Setup's output (here the system default) still stands whole.
  const setup = { setup: { output: null, outputChannels: null } };
  const devices = { devices: [device(0, 'DELL U3219Q', 'dell', 'output', 2), speakers(1, true), mic(2)], output: null };
  const { boxes, bindings, main } = peripherals({ setup, devices, endpoints: [] });
  const split = splitBoxes(boxes, { bindings, cables: [], main });
  assert.deepEqual(split.full.map((b) => b.name), ['MacBook Pro Speakers']);
  assert.deepEqual(split.others.map((b) => b.name), ['DELL U3219Q', 'MacBook Pro Microphone']);
  // Bound but absent: its hollow box is whole, the present ones listed.
  const gone = peripherals({ setup: { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID } } }, devices: { devices: [speakers(0, true)], output: { unresolved: 'not present' } }, endpoints: MAIN });
  const both = splitBoxes(gone.boxes, { bindings: gone.bindings, cables: [], main: gone.main });
  assert.deepEqual(both.full.map((b) => [b.name, b.present]), [['SSL 2+ MKII', false]]);
  assert.deepEqual(both.others.map((b) => b.name), ['MacBook Pro Speakers']);
  // No device at all: nothing whole, nothing listed.
  assert.deepEqual(splitBoxes([], {}), { full: [], others: [] });
});

// ADR 0038 (M4.1): several roles at once on the clock device.
const MAIN_AND_CUE = [...MAIN, { name: 'cue', role: 'cue', layout: 'stereo', ports: ['/cue.l', '/cue.r'] }];

test('the owner’s case: main on the SSL 2+’s Out 1–2 and cue on Out 3–4, each cabled from the Stack', () => {
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: [1, 2], play: { cue: { channels: [3, 4], device: null } } } };
  const devices = { devices: [ssl(0, 'output'), ssl(1, 'input')], output: { found: 0 } };
  const { boxes, bindings } = peripherals({ setup, devices, endpoints: MAIN_AND_CUE });
  assert.deepEqual(
    bindings.map((b) => [b.role, b.channels, b.heard, b.device]),
    [
      ['main', [1, 2], true, `id:${SSL_ID}`],
      ['cue', [3, 4], true, `id:${SSL_ID}`],
    ],
  );
  assert.equal(playbackPortOf(boxes[0], bindings[1].channels[0]).name, 'Out 3–4');
  const pins = new Map([['primary', [{ side: 'out', instrument: 'card:master', address: '/master.out', label: 'out', outside: ['/out.l', '/out.r', '/cue.l', '/cue.r'] }]]]);
  assert.deepEqual(
    setupCables(pins, bindings).map((c) => [c.port, c.channel]),
    [
      ['/out.l', 1],
      ['/out.r', 2],
      ['/cue.l', 3],
      ['/cue.r', 4],
    ],
  );
});

test('a role the machine does not bind is not heard; two roles on one channel are never summed', () => {
  const devices = { devices: [ssl(0, 'output')], output: { found: 0 } };
  const unbound = peripherals({ setup: { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID } } }, devices, endpoints: MAIN_AND_CUE });
  assert.deepEqual(unbound.bindings[1], { endpoint: 'cue', role: 'cue', layout: 'stereo', ports: ['/cue.l', '/cue.r'], device: null, channels: [], heard: false, note: 'not heard on this machine' });
  // cue on 2–3: `cue` < `main` in canonical order, so cue takes 2 and main loses it there (ADR 0038 §4).
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, play: { cue: { channels: [2, 3] } } } };
  const { bindings } = peripherals({ setup, devices, endpoints: MAIN_AND_CUE });
  assert.deepEqual(bindings[0].channels, [1, null]);
  assert.deepEqual(bindings[1].channels, [2, 3]);
  const pins = new Map([['primary', [{ side: 'out', address: '/m.out', outside: ['/out.l', '/out.r'] }]]]);
  assert.deepEqual(setupCables(pins, bindings).map((c) => c.channel), [1], 'a refused channel has no cable');
});

test('a role on another device is adapted (ADR 0052): drawn there, heard, its channels its own', () => {
  const devices = { devices: [ssl(0, 'output'), speakers(1)], output: { found: 0 } };
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, play: { cue: { channels: [1, 2], device: { name: 'MacBook Pro Speakers', id: 'coreaudio:BuiltInSpeakerDevice' } } } } };
  const { bindings } = peripherals({ setup, devices, endpoints: MAIN_AND_CUE });
  assert.equal(bindings[1].device, 'id:coreaudio:BuiltInSpeakerDevice');
  assert.equal(bindings[1].heard, true);
  assert.match(bindings[1].note, /clock adapter/);
  // cue's 1–2 on the speakers do not take main's 1–2 on the SSL 2+.
  assert.deepEqual(bindings[0].channels, [1, 2]);
  assert.deepEqual(bindings[1].channels, [1, 2]);
  const pins = new Map([['primary', [{ side: 'out', address: '/m.out', outside: ['/cue.l'] }]]]);
  assert.equal(setupCables(pins, bindings)[0].dashed, false);
});

test('a device named for main is the clock: main is drawn and heard there', () => {
  const devices = { devices: [ssl(0, 'output'), speakers(1)], output: { found: 0 } };
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID } }, local: { play: { main: { device: { name: 'MacBook Pro Speakers', id: 'coreaudio:BuiltInSpeakerDevice' } } } } };
  const { bindings } = peripherals({ setup, devices, endpoints: MAIN });
  assert.deepEqual([bindings[0].device, bindings[0].heard, bindings[0].channels], ['id:coreaudio:BuiltInSpeakerDevice', true, [1, 2]]);
});

test('devices.local.toml overrides the Setup key by key', () => {
  const roles = playRoles({ setup: { outputChannels: [1, 2], play: { cue: { channels: [3, 4], device: null } } }, local: { play: { cue: { channels: [4, 3] }, main: { channels: [3, 4] } } } });
  assert.deepEqual(roles, { cue: { channels: [4, 3], device: null }, main: { channels: [3, 4], device: null } });
});

test('a cable landing: its channels, its ports, a new role’s name, the device as the Setup names it', () => {
  const out34 = { channels: [3, 4] };
  assert.deepEqual(landingChannels(out34, 2), [3, 4]);
  assert.deepEqual(landingChannels(out34, 2, 4), [4, 5]);
  assert.deepEqual(landingChannels({ channels: [5] }, 1), [5]);
  assert.deepEqual(landingPorts('l', ['/cue.l', '/cue.r'], 1), ['/cue.l']);
  assert.deepEqual(landingPorts('out_r', ['/cue.l', '/cue.r'], 0), ['/cue.r']);
  assert.deepEqual(landingPorts('out', ['/cue.l', '/cue.r'], 1), ['/cue.r']);
  assert.equal(newRoleName([]), 'cue');
  assert.equal(newRoleName(['main', 'cue', 'cue_2']), 'cue_3');
  assert.deepEqual(deviceEntry({ devices: [ssl(0, 'output')] }, `id:${SSL_ID}`), { name: 'SSL 2+ MKII', id: SSL_ID, signature: '' });
  assert.equal(deviceEntry({ devices: [] }, 'id:x'), null);
});

test('cable drags: create a new output on free channels, join a bound role, move a binding, remove a connection', () => {
  const devices = { devices: [ssl(0, 'output'), speakers(1)], output: { found: 0 } };
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: null } };
  const { boxes, bindings, main } = peripherals({ setup, devices, endpoints: MAIN });
  const box = boxes.find((b) => b.key === `id:${SSL_ID}`);
  const out34 = { ...playbackPortOf(box, 3), channels: [3, 4] };
  const pin = { side: 'out', address: '/limiter.out', label: 'out', outside: ['/out.l', '/out.r'] };
  const context = { bindings, instances: ['limiter', 'out'], main, devices };

  // The master's pin on the SSL's free Out 3–4: a cue device slot, fed on both channels, bound to 3–4 of the clock device.
  const created = cableDrop({ kind: 'create', pin }, { box: box.key, port: out34, channel: 3 }, context);
  assert.deepEqual(created.ops, [
    { op: 'device.add', name: 'cue', play: 'stereo', role: 'cue' },
    { op: 'connection.add', from: 'limiter.out', to: 'cue.l' },
    { op: 'connection.add', from: 'limiter.out', to: 'cue.r' },
  ]);
  assert.deepEqual(created.route, { type: 'device.route', role: 'cue', channels: [3, 4], device: null, scope: 'machine' });

  // A paired pin (l) feeds its side, and its sibling (r) the other.
  const left = { address: '/master.l', label: 'l', outside: [] };
  const right = { address: '/master.r', label: 'r', outside: [] };
  const paired = cableDrop({ kind: 'create', pin: left, siblings: [right] }, { box: box.key, port: out34, channel: 4 }, { ...context, scope: 'project' });
  assert.deepEqual(paired.ops.slice(1), [
    { op: 'connection.add', from: 'master.l', to: 'cue.l' },
    { op: 'connection.add', from: 'master.r', to: 'cue.r' },
  ]);
  assert.equal(paired.route.scope, 'project');

  // On a port a role plays on: only the connections, never a binding.
  const out12 = playbackPortOf(box, 1);
  const joined = cableDrop({ kind: 'create', pin: { address: '/echo.out', label: 'out', outside: [] } }, { box: box.key, port: out12, channel: 2 }, context);
  assert.deepEqual(joined, { ops: [{ op: 'connection.add', from: 'echo.out', to: 'out.r' }], route: null, note: null });
  assert.match(cableDrop({ kind: 'create', pin }, { box: box.key, port: out12, channel: 1 }, context).note, /already plays main/);

  // A cable's device end moved: only the binding; on another device, that device is named.
  const cable = setupCables(new Map([['primary', [pin]]]), bindings)[0];
  const moved = cableDrop({ kind: 'move', cable }, { box: box.key, port: out34, channel: 3 }, context);
  assert.deepEqual(moved, { ops: [], route: { type: 'device.route', role: 'main', channels: [3, 4], device: null, scope: 'machine' }, note: 'main on Out 3–4' });
  const away = cableDrop({ kind: 'move', cable }, { box: 'id:coreaudio:BuiltInSpeakerDevice', port: { name: 'Out 1–2', channels: [1, 2] }, channel: 1 }, context);
  assert.deepEqual(away.route.device, { name: 'MacBook Pro Speakers', id: 'coreaudio:BuiltInSpeakerDevice', signature: '' });
  assert.deepEqual(cableDrop({ kind: 'move', cable }, { box: box.key, port: out12, channel: 1 }, context), { ops: [], route: null, note: null }, 'where it is: nothing');

  // Dropped on the empty plane: the connection goes, the binding stays.
  assert.deepEqual(cableDrop({ kind: 'move', cable }, null, context).ops, [{ op: 'connection.remove', from: 'limiter.out', to: 'out.l' }]);
  assert.deepEqual(cableDrop({ kind: 'create', pin }, null, context), { ops: [], route: null, note: null });
});

// ADR 0038 §7, the owner's "connect the AT2020 to the Mac Studio Speakers" (2026-11-06).
test('an input dragged onto free channels of a device makes two explicit slots and two bindings', () => {
  const devices = { devices: [ssl(0, 'output'), speakers(1), mic(2)], output: { found: 0 } };
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID } } };
  const { bindings, main, boxes } = peripherals({ setup, devices, endpoints: MAIN });
  const micBox = boxes.find((b) => b.outputs.length && !b.inputs.length);
  const speakerBox = boxes.find((b) => b.name === 'MacBook Pro Speakers');
  const drop = { box: speakerBox.key, port: speakerBox.inputs[0], channel: 1 };
  const result = cableDrop({ kind: 'capture', box: micBox.key, port: micBox.outputs[0] }, drop, { bindings, instances: ['out'], main, devices });
  assert.deepEqual(result.ops, [
    { op: 'device.add', name: 'input', capture: 'mono', role: 'input' },
    { op: 'device.add', name: 'cue', play: 'stereo', role: 'cue' },
    { op: 'connection.add', from: 'input.m', to: 'cue.l' },
    { op: 'connection.add', from: 'input.m', to: 'cue.r' },
  ]);
  assert.equal(result.routes.length, 2);
  assert.deepEqual([result.routes[0].side, result.routes[0].role, result.routes[0].channels], ['capture', 'input', [1]]);
  assert.equal(result.routes[0].device.name, micBox.name);
  assert.deepEqual([result.routes[1].role, result.routes[1].channels, result.routes[1].device.name], ['cue', [1, 2], 'MacBook Pro Speakers']);
});

test('an input dragged onto a role already playing is wired into it; nothing dropped writes nothing', () => {
  const devices = { devices: [ssl(0, 'output'), mic(1)], output: { found: 0 } };
  const setup = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID } } };
  const { bindings, main, boxes } = peripherals({ setup, devices, endpoints: MAIN });
  const micBox = boxes.find((b) => b.outputs.length && !b.inputs.length);
  const sslBox = boxes.find((b) => b.key === main.key);
  const drag = { kind: 'capture', box: micBox.key, port: micBox.outputs[0] };
  const result = cableDrop(drag, { box: sslBox.key, port: sslBox.inputs[0], channel: 1 }, { bindings, instances: ['out'], main, devices });
  assert.deepEqual(result.ops, [
    { op: 'device.add', name: 'input', capture: 'mono', role: 'input' },
    { op: 'connection.add', from: 'input.m', to: 'out.l' },
    { op: 'connection.add', from: 'input.m', to: 'out.r' },
  ]);
  assert.equal(result.routes.length, 1);
  const nothing = cableDrop(drag, null, { bindings, main, devices });
  assert.deepEqual([nothing.ops, nothing.routes], [[], []]);
  assert.match(nothing.note, /drop an input/);
});

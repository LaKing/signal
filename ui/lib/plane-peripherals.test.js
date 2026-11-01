// ADR 0051 §2–3, tests 2 and 3 (phase 1's part): the peripherals from the
// Setup and the inventory, with a scripted SSL 2+, a laptop output, no
// device and an absent device; the device's names; our signal direction
// (its playback channels are inputs on its box, its capture channels
// outputs); the role binding and the Setup-level cable it makes with the
// project's connection.

import test from 'node:test';
import assert from 'node:assert/strict';
import { boundChannels, capturePorts, deviceBoxes, endpointLabel, mainBinding, peripherals, playbackPortOf, playbackPorts, setupCables } from './plane-peripherals.js';

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

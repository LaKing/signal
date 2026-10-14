// The parameter list as the M2.5-2 panel had it (crates/signal-app/tests/
// panel.rs): per instance, its parameters editable and a composite's inner
// ones read-only; a drag is a 5 ms ramp, the release one commit, Reset a
// commit of null; "(not in file)" without a value in the file.

import test from 'node:test';
import assert from 'node:assert/strict';
import { RAMP_MS, dragMessage, fileNote, releaseMessage, resetMessage, rows, shown, valuesOf } from './parameters.js';

// The kick project's describe, in short.
const description = {
  controls: [
    { address: '/drums.kick_decay', instance: 'drums', parameter: 'kick_decay', module: 'drums', unit: 'ms', min: 1, max: 5000, default: 420, base: 420, inFile: false, modulated: true },
    { address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', unit: null, min: 0, max: 4, default: 1, base: 0.75, inFile: true, modulated: false },
  ],
  instances: [
    { address: '/drums/kick/amp_env', parameters: [{ name: 'decay_ms', address: '/drums/kick/amp_env.decay_ms', unit: 'ms', range: { min: 1, max: 5000 }, default: 200, base: 420 }] },
    { address: '/out', parameters: [{ name: 'gain', address: '/out.gain', range: { min: 0, max: 4 }, default: 1, base: 0.75 }] },
  ],
};

test('instances with their parameters, and a composite’s inside read-only', () => {
  const list = rows(description);
  assert.deepEqual(list.map((r) => r.name), ['drums', 'out']);
  const [drums, out] = list;
  assert.equal(drums.parameters[0].address, '/drums.kick_decay');
  assert.equal(drums.parameters[0].editable, true);
  assert.deepEqual(drums.inner.map((p) => [p.name, p.editable, p.base]), [['kick/amp_env.decay_ms', false, 420]]);
  assert.deepEqual(out.inner, [], 'a primitive has nothing inside');
});

test('"(not in file)" for a parameter without a value in project.toml', () => {
  const [drums, out] = rows(description);
  assert.equal(fileNote(drums.parameters[0]), '(not in file)');
  assert.equal(fileNote(out.parameters[0]), '');
});

test('a drag ramps over 5 ms, the release commits once, Reset commits null', () => {
  assert.equal(RAMP_MS, 5);
  assert.deepEqual(dragMessage('/out.gain', 0.5), { type: 'ramp', address: '/out.gain', to: 0.5, ms: 5 });
  assert.deepEqual(releaseMessage('/out.gain', 0.5), { type: 'commit', address: '/out.gain', value: 0.5 });
  assert.deepEqual(resetMessage('/drums.kick_decay'), { type: 'commit', address: '/drums.kick_decay', value: null });
});

test('effective values by address, and how they read', () => {
  const values = valuesOf(['/out.gain', '/drums.kick_decay'], { values: [{ base: 0.75, effective: 0.75 }, { base: 420, effective: 391.2345 }] });
  assert.equal(values.get('/drums.kick_decay').effective, 391.2345);
  assert.equal(shown(391.2345, 'ms'), '391.2 ms');
  assert.equal(shown(420, 'ms'), '420 ms');
  assert.equal(shown(Number.NaN, null), '—');
});

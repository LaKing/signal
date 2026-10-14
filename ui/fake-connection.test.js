import test from 'node:test';
import assert from 'node:assert/strict';

import { FakeConnection } from './fake-connection.js';
import { findParameter } from './signal-ui.js';

function timers() {
  const t = { intervals: 0, setInterval: () => ++t.intervals, clearInterval: () => {} };
  return t;
}

test('nothing runs until start()', () => {
  const clock = timers();
  const fake = new FakeConnection({ timers: clock });
  fake.subscribe({ stream: 'meters' }, () => assert.fail('a frame before start'));
  assert.equal(clock.intervals, 0);
  assert.equal(fake.status, 'connecting');
  fake.start();
  assert.equal(clock.intervals, 1);
  assert.equal(fake.status, 'open');
});

test('edits change the base; one control is modulated', async () => {
  const fake = new FakeConnection({ timers: timers() });
  const frames = [];
  fake.subscribe({ stream: 'parameters', addresses: ['/synth.level', '/synth.drive', '/nope'] }, (f) => frames.push(f.values));
  await fake.set('/synth.level', 9);
  await fake.commit('/synth.drive', 0.8);
  fake.frameNumber = 48000; // a second in: the LFO is away from its middle
  fake.tick();
  const [level, drive, unknown] = frames[0];
  assert.deepEqual(level, { base: 4, effective: 4 }, 'clamped to the range');
  assert.equal(drive.base, 0.8);
  assert.notEqual(drive.effective, drive.base);
  assert.ok(Number.isNaN(unknown.base));
  await assert.rejects(fake.set('/nope', 1), { code: 'unknown-address' });
  assert.equal(findParameter(fake.description, '/synth.cutoff').unit, 'Hz');
});

test('meters clip, scopes share frames, triggers are recorded', async () => {
  const fake = new FakeConnection({ timers: timers() });
  const peaks = [];
  const inputs = [];
  const blocks = {};
  const chain = [];
  fake.subscribe({ stream: 'meters' }, (f) => peaks.push(f.values[0].peak));
  fake.subscribe({ stream: 'meters', source: 'input' }, (f) => inputs.push(f.values.length));
  fake.subscribe({ stream: 'scope', port: '/osc.out', rate: 12000 }, (f) => {
    blocks.x = f;
    chain.push(f);
  });
  fake.subscribe({ stream: 'scope', port: '/lfo.out', rate: 12000 }, (f) => (blocks.y = f));
  for (let i = 0; i < 120; i++) fake.tick();
  assert.ok(Math.max(...peaks) >= 1, 'a peak reaches full scale');
  assert.ok(inputs.length > 0 && inputs.every((n) => n === 1), 'the input meter has its own channel');
  assert.equal(blocks.x.extra, 4);
  assert.equal(blocks.x.firstFrame, blocks.y.firstFrame);
  assert.equal(blocks.x.values.length, 256);
  // Each scope block follows the last, as the server's do.
  for (let i = 1; i < chain.length; i++) assert.equal(chain[i].firstFrame, chain[i - 1].firstFrame + 256 * 4);
  await fake.trigger('/synth.trigger');
  assert.equal(fake.triggers.length, 1);
  let refused = null;
  fake.subscribe({ stream: 'scope', port: '/synth.cutoff' }, () => {}, { onError: (e) => (refused = e.code) });
  assert.equal(refused, 'not-a-signal-output');
});

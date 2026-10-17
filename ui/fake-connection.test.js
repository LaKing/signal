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

test('vector streams arrive at 60 snapshots per second on whole frames', () => {
  const fake = new FakeConnection({ timers: timers() });
  const frames = [];
  let reply = null;
  fake.subscribe({ stream: 'vector', port: '/membrane.z' }, (f) => frames.push(f), { onSubscribed: (r) => (reply = r) });
  for (let i = 0; i < 30; i++) fake.tick();
  assert.equal(reply.rows, 256);
  assert.equal(reply.decimation, 800);
  assert.equal(frames.length, 60);
  assert.ok(frames.every((f) => f.kind === 4 && f.values.length === 256 && f.firstFrame % 800 === 0));
  let refused = null;
  fake.subscribe({ stream: 'vector', port: '/osc.out' }, () => {}, { onError: (e) => (refused = e.code) });
  assert.equal(refused, 'not-a-vector-output');
});

test('patterns live in memory: read, write on the base, conflict, refusal', async () => {
  const fake = new FakeConnection({ timers: timers() });
  const melody = await fake.readPattern('melody');
  assert.equal(melody.exists, true);
  assert.equal(melody.kind, 'notes');
  assert.equal(melody.sha, melody.base);
  assert.match(melody.sha, /^[0-9a-f]{16}$/);
  assert.equal((await fake.readPattern('drums')).kind, 'steps');

  const pushed = [];
  fake.on('pattern.written', (m) => pushed.push(m));
  const text = melody.text.replace('key = 67', 'key = 69');
  const written = await fake.writePattern('melody', text, melody.base);
  assert.notEqual(written.sha, melody.sha);
  assert.equal(written.base, written.sha);
  assert.equal(written.created, false);
  assert.deepEqual(await fake.readPattern('melody'), { text, sha: written.sha, base: written.sha, exists: true, kind: 'notes' });
  assert.deepEqual(pushed, [{ type: 'pattern.written', name: 'melody', base: written.sha, text }]);

  // The old base is stale now.
  await assert.rejects(fake.writePattern('melody', melody.text, melody.base), (error) => {
    assert.equal(error.code, 'conflict');
    assert.equal(error.reply.base, written.sha);
    assert.equal(error.reply.text, text);
    return true;
  });
  await assert.rejects(fake.writePattern('melody', 'schema_version = 1\n', written.sha), { code: 'does-not-load' });
  await assert.rejects(fake.readPattern('../etc'), { code: 'bad-value' });

  // A missing pattern reads as not existing, and a first write names no base.
  assert.deepEqual(await fake.readPattern('fresh'), { text: null, sha: null, base: null, exists: false, kind: null });
  await assert.rejects(fake.writePattern('fresh', melody.text, 'abc'), { code: 'conflict' });
  assert.equal((await fake.writePattern('fresh', melody.text, null)).created, true);
  assert.equal(new FakeConnection({ timers: timers(), patterns: {} }).patterns.size, 0);
});

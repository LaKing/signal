// The M2.5-1 meter ballistics in JS, against the Rust reference numbers:
// each scenario and tolerance below is one of crates/signal-app/tests/meter.rs.

import test from 'node:test';
import assert from 'node:assert/strict';

import { ChannelMeter, blocksFromFrame, dbfs } from './ballistics.js';

const RATE = 48000;

function block(firstFrame, frames, peak, sumSquares) {
  return { firstFrame, frames, peak, sumSquares };
}

/** `f64::from(x as f32)`: the peaks are f32 in Rust. */
const f32 = Math.fround;

test('the peak is held for one and a half seconds (meter.rs)', () => {
  const meter = new ChannelMeter(RATE);
  meter.push(block(0, 128, 0.5, 0));
  const hold = Math.trunc(1.5 * RATE);
  assert.equal(hold, 72000);
  // Quieter blocks up to exactly 1.5 s after the loud one ended.
  let at = 128;
  while (at + 128 <= 128 + hold) {
    meter.push(block(at, 128, 0.1, 0));
    at += 128;
  }
  let r = meter.reading();
  assert.ok(Math.abs(r.held - dbfs(0.5)) < 1e-9, `held: ${r.held}`);
  assert.ok(Math.abs(r.peak - dbfs(0.1)) < 1e-6);
  assert.equal(r.peak, dbfs(f32(0.1)), 'the same f32 peak as Rust');
  // The next block is past the hold: the held peak falls to it.
  meter.push(block(at, 128, 0.1, 0));
  assert.ok(Math.abs(meter.reading().held - dbfs(0.1)) < 1e-6);
  // A louder block is held at once.
  meter.push(block(at + 128, 128, 0.8, 0));
  r = meter.reading();
  assert.ok(Math.abs(r.held - dbfs(0.8)) < 1e-6);
});

test('a full-scale sine reads −3.01 dBFS RMS (meter.rs)', () => {
  const meter = new ChannelMeter(RATE);
  let phase = 0;
  for (let b = 0; b < RATE / 128; b++) {
    const samples = Array.from({ length: 128 }, (_, i) => Math.sin((2 * Math.PI * 1000 * (phase + i)) / RATE));
    phase += 128;
    const peak = samples.reduce((m, x) => Math.max(m, Math.abs(x)), 0);
    const sum = samples.reduce((s, x) => s + x * x, 0);
    meter.push(block(b * 128, 128, peak, sum));
  }
  const r = meter.reading();
  assert.ok(Math.abs(r.rms - -3.0103) < 0.01, `rms ${r.rms}`);
  assert.ok(Math.abs(r.peak) < 0.01, `peak ${r.peak}`);
});

test('the clip indicator latches until cleared (meter.rs)', () => {
  const meter = new ChannelMeter(RATE);
  meter.push(block(0, 64, 0.9, 1.0));
  assert.equal(meter.reading().clip, false);
  meter.push(block(64, 64, 1.0, 1.0));
  assert.equal(meter.reading().clip, true);
  for (let b = 2; b < 1000; b++) meter.push(block(b * 64, 64, 0.1, 0.1));
  assert.equal(meter.reading().clip, true, 'latched long after');
  meter.clearClip();
  assert.equal(meter.reading().clip, false);
});

test('silence reads minus infinity (meter.rs)', () => {
  const meter = new ChannelMeter(RATE);
  let r = meter.reading();
  assert.deepEqual([r.peak, r.held, r.rms], [-Infinity, -Infinity, -Infinity]);
  meter.push(block(0, 128, 0, 0));
  r = meter.reading();
  assert.deepEqual([r.peak, r.held, r.rms], [-Infinity, -Infinity, -Infinity]);
  assert.equal(dbfs(1), 0);
});

test('the RMS covers the newest blocks of the 300 ms window', () => {
  // 14 400 frames at 48 kHz. Blocks of 4 800: three loud (sum 4800, mean
  // square 1) are followed by one quiet (sum 0). The window keeps the newest
  // blocks whose total stays at least 14 400 without the oldest: after the
  // fourth block, the last three (frames 14 400, sum 9 600).
  const meter = new ChannelMeter(RATE);
  for (let b = 0; b < 3; b++) meter.push(block(b * 4800, 4800, 1, 4800));
  assert.ok(Math.abs(meter.reading().rms - 0) < 1e-12);
  meter.push(block(14400, 4800, 0, 0));
  assert.ok(Math.abs(meter.reading().rms - dbfs(Math.sqrt(9600 / 14400))) < 1e-12);
});

test('a protocol meter frame becomes one block per channel', () => {
  const frame = { firstFrame: 4800, extra: 1600, values: [{ peak: 0.5, rms: 0.25 }, { peak: 1, rms: 0.5 }] };
  assert.deepEqual(blocksFromFrame(frame), [
    { firstFrame: 4800, frames: 1600, peak: 0.5, sumSquares: 0.0625 * 1600 },
    { firstFrame: 4800, frames: 1600, peak: 1, sumSquares: 0.25 * 1600 },
  ]);
  const meter = new ChannelMeter(RATE);
  meter.push(blocksFromFrame(frame)[0]);
  assert.ok(Math.abs(meter.reading().rms - dbfs(0.25)) < 1e-12, 'the RMS comes back');
});

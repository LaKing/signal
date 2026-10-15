import test from 'node:test';
import assert from 'node:assert/strict';
import { COMPACT_WIDTH, compactStatus, fit, statusFields } from './status.js';

const extremes = [
  null,
  {},
  { frame: 0, elapsed: 0, xruns: 0, loadAverage: 0, loadWorst: 0, clipped: 0, buffer: 128, threads: 4, workers: 3, scheduling: 'workgroup' },
  { frame: 48_000, elapsed: 61.5, xruns: 3, loadAverage: 0.125, loadWorst: 0.5, clipped: 2, buffer: 8192, threads: 18, workers: 17, scheduling: 'time-constraint' },
  { frame: 1e15, elapsed: 1e9, xruns: 1e6, loadAverage: 50, loadWorst: 1e9, clipped: 1e12, buffer: 1e6, threads: 1e4, workers: 1e4, scheduling: 'a scheduling name far longer than its field' },
  { frame: NaN, elapsed: -1, xruns: -5, loadAverage: NaN, loadWorst: Infinity },
];

// M2.8-11 (docs/ui-controls.md "Stable layout"): the toolbar's status never changes width.
test('the compact status is always the same width', () => {
  for (const status of extremes) {
    for (const power of ['on', 'off']) {
      for (const playing of [false, true]) {
        const text = compactStatus({ power, playing, status });
        assert.equal(text.length, COMPACT_WIDTH, `"${text}"`);
      }
    }
  }
  assert.equal(compactStatus({ power: 'off' }).trim(), '⏻ off');
  assert.equal(compactStatus({ playing: false }).trim(), '■ stopped');
  assert.equal(compactStatus({ playing: true, status: extremes[3] }), '▶   1:01    13%  xr 3    ');
  assert.equal(compactStatus({ playing: true, status: extremes[4] }), '▶ 999:59  999+%  xr 9999+');
});

test('each status field keeps its width', () => {
  const widths = statusFields(null).map(([, text]) => text.length);
  for (const status of extremes) {
    assert.deepEqual(statusFields(status).map(([, text]) => text.length), widths, JSON.stringify(status));
  }
  assert.deepEqual(statusFields(extremes[3]).map(([label]) => label), ['frame', 'elapsed', 'xruns', 'load avg', 'load worst', 'clipped', 'buffer', 'threads', 'scheduling']);
  assert.equal(statusFields(extremes[3])[3][1], ' 12.5 %');
});

test('fit pads or cuts to the width', () => {
  assert.equal(fit('ab', 4), 'ab  ');
  assert.equal(fit('ab', 4, 'start'), '  ab');
  assert.equal(fit('abcdef', 4), 'abc…');
});

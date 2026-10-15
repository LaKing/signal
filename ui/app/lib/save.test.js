import test from 'node:test';
import assert from 'node:assert/strict';
import { saveReport, saveRequests } from './save.js';

const unsaved = {
  count: 3,
  parameters: [
    { address: '/out.gain', control: true, error: 'Permission denied' },
    { address: '/drums.kick_decay', control: true, error: null },
    { address: '/drums/kick/amp_env.decay_ms', control: false, error: null },
  ],
};

// AC14: Save writes every unsaved parameter through the existing retry.
test('Save retries every unsaved control', () => {
  assert.deepEqual(saveRequests(unsaved), [
    { type: 'retry', address: '/out.gain' },
    { type: 'retry', address: '/drums.kick_decay' },
  ]);
  assert.deepEqual(saveRequests(null), []);
});

test('it says "saved", or what was not', () => {
  assert.deepEqual(saveReport({ count: 0, parameters: [] }, []), { saved: true, text: 'saved' }, 'nothing unsaved');
  const controlsOnly = { parameters: unsaved.parameters.slice(0, 2) };
  assert.deepEqual(saveReport(controlsOnly, [{ address: '/out.gain' }, { address: '/drums.kick_decay', error: { code: 'not-unsaved', message: 'already saved' } }]), { saved: true, text: 'saved' });
  const report = saveReport(unsaved, [{ address: '/out.gain', error: { code: 'write-failed', message: 'Permission denied' } }, { address: '/drums.kick_decay' }]);
  assert.equal(report.saved, false);
  assert.equal(report.text, 'not saved: /out.gain: Permission denied; /drums/kick/amp_env.decay_ms: a live edit of a port inside the plan, not in project.toml');
});

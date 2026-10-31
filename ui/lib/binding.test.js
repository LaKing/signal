// M2.8-8: a control whose commit is refused stays unsaved and shows why,
// until a commit (or retry, or revert) is confirmed; the host's unsaved
// state is authoritative; no commit is swallowed by `quietly`.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { Binding } from './binding.js';

/** A connection whose requests settle as the test says. */
function fakeConnection() {
  const sent = [];
  const answer = (type) => (address, value, options) => {
    let settle;
    const promise = new Promise((resolve, reject) => {
      settle = { resolve, reject };
    });
    // retry(address, options) and revert(address, options) take no value.
    const opts = typeof value === 'object' && value !== null ? value : options;
    sent.push({ type, address, value, options: opts, promise, ...settle });
    return promise;
  };
  return {
    sent,
    commit: answer('commit'),
    retry: answer('retry'),
    revert: answer('revert'),
    set: answer('set'),
    ramp: answer('ramp'),
    last(type) {
      return sent.filter((s) => s.type === type).at(-1);
    },
  };
}

/** Every reply handled (a macrotask later). */
function settled() {
  return new Promise((resolve) => setImmediate(resolve));
}

function refusal(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function bound() {
  const connection = fakeConnection();
  const element = { connection, dragging: false, getAttribute: () => null };
  const binding = new Binding(element, '/out.gain');
  let changes = 0;
  binding.onChange = () => {
    changes += 1;
  };
  return { binding, connection, changes: () => changes };
}

const WRITE_FAILED = 'out.gain was not written to /p/project.toml: Permission denied (os error 13)';

test('a refused commit keeps the edit unsaved with its error, until a commit is confirmed', async () => {
  const { binding, connection, changes } = bound();
  binding.setValue(0.125, 'commit');
  const first = connection.last('commit');
  assert.equal(first.value, 0.125);
  first.reject(refusal('write-failed', WRITE_FAILED));
  await settled();
  assert.deepEqual(binding.unsaved, { error: WRITE_FAILED });
  assert.equal(changes(), 1);

  // The host lists it with the same error: still unsaved.
  binding.fromHost({ count: 1, parameters: [{ address: '/out.gain', error: WRITE_FAILED, control: true }], lastError: WRITE_FAILED });
  assert.deepEqual(binding.unsaved, { error: WRITE_FAILED });

  // A later commit is confirmed: saved.
  binding.setValue(0.25, 'commit');
  const second = connection.last('commit');
  second.resolve({ type: 'committed', address: '/out.gain', value: 0.25 });
  await settled();
  assert.equal(binding.unsaved, null);
});

test('a debounced commit and a reset are kept the same way', async () => {
  const { binding, connection } = bound();
  binding.pending = true;
  binding.base = 0.5;
  binding.flush();
  const flushed = connection.last('commit');
  assert.equal(flushed.value, 0.5);
  flushed.reject(refusal('write-failed', WRITE_FAILED));
  await settled();
  assert.deepEqual(binding.unsaved, { error: WRITE_FAILED });

  // A commit lost with its connection is unsaved too: the host never had it.
  binding.reset();
  const reset = connection.last('commit');
  reset.reject(refusal('closed', 'connection closed'));
  await settled();
  assert.deepEqual(binding.unsaved, { error: 'connection closed' });
});

test('retry and revert are sent, and a confirmed one clears the mark', async () => {
  const { binding, connection } = bound();
  binding.setUnsaved({ error: WRITE_FAILED });
  binding.retry();
  const retry = connection.last('retry');
  assert.equal(retry.address, '/out.gain');
  retry.reject(refusal('write-failed', 'still read-only'));
  await settled();
  assert.deepEqual(binding.unsaved, { error: 'still read-only' });

  binding.revert();
  const revert = connection.last('revert');
  assert.equal(revert.address, '/out.gain');
  revert.resolve({ type: 'reverted', address: '/out.gain', value: 1, sent: true });
  await settled();
  assert.equal(binding.unsaved, null);

  // Nothing unsaved at the host: nothing to show.
  binding.setUnsaved({ error: WRITE_FAILED });
  binding.retry();
  const nothing = connection.last('retry');
  nothing.reject(refusal('not-unsaved', '/out.gain is not unsaved'));
  await settled();
  assert.equal(binding.unsaved, null);
});

test('the host is authoritative: its error is shown, a parameter it does not list is saved', () => {
  const { binding } = bound();
  // Another client's failed write: shown here too (a page reload sees it).
  binding.describe({ controls: [{ address: '/out.gain', min: 0, max: 4, default: 1, base: 0.125 }], unsaved: { count: 1, parameters: [{ address: '/out.gain', error: WRITE_FAILED, applied: false, control: true }], lastError: WRITE_FAILED } });
  assert.deepEqual(binding.unsaved, { error: WRITE_FAILED });
  assert.equal(binding.base, 0.125);
  // Listed without an error (an edit not committed yet): a refusal seen here stays.
  binding.fromHost({ count: 1, parameters: [{ address: '/out.gain', error: null, control: true }] });
  assert.deepEqual(binding.unsaved, { error: WRITE_FAILED });
  // No longer listed (retried or reverted elsewhere): saved.
  binding.fromHost({ count: 0, parameters: [], lastError: null });
  assert.equal(binding.unsaved, null);
});

const UI = fileURLToPath(new URL('..', import.meta.url));

function sources(dir = UI, found = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) sources(path, found);
    else if (/\.(js|html)$/.test(entry.name) && !entry.name.endsWith('.test.js')) found.push(path);
  }
  return found;
}

test('no commit is swallowed by quietly', () => {
  const swallowed = [];
  for (const path of sources()) {
    const text = readFileSync(path, 'utf8');
    for (const match of text.matchAll(/quietly\(([^;]*)\)/g)) {
      if (/\.commit\(|\.retry\(|\.revert\(/.test(match[1])) swallowed.push(`${relative(UI, path)}: ${match[0]}`);
    }
  }
  assert.deepEqual(swallowed, []);
});

// External review (A, ADR 0049): a delayed edit is written only into the
// opening of the project it began under. The commit names the generation
// the edit began with, so the host refuses it once another project (or a
// new opening of this one) is open, instead of writing it there.
test('a debounced commit names the generation its edit began under', async () => {
  const { binding, connection } = bound();
  connection.generation = 7;
  binding.setValue(0.3, 'nudge');
  // Another project opens before the debounce ends.
  connection.generation = 8;
  binding.flush();
  const commit = connection.last('commit');
  assert.equal(commit.value, 0.3);
  assert.deepEqual(commit.options, { generation: 7 });
  commit.resolve({ type: 'committed' });
  await settled();
  // A new edit begins under the new opening.
  binding.setValue(0.4, 'nudge');
  binding.flush();
  assert.deepEqual(connection.last('commit').options, { generation: 8 });
  connection.last('commit').resolve({ type: 'committed' });
  await settled();
});

test('a drag released after another project opened names the opening it began under', async () => {
  const { binding, connection } = bound();
  connection.generation = 3;
  binding.setValue(0.2, 'ramp');
  connection.generation = 4;
  binding.setValue(0.25, 'commit');
  assert.deepEqual(connection.last('commit').options, { generation: 3 });
  connection.last('commit').resolve({ type: 'committed' });
  await settled();
  // A commit with no edit before it: the opening now.
  binding.setValue(0.5, 'commit');
  assert.deepEqual(connection.last('commit').options, { generation: 4 });
  binding.retry();
  assert.deepEqual(connection.last('retry').options, { generation: 4 });
  connection.last('commit').resolve({ type: 'committed' });
  connection.last('retry').resolve({ type: 'committed' });
  await settled();
});

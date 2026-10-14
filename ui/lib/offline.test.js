// Everything the UI library and the project panels load is local (no
// network, no CDN) and plain ES modules (no npm, no bare import specifiers;
// the library uses relative ones, so the gallery also loads from a file).

import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const UI = fileURLToPath(new URL('..', import.meta.url));

/** The library's files: tests are Node-only and may name remote URLs as data. */
function files(dir = UI, found = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) files(path, found);
    else if (/\.(html|js|css|svg)$/.test(entry.name) && !entry.name.endsWith('.test.js')) found.push(path);
  }
  return found;
}

const sources = files().map((path) => ({ name: relative(UI, path), text: readFileSync(path, 'utf8') }));

test('the library is there to scan', () => {
  for (const name of ['signal-ui.js', 'gallery.html', 'controls/index.js', 'lib/taper.js']) {
    assert.ok(sources.some((s) => s.name === name), name);
  }
});

test('no remote resources', () => {
  for (const { name, text } of sources) {
    // XML namespace names are identifiers, not resources.
    const remote = text.match(/https?:\/\/(?!www\.w3\.org\/)[^\s'"`)<>]*/gi);
    assert.equal(remote, null, `${name} names ${remote}`);
    if (!name.endsWith('.js')) {
      assert.doesNotMatch(text, /(?:src|href)\s*=\s*["']?\s*\/\//i, `${name}: a protocol-relative src or href`);
      assert.doesNotMatch(text, /url\(\s*['"]?\s*\/\//i, `${name}: a protocol-relative url()`);
      assert.doesNotMatch(text, /@import/i, `${name}: @import`);
    }
  }
});

test('browser modules import only relative paths', () => {
  for (const { name, text } of sources) {
    // ui/index.html is the M2.8-1 raw protocol page, served only by `signal ui`.
    if (name === 'index.html' || !/\.(js|html)$/.test(name)) continue;
    const specifiers = [...text.matchAll(/\bimport\s+(?:[\w*{}\s,$]+\s+from\s+)?['"]([^'"]+)['"]/g), ...text.matchAll(/\bimport\(\s*['"]([^'"]+)['"]/g)].map((m) => m[1]);
    for (const specifier of specifiers) {
      assert.match(specifier, /^\.\.?\//, `${name} imports "${specifier}"`);
    }
  }
});

// Project panels (projects/*/ui/) are served beside the library: the same
// rules, except that they import the library by its served path (/ui/…).
const PROJECTS = fileURLToPath(new URL('../../projects/', import.meta.url));
const panels = readdirSync(PROJECTS)
  .map((project) => join(PROJECTS, project, 'ui'))
  .filter((dir) => existsSync(dir))
  .flatMap((dir) => files(dir))
  .map((path) => ({ name: relative(PROJECTS, path), text: readFileSync(path, 'utf8') }));

test('project panels load nothing remote and import only the library or their own files', () => {
  assert.ok(panels.some((p) => p.name === 'kick/ui/main.html'));
  assert.ok(panels.some((p) => p.name === 'pulse/ui/main.html'));
  for (const { name, text } of panels) {
    const remote = text.match(/https?:\/\/(?!www\.w3\.org\/)[^\s'"`)<>]*/gi);
    assert.equal(remote, null, `${name} names ${remote}`);
    assert.doesNotMatch(text, /(?:src|href)\s*=\s*["']?\s*\/\//i, `${name}: a protocol-relative src or href`);
    const specifiers = [...text.matchAll(/\bimport\s+(?:[\w*{}\s,$]+\s+from\s+)?['"]([^'"]+)['"]/g)].map((m) => m[1]);
    for (const specifier of specifiers) assert.match(specifier, /^(?:\/ui\/|\.\.?\/)/, `${name} imports "${specifier}"`);
  }
});

test('no npm', () => {
  for (const name of ['package.json', 'package-lock.json', 'node_modules']) {
    assert.ok(!existsSync(join(UI, name)), name);
  }
});

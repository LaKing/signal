// ADR 0053: the main window's page holds the session token, so no module it
// loads may run project content: no frame (or object or embed) is made, and
// no URL of the project's pages (`/project/…`) is loaded as a document. The
// page's whole module graph is walked from ui/app/index.html's scripts and
// every module read as text. Panel windows (ui/app/panel-window.html, a page
// of its own with the panel token) are not part of it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ui = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const page = resolve(ui, 'app/index.html');

/** The relative imports of module text `text` (static and dynamic), resolved from `from`. */
function imports(text, from) {
  const found = [];
  for (const m of text.matchAll(/(?:\bimport\s*(?:[^'"()]*?\bfrom\s*)?|\bimport\s*\(\s*|\bexport\s+[^'"]*?\bfrom\s*)['"](\.{1,2}\/[^'"]+|\/ui\/[^'"]+)['"]/g)) {
    const spec = m[1];
    found.push(spec.startsWith('/ui/') ? resolve(ui, spec.slice('/ui/'.length)) : resolve(dirname(from), spec));
  }
  return found;
}

/** Every module the main page loads, by path, with its text. */
function graph() {
  const html = readFileSync(page, 'utf8');
  const scripts = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n');
  const seen = new Map();
  const todo = imports(scripts, page);
  while (todo.length) {
    const file = todo.pop();
    if (seen.has(file) || !file.endsWith('.js')) continue;
    const text = readFileSync(file, 'utf8');
    seen.set(file, text);
    todo.push(...imports(text, file));
  }
  return seen;
}

test('the main page never makes a frame of project content: no module it loads creates a frame, object or embed, or loads /project/ as a document', () => {
  const modules = graph();
  const names = [...modules.keys()].map((f) => f.slice(ui.length + 1));
  assert.ok(names.includes('controls/plane.js'), 'the plane is part of it');
  assert.ok(names.includes('app/lib/cards.js'));
  const made = /(?:createElement(?:NS)?\(\s*(?:[^,()]+,\s*)?|\bhtml\(\s*|\bel\(\s*)['"](?:iframe|frame|object|embed)['"]|<(?:iframe|frame|object|embed)\b/i;
  // `panelSource` builds a panel window's frame URL (its own page, its own token): never called here.
  const calls = /\bpanelSource\s*\(/;
  for (const [file, text] of modules) {
    const name = file.slice(ui.length + 1);
    const code = text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '').replace(/\/\/ .*$/gm, '');
    assert.doesNotMatch(code, made, `${name} makes a frame`);
    if (name !== 'app/lib/menu.js') assert.doesNotMatch(code, calls, `${name} builds a panel frame's URL`);
    assert.doesNotMatch(code, /\.src\s*=\s*['"`]\/project\//, `${name} loads a project page`);
  }
});

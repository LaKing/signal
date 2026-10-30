// <signal-schematic>'s Show in Schematic (Signal.app's context menu, the
// owner's 2026-10-29): `revealable` and `revealAddress`, driven through the
// real reveal, selection and render code with the least of a DOM it needs
// (as ./schematic.test.js does).

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from '../lib/schematic-layout.js';
import { fit } from '../lib/schematic-view.js';
import { KICK } from '../lib/schematic-scene.test.js';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSchematic } = await import('./schematic.js');

const viewport = { w: 1000, h: 600 };

function schematic() {
  const top = { ...KICK.top, instances: [...KICK.top.instances, { name: 'lib', module: 'lib.voice', composite: true, ports: [], parameters: [] }] };
  const layouts = new Map();
  const layoutOf = (level) => {
    if (!layouts.has(level)) layouts.set(level, level === '' ? layout(top) : KICK.composites[level] ? layout(KICK.composites[level]) : null);
    return layouts.get(level);
  };
  const s = Object.create(SignalSchematic.prototype);
  const classes = new Set();
  Object.assign(s, {
    graph: { project: 'kick' },
    viewport,
    view: fit(layoutOf('').bounds, viewport),
    fitted: true,
    open: new Set(),
    pinned: new Set(),
    selected: [],
    gesture: null,
    tracing: null,
    groups: new Map(),
    order: [],
    editor: { editing: false, placing: false, starts: () => false, key: () => false },
    menu: { list: {} },
    svg: { getBoundingClientRect: () => ({ left: 0, top: 0 }), classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle() {}, has: (c) => classes.has(c) }, replaceChildren() {} },
  });
  s.layoutOf = layoutOf;
  s.select = (parts) => (s.selected = parts);
  for (const name of ['focus', 'hover', 'say', 'closeMenu', 'collectBars', 'applyMarks', 'applyTrace', 'applyDrag', 'drawCrumbs', 'showSide', 'applyBars', 'updateLevels', 'measure']) s[name] = () => {};
  s.drawLevel = (level) => {
    if (!s.groups.has(level.path)) s.groups.set(level.path, { g: {}, nodeCache: new Map() });
    return false;
  };
  s.sideStateKey = () => '';
  s.schedule = () => s.render();
  s.render();
  return s;
}

const described = (selected) => selected.map((h) => (h.kind === 'wire' ? `wire ${h.from} → ${h.to}` : `${h.kind} ${h.address}`));

test('what can be revealed: an instance or a pin of a level in the graph; not inside a library composite, not a pin it lacks', () => {
  const s = schematic();
  assert.equal(s.revealable('/drums'), true);
  assert.equal(s.revealable('/drums.kick_decay'), true);
  assert.equal(s.revealable('/drums/kick/env.decay_ms'), true);
  assert.equal(s.revealable('/drums.nothing'), false);
  assert.equal(s.revealable('/lib/inner'), false, 'a library composite’s inside is not in the graph');
  assert.equal(s.revealable('/nope'), false);
  assert.equal(s.revealAddress({ kind: 'pin', address: '/nope.x' }), false);
});

test('a wired parameter: its pin selected with its wire, and flashed', () => {
  const s = schematic();
  assert.equal(s.revealAddress({ kind: 'pin', address: '/drums.kick_decay' }), true);
  assert.deepEqual(described(s.selected), ['port /drums.kick_decay', 'wire /lfo.out → /drums.kick_decay']);
  assert.equal(s.svg.classList.has('shown-flash'), true);
  clearTimeout(s.flashTimer);
});

test('a wire: it and both its ends selected', () => {
  const s = schematic();
  assert.equal(s.revealAddress({ kind: 'wire', from: '/drums.out', to: '/out.l' }), true);
  assert.deepEqual(described(s.selected), ['wire /drums.out → /out.l', 'port /drums.out', 'port /out.l']);
  clearTimeout(s.flashTimer);
});

test('an instance inside composites: they open down to it, and it is selected', () => {
  const s = schematic();
  assert.equal(s.revealAddress({ kind: 'instance', address: '/drums/kick/env' }), true);
  assert.deepEqual(described(s.selected), ['instance /drums/kick/env']);
  assert.ok(s.pinned.has('/drums') && s.pinned.has('/drums/kick'), 'opened down to it');
  assert.equal(s.view.path, '/drums/kick');
  clearTimeout(s.flashTimer);
});

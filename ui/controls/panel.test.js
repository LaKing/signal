// M2.8-11 (docs/ui-controls.md "Stable layout"): the generated panel is
// rebuilt only when its structure changes, not on every plan version. Drives
// <signal-panel generated>'s own `described` with the least of a DOM it needs.

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {
  attachShadow() {
    const parts = new Map();
    this.shadowRoot = {
      innerHTML: '',
      querySelector(selector) {
        if (!parts.has(selector)) parts.set(selector, { textContent: '', title: '', className: '', style: {} });
        return parts.get(selector);
      },
    };
    return this.shadowRoot;
  }
  hasAttribute(name) {
    return this.attributes?.has(name) ?? false;
  }
};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalPanel } = await import('./panel.js');

function description(planVersion, gain, extra = [], project = 'tone') {
  const control = (instance, parameter, base) => ({ address: `/${instance}.${parameter}`, instance, parameter, module: 'std.x', unit: null, min: 0, max: 4, base, ports: [`/${instance}.${parameter}`] });
  return {
    project,
    planVersion,
    controls: [control('osc', 'freq', 440), control('out', 'gain', gain), ...extra.map((p) => control('osc', p, 0))],
    sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 2 }],
    instances: [{ address: '/out', ports: [{ name: 'l', address: '/out.l', direction: 'in', shape: 'signal' }, { name: 'r', address: '/out.r', direction: 'in', shape: 'signal' }] }],
  };
}

test('a describe with the same structure keeps the generated panel; a new structure rebuilds it', () => {
  const panel = new SignalPanel();
  panel.attributes = new Set(['generated']);
  let builds = 0;
  panel.generate = () => {
    builds += 1;
  };
  panel.described(description(0, 0.5));
  assert.equal(builds, 1, 'built once');
  panel.described(description(0, 0.75));
  panel.described(description(1, 1.25));
  panel.described(description(7, 2));
  assert.equal(builds, 1, 'new values and plan versions with the same structure: no rebuild');
  panel.described(description(8, 2, ['phase']));
  assert.equal(builds, 2, 'a new control rebuilds');
  panel.described(description(9, 2, ['phase']));
  assert.equal(builds, 2);
});

// auditor-1-generated-rebuild: the host names the project in `describe` only
// while playing, so Play and Stop change `project` (null → name → null) and
// nothing the generated panel shows.
test('Play and Stop do not rebuild the generated panel', () => {
  const panel = new SignalPanel();
  panel.attributes = new Set(['generated']);
  let builds = 0;
  panel.generate = () => {
    builds += 1;
  };
  panel.described(description(null, 0.5, [], null));
  panel.described(description(0, 0.5, [], 'tone'));
  panel.described(description(0, 0.75, [], 'tone'));
  panel.described(description(null, 0.75, [], null));
  assert.equal(builds, 1, 'stopped → playing → stopped, same controls, meters and scope: one build');
  assert.equal(panel.shadowRoot.querySelector('.title').textContent, '', 'the title still follows the project');
  panel.described(description(1, 0.75, ['phase'], 'tone'));
  assert.equal(builds, 2, 'a new structure rebuilds');
});

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
        if (!parts.has(selector)) {
          const attributes = new Map();
          parts.set(selector, {
            textContent: '',
            title: '',
            className: '',
            style: {},
            setAttribute: (name, value) => attributes.set(name, String(value)),
            getAttribute: (name) => attributes.get(name) ?? null,
          });
        }
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

// M2.9-2 (docs/ui-controls.md "Editing a panel"): the bar's Edit layout
// switch can be used once there is a plan, and says why not before.
test('Edit layout is usable once described with a plan', () => {
  const panel = new SignalPanel();
  panel.attributes = new Set(['generated']);
  panel.generate = () => {};
  const edit = panel.shadowRoot.querySelector('.edit');
  panel.described({ ...description(null, 0.5), instances: [] });
  assert.equal(edit.getAttribute('aria-disabled'), 'true');
  assert.match(edit.title, /no plan/);
  panel.described(description(0, 0.5));
  assert.equal(edit.getAttribute('aria-disabled'), 'false');
});

// ADR 0046 §5: the host's binding check counts the host panel's controls
// only (a slot's part, drawn inside its <signal-slot>, is a file of its
// own); one undo history across the host's and the parts' editors; a
// part's events are its own editor's.
test("the host's controls leave out a part's; one history across the files; a part's events are its own", async () => {
  const { controlsOf } = await import('./panel.js');
  const { SharedHistory, inPart } = await import('./panel-editor.js');
  const slot = { nodeType: 1, localName: 'signal-slot', parentElement: { closest: () => null } };
  const element = (name, inSlot) => ({ nodeType: 1, localName: name, parentElement: { closest: (q) => (q === 'signal-slot' && inSlot ? slot : null) } });
  const knob = element('signal-knob', false);
  const partKnob = element('signal-knob', true);
  const root = { querySelectorAll: () => [element('div', false), knob, slot, partKnob, element('signal-panel', false)] };
  assert.deepEqual(controlsOf(root), [knob, slot]);

  const steps = [];
  const editor = (name) => ({ name, editing: true, stepOwn: (d) => steps.push(`${name} ${d}`) });
  const host = editor('host');
  const part = editor('part');
  const shared = new SharedHistory(host);
  shared.step(-1);
  assert.deepEqual(steps, ['host -1'], 'nothing written yet: the host’s own history');
  shared.wrote(host);
  shared.wrote(part);
  shared.step(-1);
  shared.step(-1);
  shared.step(1);
  assert.deepEqual(steps.slice(1), ['part -1', 'host -1', 'host 1'], 'the last written first, redone in turn');
  shared.wrote(part);
  const before = steps.length;
  shared.step(1);
  assert.equal(steps.length, before, 'a new write drops the redo');

  const panel = { nodeType: 1 };
  const tools = { nodeType: 1, dataset: { partTools: '' }, localName: 'div', parentElement: null };
  assert.equal(inPart({ composedPath: () => [partKnob, slot, panel] }, panel), true);
  assert.equal(inPart({ composedPath: () => [tools, slot, panel] }, panel), true);
  assert.equal(inPart({ composedPath: () => [knob, panel] }, panel), false);
  assert.equal(inPart({ composedPath: () => [slot, panel] }, panel), false, 'the slot card itself is the host’s');
});

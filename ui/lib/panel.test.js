import test from 'node:test';
import assert from 'node:assert/strict';

import { BindingCheck, controlSpec, defaultPanel, formatDiagnostic, panelFile } from './panel.js';

test('a page names its panel file by its path under /project/', () => {
  assert.equal(panelFile('/project/main.html'), 'main.html');
  assert.equal(panelFile('/project/mixer.html'), 'mixer.html');
  assert.equal(panelFile('/project/assets/x.html'), null);
  assert.equal(panelFile('/ui/gallery.html'), null);
  assert.equal(panelFile('/project/main.js'), null);
  assert.equal(panelFile(undefined), null);
});

test('a diagnostic reads as signal ui check prints it', () => {
  const d = { severity: 'error', file: 'ui/main.html', line: 12, column: 21, message: 'unknown port /drums.kick_decya: /drums has kick_decay, kick_drive', control: 0 };
  assert.equal(formatDiagnostic(d), 'ui/main.html:12:21: error: unknown port /drums.kick_decya: /drums has kick_decay, kick_drive');
});

// The kick project's describe, cut to what the default panel reads.
const KICK = {
  project: 'kick',
  planVersion: 0,
  instances: [
    { address: '/drums/kick/amp_env', parameters: [{ address: '/drums/kick/amp_env.decay_ms', kind: 'control', unit: 'ms' }], ports: [] },
    { address: '/drums/kick/shaper', parameters: [{ address: '/drums/kick/shaper.drive', kind: 'gain', unit: null }], ports: [] },
    { address: '/drums/kick/dc', parameters: [], ports: [{ name: 'out', address: '/drums/kick/dc.out', direction: 'out', shape: 'signal' }] },
    { address: '/transport', parameters: [{ address: '/transport.bpm', kind: 'control', unit: 'bpm' }], ports: [] },
    {
      address: '/out',
      parameters: [{ address: '/out.gain', kind: 'gain', unit: null }],
      ports: [
        { name: 'l', address: '/out.l', direction: 'in', shape: 'signal' },
        { name: 'r', address: '/out.r', direction: 'in', shape: 'signal' },
      ],
    },
  ],
  controls: [
    { address: '/transport.bpm', instance: 'transport', parameter: 'bpm', module: 'std.transport', ports: ['/transport.bpm'], unit: 'bpm', min: 1, max: 999 },
    { address: '/drums.kick_decay', instance: 'drums', parameter: 'kick_decay', module: 'drums', ports: ['/drums/kick/amp_env.decay_ms'], unit: 'ms', min: 0.1, max: 60000 },
    { address: '/drums.kick_drive', instance: 'drums', parameter: 'kick_drive', module: 'drums', ports: ['/drums/kick/shaper.drive'], unit: null, min: 0.1, max: 20 },
    { address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', ports: ['/out.gain'], unit: null, min: 0, max: 4 },
  ],
  sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 2 }],
};

test('the default panel groups the top-level parameters by instance, with meters and a scope', () => {
  const panel = defaultPanel(KICK);
  assert.equal(panel.project, 'kick');
  assert.deepEqual(
    panel.groups.map((g) => [g.instance, g.module, g.controls.map((c) => c.bind)]),
    [
      ['transport', 'std.transport', ['/transport.bpm']],
      ['drums', 'drums', ['/drums.kick_decay', '/drums.kick_drive']],
      ['out', 'std.audio_out', ['/out.gain']],
    ],
  );
  assert.deepEqual(panel.meters, [
    { bind: '/out.l', label: 'l' },
    { bind: '/out.r', label: 'r' },
  ]);
  // The sink's input itself (the sum of what feeds it), not one source.
  assert.deepEqual(panel.scope, { bind: '/out.l', label: '/out.l' });
});

test('levels get faders and everything else knobs, with a taper by unit', () => {
  const spec = (address) => controlSpec(KICK.controls.find((c) => c.address === address), KICK);
  assert.deepEqual(spec('/out.gain'), { element: 'signal-fader', bind: '/out.gain', label: 'gain', taper: 'db' });
  assert.deepEqual(spec('/drums.kick_decay'), { element: 'signal-knob', bind: '/drums.kick_decay', label: 'kick_decay', taper: 'log' });
  assert.deepEqual(spec('/transport.bpm'), { element: 'signal-knob', bind: '/transport.bpm', label: 'bpm', taper: 'linear' });
  // The shaper's drive has no unit and its plan parameter is a gain.
  assert.equal(spec('/drums.kick_drive').element, 'signal-fader');
  assert.equal(controlSpec({ address: '/fb.db', parameter: 'db', unit: 'dB', min: -96, max: 24, ports: [] }).element, 'signal-fader');
  assert.equal(controlSpec({ address: '/fb.db', parameter: 'db', unit: 'dB', min: -96, max: 24, ports: [] }).taper, 'linear');
  assert.equal(controlSpec({ address: '/d.time_ms', parameter: 'time_ms', unit: 'ms', min: 0, max: 2000, ports: [] }).taper, 'linear');
});

test('a project without a sink still gets its parameters', () => {
  const panel = defaultPanel({ controls: [{ address: '/a.freq', instance: 'a', parameter: 'freq', unit: 'Hz', min: 0, max: 24000, ports: [] }] });
  assert.equal(panel.groups.length, 1);
  assert.deepEqual(panel.meters, []);
  assert.equal(panel.scope, null);
  assert.deepEqual(defaultPanel(null), { project: null, groups: [], slots: [], meters: [], scope: null });
});

/** A BindingCheck whose requests are answered by the test. */
function harness() {
  const requests = [];
  const shown = [];
  const check = new BindingCheck({
    request: (message) =>
      new Promise((resolve, reject) => requests.push({ message, resolve, reject })),
    show: (reply, error) => shown.push(reply ?? { error: error.code }),
  });
  return { check, requests, shown };
}

const closed = () => Object.assign(new Error('connection closed'), { code: 'closed' });
const REPLY = { type: 'panel.check', file: 'ui/main.html', ok: false, diagnostics: [{ severity: 'error', file: 'ui/main.html', line: 3, column: 22, message: 'unknown port', control: 0 }] };

test('the binding check is asked once per plan version', async () => {
  const { check, requests, shown } = harness();
  const asked = check.described({ planVersion: 0 }, 'main.html');
  assert.equal(check.described({ planVersion: 0 }, 'main.html'), null, 'already asking');
  assert.deepEqual(requests[0].message, { type: 'panel.check', file: 'main.html' });
  requests[0].resolve(REPLY);
  await asked;
  assert.deepEqual(shown, [REPLY]);
  assert.equal(check.described({ planVersion: 0 }, 'main.html'), null, 'a pushed describe of the same plan');
  check.described({ planVersion: 1 }, 'main.html');
  assert.equal(requests.length, 2, 'a new plan is checked again');
});

test('a check the connection dropped is asked again on the reconnect describe, and then shown', async () => {
  const { check, requests, shown } = harness();
  const first = check.described({ planVersion: 0 }, 'main.html');
  requests[0].reject(closed());
  await first;
  assert.deepEqual(shown, [], 'nothing to show for a check that did not happen');
  // The client asks describe again once welcomed back: the same plan version.
  const again = check.described({ planVersion: 0 }, 'main.html');
  assert.equal(requests.length, 2);
  requests[1].resolve(REPLY);
  await again;
  assert.deepEqual(shown, [REPLY]);
});

test('a refused check is shown and not asked again for that plan', async () => {
  const { check, requests, shown } = harness();
  const asked = check.described({ planVersion: 0 }, 'gone.html');
  requests[0].reject(Object.assign(new Error('no panel'), { code: 'unknown-panel' }));
  await asked;
  assert.deepEqual(shown, [{ error: 'unknown-panel' }]);
  assert.equal(check.described({ planVersion: 0 }, 'gone.html'), null);
});

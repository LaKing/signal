import test from 'node:test';
import assert from 'node:assert/strict';
import { FLASH_CLASS, FLASH_CSS, isInstance, matchIn, missingReason, placesOf, subjectAddresses, subjectOf, tabItems, within } from './show-in.js';
import { contextItems } from './context.js';

const PANEL = ['/drums.kick_decay', '/synth/osc.freq', '/out.l'];
const PARAMETERS = ['/drums', '/drums.kick_decay', '/drums/kick.tone', '/synth', '/synth.level'];

test('instance and pin addresses', () => {
  assert.equal(isInstance('/drums/kick'), true);
  assert.equal(isInstance('/drums/kick.out'), false);
  assert.equal(isInstance('/drums'), true);
  assert.equal(within('/drums/kick.out', '/drums'), true);
  assert.equal(within('/drums.decay', '/drums'), true);
  assert.equal(within('/drumsx.decay', '/drums'), false, 'a name prefix is not inside');
});

test('the subject of a right click: a schematic hit, else a control', () => {
  assert.deepEqual(subjectOf({ hit: { kind: 'instance', address: '/drums/kick', node: {} } }), { kind: 'instance', address: '/drums/kick' });
  assert.deepEqual(subjectOf({ hit: { kind: 'port', address: '/drums/kick.out' } }), { kind: 'pin', address: '/drums/kick.out' });
  assert.deepEqual(subjectOf({ hit: { kind: 'group', address: '/synth' } }), { kind: 'instance', address: '/synth' });
  assert.deepEqual(subjectOf({ hit: { kind: 'wire', from: '/lfo.out', to: '/synth.level' } }), { kind: 'wire', from: '/lfo.out', to: '/synth.level' });
  assert.equal(subjectOf({ hit: { kind: 'background', path: '' } }), null);
  assert.equal(subjectOf({ hit: { kind: 'bundle', wires: [] } }), null);
  assert.deepEqual(subjectOf({ control: { address: '/drums.kick_decay', label: 'decay' } }), { kind: 'pin', address: '/drums.kick_decay' });
  assert.deepEqual(subjectOf({ control: { address: '/drums', part: true } }), { kind: 'instance', address: '/drums' });
  assert.equal(subjectOf({}), null);
  assert.deepEqual(subjectAddresses({ kind: 'wire', from: '/a.out', to: '/b.in' }), ['/a.out', '/b.in']);
  assert.deepEqual(subjectAddresses(null), []);
});

test('a parameter: its control or its row', () => {
  const s = { kind: 'pin', address: '/drums.kick_decay' };
  assert.deepEqual(matchIn(s, PANEL), { addresses: ['/drums.kick_decay'], whole: false });
  assert.deepEqual(matchIn(s, PARAMETERS), { addresses: ['/drums.kick_decay'], whole: false });
  assert.deepEqual(matchIn({ kind: 'pin', address: '/osc.out' }, PANEL), { addresses: [], whole: false }, 'a pin has no inside');
});

test('an instance: its own card or group, else what is inside it (its card or group shown)', () => {
  assert.deepEqual(matchIn({ kind: 'instance', address: '/drums' }, PARAMETERS), { addresses: ['/drums'], whole: false }, 'its group in the Parameters tab');
  assert.deepEqual(matchIn({ kind: 'instance', address: '/drums' }, PANEL), { addresses: ['/drums.kick_decay'], whole: true }, 'its controls’ card in the panel');
  assert.deepEqual(matchIn({ kind: 'instance', address: '/synth/osc' }, PANEL), { addresses: ['/synth/osc.freq'], whole: true });
  assert.deepEqual(matchIn({ kind: 'instance', address: '/drums/kick' }, PARAMETERS), { addresses: ['/drums/kick.tone'], whole: true }, 'a composite’s inner rows');
  assert.deepEqual(matchIn({ kind: 'instance', address: '/lfo' }, PANEL), { addresses: [], whole: false });
});

test('a wire: each end the view has', () => {
  const wire = { kind: 'wire', from: '/lfo.out', to: '/synth.level' };
  assert.deepEqual(matchIn(wire, PARAMETERS).addresses, ['/synth.level'], 'the parameter it drives');
  assert.deepEqual(matchIn(wire, PANEL).addresses, [], 'neither end in the panel');
  assert.deepEqual(matchIn({ kind: 'wire', from: '/synth/osc.freq', to: '/out.l' }, PANEL).addresses, ['/synth/osc.freq', '/out.l'], 'both ends');
  assert.deepEqual(matchIn({ kind: 'wire', from: '/drums.out', to: '/drums.x' }, ['/drums']), { addresses: [], whole: false }, 'a wire is its ends, not their instance');
  assert.equal(missingReason(wire, 'panel'), 'Neither end of /lfo.out → /synth.level is in the panel');
  assert.equal(missingReason({ kind: 'pin', address: '/osc.out' }, 'parameters'), '/osc.out is not in the Parameters tab');
});

test('places: each view’s matches, or why it has none', () => {
  const places = placesOf({ kind: 'pin', address: '/synth.level' }, { panel: PANEL, parameters: PARAMETERS, schematic: ['/synth.level'] });
  assert.equal(places.panel.reason, '/synth.level is not in the panel');
  assert.deepEqual(places.parameters, { addresses: ['/synth.level'], whole: false });
  assert.deepEqual(places.schematic.addresses, ['/synth.level']);
  assert.equal(placesOf({ kind: 'pin', address: '/a.b' }).schematic.reason, '/a.b is not in the schematic', 'a view not known has nothing');
});

test('with a subject: Show in each other view, off with its reason where it is not; Setup switched to plainly', () => {
  const places = placesOf({ kind: 'pin', address: '/synth.level' }, { panel: PANEL, parameters: PARAMETERS, schematic: ['/synth.level'] });
  assert.deepEqual(tabItems('parameters', places), [
    { id: 'show:panel', label: 'Show in Panel', disabled: true, title: '/synth.level is not in the panel' },
    { id: 'show:schematic', label: 'Show in Schematic' },
    { id: 'tab:setup', label: 'Show Setup' },
  ]);
  assert.deepEqual(tabItems('setup', places).map((i) => i.id), ['show:panel', 'show:schematic', 'show:parameters']);
});

test('without a subject: plain tab switching, Show Panel and so on', () => {
  assert.deepEqual(tabItems('schematic'), [
    { id: 'tab:panel', label: 'Show Panel' },
    { id: 'tab:parameters', label: 'Show Parameters' },
    { id: 'tab:setup', label: 'Show Setup' },
  ]);
});

test('the context menu: a control’s Show in items; the background’s plain tabs', () => {
  const control = { address: '/drums.kick_decay', label: 'decay', editable: true };
  const places = placesOf(subjectOf({ control }), { panel: PANEL, parameters: PARAMETERS, schematic: ['/drums.kick_decay'] });
  const items = contextItems({ control, tab: 'panel', places });
  assert.deepEqual(items.map((i) => i.label ?? i.id), ['Reset decay to default', 'Copy address', 'separator', 'Restart from Files', 'separator', 'Show in Schematic', 'Show in Parameters', 'Show Setup']);
  const wire = { kind: 'wire', path: '', level: '', from: '/lfo.out', to: '/synth.level' };
  const wired = contextItems({ part: [{ id: 'inspect', label: 'Inspect' }], tab: 'schematic', places: placesOf(subjectOf({ hit: wire }), { panel: PANEL, parameters: PARAMETERS }) });
  assert.deepEqual(wired.slice(-3), [
    { id: 'show:panel', label: 'Show in Panel', disabled: true, title: 'Neither end of /lfo.out → /synth.level is in the panel' },
    { id: 'show:parameters', label: 'Show in Parameters' },
    { id: 'tab:setup', label: 'Show Setup' },
  ]);
  assert.deepEqual(contextItems({ tab: 'schematic' }).slice(-3).map((i) => i.label), ['Show Panel', 'Show Parameters', 'Show Setup']);
});

test('the flash: a class of its own, a pulse, and a still outline that fades with reduced motion', () => {
  assert.match(FLASH_CLASS, /^signal-/);
  assert.match(FLASH_CSS, /animation: signal-shown-pulse/);
  assert.match(FLASH_CSS, /prefers-reduced-motion: reduce[^}]*signal-shown-fade/);
});

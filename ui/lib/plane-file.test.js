// ADR 0051 §4, test 1: `ui/plane.toml` parsed, checked and written; key
// order irrelevant; card ids; the derived default Stack of every example
// project with a panel file snapshotted (read from their files, never
// written), and of a generated panel.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { generatedTree, parseContent } from './panel-tree.js';
import { defaultPanel } from './panel.js';
import { CARD_PREFIX, PRIMARY, STACK_PU, cardSlug, checkPlane, derivePlane, panelCards, parsePlane, planeOf, writePlane } from './plane-file.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));

/** A panel file's cards and plate, as the plane reads them. */
function cardsOf(project) {
  const text = read(`projects/${project}/ui/main.html`);
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  const parsed = parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' });
  assert.equal(parsed.error, undefined, parsed.error);
  const plate = /<signal-panel[^>]*\splate="(\d+) \d+"/.exec(text)?.[1];
  return { cards: panelCards(parsed.tree), plate: plate ? Number(plate) : null };
}

test('a card id is its heading made a slug, else its slot, else its place in the panel; repeated ids are numbered', () => {
  assert.equal(cardSlug('RHYTHM COMPOSER'), 'rhythm-composer');
  assert.equal(cardSlug('  Tape · Echo!  '), 'tape-echo');
  assert.equal(cardSlug('Ébène'), 'ebene');
  const parsed = parseContent(
    '<div class="signal-cards"><section class="signal-card"><h2>out</h2></section><section class="signal-card"><h2>Out</h2></section><signal-slot bind="/deck_a"></signal-slot><section class="signal-card"><p>x</p></section><section class="signal-card"><h2>out-2</h2></section></div>',
    whitelist,
    { mode: 'read' },
  );
  assert.equal(parsed.error, undefined, parsed.error);
  assert.deepEqual(
    panelCards(parsed.tree).map((c) => c.id),
    ['out', 'out-2', 'deck-a', 'card-4', 'out-2-2'],
  );
});

test('a card’s binds are every address its controls bind or read (a piano roll’s position, a step grid’s step)', () => {
  const { cards } = cardsOf('studio');
  const synth = cards.find((c) => c.id === 'mono-synth');
  assert.ok(synth.binds.includes('/synth.cutoff'));
  assert.ok(synth.binds.includes('/melody.position'));
  assert.ok(cards.find((c) => c.id === 'rhythm-composer').binds.includes('/drums.step'));
  assert.deepEqual(cards.find((c) => c.id === 'transport').binds, ['/transport.bpm']);
});

test('the derived default Stack of every example panel: one primary Stack of its cards in panel order, at the plate’s width', () => {
  const expected = {
    kick: ['kick', 'kick-2', 'output', 'scope'],
    'physical-av': ['physical-av', 'membrane-seen', 'sparks', 'bodies', 'strike', 'out', 'membrane-heard'],
    pulse: ['pulse', 'filter', 'delay', 'sequence', 'output', 'scope'],
    studio: ['studio', 'transport', 'mono-synth', 'rhythm-composer', 'tape-echo', 'master'],
    'studio-transport': ['studio-transport', 'transport', 'mono-synth', 'rhythm-composer', 'tape-echo', 'master'],
    'two-decks': ['two-decks', 'transport', 'deck-a', 'deck-b', 'mix'],
  };
  for (const [project, ids] of Object.entries(expected)) {
    const { cards, plate } = cardsOf(project);
    assert.deepEqual(derivePlane(cards, plate), { schema_version: 1, stacks: [{ name: PRIMARY, at: null, width: plate ?? STACK_PU, instruments: ids.map((id) => `${CARD_PREFIX}${id}`) }] }, project);
  }
});

test('a generated panel derives its Stack from its cards: an instance per card, then the output card', () => {
  const description = {
    project: 'p',
    controls: [
      { instance: 'osc', module: 'std.sine', name: 'hz', address: '/osc.hz', min: 20, max: 2000 },
      { instance: 'amp', module: 'std.gain', name: 'gain', address: '/amp.gain', min: 0, max: 1 },
    ],
    sinks: [{ kind: 'audio', address: '/out' }],
    instances: [{ address: '/out', ports: [{ direction: 'in', shape: 'signal', address: '/out.l', name: 'l' }] }],
  };
  const tree = generatedTree(defaultPanel(description));
  const plate = Number(tree.plate.split(' ')[0]);
  assert.deepEqual(derivePlane(panelCards(tree), plate).stacks[0].instruments, ['card:osc', 'card:amp', 'card:output']);
});

test('the studio’s ui/plane.toml reads, checks clean against its panel and writes back byte for byte', () => {
  const text = read('projects/studio/ui/plane.toml');
  const { cards } = cardsOf('studio');
  const parsed = parsePlane(text);
  assert.deepEqual(parsed.problems, []);
  assert.deepEqual(parsed.plane.stacks, [{ name: 'primary', at: [0, 0], width: 160, instruments: ['card:transport', 'card:mono-synth', 'card:rhythm-composer', 'card:tape-echo', 'card:master'] }]);
  assert.deepEqual(checkPlane(parsed.plane, { cards: cards.map((c) => c.id) }), []);
  assert.equal(writePlane(parsed.plane), text);
  const shown = planeOf(text, cards);
  assert.equal(shown.derived, false);
});

test('key order and comments never matter; the canonical text reads back to the same plane', () => {
  const a = parsePlane('schema_version = 1\n[[stack]]\nname = "a"\nwidth = 160\ninstruments = ["card:x", "y"]\nat = [3, -4]\n');
  const b = parsePlane('# a plane\nschema_version = 1\n\n[[stack]]\ninstruments = [\n  "card:x", # first\n  "y",\n]\nat = [3, -4]\nname = "a"\n');
  assert.deepEqual(a.problems, []);
  assert.deepEqual(b.problems, []);
  assert.deepEqual(a.plane, b.plane);
  assert.deepEqual(parsePlane(writePlane(a.plane)).plane, a.plane);
  assert.equal(writePlane(parsePlane(writePlane(a.plane)).plane), writePlane(a.plane));
  // Without `at` and `width`: laid out, at the reference width; written without `at`.
  const c = parsePlane('schema_version = 1\n[[stack]]\nname = "b"\ninstruments = []\n');
  assert.deepEqual(c.plane.stacks[0], { name: 'b', at: null, width: STACK_PU, instruments: [] });
  assert.equal(writePlane(c.plane), 'schema_version = 1\n\n[[stack]]\nname = "b"\nwidth = 160\ninstruments = []\n');
});

test('what the file gets wrong is an error naming it', () => {
  const errors = (text) => parsePlane(text).problems.map((p) => p.message);
  assert.match(errors('schema_version = 2\n')[0], /schema_version must be 1/);
  assert.match(errors('')[0], /schema_version is missing/);
  assert.match(errors('schema_version = 1\nzoom = 2\n')[0], /unknown key "zoom"/);
  assert.match(errors('schema_version = 1\n[[stack]]\nname = "Primary"\ninstruments = []\n')[0], /name must be a name/);
  assert.match(errors('schema_version = 1\n[[stack]]\nname = "a"\nwidth = 80.0\ninstruments = []\n')[0], /width must be a whole number/);
  assert.match(errors('schema_version = 1\n[[stack]]\nname = "a"\nat = [1]\ninstruments = []\n')[0], /at must be two whole numbers/);
  assert.match(errors('schema_version = 1\n[[stack]]\nname = "a"\n')[0], /instruments is missing/);
  assert.match(errors('schema_version = 1\n[[stack]]\nname = "a"\ninstruments = [1]\n')[0], /instruments must be an array of names/);
  assert.match(errors('schema_version = 1\n[[stack]\n')[0], /^plane\.toml: line \d+: "\]\]" expected/);
});

test('plane.check: an unknown entry or one listed twice is an error; an instance in no instrument a warning', () => {
  const plane = parsePlane('schema_version = 1\n[[stack]]\nname = "a"\ninstruments = ["card:x", "card:nope", "deck", "card:x"]\n[[stack]]\nname = "b"\ninstruments = ["deck", "ghost"]\n[[stack]]\nname = "a"\ninstruments = []\n').plane;
  const problems = checkPlane(plane, { cards: ['x'], slots: ['deck'], unstacked: ['bus'] });
  assert.deepEqual(
    problems.map((p) => `${p.severity}: ${p.message}`),
    [
      'error: plane.toml: "card:nope" names no card of ui/main.html (cards: x)',
      'error: plane.toml: "card:x" is listed twice (in "a")',
      'error: plane.toml: "deck" is listed twice (in "a" and "b")',
      'error: plane.toml: "ghost" names no top-level slot with a part (a card is "card:<id>")',
      'error: plane.toml: the Stack "a" is named twice',
      'warning: plane.toml: the instance "bus" is in no instrument of any Stack: it is drawn in the unstacked tray',
    ],
  );
});

test('a file with errors is reported and the derived Stack drawn: a project never stops opening', () => {
  const { cards } = cardsOf('kick');
  const shown = planeOf('schema_version = 1\n[[stack]]\nname = "a"\ninstruments = ["card:nope"]\n', cards);
  assert.equal(shown.derived, true);
  assert.equal(shown.plane.stacks[0].name, PRIMARY);
  assert.equal(shown.problems[0].severity, 'error');
  assert.deepEqual(planeOf(null, cards), { plane: derivePlane(cards), derived: true, problems: [] });
});

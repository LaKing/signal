// Pattern files as text (ADR 0028 §2): both kinds parsed and written,
// round trips, determinism, sorting, the subset's refusals and validation.

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  PatternError,
  isIntegerToken,
  notesOverlap,
  emptyPattern,
  formatNumber,
  formatString,
  normalizePattern,
  overlaps,
  parsePattern,
  parseToml,
  samePattern,
  serializePattern,
  validatePattern,
} from './pattern-text.js';

const MELODY = {
  schema_version: 1,
  kind: 'notes',
  length: 16,
  notes: [
    { key: 60, start: 0, length: 0.5, velocity: 0.8 },
    { key: 64, start: 0, length: 1, velocity: 1 },
    { key: 67, start: 1.5, length: 0.25, velocity: 0.333 },
    { key: 72, start: 2 / 3, length: 1 / 3, velocity: 0 },
  ],
};

const BEAT = {
  schema_version: 1,
  kind: 'steps',
  steps: 16,
  step: 0.25,
  lanes: [
    { name: 'kick', hits: 'X...x...x...x...', level: 1 },
    { name: 'snare', hits: '....X.......X...' },
    { name: 'closed_hat', hits: 'x.x.x.x.x.x.x.x.', level: 0.7 },
    { name: 'open hat', hits: '..x...x...x...x.' },
  ],
};

test('a notes pattern is written readably: one note per line, sorted by (start, key)', () => {
  const text = serializePattern(MELODY);
  assert.equal(
    text,
    `schema_version = 1
kind = "notes"
length = 16

notes = [
  { key = 60, start = 0, length = 0.5, velocity = 0.8 },
  { key = 64, start = 0, length = 1, velocity = 1 },
  { key = 72, start = 0.6666666666666666, length = 0.3333333333333333, velocity = 0 },
  { key = 67, start = 1.5, length = 0.25, velocity = 0.333 },
]
`,
  );
});

test('a steps pattern is written as an aligned grid, levels in their own table', () => {
  assert.equal(
    serializePattern(BEAT),
    `schema_version = 1
kind = "steps"
steps = 16
step = 0.25

[lanes]
kick       = "X...x...x...x..."
snare      = "....X.......X..."
closed_hat = "x.x.x.x.x.x.x.x."
"open hat" = "..x...x...x...x."

[levels]
kick       = 1
closed_hat = 0.7
`,
  );
});

test('parse(serialise(x)) is x, in canonical order, for both kinds', () => {
  for (const pattern of [MELODY, BEAT, emptyPattern('notes'), emptyPattern('steps', { steps: 32 })]) {
    const back = parsePattern(serializePattern(pattern));
    assert.deepEqual(back, normalizePattern(pattern));
    assert.equal(serializePattern(back), serializePattern(pattern), 'and the text is a fixed point');
  }
});

test('serialising is deterministic: the order notes come in does not matter', () => {
  const shuffled = { ...MELODY, notes: [...MELODY.notes].reverse() };
  assert.equal(serializePattern(shuffled), serializePattern(MELODY));
  assert.ok(samePattern(shuffled, MELODY));
  assert.ok(!samePattern(MELODY, { ...MELODY, length: 8 }));
});

test('notes are sorted by (start, key) when read', () => {
  const p = parsePattern(`schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 62, start = 1, length = 1, velocity = 0.5 },
  { key = 60, start = 1, length = 1, velocity = 0.5 },
  { key = 70, start = 0, length = 1, velocity = 0.5 },
]`);
  assert.deepEqual(
    p.notes.map((n) => [n.start, n.key]),
    [
      [0, 70],
      [1, 60],
      [1, 62],
    ],
  );
});

test('[[notes]] tables, comments, blank lines, CRLF and literal strings are read', () => {
  const text = [
    '# a melody',
    "schema_version = 1   # always 1",
    "kind = 'notes'",
    'length = 8.0',
    '',
    '[[notes]]',
    'key = 60',
    'start = 0',
    'length = 1e0',
    'velocity = 0.5',
    '',
    '[[notes]]  # the second',
    'velocity = 1',
    'key = 62',
    'start = 2.5',
    'length = 0.5',
  ].join('\r\n');
  assert.deepEqual(parsePattern(text), {
    schema_version: 1,
    kind: 'notes',
    length: 8,
    notes: [
      { key: 60, start: 0, length: 1, velocity: 0.5 },
      { key: 62, start: 2.5, length: 0.5, velocity: 1 },
    ],
  });
});

test('arrays may span lines with comments and a trailing comma; inline lanes work too', () => {
  const p = parsePattern(`schema_version = 1
kind = "steps"
steps = 4
step = 0.5
lanes = { kick = "x.x.", "hi hat" = "xxxX" }
levels = { "hi hat" = 0.25 }
`);
  assert.deepEqual(p.lanes, [
    { name: 'kick', hits: 'x.x.' },
    { name: 'hi hat', hits: 'xxxX', level: 0.25 },
  ]);
  const n = parsePattern(`schema_version = 1
kind = "notes"
length = 4
notes = [ # the notes
  { key = 1, start = 0, length = 1, velocity = 1 }, # one

  { key = 2, start = 0, length = 1, velocity = 1 },
]`);
  assert.equal(n.notes.length, 2);
});

test('lane order is file order, also for names that look like numbers', () => {
  const p = parsePattern(`schema_version = 1
kind = "steps"
steps = 2
step = 0.25
[lanes]
b = "x."
2 = ".x"
a = "xx"
1 = ".."
`);
  assert.deepEqual(
    p.lanes.map((l) => l.name),
    ['b', '2', 'a', '1'],
  );
});

test('strings: escapes are read and written', () => {
  assert.equal(formatString('a"b\\c\nd\u0001'), '"a\\"b\\\\c\\nd\\u0001"');
  const root = parseToml('a = "x\\"y\\\\z\\u00e9\\t"');
  assert.equal(root.get('a'), 'x"y\\zé\t');
  const lane = { schema_version: 1, kind: 'steps', steps: 1, step: 1, lanes: [{ name: 'say "hi"\\', hits: 'x' }] };
  assert.deepEqual(parsePattern(serializePattern(lane)), lane);
});

test('numbers are printed minimally and read back exactly', () => {
  assert.equal(formatNumber(1), '1');
  assert.equal(formatNumber(0.25), '0.25');
  assert.equal(formatNumber(-0), '0');
  assert.equal(formatNumber(1e21), '1e21');
  assert.equal(formatNumber(1.5e-7), '1.5e-7');
  for (const n of [0.1, 1 / 3, 2 / 3, 0.1 + 0.2, 1e-7, 123456.789, 1e21, 5e-324]) {
    assert.equal(parseToml(`n = ${formatNumber(n)}`).get('n'), n, `${n}`);
  }
  assert.throws(() => formatNumber(Infinity), PatternError);
});

/** Asserts that `text` is refused with a message matching `pattern`. */
function refuses(text, pattern) {
  assert.throws(
    () => parsePattern(text),
    (error) => error instanceof PatternError && pattern.test(error.message),
    `${JSON.stringify(text)} should be refused with ${pattern}`,
  );
}

const HEAD = 'schema_version = 1\nkind = "notes"\nlength = 4\n';

test('text outside the subset is refused with the line', () => {
  refuses('schema_version = true', /booleans/);
  refuses('a.b = 1', /dotted keys/);
  refuses('a = """x"""', /multi-line/);
  refuses('a = "open', /not closed/);
  refuses('a = 1_000', /underscores/);
  refuses('a = 01', /leading zeros/);
  refuses('a = 0x10', /not a number/);
  refuses('a = inf', /finite/);
  refuses('a = nan', /finite/);
  refuses('a = bare', /quoted/);
  refuses('a = 1 b = 2', /after a value/);
  refuses('a =', /missing/);
  refuses('a = 1\na = 2', /line 2: a is given twice/);
  refuses('[lanes]\n[lanes]', /given twice/);
  refuses('a = { b = 1, }', /key was expected/);
  refuses('a = { b = 1\n}', /inline table/);
  refuses('a = [1 2]', /"," or "]"/);
  refuses('a = "\\q"', /unknown escape/);
  refuses('notes = []\n[[notes]]', /given twice/);
});

test('the schema: version, kind, unknown keys and types', () => {
  refuses('kind = "notes"\nlength = 4', /schema_version is missing/);
  refuses('schema_version = 2\nkind = "notes"\nlength = 4', /not supported/);
  refuses('schema_version = 1\nlength = 4', /kind is missing/);
  refuses('schema_version = 1\nkind = "chords"', /not "notes" or "steps"/);
  refuses(`${HEAD}tempo = 120`, /unknown key "tempo"/);
  refuses('schema_version = 1\nkind = "notes"', /length is missing/);
  refuses(`${HEAD}notes = [{ key = 60, start = 0, length = 1 }]`, /velocity is missing/);
  refuses(`${HEAD}notes = [{ key = 60, start = 0, length = 1, velocity = 1, pan = 0 }]`, /unknown key "pan"/);
  refuses(`${HEAD}notes = [{ key = "C4", start = 0, length = 1, velocity = 1 }]`, /must be a number/);
  refuses(`${HEAD}notes = 3`, /array of tables/);
  refuses(`${HEAD}notes = [1]`, /must be a table/);
  const steps = 'schema_version = 1\nkind = "steps"\nsteps = 4\nstep = 0.25\n';
  refuses(`${steps}[lanes]\nkick = 1`, /must be a string/);
  refuses(`${steps}[lanes]\nkick = "x..."\n[levels]\nsnare = 1`, /not a lane/);
  refuses(`${steps}[lanes]\nkick = "x..."\n[levels]\nkick = "loud"`, /must be a number/);
  refuses(`${steps}lanes = 4`, /must be a table/);
  refuses(`${steps}[lanes]\nkick = "x..."\n[swing]`, /unknown key "swing"/);
});

test('ranges and lane strings are validated', () => {
  const note = (fields) => `${HEAD}notes = [{ key = ${fields.key ?? 60}, start = ${fields.start ?? 0}, length = ${fields.length ?? 1}, velocity = ${fields.velocity ?? 1} }]`;
  refuses(note({ key: 128 }), /key must be an integer 0\.\.127/);
  refuses(note({ key: -1 }), /key must be an integer/);
  refuses(note({ key: 60.5 }), /key must be an integer/);
  refuses(note({ start: -0.25 }), /start must be/);
  refuses(note({ length: 0 }), /length must be/);
  refuses(note({ velocity: 1.01 }), /velocity must be in \[0, 1\]/);
  refuses(note({ velocity: -0.1 }), /velocity/);
  refuses('schema_version = 1\nkind = "notes"\nlength = 0\nnotes = []', /length \(beats\)/);
  const steps = (n, hits, extra = '') => `schema_version = 1\nkind = "steps"\nsteps = ${n}\nstep = 0.25\n[lanes]\nkick = "${hits}"\n${extra}`;
  refuses(steps(4, 'x..'), /3 steps, not 4/);
  refuses(steps(4, 'x.o.'), /only "\."/);
  refuses(steps(0, ''), /steps must be an integer ≥ 1/);
  refuses(steps(2.5, 'x.'), /steps must be an integer/);
  refuses(steps(4, 'x...', '[levels]\nkick = 2'), /level must be in \[0, 1\]/);
  refuses('schema_version = 1\nkind = "steps"\nsteps = 4\nstep = 0\n[lanes]\nkick = "x..."\n', /step \(beats per step\)/);
});

test('overlapping notes on one key are an error; touching and other keys are not', () => {
  const at = (key, start, length) => ({ key, start, length, velocity: 1 });
  const p = (notes) => ({ schema_version: 1, kind: 'notes', length: 8, notes });
  assert.deepEqual(validatePattern(p([at(60, 0, 1), at(60, 1, 1), at(61, 0.5, 1)])), []);
  assert.deepEqual(overlaps([at(60, 0, 2), at(60, 1, 1)]), [[0, 1]]);
  // A long note overlapping a later one past a short one in between.
  assert.deepEqual(overlaps([at(60, 0, 4), at(60, 1, 1), at(60, 3, 1)]), [
    [0, 1],
    [0, 2],
  ]);
  const problems = validatePattern(p([at(60, 0, 2), at(60, 1, 1)]));
  assert.equal(problems.length, 1);
  assert.match(problems[0].message, /overlaps notes\[0\] on key 60/);
  refuses(`${HEAD}notes = [{ key = 60, start = 0, length = 2, velocity = 1 }, { key = 60, start = 1.5, length = 1, velocity = 1 }]`, /overlaps/);
  assert.throws(() => serializePattern(p([at(60, 0, 2), at(60, 1, 1)])), /overlaps/);
});

test('validation can be left to the caller', () => {
  const text = `${HEAD}notes = [{ key = 200, start = 0, length = 1, velocity = 1 }]`;
  assert.equal(parsePattern(text, { validate: false }).notes[0].key, 200);
  assert.equal(validatePattern(parsePattern(text, { validate: false })).length, 1);
  assert.deepEqual(validatePattern(null), [{ path: '', message: 'not a pattern' }]);
});

test('a duplicate lane name is an error', () => {
  const p = { schema_version: 1, kind: 'steps', steps: 1, step: 1, lanes: [{ name: 'a', hits: 'x' }, { name: 'a', hits: '.' }] };
  assert.match(validatePattern(p)[0].message, /given twice/);
});

test('empty patterns are valid', () => {
  assert.deepEqual(validatePattern(emptyPattern('notes')), []);
  const steps = emptyPattern('steps');
  assert.deepEqual(validatePattern(steps), []);
  assert.equal(steps.lanes.length, 8);
  assert.equal(serializePattern(emptyPattern('notes', { length: 4 })), 'schema_version = 1\nkind = "notes"\nlength = 4\n\nnotes = []\n');
  assert.throws(() => emptyPattern('chords'), PatternError);
});

test('a random pattern round-trips byte for byte', () => {
  let seed = 1;
  const random = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  for (let round = 0; round < 20; round++) {
    const notes = [];
    for (let key = 0; key < 128; key += 1 + Math.floor(random() * 20)) {
      let at = 0;
      for (let i = 0; i < 5; i++) {
        at += Math.floor(random() * 8) / 8;
        const length = (1 + Math.floor(random() * 16)) / 16;
        notes.push({ key, start: at, length, velocity: Math.round(random() * 1000) / 1000 });
        at += length;
      }
    }
    const pattern = { schema_version: 1, kind: 'notes', length: 32, notes };
    const text = serializePattern(pattern);
    assert.equal(serializePattern(parsePattern(text)), text);
    assert.deepEqual(parsePattern(text), normalizePattern(pattern));
  }
});

test('integers are integer tokens: key, steps and schema_version refuse 60.0', () => {
  refuses(`${HEAD}notes = [{ key = 60.0, start = 0, length = 1, velocity = 1 }]`, /key must be an integer, not the float 60\.0/);
  refuses('schema_version = 1\nkind = "steps"\nsteps = 2.0\nstep = 0.25\n[lanes]\nk = "x."\n', /steps must be an integer/);
  refuses('schema_version = 1.0\nkind = "notes"\nlength = 4\nnotes = []', /integer 1, not 1\.0/);
  // Floats may be written as integers (start = 0, length = 4).
  assert.equal(parsePattern(`${HEAD}notes = [{ key = 60, start = 1, length = 2, velocity = 1 }]`).notes[0].length, 2);
  const root = parseToml('a = 1\nb = 1.0\nc = 1e2\n');
  assert.deepEqual(['a', 'b', 'c'].map((k) => isIntegerToken(root, k)), [true, false, false]);
  assert.throws(() => parseToml('a = 9223372036854775808'), /64-bit integer range/);
  assert.equal(parseToml('a = -9223372036854775808').get('a'), -9223372036854775808);
});

test('an integer-valued float past 2^53 is written as a float', () => {
  assert.equal(formatNumber(2 ** 53), '9007199254740992.0');
  assert.equal(formatNumber(12345678901234567890), '12345678901234567000.0');
  assert.equal(formatNumber(2 ** 53 - 1), '9007199254740991');
  const p = { schema_version: 1, kind: 'notes', length: 2 ** 60, notes: [{ key: 1, start: 2 ** 55, length: 1, velocity: 1 }] };
  assert.deepEqual(parsePattern(serializePattern(p)), p);
});

test('strings: no surrogate or out-of-range escapes, no raw control characters', () => {
  assert.throws(() => parseToml('a = "\\uD800"'), /not a Unicode scalar value/);
  assert.throws(() => parseToml('a = "\\U00110000"'), (e) => e instanceof PatternError && /scalar/.test(e.message));
  assert.equal(parseToml('a = "\\U0001F600"').get('a'), '😀');
  assert.throws(() => parseToml('a = "x\u0001y"'), /control character \(U\+0001\)/);
  assert.throws(() => parseToml("a = 'x\u007fy'"), /control character/);
  assert.equal(parseToml('a = "x\ty"').get('a'), 'x\ty', 'a tab is allowed');
  assert.deepEqual([...parseToml('a = "x#y" # c "q"\n')], [['a', 'x#y']]);
});

test('notes and [lanes] are required; a steps pattern has a lane', () => {
  refuses(HEAD, /notes is missing/);
  assert.deepEqual(parsePattern(`${HEAD}notes = []`).notes, []);
  refuses('schema_version = 1\nkind = "steps"\nsteps = 2\nstep = 0.25\n', /\[lanes\] is missing/);
  refuses('schema_version = 1\nkind = "steps"\nsteps = 2\nstep = 0.25\n[lanes]\n', /at least one lane/);
  assert.throws(() => serializePattern({ schema_version: 1, kind: 'steps', steps: 1, step: 1, lanes: [] }), /at least one lane/);
});

test('the overlap rule is strict float, with no tolerance', () => {
  const n = (key, start, length) => ({ key, start, length, velocity: 1 });
  assert.ok(notesOverlap(n(60, 0.1, 0.2), n(60, 0.3, 1)), '0.1 + 0.2 > 0.3 as floats');
  assert.ok(!notesOverlap(n(60, 0.25, 0.5), n(60, 0.75, 1)), 'touching exactly');
  assert.ok(notesOverlap(n(60, 1, 1), n(60, 0, 1.0000000000000002)));
  assert.ok(!notesOverlap(n(60, 0, 4), n(61, 1, 1)), 'other key');
  assert.ok(notesOverlap(n(60, 2, 1), n(60, 0, 4)), 'either order');
  assert.equal(validatePattern({ schema_version: 1, kind: 'notes', length: 4, notes: [n(60, 0.1, 0.2), n(60, 0.3, 1)] }).length, 1);
});

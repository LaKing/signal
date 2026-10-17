// Pattern files as text (ADR 0028 §2): `patterns/<name>.toml`, either
// `kind = "notes"` (a melody: notes of key, start, length, velocity) or
// `kind = "steps"` (a drum grid: lanes of `.`, `x`, `X`). Parsed and
// serialised here for <signal-piano-roll> and <signal-step-grid>; no DOM.
//
// The parser reads only the TOML these files use (bare or quoted keys,
// strings, numbers, inline tables, arrays, `[table]` and `[[array]]`
// headers, comments) and refuses anything else with a message naming the
// line. The server is the authority: it parses and validates every write
// again (ADR 0028 §7). The serialiser is deterministic: the same pattern
// always gives the same bytes, notes sorted by (start, key), one per line,
// numbers in the shortest form that reads back to the same value.
//
// A pattern object:
//   { schema_version: 1, kind: 'notes', length, notes: [{ key, start, length, velocity }] }
//   { schema_version: 1, kind: 'steps', steps, step, lanes: [{ name, hits, level? }] }
// Lanes are an array because their order is the sequencer's output order.
// A lane's optional `level` is written in a `[levels]` table beside
// `[lanes]`, so the hit strings stay aligned as a readable grid.

export const SCHEMA_VERSION = 1;
export const KINDS = Object.freeze(['notes', 'steps']);
/** The characters of a lane: rest, hit, accent. */
export const REST = '.';
export const HIT = 'x';
export const ACCENT = 'X';

/** A parse or validation error; `line` is 1-based when known. */
export class PatternError extends Error {
  constructor(message, line = null) {
    super(line ? `line ${line}: ${message}` : message);
    this.name = 'PatternError';
    this.line = line;
  }
}

// ---------------------------------------------------------------- the TOML subset

const BARE_KEY = /^[A-Za-z0-9_-]+$/;
const NUMBER = /^[+-]?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;
const INTEGER = /^[+-]?(?:0|[1-9][0-9]*)$/;
const I64_MAX = 2n ** 63n - 1n;

/** The keys of each parsed table whose value was written as an integer (`60`, not `60.0`). */
const INTEGER_KEYS = new WeakMap();

/** Whether `map`'s `key` was an integer token in the text (TOML tells integers from floats). */
export function isIntegerToken(map, key) {
  return INTEGER_KEYS.get(map)?.has(key) ?? false;
}

/** Sets `key` in `table` from the reader's next value, remembering an integer token. */
function setValue(reader, table, key) {
  if (table.has(key)) reader.fail(`${key} is given twice`);
  reader.lastInteger = false;
  table.set(key, reader.value());
  if (reader.lastInteger) {
    if (!INTEGER_KEYS.has(table)) INTEGER_KEYS.set(table, new Set());
    INTEGER_KEYS.get(table).add(key);
  }
}

/** A cursor over the text, tracking lines. */
class Reader {
  constructor(text) {
    this.text = text;
    this.at = 0;
    this.line = 1;
  }

  peek() {
    return this.text[this.at];
  }

  next() {
    const c = this.text[this.at++];
    if (c === '\n') this.line++;
    return c;
  }

  get done() {
    return this.at >= this.text.length;
  }

  fail(message) {
    throw new PatternError(message, this.line);
  }

  /** Spaces and tabs only. */
  skipBlanks() {
    while (this.peek() === ' ' || this.peek() === '\t') this.next();
  }

  /** Blanks, comments and newlines (inside arrays). */
  skipAll() {
    for (;;) {
      this.skipBlanks();
      const c = this.peek();
      if (c === '#') this.skipComment();
      else if (c === '\n') this.next();
      else if (c === '\r' && this.text[this.at + 1] === '\n') {
        this.next();
        this.next();
      } else return;
    }
  }

  skipComment() {
    while (!this.done && this.peek() !== '\n') this.next();
  }

  /** The end of a line: blanks, an optional comment, then a newline or the end. */
  endOfLine() {
    this.skipBlanks();
    if (this.peek() === '#') this.skipComment();
    if (this.peek() === '\r' && this.text[this.at + 1] === '\n') this.next();
    if (this.done) return;
    if (this.peek() !== '\n') this.fail(`unexpected ${JSON.stringify(this.peek())} after a value`);
    this.next();
  }

  key() {
    const c = this.peek();
    if (c === '"') return this.basicString();
    if (c === "'") return this.literalString();
    let key = '';
    while (!this.done && /[A-Za-z0-9_-]/.test(this.peek())) key += this.next();
    if (!key) this.fail(c === undefined ? 'a key is missing' : `unexpected ${JSON.stringify(c)} where a key was expected`);
    this.skipBlanks();
    if (this.peek() === '.') this.fail('dotted keys are not used in pattern files');
    return key;
  }

  basicString() {
    this.next();
    let out = '';
    for (;;) {
      if (this.done || this.peek() === '\n') this.fail('a string is not closed on its line');
      const c = this.next();
      if (c === '"') return out;
      if (c !== '\\') {
        this.plain(c);
        out += c;
        continue;
      }
      const e = this.next();
      const simple = { '"': '"', '\\': '\\', n: '\n', t: '\t', r: '\r', b: '\b', f: '\f' }[e];
      if (simple !== undefined) out += simple;
      else if (e === 'u' || e === 'U') {
        const digits = e === 'u' ? 4 : 8;
        const hex = this.text.slice(this.at, this.at + digits);
        if (!new RegExp(`^[0-9A-Fa-f]{${digits}}$`).test(hex)) this.fail('a bad \\u escape');
        this.at += digits;
        const code = parseInt(hex, 16);
        if (code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) this.fail(`\\${e}${hex} is not a Unicode scalar value`);
        out += String.fromCodePoint(code);
      } else this.fail(`an unknown escape \\${e ?? ''}`);
    }
  }

  /** A raw character in a string: control characters (but tab) must be escaped. */
  plain(c) {
    const code = c.charCodeAt(0);
    if ((code < 0x20 && c !== '\t') || code === 0x7f) this.fail(`a control character (U+${code.toString(16).padStart(4, '0').toUpperCase()}) in a string must be escaped`);
  }

  literalString() {
    this.next();
    let out = '';
    for (;;) {
      if (this.done || this.peek() === '\n') this.fail('a string is not closed on its line');
      const c = this.next();
      if (c === "'") return out;
      this.plain(c);
      out += c;
    }
  }

  value() {
    const c = this.peek();
    if (c === '"') {
      if (this.text.startsWith('"""', this.at)) this.fail('multi-line strings are not used in pattern files');
      return this.basicString();
    }
    if (c === "'") {
      if (this.text.startsWith("'''", this.at)) this.fail('multi-line strings are not used in pattern files');
      return this.literalString();
    }
    if (c === '{') return this.inlineTable();
    if (c === '[') return this.array();
    let word = '';
    while (!this.done && /[^\s,\]}#]/.test(this.peek())) word += this.next();
    if (!word) this.fail(c === undefined || c === '\n' ? 'a value is missing' : `unexpected ${JSON.stringify(c)} where a value was expected`);
    if (NUMBER.test(word)) {
      const n = Number(word);
      if (!Number.isFinite(n)) this.fail(`${word} is not a finite number`);
      if (INTEGER.test(word)) {
        const big = BigInt(word);
        if (big > I64_MAX || big < -I64_MAX - 1n) this.fail(`${word} is out of the 64-bit integer range`);
        this.lastInteger = true;
      }
      return n;
    }
    if (word === 'true' || word === 'false') this.fail('booleans are not used in pattern files');
    if (/^[+-]?(inf|nan)$/.test(word)) this.fail(`${word} is not a finite number`);
    if (/^[+-]?[0-9]/.test(word)) this.fail(`${word} is not a number pattern files use (decimal, no underscores, no leading zeros)`);
    return this.fail(`unexpected ${JSON.stringify(word)} (strings are quoted)`);
  }

  inlineTable() {
    this.next();
    const table = new Map();
    this.skipBlanks();
    if (this.peek() === '}') {
      this.next();
      return table;
    }
    for (;;) {
      this.skipBlanks();
      const key = this.key();
      this.skipBlanks();
      if (this.next() !== '=') this.fail(`"=" expected after ${key}`);
      this.skipBlanks();
      setValue(this, table, key);
      this.skipBlanks();
      const c = this.next();
      if (c === '}') return table;
      if (c !== ',') this.fail('"," or "}" expected in an inline table (one line)');
    }
  }

  array() {
    this.next();
    const items = [];
    for (;;) {
      this.skipAll();
      if (this.peek() === ']') {
        this.next();
        return items;
      }
      items.push(this.value());
      this.skipAll();
      const c = this.next();
      if (c === ']') return items;
      if (c !== ',') this.fail('"," or "]" expected in an array');
    }
  }
}

/**
 * Parses the TOML subset into Maps (keys in file order), arrays, strings
 * and numbers. `[[name]]` gives an array of Maps.
 */
export function parseToml(text) {
  if (typeof text !== 'string') throw new PatternError('the pattern text is not a string');
  const r = new Reader(text.replace(/^﻿/, ''));
  const root = new Map();
  const defined = new Set();
  let table = root;
  for (;;) {
    r.skipAll();
    if (r.done) return root;
    if (r.peek() === '[') {
      r.next();
      const array = r.peek() === '[';
      if (array) r.next();
      r.skipBlanks();
      const name = r.key();
      r.skipBlanks();
      if (r.next() !== ']' || (array && r.next() !== ']')) r.fail(`"${array ? ']]' : ']'}" expected after [${name}`);
      if (array) {
        const existing = root.get(name);
        if (existing !== undefined && !(Array.isArray(existing) && existing.tables)) r.fail(`${name} is given twice`);
        const list = existing ?? Object.assign([], { tables: true });
        if (!existing) root.set(name, list);
        table = new Map();
        list.push(table);
      } else {
        if (defined.has(name) || root.has(name)) r.fail(`[${name}] is given twice`);
        defined.add(name);
        table = new Map();
        root.set(name, table);
      }
      r.endOfLine();
      continue;
    }
    const key = r.key();
    r.skipBlanks();
    if (r.next() !== '=') r.fail(`"=" expected after ${key}`);
    r.skipBlanks();
    setValue(r, table, key);
    r.endOfLine();
  }
}

// ---------------------------------------------------------------- the pattern schema

const isMap = (v) => v instanceof Map;
const typeName = (v) => (isMap(v) ? 'a table' : Array.isArray(v) ? 'an array' : typeof v === 'string' ? 'a string' : typeof v === 'number' ? 'a number' : typeof v);

function only(map, allowed, where) {
  for (const key of map.keys()) {
    if (!allowed.includes(key)) throw new PatternError(`unknown key ${JSON.stringify(key)}${where ? ` in ${where}` : ''} (expected ${allowed.join(', ')})`);
  }
}

function number(map, key, where) {
  const v = map.get(key);
  if (v === undefined) throw new PatternError(`${where ? `${where}: ` : ''}${key} is missing`);
  if (typeof v !== 'number') throw new PatternError(`${where ? `${where}: ` : ''}${key} must be a number, not ${typeName(v)}`);
  return v;
}

/** A value that must be an integer token (`key`, `steps`, `schema_version`): `60.0` is refused, as TOML integers are. */
function integer(map, key, where) {
  const v = number(map, key, where);
  if (!isIntegerToken(map, key)) throw new PatternError(`${where ? `${where}: ` : ''}${key} must be an integer, not the float ${formatNumber(v)}${Number.isInteger(v) ? '.0' : ''}`);
  return v;
}

/**
 * Whether two notes overlap: on one key, and one starts before the other
 * ends, compared as floats with no tolerance (`a.start + a.length > b.start`
 * for a starting first). Touching notes do not overlap. The one rule of the
 * client and the server (ADR 0028 §2).
 */
export function notesOverlap(a, b) {
  if (a.key !== b.key) return false;
  return a.start <= b.start ? a.start + a.length > b.start : b.start + b.length > a.start;
}

/** Compares notes by (start, key), then length and velocity, so the order is total. */
export function compareNotes(a, b) {
  return a.start - b.start || a.key - b.key || a.length - b.length || a.velocity - b.velocity;
}

/** The notes sorted by (start, key), as read and as written. */
export function sortNotes(notes) {
  return [...notes].sort(compareNotes);
}

/**
 * The problems of a pattern object, `[{ path, message }]`, empty when it is
 * valid: ranges (key an integer 0..127, start ≥ 0, length > 0, velocity in
 * [0, 1], finite numbers), overlapping notes on one key, lane strings and
 * levels. Everything ADR 0028 §2 makes an error; the server checks again.
 */
export function validatePattern(pattern) {
  const problems = [];
  const add = (path, message) => problems.push({ path, message });
  const finite = (v) => typeof v === 'number' && Number.isFinite(v);
  if (!pattern || typeof pattern !== 'object') return [{ path: '', message: 'not a pattern' }];
  if (pattern.schema_version !== SCHEMA_VERSION) add('schema_version', `schema_version must be ${SCHEMA_VERSION}`);
  if (pattern.kind === 'notes') {
    if (!finite(pattern.length) || pattern.length <= 0) add('length', 'length (beats) must be a finite number > 0');
    if (!Array.isArray(pattern.notes)) {
      add('notes', 'notes must be an array');
      return problems;
    }
    pattern.notes.forEach((n, i) => {
      const at = `notes[${i}]`;
      if (!n || typeof n !== 'object') return add(at, 'a note must be a table');
      if (!Number.isInteger(n.key) || n.key < 0 || n.key > 127) add(`${at}.key`, `key must be an integer 0..127 (is ${n.key})`);
      if (!finite(n.start) || n.start < 0) add(`${at}.start`, `start must be a finite number ≥ 0 (is ${n.start})`);
      if (!finite(n.length) || n.length <= 0) add(`${at}.length`, `length must be a finite number > 0 (is ${n.length})`);
      if (!finite(n.velocity) || n.velocity < 0 || n.velocity > 1) add(`${at}.velocity`, `velocity must be in [0, 1] (is ${n.velocity})`);
    });
    for (const [a, b] of overlaps(pattern.notes)) {
      add(`notes[${b}]`, `overlaps notes[${a}] on key ${pattern.notes[a].key}`);
    }
  } else if (pattern.kind === 'steps') {
    const steps = pattern.steps;
    if (!Number.isInteger(steps) || steps < 1) add('steps', 'steps must be an integer ≥ 1');
    if (!finite(pattern.step) || pattern.step <= 0) add('step', 'step (beats per step) must be a finite number > 0');
    if (!Array.isArray(pattern.lanes)) {
      add('lanes', 'lanes must be an array');
      return problems;
    }
    if (pattern.lanes.length === 0) add('lanes', 'a steps pattern has at least one lane');
    const names = new Set();
    pattern.lanes.forEach((lane, i) => {
      const at = `lanes[${i}]`;
      if (!lane || typeof lane.name !== 'string' || lane.name === '') return add(at, 'a lane needs a name');
      if (names.has(lane.name)) add(at, `lane ${JSON.stringify(lane.name)} is given twice`);
      names.add(lane.name);
      if (typeof lane.hits !== 'string' || !/^[.xX]*$/.test(lane.hits)) add(`${at}.hits`, `lane ${lane.name}: only "." (rest), "x" (hit) and "X" (accent)`);
      else if (Number.isInteger(steps) && lane.hits.length !== steps) add(`${at}.hits`, `lane ${lane.name} has ${lane.hits.length} steps, not ${steps}`);
      if (lane.level !== undefined && (!finite(lane.level) || lane.level < 0 || lane.level > 1)) add(`${at}.level`, `lane ${lane.name}: level must be in [0, 1]`);
    });
  } else {
    add('kind', `kind must be "notes" or "steps"`);
  }
  return problems;
}

/** Index pairs `[a, b]` of notes on one key that overlap (touching is not overlapping). */
export function overlaps(notes) {
  const order = notes.map((n, i) => i).filter((i) => notes[i] && Number.isFinite(notes[i].start) && Number.isFinite(notes[i].length));
  order.sort((i, j) => notes[i].key - notes[j].key || notes[i].start - notes[j].start || i - j);
  const pairs = [];
  let open = null; // the note on this key reaching furthest so far
  for (const i of order) {
    const n = notes[i];
    if (open !== null && notesOverlap(notes[open], n)) {
      pairs.push([open, i]);
      if (n.start + n.length > notes[open].start + notes[open].length) open = i;
    } else open = i;
  }
  return pairs;
}

function check(pattern) {
  const problems = validatePattern(pattern);
  if (problems.length) throw new PatternError(problems[0].message);
  return pattern;
}

function readNote(value, where) {
  if (!isMap(value)) throw new PatternError(`${where} must be a table, not ${typeName(value)}`);
  only(value, ['key', 'start', 'length', 'velocity'], where);
  return {
    key: integer(value, 'key', where),
    start: number(value, 'start', where),
    length: number(value, 'length', where),
    velocity: number(value, 'velocity', where),
  };
}

/** A pattern object from TOML Maps (parseToml's result), checked. */
export function patternFromToml(root, { validate = true } = {}) {
  const version = root.get('schema_version');
  if (version === undefined) throw new PatternError('schema_version is missing');
  if (version === SCHEMA_VERSION && !isIntegerToken(root, 'schema_version')) throw new PatternError('schema_version must be the integer 1, not 1.0');
  if (version !== SCHEMA_VERSION) throw new PatternError(`schema_version ${JSON.stringify(version)} is not supported (expected ${SCHEMA_VERSION})`);
  const kind = root.get('kind');
  if (kind === undefined) throw new PatternError('kind is missing');
  let pattern;
  if (kind === 'notes') {
    only(root, ['schema_version', 'kind', 'length', 'notes'], '');
    const length = number(root, 'length', '');
    if (!root.has('notes')) throw new PatternError('notes is missing (an empty pattern has notes = [])');
    const raw = root.get('notes');
    if (!Array.isArray(raw)) throw new PatternError(`notes must be an array of tables, not ${typeName(raw)}`);
    const notes = sortNotes(raw.map((n, i) => readNote(n, `notes[${i}]`)));
    pattern = { schema_version: SCHEMA_VERSION, kind, length, notes };
  } else if (kind === 'steps') {
    only(root, ['schema_version', 'kind', 'steps', 'step', 'lanes', 'levels'], '');
    const steps = integer(root, 'steps', '');
    const step = number(root, 'step', '');
    if (!root.has('lanes')) throw new PatternError('[lanes] is missing: a steps pattern has at least one lane');
    const lanesTable = root.get('lanes');
    if (!isMap(lanesTable)) throw new PatternError(`lanes must be a table of name = "x..." strings, not ${typeName(lanesTable)}`);
    const levels = root.get('levels') ?? new Map();
    if (!isMap(levels)) throw new PatternError(`levels must be a table of name = level, not ${typeName(levels)}`);
    const lanes = [];
    for (const [name, hits] of lanesTable) {
      if (typeof hits !== 'string') throw new PatternError(`lane ${name} must be a string of ".", "x" and "X", not ${typeName(hits)}`);
      const lane = { name, hits };
      if (levels.has(name)) {
        const level = levels.get(name);
        if (typeof level !== 'number') throw new PatternError(`level of ${name} must be a number, not ${typeName(level)}`);
        lane.level = level;
      }
      lanes.push(lane);
    }
    for (const name of levels.keys()) {
      if (!lanesTable.has(name)) throw new PatternError(`levels names ${JSON.stringify(name)}, which is not a lane`);
    }
    pattern = { schema_version: SCHEMA_VERSION, kind, steps, step, lanes };
  } else {
    throw new PatternError(`kind ${JSON.stringify(kind)} is not "notes" or "steps"`);
  }
  return validate ? check(pattern) : pattern;
}

/**
 * Parses a pattern file. Throws a PatternError for text outside the subset,
 * an unknown key, a wrong type, and (unless `validate: false`) every problem
 * `validatePattern` finds. Notes come back sorted by (start, key).
 */
export function parsePattern(text, options = {}) {
  return patternFromToml(parseToml(text), options);
}

// ---------------------------------------------------------------- serialising

/** The shortest text that reads back to `n`; integers without a point (below 2^53), never -0. */
export function formatNumber(n) {
  if (!Number.isFinite(n)) throw new PatternError(`${n} is not a finite number`);
  if (Object.is(n, -0)) return '0';
  const text = String(n).replace('e+', 'e');
  // An integer literal past 2^53 would not read back exactly (and may leave
  // the 64-bit range): written as a float instead.
  if (Number.isInteger(n) && Math.abs(n) >= 2 ** 53 && !text.includes('e')) return `${text}.0`;
  return text;
}

/** A key as written: bare when it can be, else a quoted string. */
export function formatKey(key) {
  return BARE_KEY.test(key) ? key : formatString(key);
}

/** A basic string with the escapes TOML needs. */
export function formatString(s) {
  let out = '"';
  for (const ch of s) {
    const code = ch.codePointAt(0);
    if (ch === '"') out += '\\"';
    else if (ch === '\\') out += '\\\\';
    else if (ch === '\n') out += '\\n';
    else if (ch === '\t') out += '\\t';
    else if (ch === '\r') out += '\\r';
    else if (code < 0x20 || code === 0x7f) out += `\\u${code.toString(16).padStart(4, '0')}`;
    else out += ch;
  }
  return `${out}"`;
}

function noteLine(n) {
  return `  { key = ${formatNumber(n.key)}, start = ${formatNumber(n.start)}, length = ${formatNumber(n.length)}, velocity = ${formatNumber(n.velocity)} },`;
}

/**
 * The text of a pattern: deterministic and readable. Throws a PatternError
 * when the pattern is not valid (`validate: false` skips the check, the
 * numbers must still be finite).
 */
export function serializePattern(pattern, { validate = true } = {}) {
  if (validate) check(pattern);
  const head = [`schema_version = ${SCHEMA_VERSION}`, `kind = ${formatString(pattern.kind)}`];
  if (pattern.kind === 'notes') {
    head.push(`length = ${formatNumber(pattern.length)}`, '');
    const notes = sortNotes(pattern.notes);
    if (notes.length === 0) head.push('notes = []');
    else head.push('notes = [', ...notes.map(noteLine), ']');
    return `${head.join('\n')}\n`;
  }
  if (pattern.kind === 'steps') {
    head.push(`steps = ${formatNumber(pattern.steps)}`, `step = ${formatNumber(pattern.step)}`, '', '[lanes]');
    const keys = pattern.lanes.map((lane) => formatKey(lane.name));
    const width = Math.max(0, ...keys.map((k) => k.length));
    pattern.lanes.forEach((lane, i) => head.push(`${keys[i].padEnd(width)} = ${formatString(lane.hits)}`));
    const levelled = pattern.lanes.map((lane, i) => [lane, keys[i]]).filter(([lane]) => lane.level !== undefined);
    if (levelled.length) {
      head.push('', '[levels]');
      const w = Math.max(...levelled.map(([, k]) => k.length));
      for (const [lane, key] of levelled) head.push(`${key.padEnd(w)} = ${formatNumber(lane.level)}`);
    }
    return `${head.join('\n')}\n`;
  }
  throw new PatternError(`kind ${JSON.stringify(pattern.kind)} is not "notes" or "steps"`);
}

/** A deep copy in canonical form (notes sorted, lanes copied), for comparing and history. */
export function normalizePattern(pattern) {
  if (pattern.kind === 'notes') {
    return {
      schema_version: pattern.schema_version,
      kind: 'notes',
      length: pattern.length,
      notes: sortNotes(pattern.notes.map(({ key, start, length, velocity }) => ({ key, start, length, velocity }))),
    };
  }
  return {
    schema_version: pattern.schema_version,
    kind: 'steps',
    steps: pattern.steps,
    step: pattern.step,
    lanes: pattern.lanes.map(({ name, hits, level }) => (level === undefined ? { name, hits } : { name, hits, level })),
  };
}

/** Whether two patterns mean the same (canonical forms equal). */
export function samePattern(a, b) {
  if (!a || !b) return a === b;
  return serializePattern(normalizePattern(a), { validate: false }) === serializePattern(normalizePattern(b), { validate: false });
}

/** An empty pattern of a kind, valid. */
export function emptyPattern(kind, options = {}) {
  if (kind === 'notes') return { schema_version: SCHEMA_VERSION, kind, length: options.length ?? 16, notes: [] };
  if (kind === 'steps') {
    const steps = options.steps ?? 16;
    const names = options.lanes ?? ['kick', 'snare', 'clap', 'closed_hat', 'open_hat', 'low_tom', 'rim', 'cymbal'];
    return { schema_version: SCHEMA_VERSION, kind, steps, step: options.step ?? 0.25, lanes: names.map((name) => ({ name, hits: REST.repeat(steps) })) };
  }
  throw new PatternError(`kind ${JSON.stringify(kind)} is not "notes" or "steps"`);
}

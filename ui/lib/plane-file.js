// The Signal Project plane's file (ADR 0051 §4): `ui/plane.toml`, the
// project's Stacks, each a vertical column of instruments of equal width,
// in order. It is presentation, text in the project like
// `ui/schematic.toml`: no address names a Stack and no plan, render or log
// depends on it (rule 4; ADR 0051 §14). Without the file one primary Stack
// is derived from the panel's cards in their order, at the plate's width.
//
//     schema_version = 1
//     [[stack]]
//     name = "primary"             # [a-z][a-z0-9_]*, unique
//     at = [0, 0]                  # top-left on the plane, in pu; absent: laid out
//     width = 160                  # pu; every front panel is this wide
//     instruments = ["card:transport", "card:mono-synth"]
//
// An instrument entry names a card of `ui/main.html` (`card:<id>`, the
// migration form; a card's id is its heading's text made a slug, `cardIds`)
// or a top-level slot with a part (`mono_synth`, the full form, ADR 0051
// §5). Here: parsing (the TOML subset of ./pattern-text.js), the check
// (`plane.check`'s rules), the canonical text, card ids and the derived
// default. Pure; no DOM.

import { parseToml, isIntegerToken } from './pattern-text.js';

/** The file, in the project's `ui/`. */
export const PLANE_FILE = 'plane.toml';
export const PLANE_SCHEMA = 1;
/** A Stack's width in pu when the file gives none (ADR 0047's reference width; the owner's answer 2: no half widths). */
export const STACK_PU = 160;
/** The rail between two instruments, in pu, at every level (ADR 0051 §4). */
export const RAIL_PU = 2;
/** The largest coordinate or width, in pu (as a plate's side, ADR 0047 §2). */
export const MAX_PU = 1000;
export const STACK_NAME = /^[a-z][a-z0-9_]*$/;
/** The migration form's prefix: a card of the panel drawn as an instrument. */
export const CARD_PREFIX = 'card:';
/** The name of the Stack derived without a file. */
export const PRIMARY = 'primary';

const TOP_KEYS = ['schema_version', 'stack'];
const STACK_KEYS = ['name', 'at', 'width', 'instruments'];

/** A card's id from its heading's text: lowercase, every run of other characters one `-`, none at the ends. */
export function cardSlug(text) {
  return String(text ?? '')
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/** The first heading's text of a card node (`parseContent`'s tree: its h1 or h2 children). */
function headingOf(card) {
  for (const child of card.children ?? []) if (child.kind === 'h1' || child.kind === 'h2') return child.text ?? '';
  return '';
}

/** Every address a control node binds or reads (`bind`, `position-bind`, `step-bind`: readers count, ADR 0050 §2). */
export function controlBinds(node) {
  const out = [];
  for (const name of ['bind', 'position-bind', 'step-bind']) {
    const value = node.attributes?.[name];
    if (typeof value === 'string') for (const a of value.split(/\s+/)) if (a.startsWith('/')) out.push(a);
  }
  return out;
}

/** The first `<signal-slot>` of a card node (the open slot card itself, or one inside a card). */
function slotIn(node) {
  if (node.kind === 'control') return node.tag === 'signal-slot' ? node : null;
  for (const child of node.children ?? []) {
    const found = slotIn(child);
    if (found) return found;
  }
  return null;
}

function bindsUnder(node, out = []) {
  if (node.kind === 'control') out.push(...controlBinds(node));
  for (const child of node.children ?? []) bindsUnder(child, out);
  return out;
}

/**
 * The cards of a panel's tree (`parseContent(html).tree`, or
 * `generatedTree`'s), in panel order: `{id, index, heading, place, slot,
 * binds}`. The id is the heading's slug, else an open slot card's slot
 * (`<signal-slot bind="/deck">`: `deck`), else `card-<n>` (1-based); a
 * repeated id gets `-2`, `-3`, … in order. `binds`: every address its
 * controls bind or read.
 */
export function panelCards(tree) {
  const grid = (tree?.children ?? []).find((c) => c.kind === 'cards');
  const cards = [];
  const used = new Set();
  (grid?.children ?? []).forEach((node, index) => {
    const heading = node.kind === 'card' ? headingOf(node) : '';
    const slotNode = slotIn(node);
    const slot = slotNode ? String(slotNode.attributes?.bind ?? '').replace(/^\//, '') || null : null;
    const base = cardSlug(heading) || cardSlug(slot) || `card-${index + 1}`;
    let id = base;
    for (let n = 2; used.has(id); n++) id = `${base}-${n}`;
    used.add(id);
    cards.push({ id, index, heading, place: node.place ?? node.attributes?.place ?? null, slot, binds: bindsUnder(node) });
  });
  return cards;
}

/** A problem of the file: `{severity: 'error' | 'warning', message, line}`. */
function problem(severity, message, line = null) {
  return { severity, message, line };
}

/**
 * `ui/plane.toml`'s text read: `{plane, problems}`. `plane` is
 * `{schema_version, stacks: [{name, at, width, instruments}]}` (`at` null
 * when absent, `width` STACK_PU when absent), or null when the text cannot
 * be read as a plane at all; `problems` lists what is wrong (errors) in
 * file order. Key order never matters.
 */
export function parsePlane(text) {
  let root;
  try {
    root = parseToml(String(text ?? ''));
  } catch (error) {
    return { plane: null, problems: [problem('error', `plane.toml: ${error.message.replace(/pattern files/g, 'plane files')}`, error.line ?? null)] };
  }
  const problems = [];
  for (const key of root.keys()) if (!TOP_KEYS.includes(key)) problems.push(problem('error', `plane.toml: unknown key "${key}" (expected ${TOP_KEYS.join(', ')})`));
  const version = root.get('schema_version');
  if (version === undefined) problems.push(problem('error', 'plane.toml: schema_version is missing'));
  else if (version !== PLANE_SCHEMA || !isIntegerToken(root, 'schema_version')) problems.push(problem('error', `plane.toml: schema_version must be ${PLANE_SCHEMA}`));
  const list = root.get('stack');
  const stacks = [];
  if (list !== undefined && !(Array.isArray(list) && list.tables)) problems.push(problem('error', 'plane.toml: stack must be an array of tables ([[stack]])'));
  for (const [i, table] of (Array.isArray(list) && list.tables ? list : []).entries()) {
    const where = `plane.toml: stack ${i + 1}`;
    for (const key of table.keys()) if (!STACK_KEYS.includes(key)) problems.push(problem('error', `${where}: unknown key "${key}" (expected ${STACK_KEYS.join(', ')})`));
    const name = table.get('name');
    if (typeof name !== 'string' || !STACK_NAME.test(name)) problems.push(problem('error', `${where}: name must be a name ([a-z][a-z0-9_]*)`));
    let at = null;
    if (table.has('at')) {
      const v = table.get('at');
      if (Array.isArray(v) && v.length === 2 && v.every((n) => Number.isInteger(n) && Math.abs(n) <= MAX_PU)) at = [v[0], v[1]];
      else problems.push(problem('error', `${where}: at must be two whole numbers of pu, [x, y], each within ±${MAX_PU}`));
    }
    let width = STACK_PU;
    if (table.has('width')) {
      const v = table.get('width');
      if (Number.isInteger(v) && isIntegerToken(table, 'width') && v >= 1 && v <= MAX_PU) width = v;
      else problems.push(problem('error', `${where}: width must be a whole number of pu, 1 to ${MAX_PU}`));
    }
    const raw = table.get('instruments');
    let instruments = [];
    if (raw === undefined) problems.push(problem('error', `${where}: instruments is missing`));
    else if (!Array.isArray(raw) || !raw.every((e) => typeof e === 'string')) problems.push(problem('error', `${where}: instruments must be an array of names`));
    else instruments = [...raw];
    stacks.push({ name: typeof name === 'string' ? name : '', at, width, instruments });
  }
  return { plane: { schema_version: PLANE_SCHEMA, stacks }, problems };
}

/** Whether an entry is the migration form (`card:<id>`). */
export function isCardEntry(entry) {
  return typeof entry === 'string' && entry.startsWith(CARD_PREFIX);
}

/**
 * `plane.check` (ADR 0051 §4) of a parsed plane against the project:
 * `cards` (the panel's card ids), `slots` (the top-level slots with a
 * part), `unstacked` (the top-level instances in no instrument of any
 * Stack, as `stackMembers` finds them). Errors: a Stack's name twice, an
 * unknown entry, an entry listed twice (in one Stack or two). Warning: an
 * instance in no instrument (drawn in the "unstacked" tray). Problems in
 * file order.
 */
export function checkPlane(plane, { cards = [], slots = [], unstacked = [] } = {}) {
  const problems = [];
  const names = new Set();
  const seen = new Map();
  const cardSet = new Set(cards);
  const slotSet = new Set(slots);
  for (const stack of plane?.stacks ?? []) {
    if (stack.name && names.has(stack.name)) problems.push(problem('error', `plane.toml: the Stack "${stack.name}" is named twice`));
    names.add(stack.name);
    for (const entry of stack.instruments) {
      if (seen.has(entry)) problems.push(problem('error', `plane.toml: "${entry}" is listed twice (in ${seen.get(entry) === stack.name ? `"${stack.name}"` : `"${seen.get(entry)}" and "${stack.name}"`})`));
      else seen.set(entry, stack.name);
      if (isCardEntry(entry)) {
        if (!cardSet.has(entry.slice(CARD_PREFIX.length))) problems.push(problem('error', `plane.toml: "${entry}" names no card of ui/main.html${cards.length ? ` (cards: ${cards.join(', ')})` : ''}`));
      } else if (!slotSet.has(entry)) {
        problems.push(problem('error', `plane.toml: "${entry}" names no top-level slot with a part (a card is "card:<id>")`));
      }
    }
  }
  for (const name of unstacked) problems.push(problem('warning', `plane.toml: the instance "${name}" is in no instrument of any Stack: it is drawn in the unstacked tray`));
  return problems;
}

/** A TOML basic string. */
function quoted(text) {
  return `"${String(text).replace(/[\\"]/g, (c) => `\\${c}`).replace(/[\u0000-\u001f\u007f]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`)}"`;
}

/**
 * The canonical text of `plane`: `schema_version`, then each Stack as a
 * `[[stack]]` table with its keys in the order name, at (only when set),
 * width, instruments (one per line). The same plane always gives the same
 * bytes, and `parsePlane` reads them back to it.
 */
export function writePlane(plane) {
  const lines = [`schema_version = ${PLANE_SCHEMA}`];
  for (const stack of plane?.stacks ?? []) {
    lines.push('', '[[stack]]', `name = ${quoted(stack.name)}`);
    if (Array.isArray(stack.at)) lines.push(`at = [${stack.at[0]}, ${stack.at[1]}]`);
    lines.push(`width = ${stack.width ?? STACK_PU}`);
    if (!stack.instruments.length) lines.push('instruments = []');
    else lines.push('instruments = [', ...stack.instruments.map((e) => `  ${quoted(e)},`), ']');
  }
  return `${lines.join('\n')}\n`;
}

/**
 * The plane derived without a file (ADR 0051 §4): one primary Stack of the
 * panel's cards (`panelCards`) in panel order, at the plate's width
 * (`plateWidth` in pu, else STACK_PU), laid out (no `at`).
 */
export function derivePlane(cards, plateWidth = null) {
  const width = Number.isInteger(plateWidth) && plateWidth >= 1 && plateWidth <= MAX_PU ? plateWidth : STACK_PU;
  return { schema_version: PLANE_SCHEMA, stacks: [{ name: PRIMARY, at: null, width, instruments: cards.map((c) => `${CARD_PREFIX}${c.id}`) }] };
}

/**
 * The plane shown: the file's when it reads without errors, else the
 * derived one (`{plane, derived, problems}`; a file with errors is reported
 * and the derived Stack drawn, so a project never stops opening, ADR 0051
 * §12). `text` null: no file.
 */
export function planeOf(text, cards, plateWidth = null, context = {}) {
  if (text === null || text === undefined) return { plane: derivePlane(cards, plateWidth), derived: true, problems: [] };
  const read = parsePlane(text);
  const problems = [...read.problems];
  if (read.plane) problems.push(...checkPlane(read.plane, { cards: cards.map((c) => c.id), ...context }));
  if (!read.plane || problems.some((p) => p.severity === 'error')) return { plane: derivePlane(cards, plateWidth), derived: true, problems };
  return { plane: read.plane, derived: false, problems };
}

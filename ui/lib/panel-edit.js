// The panel editor's operations (docs/ui-controls.md "Editing a panel", ADR
// 0026 §4): each a pure function from a tree (ui/lib/panel-tree.js) to a new
// tree, `{tree}` or `{error}`, never changing its argument; the palette of
// what can be added, from `describe`; the history of written versions; and
// where a drag drops. No DOM: ui/controls/panel-editor.js uses them.

import { controlSpec } from './panel.js';
import { keyCommand } from './keys.js';

/** Every node of `tree` with its parent and index, in document order. */
function* walk(node, parent = null) {
  for (const [index, child] of (node.children ?? []).entries()) {
    yield { node: child, parent: node, index };
    yield* walk(child, node);
  }
}

/** The node `id` in `tree`, with its parent and index; null when absent. */
export function find(tree, id) {
  for (const found of walk(tree)) if (found.node.id === id) return found;
  return null;
}

/** The card that holds node `id` (itself, if it is one); null when none. */
export function cardOf(tree, id) {
  for (const { node } of walk(tree)) {
    if (node.kind !== 'card') continue;
    if (node.id === id || find(node, id)) return node;
  }
  return null;
}

/** The controls of `node`, in document order. */
export function controlsOf(node) {
  return [...walk(node)].map((f) => f.node).filter((n) => n.kind === 'control');
}

/**
 * What edit mode labels each card, row, group and control with, in the
 * panel's order: `card N` (N its place in the grid order), `row`, `group`,
 * a control's address. A map from node id to label.
 */
export function editLabels(tree) {
  const labels = new Map();
  let cards = 0;
  for (const { node } of walk(tree)) {
    if (node.kind === 'card') labels.set(node.id, `card ${++cards}`);
    else if (node.kind === 'row' || node.kind === 'group') labels.set(node.id, node.kind);
    else if (node.kind === 'control') labels.set(node.id, node.attributes.bind ?? node.tag);
  }
  return labels;
}

/** A copy of `tree` to change, and a fresh id in it. */
function edit(tree) {
  const copy = structuredClone(tree);
  copy.next ??= 1;
  return copy;
}

function fresh(tree) {
  return `n${tree.next++}`;
}

const missing = (id) => ({ error: `no ${id} in the panel` });

/** Inserts `node` into `into` before `anchor` (a child, or null: at the end). */
function insertBefore(into, anchor, node) {
  const at = anchor ? into.children.indexOf(anchor) : -1;
  into.children.splice(at === -1 ? into.children.length : at, 0, node);
}

/** Takes node `id` out of `tree`; an emptied group goes with it. */
function take(tree, id) {
  const found = find(tree, id);
  if (!found) return null;
  found.parent.children.splice(found.index, 1);
  if (found.parent.kind === 'group' && !found.parent.children.length) {
    const group = find(tree, found.parent.id);
    group.parent.children.splice(group.index, 1);
  }
  return found;
}

// Cards.

/**
 * Moves card `card` into the grid `cards` before the card now at `index`
 * (the grid's order; `index` past the end appends).
 */
export function moveCard(tree, card, { cards, index }) {
  const copy = edit(tree);
  const from = find(copy, card);
  const grid = find(copy, cards)?.node;
  if (!from || from.node.kind !== 'card') return missing(card);
  if (!grid || grid.kind !== 'cards') return missing(cards);
  const anchor = grid.children[index] === from.node ? grid.children[index + 1] : grid.children[index];
  from.parent.children.splice(from.index, 1);
  insertBefore(grid, anchor ?? null, from.node);
  return { tree: copy };
}

/** Card `card` spanning the grid (`wide`) or not. */
export function setWide(tree, card, wide) {
  const copy = edit(tree);
  const found = find(copy, card)?.node;
  if (!found || found.kind !== 'card') return missing(card);
  found.classes = found.classes.filter((c) => c !== 'wide');
  if (wide) found.classes.unshift('wide');
  return { tree: copy };
}

/** Renames card `card`: its `<h2>`, else its `<h1>`; a card without either gets an `<h2>`. */
export function renameCard(tree, card, text) {
  const copy = edit(tree);
  const found = find(copy, card)?.node;
  if (!found || found.kind !== 'card') return missing(card);
  const heading = found.children.find((c) => c.kind === 'h2') ?? found.children.find((c) => c.kind === 'h1');
  if (heading) heading.text = text;
  else found.children.unshift({ id: fresh(copy), kind: 'h2', classes: [], text });
  return { tree: copy };
}

/** Sets card `card`'s description (its first `<p>`, after the heading); empty text removes it. */
export function describeCard(tree, card, text) {
  const copy = edit(tree);
  const found = find(copy, card)?.node;
  if (!found || found.kind !== 'card') return missing(card);
  const at = found.children.findIndex((c) => c.kind === 'p');
  if (!text) {
    if (at !== -1) found.children.splice(at, 1);
  } else if (at !== -1) {
    found.children[at].text = text;
  } else {
    const heading = found.children.findIndex((c) => c.kind === 'h1' || c.kind === 'h2');
    found.children.splice(heading + 1, 0, { id: fresh(copy), kind: 'p', classes: [], text });
  }
  return { tree: copy };
}

/**
 * Adds an empty card (a heading and an empty row) to grid `cards` (by
 * default the last, made when there is none) before `index` (by default at
 * the end). `{tree, id}`.
 */
export function addCard(tree, { cards = null, index = Infinity, title = 'card' } = {}) {
  const copy = edit(tree);
  let grid = cards ? find(copy, cards)?.node : copy.children[copy.children.length - 1];
  if (cards && (!grid || grid.kind !== 'cards')) return missing(cards);
  if (!grid) {
    grid = { id: fresh(copy), kind: 'cards', classes: [], children: [] };
    copy.children.push(grid);
  }
  const id = fresh(copy);
  const card = {
    id,
    kind: 'card',
    classes: [],
    children: [
      { id: fresh(copy), kind: 'h2', classes: [], text: title },
      { id: fresh(copy), kind: 'row', classes: [], children: [] },
    ],
  };
  grid.children.splice(Math.min(index, grid.children.length), 0, card);
  return { tree: copy, id };
}

/** Whether card `card` holds no control (so it is removed without asking). */
export function cardIsEmpty(tree, card) {
  const found = find(tree, card)?.node;
  return Boolean(found) && controlsOf(found).length === 0;
}

/** Removes card `card`, whatever it holds (`removeCardStep` asks first). */
export function removeCard(tree, card) {
  const copy = edit(tree);
  const found = find(copy, card);
  if (!found || found.node.kind !== 'card') return missing(card);
  found.parent.children.splice(found.index, 1);
  return { tree: copy };
}

/**
 * A click on Remove card for `card`, with `pending` the card a first click
 * asked about (or null): an empty card goes at once; a card with controls
 * asks first (`confirm`) and goes on a second click for the same card.
 * Anything else the editor does sets `pending` back to null.
 */
export function removeCardStep(pending, tree, card) {
  if (cardIsEmpty(tree, card) || pending === card) return { action: 'remove', pending: null };
  return { action: 'confirm', pending: card };
}

// Controls.

/** The container a control goes into for a drop on `into`: a row or group, or a card's last row (made if it has none). */
function target(copy, into) {
  const node = find(copy, into)?.node;
  if (!node) return null;
  if (node.kind === 'row' || node.kind === 'group') return node;
  if (node.kind !== 'card') return null;
  let row = [...node.children].reverse().find((c) => c.kind === 'row');
  if (!row) {
    row = { id: fresh(copy), kind: 'row', classes: [], children: [] };
    node.children.push(row);
  }
  return row;
}

/**
 * Moves control (or group) `id` into `into` (a row, a group, or a card: its
 * last row) before the child now at `index` (by default at the end). A group
 * goes only into a row; a group left empty is removed.
 */
export function moveControl(tree, id, { into, index = Infinity }) {
  const copy = edit(tree);
  const from = find(copy, id);
  if (!from || (from.node.kind !== 'control' && from.node.kind !== 'group')) return missing(id);
  if (into === id || find(from.node, into)) return { error: 'a group does not go into itself' };
  const container = target(copy, into);
  if (!container) return missing(into);
  if (from.node.kind === 'group' && container.kind !== 'row') return { error: 'a group goes only into a row' };
  // Its own place in a group of one: nothing moves.
  if (container === from.parent && container.children.length === 1) return { tree: copy };
  const anchor = container.children[index] === from.node ? container.children[index + 1] : container.children[index];
  const at = anchor ? container.children.indexOf(anchor) : -1;
  take(copy, id);
  // The anchor was the group of one the control left, removed with it: the
  // control takes the group's place.
  if (anchor && !container.children.includes(anchor)) container.children.splice(Math.min(at, container.children.length), 0, from.node);
  else insertBefore(container, anchor ?? null, from.node);
  return { tree: copy };
}

/**
 * Groups the controls `ids` (adjacent in one row, at least two) into a
 * `.signal-group`, in their order, where the first was. `{tree, id}`.
 */
export function makeGroup(tree, ids) {
  const copy = edit(tree);
  const found = ids.map((id) => find(copy, id));
  if (found.length < 2) return { error: 'a group needs at least two controls' };
  if (found.some((f) => !f || f.node.kind !== 'control')) return { error: 'only controls are grouped' };
  const parent = found[0].parent;
  if (parent.kind !== 'row' || found.some((f) => f.parent !== parent)) return { error: 'the controls of a group are in one row' };
  const indexes = found.map((f) => f.index).sort((a, b) => a - b);
  if (indexes.some((i, k) => k > 0 && i !== indexes[k - 1] + 1)) return { error: 'the controls of a group are next to each other' };
  const children = parent.children.splice(indexes[0], indexes.length);
  const group = { id: fresh(copy), kind: 'group', classes: [], children };
  parent.children.splice(indexes[0], 0, group);
  return { tree: copy, id: group.id };
}

/** Puts the controls of group `group` back in its place. */
export function ungroup(tree, group) {
  const copy = edit(tree);
  const found = find(copy, group);
  if (!found || found.node.kind !== 'group') return missing(group);
  found.parent.children.splice(found.index, 1, ...found.node.children);
  return { tree: copy };
}

/** Removes control `id`; a group left empty goes with it. */
export function removeControl(tree, id) {
  const copy = edit(tree);
  const found = find(copy, id);
  if (!found || found.node.kind !== 'control') return missing(id);
  take(copy, id);
  return { tree: copy };
}

/** Sets attributes of control `id` (`null` or `undefined` removes one). */
export function setAttributes(tree, id, patch) {
  const copy = edit(tree);
  const found = find(copy, id)?.node;
  if (!found || found.kind !== 'control') return missing(id);
  for (const [name, value] of Object.entries(patch)) {
    if (value === null || value === undefined) delete found.attributes[name];
    else found.attributes[name] = String(value);
  }
  return { tree: copy };
}

/** A scope's card spanning the grid (`wide`, with the `scope` class) or not. */
export function setScopeWide(tree, id, wide) {
  const card = cardOf(tree, id);
  if (!card) return missing(id);
  const result = setWide(tree, card.id, wide);
  if (result.error) return result;
  const node = find(result.tree, card.id).node;
  node.classes = node.classes.filter((c) => c !== 'scope');
  if (wide) node.classes.splice(1, 0, 'scope');
  return result;
}

/** Adds the control `{tag, attributes}` into `into` (as `moveControl`). `{tree, id}`. */
export function addControl(tree, { tag, attributes }, { into, index = Infinity }) {
  const copy = edit(tree);
  const container = target(copy, into);
  if (!container) return missing(into);
  const id = fresh(copy);
  container.children.splice(Math.min(index, container.children.length), 0, { id, kind: 'control', tag, attributes: { ...attributes } });
  return { tree: copy, id };
}

// The palette.

/** The kinds of control the palette offers, with their elements. */
export const KINDS = Object.freeze({
  knob: 'signal-knob',
  fader: 'signal-fader',
  button: 'signal-button',
  toggle: 'signal-toggle',
  meter: 'signal-meter',
  scope: 'signal-scope',
  value: 'signal-value',
  xy: 'signal-xy',
});

/**
 * Every parameter and port of a `describe` reply, as the palette lists it:
 * `{address, label, what, kinds}` (the kinds of control that bind it; none
 * for a port no control in the palette shows), with `unit`, `min`, `max`
 * for a parameter. The controls (top-level parameters) first.
 */
export function paletteEntries(description) {
  const d = description ?? {};
  const entries = [];
  const listed = new Set();
  for (const c of d.controls ?? []) {
    listed.add(c.address);
    entries.push({ address: c.address, label: `${c.instance}.${c.parameter}`, what: 'control', unit: c.unit, min: c.min, max: c.max, kinds: ['knob', 'fader', 'toggle', 'xy', 'value'] });
  }
  const sinks = new Set((d.sinks ?? []).filter((s) => s.kind === 'audio').map((s) => s.address));
  for (const instance of d.instances ?? []) {
    for (const p of instance.parameters ?? []) {
      if (listed.has(p.address)) continue;
      entries.push({ address: p.address, label: p.address, what: 'parameter', unit: p.unit ?? null, min: p.range?.min, max: p.range?.max, kinds: ['value'] });
    }
    for (const p of instance.ports ?? []) {
      let kinds = [];
      if (p.shape === 'events' && p.direction === 'in') kinds = ['button'];
      else if (p.shape === 'signal' && p.direction === 'in' && sinks.has(instance.address)) kinds = ['meter', 'scope'];
      else if (p.shape === 'signal') kinds = ['scope'];
      entries.push({ address: p.address, label: p.address, what: `${p.shape} ${p.direction}`, kinds });
    }
  }
  return entries;
}

/**
 * The entries matching `query` (every word, in the address or label, any
 * case): at most `limit` of them, and how many more there are, so the list
 * keeps its height.
 */
export function searchPalette(entries, query, limit = 8) {
  const words = String(query ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  const matches = entries.filter((e) => words.every((w) => `${e.address} ${e.label}`.toLowerCase().includes(w)));
  return { shown: matches.slice(0, limit), more: Math.max(0, matches.length - limit) };
}

/**
 * The control of kind `kind` for `entries` (one; two for a 2D input, x then
 * y) with its defaults from the parameter: a knob's or fader's range and
 * taper as the generated panel gives them (`controlSpec`), a toggle's range.
 * `{tag, attributes}` or `{error}`.
 */
export function newControl(kind, entries, description = {}) {
  const tag = KINDS[kind];
  const [first, second] = entries;
  if (!tag || !first) return { error: `no control kind ${kind}` };
  if (!first.kinds.includes(kind) || (kind === 'xy' && !second?.kinds.includes('xy'))) {
    return { error: `${first.address} cannot be shown with a ${kind}` };
  }
  const name = (e) => e.label.split('.').pop();
  const control = (e) => (description.controls ?? []).find((c) => c.address === e.address) ?? { address: e.address, unit: e.unit, min: e.min, max: e.max, ports: [] };
  const range = (e) => (Number.isFinite(e.min) && Number.isFinite(e.max) ? { min: String(e.min), max: String(e.max) } : {});
  if (kind === 'knob' || kind === 'fader') {
    const spec = controlSpec(control(first), description);
    return { tag, attributes: { bind: first.address, taper: spec.taper, ...range(first), label: name(first) } };
  }
  if (kind === 'toggle') return { tag, attributes: { bind: first.address, ...range(first), label: name(first) } };
  if (kind === 'xy') {
    const spec = controlSpec(control(first), description);
    return { tag, attributes: { bind: `${first.address} ${second.address}`, taper: spec.taper, label: `${name(first)} · ${name(second)}` } };
  }
  return { tag, attributes: { bind: first.address, label: name(first) } };
}

// History and input.

/**
 * The written versions of this session (canonical texts), and which one
 * the file holds. Undo and redo name the version to write; it becomes the
 * current one once written (`moved`). A new write drops the versions after
 * the current one.
 */
export class History {
  constructor(start) {
    this.versions = [start];
    this.at = 0;
  }

  get current() {
    return this.versions[this.at];
  }

  get canUndo() {
    return this.at > 0;
  }

  get canRedo() {
    return this.at < this.versions.length - 1;
  }

  /** The version undo writes, or null. */
  undo() {
    return this.canUndo ? this.versions[this.at - 1] : null;
  }

  /** The version redo writes, or null. */
  redo() {
    return this.canRedo ? this.versions[this.at + 1] : null;
  }

  /** Undo (-1) or redo (+1) was written. */
  moved(step) {
    this.at = Math.max(0, Math.min(this.versions.length - 1, this.at + step));
  }

  /** A new version was written. */
  push(text) {
    this.versions.splice(this.at + 1);
    this.versions.push(text);
    this.at = this.versions.length - 1;
  }
}

/**
 * Where a drop at `point` goes among boxes laid out in reading order (a
 * grid's cards, a wrapping row's controls): before the first box on a line
 * below the point, or on its line with its middle right of the point; else
 * at the end.
 */
export function insertionIndex(rects, { x, y }) {
  for (const [i, r] of rects.entries()) {
    if (y < r.top) return i;
    if (y <= r.bottom && x < r.left + r.width / 2) return i;
  }
  return rects.length;
}

const FIELDS = new Set(['input', 'textarea', 'select']);

/**
 * What a key does in edit mode (the `panel_edit` scope, ui/lib/keys.default.js):
 * `undo` (⌘Z), `redo` (⇧⌘Z), `leave` (Escape, wherever the focus is), or
 * null; ⌘Z in a text field is the field's own.
 */
export function editKey(event, scope = 'panel_edit') {
  const command = keyCommand(scope, event)?.id;
  // Escape leaves from anywhere, a field included (its text not applied).
  if (command === `${scope}.leave`) return 'leave';
  // The element itself, inside a shadow root too (the editor's own fields).
  const target = event?.composedPath?.()[0] ?? event?.target;
  if (FIELDS.has(target?.localName ?? '')) return null;
  if (command === `${scope}.undo`) return 'undo';
  if (command === `${scope}.redo`) return 'redo';
  return null;
}

/**
 * What the editor writes against, kept for the page's session: the file,
 * the base (the SHA-256 the next write names), the history of written
 * versions, and why writing is stopped, if it is. `read(html)` reads a
 * panel's content as the editor shows it: `{tree, text}` (its canonical
 * text), or `{error}` when it is not editable.
 */
export class EditSession {
  constructor(read) {
    this.read = read;
    this.file = null;
    this.base = null;
    this.history = null;
    this.blocked = null;
    this.busy = false;
  }

  /**
   * Edit mode entered on a `panel.read` reply, showing a version whose
   * canonical text is `text`: this session's history goes on while the file
   * is still the version it last wrote or read (the same file and base);
   * otherwise it starts again at `text`. Whether it went on.
   */
  enter(reply, text) {
    const kept = this.history !== null && !this.blocked && this.file === reply.file && this.base === reply.base;
    if (!kept) this.history = new History(text);
    this.file = reply.file;
    this.base = reply.base;
    this.blocked = null;
    this.busy = false;
    return kept;
  }

  /** The `panel.write` of `text`, or `{refused}` (writing stopped, or a write pending). */
  request(text) {
    if (this.blocked) return { refused: this.blocked };
    if (this.busy) return { refused: 'Writing the last change: try again' };
    this.busy = true;
    return { message: { type: 'panel.write', file: this.file, html: text, base: this.base } };
  }

  /** The write of `text` (`step` 0: a new version; -1, +1: undo, redo) was taken. */
  written(reply, text, step) {
    this.busy = false;
    this.base = reply.base;
    if (step === 0) this.history.push(text);
    else this.history.moved(step);
  }

  /**
   * The file is another version than this session's (a `conflict`'s
   * `{base, html}`, or another client's `panel.written`): `{adopt: tree}`
   * when it is editable (its base taken, the history starting at it);
   * otherwise writing stops (`{blocked}`) until the panel is read again,
   * the base not taken, so nothing the page holds overwrites what it
   * cannot show.
   */
  changed({ base, html }) {
    const read = typeof html === 'string' ? this.read(html) : { error: 'the file has no one <signal-panel> to edit' };
    if (read.error) {
      this.blocked = `The file changed elsewhere and is not editable here (${read.error}): nothing more is written; edit it by hand`;
      return { blocked: this.blocked };
    }
    this.base = base;
    this.history = new History(read.text);
    return { adopt: read.tree };
  }

  /** A write was refused: a conflict as `changed`, else `{restore: why}` (the last written version shown again). */
  refused(error) {
    this.busy = false;
    if (error?.code === 'conflict') return this.changed(error.reply ?? {});
    return { restore: error?.message ?? String(error) };
  }
}

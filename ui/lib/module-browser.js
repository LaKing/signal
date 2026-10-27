// The Add module browser (Edit patch ▸ Add module here…, docs/ui-controls.md
// "Editing the patch"; the owner's request of 2026-10-27): the module types
// of a `modules` reply as a tree, by source and then by category; a search
// that filters it; the rows it shows, the cursor and its keys; and what the
// detail pane says of the type under the cursor (its description, its inputs,
// outputs and parameters with their kinds, ranges and value sets). No DOM.

import { keyCommand } from './keys.js';
import { OTHER, REFERENCE, groupOf, groupOrder, words } from './schematic-palette.js';
import { isDiscrete, nearestValue } from './value-set.js';

/**
 * Where a type comes from: the tree's top level, in this order. `match`
 * says which entries are a source's; the first that matches takes it. A
 * primitive's source is its namespace (the module name before its first
 * dot): `std` and `lab` are named here, another namespace becomes a source
 * of its own (`<ns>.* primitives`, after these, in name order), and a name
 * without a dot goes under `other`.
 */
export const SOURCES = Object.freeze([
  Object.freeze({ id: 'project', label: 'This project', about: 'the project’s own composites (composites/*.toml)' }),
  Object.freeze({ id: 'library', label: 'Library', about: 'the built-in library’s composites (lib.*, ADR 0027 §4)' }),
  Object.freeze({ id: 'folders', label: 'Projects', about: 'projects beside this one, loaded into a slot as content (ADR 0035, 0037; the Load dialog only)' }),
  Object.freeze({ id: 'std', label: 'Standard primitives', about: 'the native types of signal-std (std.*)' }),
  Object.freeze({ id: 'lab', label: 'Lab primitives', about: 'experimental native types (lab.*, a lab build only)' }),
]);

/** The sources expanded until the viewer changes them (keys, as `expanded` holds them). */
export const EXPANDED = Object.freeze(['s:project', 's:std']);

/** The source of an entry without a namespace. */
const NO_SOURCE = 'other';

/**
 * The source of a `modules` entry (or a `browserEntries` one): `project` for
 * a project composite, `library` for a library composite (`library`, or a
 * `lib.*` name), else the primitive's namespace (`std`), else `other`.
 */
export function sourceOf(m) {
  const module = String(m?.module ?? '');
  if (m?.project === true) return 'folders';
  if (m?.library === true || /^lib\./.test(module)) return 'library';
  if (m?.composite) return 'project';
  const dot = module.indexOf('.');
  return dot > 0 ? module.slice(0, dot) : NO_SOURCE;
}

/** A source's name in the tree. */
export function sourceLabel(id) {
  return SOURCES.find((s) => s.id === id)?.label ?? (id === NO_SOURCE ? 'Other' : `${id}.* primitives`);
}

/** Sorts sources: `SOURCES` in their order, then the others by name, `other` last. */
export function sourceOrder(a, b) {
  const rank = (id) => {
    const at = SOURCES.findIndex((s) => s.id === id);
    return at >= 0 ? at : id === NO_SOURCE ? SOURCES.length + 1 : SOURCES.length;
  };
  return rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0);
}

/**
 * The category of an entry within its source: `reference` for a validation
 * reference (it names a `reference_for`), else its `category`, else null.
 */
export function categoryOf(m) {
  if (typeof m?.reference_for === 'string' && m.reference_for) return REFERENCE;
  if (typeof m?.referenceFor === 'string' && m.referenceFor) return REFERENCE;
  return typeof m?.category === 'string' && m.category ? m.category : null;
}

/**
 * The browser's entries of a `modules` reply: primitives, then composites,
 * each `{module, composite, library, source, category, group, description,
 * referenceFor, ports, parameters, inside}` (`inside` a composite's level as
 * `graph` gives it, else null).
 */
export function browserEntries(modules) {
  const entry = (m) => ({
    module: m.module,
    composite: Boolean(m.composite),
    library: sourceOf(m) === 'library',
    source: sourceOf(m),
    category: categoryOf(m),
    group: groupOf(m),
    description: typeof m.description === 'string' ? m.description : null,
    referenceFor: typeof m.reference_for === 'string' && m.reference_for ? m.reference_for : null,
    ports: Array.isArray(m.ports) ? m.ports : [],
    parameters: Array.isArray(m.parameters) ? m.parameters : [],
    inside: m.inside && typeof m.inside === 'object' ? m.inside : null,
  });
  return [...(modules?.primitives ?? []).map(entry), ...(modules?.composites ?? []).map(entry)];
}

/** Whether `entry` matches every word of a search (any case) in its name, description, category or source. */
export function entryMatches(entry, searchWords) {
  const text = `${entry.module} ${entry.description ?? ''} ${entry.category ?? ''} ${sourceLabel(entry.source)}`.toLowerCase();
  return searchWords.every((w) => text.includes(w));
}

/** A category's key in the tree. */
function categoryKey(source, category) {
  return `c:${source}/${category}`;
}

/**
 * The tree of `entries` that match `query`: `{sources, matched, searching}`.
 * A source `{kind: 'source', key: 's:<id>', id, label, count, children}`
 * holds its categories `{kind: 'category', key: 'c:<source>/<category>',
 * source, id, label, count, children}`, in the palette's category order
 * (`groupOrder`: `GROUPS` first, an unknown one after them by name, `other`,
 * then `reference` last), each holding its entries `{kind: 'entry', key:
 * 'm:<module>', entry}` in the reply's order. A source none of whose entries
 * names a category holds its entries directly; in one that has categories,
 * an entry without one is under `other`. Only what matches is in the tree:
 * a branch without a match is left out.
 */
export function browserTree(entries, query = '') {
  const w = words(query);
  const searching = w.length > 0;
  const bySource = new Map();
  let matched = 0;
  for (const entry of entries ?? []) {
    if (searching && !entryMatches(entry, w)) continue;
    matched++;
    if (!bySource.has(entry.source)) bySource.set(entry.source, []);
    bySource.get(entry.source).push(entry);
  }
  const leaf = (entry) => ({ kind: 'entry', key: `m:${entry.module}`, entry });
  const sources = [...bySource.keys()].sort(sourceOrder).map((id) => {
    const list = bySource.get(id);
    let children;
    if (list.every((e) => !e.category)) children = list.map(leaf);
    else {
      const byCategory = new Map();
      for (const entry of list) {
        const category = entry.category ?? OTHER;
        if (!byCategory.has(category)) byCategory.set(category, []);
        byCategory.get(category).push(entry);
      }
      children = [...byCategory.keys()].sort(groupOrder).map((category) => ({
        kind: 'category',
        key: categoryKey(id, category),
        source: id,
        id: category,
        label: category,
        count: byCategory.get(category).length,
        children: byCategory.get(category).map(leaf),
      }));
    }
    return { kind: 'source', key: `s:${id}`, id, label: sourceLabel(id), count: list.length, children };
  });
  return { sources, matched, searching };
}

/**
 * The rows the tree shows: `{rows, matched, searching}`, each row a node of
 * the tree with `depth` (0 a source), `parent` (its branch's key, null for
 * a source) and, on a branch, `expanded`. Without a search, a branch's rows
 * show when it is in `expanded` (a Set of keys); with one, every branch is
 * open.
 */
export function treeRows(tree, expanded = new Set()) {
  const rows = [];
  const walk = (node, depth, parent) => {
    if (node.kind === 'entry') {
      rows.push({ ...node, depth, parent });
      return;
    }
    const open = tree.searching || expanded.has(node.key);
    rows.push({ ...node, depth, parent, expanded: open });
    if (open) for (const child of node.children) walk(child, depth + 1, node.key);
  };
  for (const source of tree.sources) walk(source, 0, null);
  return { rows, matched: tree.matched, searching: tree.searching };
}

/** The rows the cursor moves over: every row without a search, the entries alone with one (their branches are all open). */
export function cursorRows(view) {
  return view.searching ? view.rows.filter((r) => r.kind === 'entry') : view.rows;
}

/** Where the cursor (a row key) is: `key` while it shows, else the first entry shown, else the first row, else null. */
export function placeCursor(view, key) {
  const rows = cursorRows(view);
  if (key && rows.some((r) => r.key === key)) return key;
  return (rows.find((r) => r.kind === 'entry') ?? rows[0])?.key ?? null;
}

/** The cursor moved by `delta` rows (±Infinity: the ends), held at the ends. */
export function moveCursor(view, key, delta) {
  const rows = cursorRows(view);
  if (!rows.length) return null;
  const at = rows.findIndex((r) => r.key === key);
  if (at < 0) return placeCursor(view, null);
  const to = Math.max(0, Math.min(rows.length - 1, delta === Infinity ? rows.length - 1 : delta === -Infinity ? 0 : at + delta));
  return rows[to].key;
}

/**
 * What a key in the browser's search field does (the `palette` scope,
 * ui/lib/keys.default.js), by the cursor's row: `{move: delta}` for the
 * arrows, PageUp, PageDown and (with a modifier) Home and End; on a branch
 * without a search, Enter `{toggle: key}`, → `{toggle}` when it is closed
 * else `{cursor: its first row}`, ← `{toggle}` when it is open else
 * `{cursor: its parent}`; ← on an entry `{cursor: its branch}` while the
 * search field is empty (else it is the field's caret key); Enter on an
 * entry `add`; else null (the field's own, as is every key while an IME
 * composes).
 */
export function browserKey(event, view, key, { fieldEmpty = true } = {}) {
  // A key composing text (an IME's Enter or arrows) is the field's.
  if (event?.isComposing) return null;
  const row = view.rows.find((r) => r.key === key);
  const command = keyCommand('palette', event)?.id;
  const moves = { 'palette.down': 1, 'palette.up': -1, 'palette.page_down': 8, 'palette.page_up': -8, 'palette.first': -Infinity, 'palette.last': Infinity };
  if (command in moves) return { move: moves[command] };
  if (row && row.kind !== 'entry' && !view.searching) {
    if (command === 'palette.choose') return { toggle: row.key };
    if (command === 'palette.open_group') {
      if (!row.expanded) return { toggle: row.key };
      const first = view.rows[view.rows.indexOf(row) + 1];
      return first?.parent === row.key ? { cursor: first.key } : null;
    }
    if (command === 'palette.close_group') {
      if (row.expanded) return { toggle: row.key };
      return row.parent ? { cursor: row.parent } : null;
    }
  }
  if (row?.kind === 'entry' && command === 'palette.close_group' && fieldEmpty && !view.searching && row.parent) return { cursor: row.parent };
  if (command === 'palette.choose') return row?.kind === 'entry' || !row ? 'add' : null;
  return null;
}

/**
 * Whether a key pressed in the browser goes on to the page (Signal.app's
 * commands, ui/controls/keyboard.js): a ⌘ or Ctrl chord does, but for Edit
 * patch's undo and redo (the patch behind a modal is not the browser's to
 * change: there they are the field's own). Every other key is the
 * browser's and its fields'; a key that composes text (an IME) is the field's.
 */
export function passesToPage(event) {
  if (event?.isComposing) return false;
  if (!event?.metaKey && !event?.ctrlKey) return false;
  const edit = keyCommand('schematic_edit', event)?.id;
  return edit !== 'schematic_edit.undo' && edit !== 'schematic_edit.redo';
}

/** `expanded` with `key` opened or closed (a new Set). */
export function toggled(expanded, key) {
  const next = new Set(expanded);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

/** The keys of the branches above row `key` in `tree`, outermost first; null when it is not in the tree. */
export function pathTo(tree, key) {
  const find = (node, above) => {
    if (node.key === key) return above;
    for (const child of node.children ?? []) {
      const found = find(child, [...above, node.key]);
      if (found) return found;
    }
    return null;
  };
  for (const source of tree.sources) {
    const found = find(source, []);
    if (found) return found;
  }
  return null;
}

/** `expanded` with every branch above row `key` of `tree` opened (a new Set): a search's choice still shows once the search is cleared. */
export function revealed(expanded, tree, key) {
  return new Set([...expanded, ...(pathTo(tree, key) ?? [])]);
}

// What the detail pane says.

/** A number as the detail pane writes it: up to 6 significant digits, a true minus, ∞ for an unbounded end. */
export function numberText(v) {
  if (v === Infinity) return '∞';
  if (v === -Infinity) return '−∞';
  if (typeof v !== 'number' || Number.isNaN(v)) return '—';
  const text = String(Number(v.toPrecision(6)));
  return text.startsWith('-') ? `−${text.slice(1)}` : text;
}

/** A range `{min, max}` as `[min, max]`, null when there is none. */
export function rangeText(range) {
  if (!range || typeof range !== 'object') return null;
  const has = (v) => typeof v === 'number' || v === null;
  if (!has(range.min) && !has(range.max)) return null;
  const end = (v, fallback) => (typeof v === 'number' ? numberText(v) : fallback);
  return `[${end(range.min, '−∞')}, ${end(range.max, '∞')}]`;
}

/** A kind (`audio`, `trigger`, `frequency`, …) as a heading word: `Audio`; null as `—`. */
export function kindText(kind) {
  if (typeof kind !== 'string' || !kind) return '—';
  return kind.charAt(0).toUpperCase() + kind.slice(1).replace(/_/g, ' ');
}

/** A port's shape: `signal`, `events`, or `vector ×n` (`n` its rows). */
export function shapeText(shape) {
  if (shape && typeof shape === 'object' && 'vector' in shape) return `vector ×${shape.vector}`;
  return shape === 'events' ? 'events' : 'signal';
}

/** A rate divisor: `full rate` for 1, else `÷n`. */
export function rateText(rate) {
  return !Number.isFinite(rate) || rate === 1 ? 'full rate' : `÷${rate}`;
}

/** A value set as `value → label` lines, in its order (ADR 0043). */
export function valueSetText(values) {
  return (values ?? []).map((v) => `${numberText(v.value)} → ${v.label}`);
}

/**
 * A parameter's default: the number, with its entry's name for a value set
 * (`1 (saw)`), or the entry it snaps to when it is none of them (`1.4 (→ saw)`).
 */
export function defaultText(p) {
  if (typeof p?.default !== 'number') return '—';
  if (!isDiscrete(p)) return numberText(p.default);
  const exact = p.values.find((v) => v.value === p.default);
  if (exact) return `${numberText(p.default)} (${exact.label})`;
  const near = p.values.find((v) => v.value === nearestValue(p.values, p.default, p.snap));
  return near ? `${numberText(p.default)} (→ ${near.label})` : numberText(p.default);
}

/**
 * A port's row in the detail pane: `{name, kind, shape, rate, range, unit}`,
 * texts all (`—` for what is not known or does not apply).
 */
export function portRow(p) {
  return {
    name: String(p?.name ?? ''),
    kind: kindText(p?.kind),
    shape: shapeText(p?.shape),
    rate: rateText(p?.rate),
    range: rangeText(p?.range) ?? '—',
    unit: p?.unit ? String(p.unit) : '—',
  };
}

/**
 * A parameter's row: `{name, kind, range, values, default, unit, combine}`:
 * `range` its `[min, max]`, `values` the value set's lines (null for a
 * continuous one, whose range is what it takes), `default` as `defaultText`.
 */
export function parameterRow(p) {
  return {
    name: String(p?.name ?? ''),
    kind: kindText(p?.kind),
    range: rangeText(p?.range) ?? '—',
    values: isDiscrete(p) ? valueSetText(p.values) : null,
    default: defaultText(p),
    unit: p?.unit ? String(p.unit) : '—',
    combine: typeof p?.combine === 'string' ? p.combine : '—',
  };
}

/** A type's description cut to its first sentence (a full stop, ! or ? before a space or the end), for its row in the tree; '' without one. */
export function summary(text) {
  if (!text) return '';
  const match = /^(.+?[.!?])(\s|$)/s.exec(text);
  return (match ? match[1] : text).replace(/\s+/g, ' ');
}

/** What a source and category say of an entry, on one line: `Standard primitives · filter`. */
export function whereText(entry) {
  const category = entry?.category ? ` · ${entry.category}` : '';
  return `${sourceLabel(entry?.source)}${category}`;
}

/**
 * The detail pane of `entry`: `{module, where, description, referenceFor,
 * inputs, outputs, parameters, counts}`, the rows `portRow`'s and
 * `parameterRow`'s in the reply's order, `description` the full text (null
 * when the type has none), `counts` e.g. `2 inputs, 1 output, 3 parameters`.
 */
export function detail(entry) {
  if (!entry) return null;
  const ports = entry.ports ?? [];
  const inputs = ports.filter((p) => p.direction === 'in').map(portRow);
  const outputs = ports.filter((p) => p.direction === 'out').map(portRow);
  const parameters = (entry.parameters ?? []).map(parameterRow);
  const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
  return {
    module: entry.module,
    where: whereText(entry),
    description: entry.description ?? null,
    referenceFor: entry.referenceFor ?? null,
    inputs,
    outputs,
    parameters,
    counts: [plural(inputs.length, 'input'), plural(outputs.length, 'output'), plural(parameters.length, 'parameter')].join(', '),
  };
}

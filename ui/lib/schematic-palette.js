// The schematic's add palette grouped (ADR 0027 §6: "the palette groups by
// `category` (and library, project)"; docs/ui-controls.md "Editing the
// patch"): the groups in their fixed order, the rows a search and the
// expanded groups show, the cursor the arrow keys move over them, and what
// a type is (its description, a native primitive's formula, and its
// group) for the palette and the sidebar. Entries are `paletteEntries`'
// (ui/lib/schematic-patch.js). No DOM.

/**
 * The palette's groups, in the order they show: the project's composites,
 * the library's (`lib.*`, reserved: ADR 0027 §4), then the std categories
 * (`signal_std::category`). A category not named here shows after them,
 * in name order, and an entry without one under `other`, last.
 */
export const GROUPS = Object.freeze(['project', 'library', 'core', 'generator', 'filter', 'envelope', 'delay', 'physical', 'event', 'visual', 'io']);

/** The group of entries that name none. */
export const OTHER = 'other';

/** The groups the palette opens with expanded, until the viewer changes them. */
export const EXPANDED = Object.freeze(['project']);

/**
 * The group of a `modules` entry: a library composite's `library` (marked
 * `library`, or named `lib.*`), another composite's `project`, a
 * primitive's `category`, else `other`.
 */
export function groupOf(m) {
  if (m?.library === true || /^lib\./.test(String(m?.module ?? ''))) return 'library';
  if (m?.composite) return 'project';
  return typeof m?.category === 'string' && m.category ? m.category : OTHER;
}

/** Sorts group names: `GROUPS` first in their order, then the others by name, `other` last. */
export function groupOrder(a, b) {
  const rank = (g) => (g === OTHER ? GROUPS.length + 1 : GROUPS.includes(g) ? GROUPS.indexOf(g) : GROUPS.length);
  return rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0);
}

/** Whether `entry` matches every word of `query` (any case) in its name, description or group. */
export function matches(entry, words) {
  const text = `${entry.module} ${entry.label} ${entry.group}`.toLowerCase();
  return words.every((w) => text.includes(w));
}

/** The words of a search. */
export function words(query) {
  return String(query ?? '').toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * The palette's rows for `entries`, search `query` and the groups
 * `expanded` (a Set): `{rows, matched, searching}`. A row is a group's
 * heading `{kind: 'group', key, group, count, expanded}` or an entry
 * `{kind: 'entry', key, entry}`, the key `g:<group>` or `m:<module>`.
 * Without a search, every group shows, counted, with its entries when
 * expanded. With one, only the groups with a match show, every one
 * expanded, counting its matches. Entries keep their order within a group.
 */
export function paletteRows(entries, query, expanded = new Set()) {
  const w = words(query);
  const searching = w.length > 0;
  const groups = new Map();
  let matched = 0;
  for (const entry of entries ?? []) {
    if (searching && !matches(entry, w)) continue;
    matched++;
    if (!groups.has(entry.group)) groups.set(entry.group, []);
    groups.get(entry.group).push(entry);
  }
  const rows = [];
  for (const group of [...groups.keys()].sort(groupOrder)) {
    const list = groups.get(group);
    const open = searching || expanded.has(group);
    rows.push({ kind: 'group', key: `g:${group}`, group, count: list.length, expanded: open });
    if (open) for (const entry of list) rows.push({ kind: 'entry', key: `m:${entry.module}`, entry });
  }
  return { rows, matched, searching };
}

/**
 * The rows the arrow keys move over: every row without a search (a
 * heading opens or closes with Enter), the entries alone with one (their
 * headings are always open).
 */
export function cursorRows(view) {
  return view.searching ? view.rows.filter((r) => r.kind === 'entry') : view.rows;
}

/**
 * Where the cursor (a row key) is in `view`: `key` when it still shows,
 * else the first entry shown, else the first row, else null.
 */
export function placeCursor(view, key) {
  const rows = cursorRows(view);
  if (key && rows.some((r) => r.key === key)) return key;
  return (rows.find((r) => r.kind === 'entry') ?? rows[0])?.key ?? null;
}

/**
 * The cursor moved by `delta` rows (ArrowDown +1, ArrowUp −1; Home and End
 * ±Infinity), across the groups, held at the ends.
 */
export function moveCursor(view, key, delta) {
  const rows = cursorRows(view);
  if (!rows.length) return null;
  const at = rows.findIndex((r) => r.key === key);
  if (at < 0) return placeCursor(view, null);
  const to = Math.max(0, Math.min(rows.length - 1, delta === Infinity ? rows.length - 1 : delta === -Infinity ? 0 : at + delta));
  return rows[to].key;
}

/**
 * What a key in the palette's search field does, by the cursor's row:
 * `{move: delta}` for the arrows, Home and End (with a modifier only:
 * plain Home and End are the field's), `{toggle: group}` for Enter on a
 * heading or ArrowRight / ArrowLeft opening / closing it (no search), `add`
 * for Enter on an entry, else null (the field's own).
 */
export function paletteKey(event, view, key) {
  const row = view.rows.find((r) => r.key === key);
  if (event.key === 'ArrowDown') return { move: 1 };
  if (event.key === 'ArrowUp') return { move: -1 };
  if (event.key === 'PageDown') return { move: 8 };
  if (event.key === 'PageUp') return { move: -8 };
  if ((event.metaKey || event.ctrlKey) && event.key === 'Home') return { move: -Infinity };
  if ((event.metaKey || event.ctrlKey) && event.key === 'End') return { move: Infinity };
  if (row?.kind === 'group' && !view.searching) {
    if (event.key === 'Enter') return { toggle: row.group };
    if (event.key === 'ArrowRight' && !row.expanded) return { toggle: row.group };
    if (event.key === 'ArrowLeft' && row.expanded) return { toggle: row.group };
  }
  if (event.key === 'Enter') return 'add';
  return null;
}

/** `expanded` with `group` opened or closed (a new Set). */
export function toggled(expanded, group) {
  const next = new Set(expanded);
  if (next.has(group)) next.delete(group);
  else next.add(group);
  return next;
}

/**
 * What a type is, by module name, from a `modules` reply:
 * `Map(module → {module, description, group, composite})`, the
 * description a native primitive's rustdoc summary with its formula
 * (`"Adds its two signal inputs: out = a + b."`), null for a composite.
 */
export function moduleInfo(modules) {
  const info = new Map();
  for (const m of [...(modules?.primitives ?? []), ...(modules?.composites ?? [])]) {
    info.set(m.module, { module: m.module, description: m.description ?? null, group: groupOf(m), composite: Boolean(m.composite) });
  }
  return info;
}

/** Which project a `graph` is (`graph.projectId`, else its folder and name), for what is known of its types. */
export function projectKey(graph) {
  return graph ? (graph.projectId ?? `${graph.folder ?? ''}\n${graph.project ?? ''}`) : null;
}

/**
 * The first of the `size` rows the list shows (it keeps its height and
 * never scrolls: ui/app/lib/layout.test.js), from `top` before: moved as
 * little as keeps row `at` (the cursor's; −1 none) in sight, and never past
 * the end while rows above are hidden.
 */
export function rowWindow(total, at, top, size) {
  let first = Math.max(0, Math.min(top || 0, total - size));
  if (at >= 0 && at < first) first = at;
  else if (at >= first + size) first = at - size + 1;
  return Math.max(0, first);
}

/**
 * The rows the list shows: `size` of `rows` from about `top`, moved to keep
 * row `at` in sight (`rowWindow`), and, when the window starts inside a
 * group, that group's heading first in place of one row, so the group
 * shown is always named. `{top, shown, above, below}`, `above` and
 * `below` the rows (headings included) out of sight.
 */
export function windowRows(rows, at, top, size) {
  const first = rowWindow(rows.length, at, top, size - 1);
  const start = rows[first];
  let shown;
  if (start?.kind === 'entry') {
    const heading = rows.findLast((r, i) => i < first && r.kind === 'group');
    shown = [heading, ...rows.slice(first, first + size - 1)].filter(Boolean);
  } else shown = rows.slice(first, first + size);
  const end = start?.kind === 'entry' ? first + size - 1 : first + size;
  return { top: first, shown, above: start?.kind === 'entry' ? first - 1 : first, below: Math.max(0, rows.length - end) };
}

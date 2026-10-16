// The schematic's sidebar (the owner's, 2026-10-17: "a fixed sidebar on
// the left, which makes the currently selected element's things analysable
// and editable"; docs/ui-controls.md "Schematic"): what it shows for the
// selection, which replaced the floating card. Nothing selected: the
// project in numbers and the drawing's legend; an instance: what it is, its
// pins with their connections, its parameters (the card's rows,
// `ui/lib/schematic-inspector.js`); a wire: its ends, shape, rate,
// converters and scale; a pin: what it is and its connections; several
// parts: how many of each kind, the members, and what Delete would write.
// Every connection and member is a link (`{label, hit}`, a hit as
// `hitTest` gives it) that selects what it names. The actions are the
// context menu's items for the same part (`schematicMenu`), so both say
// the same, disabled with the same reasons. No DOM.

import { card, converterText } from './schematic-inspector.js';
import { EDIT_OFF, schematicMenu } from './schematic-menu.js';
import { instanceCount } from './schematic-patch.js';
import { INPUTS, OUTPUTS } from './schematic-layout.js';
import { portHit, wireHit } from './schematic-hit.js';
import { deletePlan, hitKey } from './schematic-select.js';

/** The most members the sidebar lists; the rest are counted ("and N more"). */
export const MEMBERS_SHOWN = 200;

/** Why Delete writes nothing: one level's patch would be larger than a message (and one undo). */
export function tooLargeText(plan) {
  const [first] = plan.tooLarge;
  if (!first) return null;
  const kib = Math.ceil(first.bytes / 1024);
  return `Too much to delete at once: ${first.level || 'the top level'}'s patch would be ${kib} KiB, over the 64 KiB a message (and one undo) may hold. Select fewer.`;
}

/** The legend of the drawing: `[{swatch, label}]`, a swatch the class of what it shows. */
export const LEGEND = [
  { swatch: 'wire signal', label: 'signal' },
  { swatch: 'wire events', label: 'events' },
  { swatch: 'wire vector', label: 'vector' },
  { swatch: 'wire signal dashed', label: 'converter (rate changes)' },
  { swatch: 'pin signal connected', label: 'pin, connected' },
  { swatch: 'pin signal', label: 'pin, nothing connected' },
  { swatch: 'pin parameter', label: 'parameter left to its value' },
];

/** The context menu's items `ids` for `hit`, in `menu`'s state, in that order, as the sidebar's buttons. */
function actionsOf(hit, state, ids) {
  const items = schematicMenu(hit, state);
  return ids.map((id) => items.find((i) => i.id === id)).filter(Boolean);
}

function shapeText(shape) {
  if (shape && typeof shape === 'object' && 'vector' in shape) return `vector of ${shape.vector}`;
  return shape ?? '—';
}

function rateText(rate) {
  return rate == null ? '—' : `divisor ${rate}`;
}

/** Where a level is: "top level", or "composite kick, 2 instances". */
function levelText(graph, level) {
  if (!level) return 'top level';
  const n = instanceCount(graph, level);
  return `composite ${level}, ${n} instance${n === 1 ? '' : 's'}`;
}

/**
 * The connections at level `entry` (`{path, level, layout}`) of pin `port`
 * of node `node`: `[{wire, end, removable}]`, `wire` a link selecting the
 * wire, `end` one selecting the pin at its other end (`hit` null for a
 * composite's frame, an export's end), `removable` whether Edit patch may
 * remove it (a connection; an export is not edited here).
 */
export function connectionsOf(entry, node, port) {
  const out = port.direction === 'out';
  const list = [];
  for (const wire of entry.layout?.wires ?? []) {
    const here = out ? wire.from : wire.to;
    if (here.node !== node.id || here.port !== port.name) continue;
    const there = out ? wire.to : wire.from;
    const hit = wireHit(entry, wire);
    const other = entry.layout.nodes.find((n) => n.id === there.node);
    const pin = other?.kind === 'instance' ? (out ? other.ins : other.outs).find((p) => p.name === there.port) : null;
    const label = out ? hit.to : hit.from;
    list.push({
      wire: { label: `${out ? '→' : '←'} ${label}`, hit },
      end: { label, hit: pin ? portHit(entry, other, pin) : null },
      removable: wire.kind === 'connection',
    });
  }
  return list;
}

/** A pin's line in an instance's list: `{name, direction, role, shape, kind, rate, connected, hit, connections}`. */
function pinRow(entry, node, port) {
  return {
    name: port.name,
    direction: port.direction,
    role: port.parameter ? 'parameter' : port.direction === 'out' ? 'output' : 'input',
    shape: port.shape,
    kind: port.kind ?? null,
    rate: port.rate ?? null,
    connected: Boolean(port.connected),
    hit: portHit(entry, node, port),
    connections: connectionsOf(entry, node, port),
  };
}

/** `selected`'s level entry (`{path, level, layout}`), its layout from `layoutOf`. */
function entryOf(hit, layoutOf) {
  return { path: hit.path, level: hit.level, layout: layoutOf?.(hit.level) ?? null };
}

/**
 * The sidebar for `selected` (the selection: a list of hits, the last the
 * one added last) in `context`: `graph`, `description` (as the card takes
 * them), `layoutOf(level)`, `editing` (Edit patch), `shown` (the level
 * shown, `{path, level}`) and `menu(hit)` (the context menu's state for
 * `hit`, `menuContext`'s). Returns `{kind, title, subtitle, fields:
 * [[label, text]], …}` with, by kind:
 * - `project` (nothing selected): `legend`;
 * - `instance`: `pins` (pin rows, each with its connections), the card's
 *   `parameters` and `editable`, `actions`;
 * - `wire`: `links` (`from`, `to`), `actions`;
 * - `port`: `connections`, `meter` (whether a level bar can show its level:
 *   an instance's signal output), `actions` (Disconnect all; one
 *   connection is removed by its own `remove-connection`);
 * - `group` (several): `members` (links, the first `MEMBERS_SHOWN`; `more`
 *   counts the rest), `counts`, `plan` (`deletePlan`'s), `notes` and
 *   `actions` (Delete N, what would be deleted; Copy addresses).
 * An `actions` entry is a menu item (`{id, label, disabled, title?}`).
 */
export function sidebar(selected, context = {}) {
  const { graph, description, layoutOf, editing = false, shown = { path: '', level: '' } } = context;
  const menu = (hit) => ({ editing, level: shown.level, ...(context.menu?.(hit) ?? {}) });
  const list = (selected ?? []).filter((s) => s && s.kind !== 'background');
  if (list.length === 0) return projectSidebar(graph, layoutOf, shown);
  if (list.length > 1) return groupSidebar(list, editing);
  const hit = list[0];
  const entry = entryOf(hit, layoutOf);
  const base = card(hit, { graph, description });
  if (hit.kind === 'instance') {
    const node = hit.node;
    const inside = node.composite ? (graph?.composites?.[node.module]?.instances ?? []).length : 0;
    return {
      ...base,
      fields: [
        ['address', hit.address],
        ['module', node.module],
        ['composite', node.composite ? `yes, ${inside} instance${inside === 1 ? '' : 's'} inside` : 'no'],
        ['level', levelText(graph, hit.level)],
      ],
      pins: [...node.ins, ...node.outs].map((p) => pinRow(entry, node, p)),
      // A primitive has no inside: its sidebar leaves Enter and Open out
      // (the context menu keeps them, disabled, with the reason).
      actions: actionsOf(hit, menu(hit), node.composite ? ['enter', 'open', 'close', 'copy-address', 'delete'] : ['copy-address', 'delete']),
    };
  }
  if (hit.kind === 'wire') {
    const wire = hit.wire;
    const source = entry.layout?.nodes.find((n) => n.id === wire.from.node)?.outs.find((p) => p.name === wire.from.port);
    const converters = wire.converters ?? [];
    const endOf = (side) => {
      const end = wire[side];
      if (end.node === INPUTS || end.node === OUTPUTS) return null;
      const node = entry.layout?.nodes.find((n) => n.id === end.node);
      const pin = (side === 'from' ? node?.outs : node?.ins)?.find((p) => p.name === end.port);
      return pin ? portHit(entry, node, pin) : null;
    };
    return {
      ...base,
      fields: [
        ['shape', wire.shape ?? '—'],
        ['rate', rateText(source?.rate)],
        ['scale', wire.scale == null ? '1' : String(wire.scale)],
        ['converter', converters.length ? converters.map(converterText).join('; ') : 'none'],
        ['level', levelText(graph, hit.level)],
      ],
      links: [
        { name: 'from', label: hit.from, hit: endOf('from') },
        { name: 'to', label: hit.to, hit: endOf('to') },
      ],
      actions: actionsOf(hit, menu(hit), ['scale', 'copy-address', 'delete']),
    };
  }
  // A pin.
  const port = hit.port;
  const declared = port.parameter ? (hit.node.parameters ?? []).find((p) => p.name === port.name) : null;
  const fields = [
    ['address', hit.address],
    ['direction', port.parameter ? 'parameter (in)' : port.direction === 'out' ? 'output' : 'input'],
    ['shape', shapeText(port.shape)],
    ['kind', port.kind ?? '—'],
    ['rate', rateText(port.rate)],
  ];
  if (declared) {
    const range = declared.range ?? {};
    fields.push(['unit', declared.unit ?? '—'], ['range', range.min == null && range.max == null ? '—' : `${range.min ?? '−∞'} … ${range.max ?? '∞'}`]);
    if (declared.default != null) fields.push(['default', String(declared.default)]);
  }
  const connections = connectionsOf(entry, hit.node, port);
  fields.push(['connected', connections.length ? `${connections.length} wire${connections.length === 1 ? '' : 's'}` : port.connected ? 'yes' : 'no']);
  return {
    ...base,
    fields,
    connections,
    meter: hit.node.kind === 'instance' && port.direction === 'out' && port.shape === 'signal',
    actions: actionsOf(hit, menu(hit), ['copy-address', 'disconnect']),
  };
}

function projectSidebar(graph, layoutOf, shown) {
  if (!graph) return { kind: 'project', title: 'No schematic', subtitle: '', fields: [], legend: LEGEND };
  const top = graph.top ?? {};
  const fields = [
    ['instances', String((top.instances ?? []).length)],
    ['connections', String((top.connections ?? []).length)],
    ['composites', String(Object.keys(graph.composites ?? {}).length)],
    ['level shown', shown.level ? `${shown.path} (${shown.level})` : 'top level'],
  ];
  if (shown.level) {
    const inside = layoutOf?.(shown.level);
    const instances = (inside?.nodes ?? []).filter((n) => n.kind === 'instance').length;
    const connections = (inside?.wires ?? []).filter((w) => w.kind === 'connection').length;
    fields.push(['its instances', String(instances)], ['its connections', String(connections)], ['instances of it', String(instanceCount(graph, shown.level))]);
  }
  return { kind: 'project', title: graph.project ?? 'project', subtitle: 'nothing selected: click a box, a pin or a wire', fields, legend: LEGEND };
}

function groupSidebar(list, editing) {
  const counts = { instance: 0, wire: 0, port: 0 };
  for (const s of list) counts[s.kind] = (counts[s.kind] ?? 0) + 1;
  const words = [
    [counts.instance, 'instance', 'instances'],
    [counts.wire, 'wire', 'wires'],
    [counts.port, 'pin', 'pins'],
  ]
    .filter(([n]) => n > 0)
    .map(([n, one, many]) => `${n} ${n === 1 ? one : many}`);
  const plan = deletePlan(list);
  const levels = plan.patches.length;
  const nothing = levels === 0 ? 'Nothing here is deleted: pins and exports stay' : tooLargeText(plan);
  const notes = [];
  if (plan.tooLarge.length) notes.push(tooLargeText(plan));
  if (levels > 1) notes.push(`Delete writes ${levels} patches, one per level (${plan.patches.map((p) => p.level || 'top level').join(', ')}): each is undone on its own`);
  if (plan.skipped > 0 && levels > 0) notes.push(`${plan.skipped} pin${plan.skipped === 1 ? '' : 's'} or export${plan.skipped === 1 ? '' : 's'} not deleted`);
  return {
    kind: 'group',
    title: `${list.length} selected`,
    subtitle: words.join(', '),
    fields: [
      ['instances', String(counts.instance)],
      ['wires', String(counts.wire)],
      ['pins', String(counts.port)],
    ],
    counts,
    members: list.slice(0, MEMBERS_SHOWN).map((s) => ({ label: s.kind === 'wire' ? `${s.from} → ${s.to}` : s.address, kind: s.kind, key: hitKey(s), hit: s })),
    more: Math.max(0, list.length - MEMBERS_SHOWN),
    plan,
    notes,
    actions: [
      { id: 'delete-all', label: plan.deleted ? `Delete ${plan.deleted}` : 'Delete all', disabled: !editing || Boolean(nothing), ...(editing ? (nothing ? { title: nothing } : {}) : { title: EDIT_OFF }) },
      { id: 'copy-addresses', label: 'Copy addresses', disabled: false },
    ],
  };
}

/** The text "Copy addresses" copies: one line per member, a wire as `from → to`. */
export function addressesText(selected) {
  return selected.map((s) => (s.kind === 'wire' ? `${s.from} → ${s.to}` : s.address)).join('\n');
}

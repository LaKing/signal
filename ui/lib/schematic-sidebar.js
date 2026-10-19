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
import { groupOf } from './schematic-palette.js';
import { portHit, wireHit } from './schematic-hit.js';
import { deletePlan, hitKey } from './schematic-select.js';
import { traceTree } from './schematic-trace.js';

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
    const pin = other ? (out ? other.ins : other.outs).find((p) => p.name === there.port) : null;
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
 * them), `types` (what each module type is: `moduleInfo`'s Map, null
 * until known), `layoutOf(level)`, `editing` (Edit patch), `shown` (the level
 * shown, `{path, level}`) and `menu(hit)` (the context menu's state for
 * `hit`, `menuContext`'s). Returns `{kind, title, subtitle, fields:
 * [[label, text]], …}` with, by kind:
 * - `project` (nothing selected): `legend`;
 * - `instance`: `about` (its type's description, a native primitive's
 *   formula; null for a composite or until `types` is known), its
 *   `category` field (the palette's group), `pins` (pin rows, each with its connections), the card's
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
  const { graph, description, types = null, layoutOf, editing = false, shown = { path: '', level: '' } } = context;
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
    const type = types?.get(node.module) ?? null;
    // A composite's group needs no reply; a primitive's is its type's category.
    const category = type?.group ?? (node.composite ? groupOf(node) : null);
    return {
      ...base,
      about: node.composite ? null : (type?.description ?? null),
      fields: [
        ['address', hit.address],
        ['module', node.module],
        ['category', category ?? '—'],
        ['composite', node.composite ? `yes, ${inside} instance${inside === 1 ? '' : 's'} inside` : 'no'],
        ['level', levelText(graph, hit.level)],
      ],
      pins: [...node.ins, ...node.outs].map((p) => pinRow(entry, node, p)),
      // A primitive has no inside: its sidebar leaves Enter and Open out
      // (the context menu keeps them, disabled, with the reason).
      actions: actionsOf(hit, menu(hit), node.composite ? ['trace', 'enter', 'open', 'close', 'copy-address', 'delete'] : ['trace', 'copy-address', 'delete']),
    };
  }
  if (hit.kind === 'wire') {
    const wire = hit.wire;
    const source = entry.layout?.nodes.find((n) => n.id === wire.from.node)?.outs.find((p) => p.name === wire.from.port);
    const converters = wire.converters ?? [];
    const endOf = (side) => {
      const end = wire[side];
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
      actions: actionsOf(hit, menu(hit), ['trace', 'scale', 'copy-address', 'delete']),
    };
  }
  // A pin.
  const port = hit.port;
  const declared = port.parameter ? (hit.node.parameters ?? []).find((p) => p.name === port.name) : null;
  // A composite's frame pin (ADR 0030 §1): the composite instance's port, seen from inside.
  const frame = hit.node.kind !== 'instance';
  const fields = [
    ['address', hit.address],
    ['direction', frame ? (hit.node.kind === 'input' ? 'exported input (boundary)' : 'exported output (boundary)') : port.parameter ? 'parameter (in)' : port.direction === 'out' ? 'output' : 'input'],
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
    ...(frame ? { subtitle: `boundary pin of ${hit.path}: ${hit.node.kind === 'input' ? 'what reaches its port outside comes in here' : 'what reaches it here leaves by its port'}` } : {}),
    fields,
    connections,
    meter: hit.node.kind === 'instance' && port.direction === 'out' && port.shape === 'signal',
    actions: actionsOf(hit, menu(hit), ['trace', 'copy-address', 'disconnect']),
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

/** The trace's direction toggle: `[{id, label, title}]`. */
export const TRACE_DIRECTIONS = [
  { id: 'up', label: '← Up', title: 'Where the signal comes from (upstream)' },
  { id: 'down', label: 'Down →', title: 'Where the signal goes (downstream)' },
  { id: 'both', label: 'Both', title: 'Upstream and downstream' },
];

function plural(n, one, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`;
}

/** What an endpoint is, in a few words: its module, a library composite's opacity, an open end, the converters on the way. */
function endNote(end) {
  const what = { primitive: end.module ?? 'primitive', library: `${end.module}: library composite, inside not in the graph`, unconnected: `open end of ${end.module}`, unknown: 'not in the graph' }[end.kind] ?? end.kind;
  return end.via.length ? `${what}; via converter ${end.via.join(', ')}` : what;
}

/**
 * The sidebar's Trace section for `result` (`trace()`'s, ui/lib/schematic-trace.js)
 * over `graph`: `{title, subtitle, direction, directions: [{id, label,
 * title, on}], sides: [{name, title, count, empty, endpoints: [{label,
 * note, kind, reveal}]}], levels: [{label, count, reveal, children:
 * [{label, note, reveal, children: [{label, role, reveal}]}]}], notes}`;
 * null without a trace. A `reveal` names what a click shows: `{kind:
 * 'level', path}`, `{kind: 'instance', address}` or `{kind: 'pin',
 * address}` (the control opens the composites on its way, centres and
 * selects it).
 */
export function traceSection(result, graph) {
  if (!result) return null;
  const pins = result.pins.size;
  let wires = 0;
  for (const set of result.wires.values()) wires += set.size;
  const sides = [];
  if (result.up) sides.push({ name: 'up', title: 'Comes from', ...sideOf(result.up, 'nothing upstream') });
  if (result.down) sides.push({ name: 'down', title: 'Goes to', ...sideOf(result.down, 'nothing downstream') });
  const tree = traceTree(graph, result);
  const levels = tree.map((level) => ({
    label: level.label,
    count: `${plural(level.wires, 'wire')}, ${plural(level.pins, 'pin')}`,
    reveal: { kind: 'level', path: level.path },
    children: level.children.map((inst) => ({
      label: inst.name,
      note: inst.opaque ? `${inst.module} (opaque)` : inst.module,
      reveal: { kind: 'instance', address: inst.address },
      children: inst.children.map((pin) => ({ label: pin.port, role: pin.role, direction: pin.direction, reveal: { kind: 'pin', address: pin.address } })),
    })),
  }));
  const notes = [];
  if (result.opaque.length) notes.push(`${plural(result.opaque.length, 'end')} at a library composite: its inside is not in the graph, so the trace stops at its pins`);
  if (result.cycles > 0) notes.push(`${plural(result.cycles, 'connection')} ${result.cycles === 1 ? 'leads' : 'lead'} back to a pin already traced (a loop): followed once`);
  return {
    title: result.label,
    subtitle: `${plural(result.levels.length, 'level')}, ${plural(wires, 'wire')}, ${plural(pins, 'pin')}`,
    direction: result.direction,
    directions: TRACE_DIRECTIONS.map((d) => ({ ...d, on: d.id === result.direction })),
    sides,
    levels,
    notes,
  };
}

function sideOf(side, empty) {
  return {
    count: side.endpoints.length,
    empty,
    endpoints: side.endpoints.map((end) => ({ label: end.address, note: endNote(end), kind: end.kind, reveal: { kind: 'pin', address: end.address } })),
  };
}

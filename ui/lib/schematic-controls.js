// A panel's controls in the schematic (the owner's, 2026-10-29: "a fader,
// a button and so on of the panel looks in the schematic like a mere
// connection point. Make it, as in NI Reaktor, a small square element of
// its own, with a property table … keep that it has an input and an
// output, as an automated fader … there will be UI elements with several
// inputs and outputs"). Each control of the project's panel file
// (`ui/main.html`, read with `panel.read`) that binds an address of a
// top-level instance is a node of its own of the top level, `kind:
// 'control'`: a compact box beside the instance it binds (its host), with
// a pin per bound address on each side it uses:
// - a control that writes (a fader, a knob, a toggle, a button, an XY pad,
//   a value field on a parameter) has an output per address, joined to the
//   bound parameter or input by a binding wire, and an input per address:
//   the wires the project connects to that parameter or input are drawn
//   into it, an automated fader's input (the value the parameter takes is
//   the control's and the wires' together, `semantics.md`);
// - a control that reads (a meter, a scope, a canvas, a value readout of a
//   port) has an input per address: from the bound output, or for a bound
//   input the same sources that reach it.
// A control has no position of its own: it is placed beside its host,
// writers left of it and readers of outputs right of it, each as near its
// bound pin's height as the column allows, and moves with the host, whose
// position `ui/schematic.toml` keeps (a control's own would be a new key of
// that file, or of the panel: not written here). Nested addresses
// (`/drums/kick.decay`) are shown beside their top-level instance with an
// unwired pin. No DOM.

import { formatValue } from './format.js';
import { isDiscrete, valueLabel } from './value-set.js';

/** Geometry, in the layout's units (ui/lib/schematic-layout.js). */
export const CONTROL_W = 56;
/** A control with several pins on a side is this wide, its pins named inside it. */
export const CONTROL_WIDE = 96;
export const CONTROL_MIN_H = 48;
/** Its label's band at the top, before the pins' first row. */
export const CONTROL_HEAD = 14;
export const CONTROL_ROW = 16;
/** A via (ADR 0047 §5) is this wide, its pins this far apart. */
export const VIA_W = 16;
export const VIA_ROW = 8;
/** Between a control column and its host. */
export const CONTROL_GAP = 28;
/** Between two controls of a column. */
export const CONTROL_STACK = 6;

/** The id prefix of a control node: never an instance name (`[a-z][a-z0-9_]*`). */
export const CONTROL_ID = '@control:';

/** The control types that write the addresses they bind. */
export const WRITERS = new Set(['fader', 'knob', 'toggle', 'button', 'xy', 'keyboard', 'entry']);
/** The control types that only read them. */
export const READERS = new Set(['meter', 'scope', 'canvas']);
/** Elements with a `bind` that are not controls of an address: a slot's card (its box is the instance), the pattern editors. */
export const NOT_CONTROLS = new Set(['panel', 'slot', 'piano-roll', 'step-grid']);

/** The elements `panel.check` counts as a panel's controls (ui/lib/panel-whitelist.json's `controls`), in document order. */
export const CHECKED = new Set(['fader', 'knob', 'xy', 'toggle', 'value', 'button', 'meter', 'scope', 'canvas', 'piano-roll', 'step-grid', 'slot']);

/** The control types' names, as the sidebar and a box's tooltip say them. */
export const TYPE_NAMES = Object.freeze({
  fader: 'Fader',
  knob: 'Knob',
  toggle: 'Toggle',
  button: 'Button',
  xy: 'XY pad',
  keyboard: 'Keyboard',
  entry: 'Entry field',
  value: 'Value',
  meter: 'Meter',
  scope: 'Scope',
  canvas: 'Canvas',
});

const REFERENCES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };

function decode(text) {
  return String(text).replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-z]+);/g, (whole, body) => {
    if (body in REFERENCES) return REFERENCES[body];
    const code = body.startsWith('#x') ? parseInt(body.slice(2), 16) : body.startsWith('#') ? Number(body.slice(1)) : NaN;
    return Number.isFinite(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
  });
}

/** `text` with its comments blanked (same length, so offsets hold). */
function uncommented(text) {
  return text.replace(/<!--[\s\S]*?(?:-->|$)/g, (c) => ' '.repeat(c.length));
}

/** A start tag's attributes, in order, as `{name: value}` (a bare attribute is `""`). */
export function attributesOf(source) {
  const attributes = {};
  const re = /([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  for (const m of String(source ?? '').matchAll(re)) {
    const name = m[1].toLowerCase();
    if (name in attributes) continue;
    attributes[name] = decode(m[2] ?? m[3] ?? m[4] ?? '');
  }
  return attributes;
}

/**
 * The controls of panel text `html` (a panel file, or its `<signal-panel>`'s
 * content), in the file's order: `[{index, tag, type, attributes, binds,
 * label, occurrence}]`. Only elements `signal-<type>` with a non-empty
 * `bind` that are controls of addresses (`NOT_CONTROLS` aside); comments
 * are skipped. `occurrence` counts the earlier ones of the same tag and
 * `bind`: what finds the control again in the panel editor's tree.
 */
export function panelControls(html) {
  const text = uncommented(String(html ?? ''));
  const controls = [];
  const seen = new Map();
  let checked = 0;
  for (const m of text.matchAll(/<signal-([a-z][a-z0-9-]*)((?:\s(?:[^>"']|"[^"]*"|'[^']*')*)?)\/?>/gi)) {
    const type = m[1].toLowerCase();
    // Its index as `panel.check` counts the panel's controls (every whitelisted one, bound or not): what `slot.group`'s `controls` names.
    const check = CHECKED.has(type) ? checked++ : null;
    if (NOT_CONTROLS.has(type)) continue;
    const attributes = attributesOf(m[2]);
    const binds = bindList(attributes.bind);
    if (!binds.length) continue;
    const tag = `signal-${type}`;
    const key = `${tag}\n${attributes.bind}`;
    const occurrence = seen.get(key) ?? 0;
    seen.set(key, occurrence + 1);
    controls.push({ index: controls.length, tag, type, attributes, binds, label: attributes.label || '', occurrence, check });
  }
  return controls;
}

/** A `bind` attribute's addresses, as the controls read it (`bindAddresses`, ui/controls/base.js). */
export function bindList(bind) {
  return String(bind ?? '')
    .split(/[\s,]+/)
    .filter(Boolean);
}

/** What a control says it is: its label, else its first address. */
export function controlName(control) {
  return control.label || control.binds[0] || control.tag;
}

/**
 * `address` (`/synth.glide`, `/drums/kick.decay`) as `{instance, port,
 * top}` (`top` the top-level instance it is in), or null for an address
 * that names no port (`/deck_a`).
 */
export function splitBind(address) {
  const plain = String(address ?? '').replace(/^\//, '');
  const dot = plain.lastIndexOf('.');
  if (dot <= 0 || dot === plain.length - 1) return null;
  const instance = plain.slice(0, dot);
  return { instance, port: plain.slice(dot + 1), top: instance.split('/')[0], nested: instance.includes('/') };
}

/** Whether `type`'s control writes a target of `role` (`parameter`, `in`, `out`). */
export function writes(type, role) {
  if (role === 'out') return false;
  if (READERS.has(type)) return false;
  if (WRITERS.has(type)) return true;
  // A value field edits a parameter; on a port it is a readout. A type not known here: as a value.
  return role === 'parameter';
}

/**
 * The control nodes of the top level: `controls` (`panelControls`') on the
 * nodes `byId` (the layout's, before placement: an instance's `ins`,
 * `outs` and `parameter` pins) and `pinY(node, name, out)` (a pin's height
 * in its box). Each `{id, kind: 'control', name, type, tag, control, host,
 * side, w, h, dx, dy, ins, outs, binds}`: `dx`, `dy` its corner from its
 * host's; a pin `{name, direction, shape, kind, rate, range, address,
 * port, instance, role, writes, label}`. A control none of whose
 * addresses has its top-level instance here is left out.
 */
export function controlNodes(controls, byId, pinY) {
  const nodes = [];
  for (const control of controls ?? []) {
    const ins = [];
    const outs = [];
    let host = null;
    let reads = false;
    const binds = [];
    for (const address of control.binds) {
      const at = splitBind(address);
      if (!at) continue;
      const box = byId.get(at.top);
      if (!box || box.kind !== 'instance') continue;
      let target = null;
      let role = null;
      if (!at.nested) {
        target = box.ins.find((p) => p.name === at.port && p.parameter) ?? null;
        role = target ? 'parameter' : null;
        if (!target) {
          target = box.ins.find((p) => p.name === at.port && !p.parameter) ?? null;
          role = target ? 'in' : null;
        }
        if (!target) {
          target = box.outs.find((p) => p.name === at.port) ?? null;
          role = target ? 'out' : null;
        }
      }
      const writer = writes(control.type, role ?? 'parameter');
      if (!writer && (role === 'out' || at.nested) && host === null) reads = true;
      host ??= at.top;
      const plain = address.replace(/^\//, '');
      const pin = {
        name: plain,
        label: at.port,
        address: `/${plain}`,
        instance: at.instance,
        port: at.port,
        role,
        writes: writer,
        shape: target?.shape ?? 'signal',
        kind: target?.kind ?? null,
        rate: target?.rate ?? null,
        range: target?.range ?? null,
      };
      binds.push(pin);
      ins.push({ ...pin, direction: 'in' });
      if (writer) outs.push({ ...pin, direction: 'out' });
    }
    if (!host) continue;
    const rows = Math.max(1, ins.length, outs.length);
    const w = rows > 1 ? CONTROL_WIDE : CONTROL_W;
    const h = Math.max(CONTROL_MIN_H, CONTROL_HEAD + CONTROL_ROW * rows + 14);
    nodes.push({
      id: `${CONTROL_ID}${control.index}`,
      kind: 'control',
      name: controlName(control),
      module: '',
      type: control.type,
      tag: control.tag,
      control,
      host,
      // Writers sit left of their host, readers of an output (or of a port inside it) right of it.
      side: reads ? 'right' : 'left',
      composite: false,
      w,
      h,
      dx: 0,
      dy: 0,
      ins,
      outs,
      binds,
    });
  }
  // Each host's columns, in the panel's order: a control as near its first pin's height as the one above it allows.
  const columns = new Map();
  for (const node of nodes) {
    const key = `${node.host}\n${node.side}`;
    if (!columns.has(key)) columns.set(key, []);
    columns.get(key).push(node);
  }
  for (const column of columns.values()) {
    const host = byId.get(column[0].host);
    // Many controls on one side (the studio's synth, 27 knobs): a compact grid beside the box, not one tall column (2026-10-30).
    if (column.length >= GRID_FROM) {
      gridPlace(column, host);
      continue;
    }
    let bottom = -Infinity;
    for (const node of column) {
      const first = node.binds.find((b) => b.role) ?? null;
      const y = first && first.instance === host.name ? pinY(host, first.port, first.role === 'out') : CONTROL_HEAD + CONTROL_ROW / 2;
      node.dy = Math.max(0, Math.round(y - node.h / 2), bottom === -Infinity ? 0 : bottom + CONTROL_STACK);
      bottom = node.dy + node.h;
      node.dx = node.side === 'left' ? -(CONTROL_GAP + node.w) : host.w + CONTROL_GAP;
    }
  }
  return nodes;
}

/** From this many controls on one side of a box, they stand in a grid (`gridColumns`). */
export const GRID_FROM = 6;
/** A grid has at most this many columns. */
export const GRID_MAX_COLUMNS = 6;

/**
 * How many columns a grid of `n` controls of cells `cell` high takes
 * beside a box `height` high: as few as keep the grid about as tall as the
 * box (at least two: never one tall column), at most
 * GRID_MAX_COLUMNS.
 */
export function gridColumns(n, cell, height) {
  if (n <= 1) return 1;
  const byHeight = Math.ceil((n * (cell + CONTROL_STACK)) / Math.max(height, cell));
  return Math.min(GRID_MAX_COLUMNS, n, Math.max(2, byHeight));
}

/**
 * `column` (one side's controls of `host`, in the panel's order) placed as
 * a grid: uniform cells, row by row in reading order, the grid's top at
 * the box's, its column nearest the box CONTROL_GAP from it (left of it
 * for writers, right for readers).
 */
function gridPlace(column, host) {
  const w = Math.max(...column.map((n) => n.w));
  const h = Math.max(...column.map((n) => n.h));
  const columns = gridColumns(column.length, h, host.h);
  const width = columns * w + (columns - 1) * CONTROL_STACK;
  const left = column[0].side === 'left' ? -(CONTROL_GAP + width) : host.w + CONTROL_GAP;
  column.forEach((node, i) => {
    const c = i % columns;
    const r = Math.floor(i / columns);
    node.grid = { row: r, column: c, columns };
    node.dx = left + c * (w + CONTROL_STACK) + Math.round((w - node.w) / 2);
    node.dy = r * (h + CONTROL_STACK);
  });
}

/**
 * What each host's controls take around its box: host id → `{left, right,
 * top, bottom}`, how far past its box's edges (never negative).
 */
export function controlPads(nodes, byId) {
  const pads = new Map();
  for (const node of nodes) {
    const host = byId.get(node.host);
    if (!host) continue;
    const pad = pads.get(node.host) ?? { left: 0, right: 0, top: 0, bottom: 0 };
    if (node.side === 'left') pad.left = Math.max(pad.left, -node.dx);
    else pad.right = Math.max(pad.right, node.dx + node.w - host.w);
    pad.top = Math.max(pad.top, -node.dy);
    pad.bottom = Math.max(pad.bottom, node.dy + node.h - host.h);
    pads.set(node.host, pad);
  }
  return pads;
}

/** Pin `name`'s point on control `node` (placed): its side's rows centred in the box below its label's band. */
export function controlAnchor(node, name, out) {
  const list = out ? node.outs : node.ins;
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  // A via (ADR 0047 §5): its rows VIA_ROW apart about its middle.
  if (node.via) return { x: out ? node.x + node.w : node.x, y: node.y + node.h / 2 + (i - (list.length - 1) / 2) * VIA_ROW };
  const band = node.h - CONTROL_HEAD;
  const y = node.y + CONTROL_HEAD + band / 2 + (i - (list.length - 1) / 2) * CONTROL_ROW;
  return { x: out ? node.x + node.w : node.x, y };
}

/**
 * The level's edges with its controls: `edges` (the layout's, `{from, to,
 * kind, …}`) where the wires into an address a control writes go into the
 * control's input instead (each keeps its endpoint as written in `target`;
 * the first control of an address takes them), then the binding wires
 * (`kind: 'binding'`): a writer's output to its address, an output to the
 * reader bound to it, and to a reader of an input the sources that reach
 * it. Returns the new list; `edges` is not changed.
 */
export function controlEdges(edges, controls) {
  const list = edges.map((e) => ({ ...e }));
  const claimed = new Set();
  const added = [];
  const binding = (from, to, pin, control) => ({
    from,
    to,
    kind: 'binding',
    shape: pin.shape,
    converters: [],
    scale: null,
    label: `${control.name} ⇢ ${pin.address}`,
    written: { from: `${control.id}.${from.node === control.id ? from.port : to.port}`, to: pin.address, binding: true },
  });
  const into = (pin) => (e) => e.kind === 'connection' && (e.target ?? e.to).node === pin.instance && (e.target ?? e.to).port === pin.port;
  for (const control of controls) {
    for (const pin of control.binds) {
      if (!pin.role) continue;
      const here = { node: control.id, port: pin.name };
      if (pin.writes) {
        if (!claimed.has(pin.address)) {
          claimed.add(pin.address);
          for (const e of list) {
            if (!into(pin)(e)) continue;
            e.target = e.to;
            e.to = here;
          }
        }
        added.push(binding(here, { node: pin.instance, port: pin.port }, pin, control));
      } else if (pin.role === 'out') {
        added.push(binding({ node: pin.instance, port: pin.port }, here, pin, control));
      } else {
        for (const e of list.filter(into(pin))) added.push(binding(e.from, here, pin, control));
      }
    }
  }
  return [...list, ...added];
}

/** The hit of control `node` drawn at level `entry` (`{path, level}`): its first bound address is its `address`. */
export function controlHit(entry, node) {
  return { kind: 'control', path: entry.path, level: entry.level, node, address: node.binds[0]?.address ?? '', addresses: node.binds.map((b) => b.address) };
}

/**
 * A control's properties as the sidebar lists them: `[[label, text]]`, its
 * type, label, bound addresses, then every other attribute it has, as
 * written.
 */
export function controlFields(node) {
  const attributes = node.control?.attributes ?? {};
  const fields = [
    ['type', `${TYPE_NAMES[node.type] ?? node.type} (<${node.tag}>)`],
    ['label', attributes.label || '—'],
    ['binds', node.binds.map((b) => b.address).join(', ')],
    ['host', `/${node.host}`],
  ];
  for (const [name, value] of Object.entries(attributes)) {
    if (name === 'label' || name === 'bind') continue;
    fields.push([name, value === '' ? 'yes' : value]);
  }
  return fields;
}

/** The attributes the sidebar edits in place, of those the panel editor writes for `tag` (its whitelist's list). */
export const EDITED = Object.freeze(['label', 'min', 'max', 'default', 'unit']);

/** The attributes of `tag` the sidebar may edit, by `whitelist` (ui/lib/panel-whitelist.json). */
export function editableAttributes(tag, whitelist) {
  const allowed = whitelist?.controls?.[tag] ?? [];
  return EDITED.filter((name) => allowed.includes(name));
}

/**
 * The control of `tree` (the panel editor's, ui/lib/panel-tree.js) that is
 * `control` (`panelControls`'): the `occurrence`-th of its tag and `bind`,
 * in the tree's order; null when there is none.
 */
export function findInTree(tree, control) {
  let n = 0;
  const walk = (node) => {
    for (const child of node.children ?? []) {
      if (child.kind === 'control') {
        if (child.tag === control.tag && (child.attributes?.bind ?? '') === (control.attributes.bind ?? '')) {
          if (n === control.occurrence) return child;
          n++;
        }
      } else {
        const found = walk(child);
        if (found) return found;
      }
    }
    return null;
  };
  return walk(tree);
}

/**
 * A typed attribute value checked before it is written: `{value}` (null
 * removes the attribute) or `{error}`. `min`, `max` and `default` are
 * numbers; the label and unit any text, empty removing them.
 */
export function attributeValue(name, text) {
  const value = String(text ?? '').trim();
  if (!value) return { value: null };
  if (['min', 'max', 'default'].includes(name)) {
    const n = Number(value);
    if (!Number.isFinite(n)) return { error: `${name} is a number` };
    return { value: String(n) };
  }
  return { value };
}

/**
 * The value control `node` shows now: of its first address that is a
 * top-level parameter (`description.controls`), the latest stream frame's
 * base (`values`: address → `{base}`), else `describe`'s. `{address, value,
 * row}` (`row` the control's entry), or null for a control of no parameter
 * (a meter, a scope, a button of an input).
 */
export function controlValue(node, description, values = null) {
  for (const pin of node.binds ?? []) {
    if (pin.role !== 'parameter') continue;
    const row = (description?.controls ?? []).find((c) => c.address === pin.address);
    if (!row) continue;
    const streamed = values?.get(pin.address)?.base;
    return { address: pin.address, value: Number.isFinite(streamed) ? streamed : row.base, row };
  }
  return null;
}

/** Where `value` lies in the control's range (its `min` and `max` attributes, else its parameter's), 0 … 1; NaN when unknown. */
export function controlFraction(node, shown) {
  if (!shown || !Number.isFinite(shown.value)) return NaN;
  const attributes = node.control?.attributes ?? {};
  const min = Number.isFinite(Number.parseFloat(attributes.min)) ? Number.parseFloat(attributes.min) : shown.row?.min;
  const max = Number.isFinite(Number.parseFloat(attributes.max)) ? Number.parseFloat(attributes.max) : shown.row?.max;
  if (!Number.isFinite(min) || !Number.isFinite(max) || max === min) return NaN;
  return Math.min(1, Math.max(0, (shown.value - min) / (max - min)));
}

/**
 * The glyph of a control of `type` in `area` (`{x, y, w, h}`, layout units)
 * at `fraction` of its range (NaN: at its middle, drawn as unknown): a list
 * of `{tag, attributes}` (SVG elements in layout units: `rect` `{x, y, w,
 * h, rx?}`, `circle` `{cx, cy, r}`, `line` `{x1, y1, x2, y2}`, `path`
 * `{points}`), each with its `class`. A fader is a slot with its cap, a knob
 * a dial with its pointer, a toggle a switch, a button a key, an XY pad a
 * square with its point, a meter a bar, a scope a trace.
 */
export function glyphParts(type, area, fraction = NaN) {
  const known = Number.isFinite(fraction);
  const f = known ? fraction : 0.5;
  const g = Math.max(4, Math.min(area.w, area.h));
  const cx = area.x + area.w / 2;
  const cy = area.y + area.h / 2;
  const left = cx - g / 2;
  const top = cy - g / 2;
  const mark = known ? 'glyph-mark' : 'glyph-mark unknown';
  switch (type) {
    case 'fader': {
      const capY = top + (1 - f) * (g - 4);
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: cx - 1.5, y: top, w: 3, h: g, rx: 1.5 } },
        { tag: 'rect', attributes: { class: mark, x: cx - 6, y: capY, w: 12, h: 4, rx: 1 } },
      ];
    }
    case 'knob': {
      const angle = ((-135 + 270 * f) * Math.PI) / 180;
      const r = g / 2;
      return [
        { tag: 'circle', attributes: { class: 'glyph-track', cx, cy, r } },
        { tag: 'line', attributes: { class: mark, x1: cx, y1: cy, x2: cx + Math.sin(angle) * r * 0.85, y2: cy - Math.cos(angle) * r * 0.85 } },
      ];
    }
    case 'toggle': {
      const w = g * 1.4;
      const h = g * 0.7;
      const on = known && f >= 0.5;
      return [
        { tag: 'rect', attributes: { class: on ? 'glyph-track on' : 'glyph-track', x: cx - w / 2, y: cy - h / 2, w, h, rx: h / 2 } },
        { tag: 'circle', attributes: { class: mark, cx: on ? cx + w / 2 - h / 2 : cx - w / 2 + h / 2, cy, r: h / 2 - 1.5 } },
      ];
    }
    case 'button':
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: left, y: cy - g * 0.35, w: g, h: g * 0.7, rx: 3 } },
        { tag: 'circle', attributes: { class: 'glyph-mark', cx, cy, r: g * 0.18 } },
      ];
    case 'xy':
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: left, y: top, w: g, h: g, rx: 2 } },
        { tag: 'line', attributes: { class: 'glyph-thin', x1: cx, y1: top, x2: cx, y2: top + g } },
        { tag: 'line', attributes: { class: 'glyph-thin', x1: left, y1: cy, x2: left + g, y2: cy } },
        { tag: 'circle', attributes: { class: mark, cx: left + f * g, cy: top + (1 - f) * g, r: 2.5 } },
      ];
    case 'meter':
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: cx - 3, y: top, w: 6, h: g, rx: 1 } },
        { tag: 'rect', attributes: { class: 'glyph-mark', x: cx - 3, y: top + g * 0.4, w: 6, h: g * 0.6, rx: 1 } },
      ];
    case 'scope':
    case 'canvas': {
      const points = [];
      for (let i = 0; i <= 12; i++) points.push({ x: left + (g * i) / 12, y: cy - Math.sin((i / 12) * Math.PI * 2) * g * 0.3 });
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: left, y: top, w: g, h: g, rx: 2 } },
        { tag: 'path', attributes: { class: 'glyph-line', points } },
      ];
    }
    case 'keyboard': {
      const parts = [{ tag: 'rect', attributes: { class: 'glyph-track', x: left, y: top + g * 0.15, w: g, h: g * 0.7, rx: 1 } }];
      for (let i = 1; i < 4; i++) parts.push({ tag: 'line', attributes: { class: 'glyph-thin', x1: left + (g * i) / 4, y1: top + g * 0.15, x2: left + (g * i) / 4, y2: top + g * 0.85 } });
      return parts;
    }
    default:
      // A value field, an entry, a type not known here: a field.
      return [
        { tag: 'rect', attributes: { class: 'glyph-track', x: left, y: cy - g * 0.3, w: g, h: g * 0.6, rx: 2 } },
        { tag: 'line', attributes: { class: 'glyph-thin', x1: left + 3, y1: cy, x2: left + g * 0.6, y2: cy } },
      ];
  }
}

/** The text of `shown` (`controlValue`'s): its value in its unit, a discrete one by name; empty when there is none. */
export function controlValueText(shown) {
  if (!shown || !Number.isFinite(shown.value)) return '';
  const row = shown.row ?? {};
  return isDiscrete(row) ? valueLabel(row.values, shown.value, row.snap) : formatValue(shown.value, row.unit ?? null);
}

/** A label cut to `chars` characters, an ellipsis at its end when cut. */
export function fitText(text, chars) {
  const t = String(text ?? '');
  return t.length <= chars ? t : `${t.slice(0, Math.max(1, chars - 1))}…`;
}

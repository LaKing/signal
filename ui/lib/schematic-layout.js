// The schematic's layout (M2.9-1, ADR 0026 §3): a layered drawing of one
// graph level (the top level, or a composite's inside, as `graph` gives
// it), left to right by signal flow. Feedback connections (back edges of a
// depth-first search in source order) are reversed for layering and drawn
// back around below; layers come from the longest path, and each layer is
// ordered by a few barycentre sweeps. A node with a stored position keeps it,
// and the others move down until they overlap nothing. Deterministic: the
// result depends on the arrays' order (source order) and never on an
// object's key order. Wires are smooth curves (ui/lib/schematic-wire.js);
// `movedWires` gives them again for a box being dragged. No DOM.

import { insideOf, slotLabel, slotOf } from './slot.js';
import { wireGeometry } from './schematic-wire.js';
import { exportFacts, orderedPins, portGroups } from './schematic-groups.js';
import { controlAnchor, controlEdges, controlNodes, controlPads } from './schematic-controls.js';
import { frameFace, placeFace } from './schematic-face.js';

/**
 * Where a slot box's "+" pin of `side` (`in`, `out`) is: the row after its
 * last pin of that side (ADR 0037 §7).
 */
export function addPinAt(node, side) {
  const out = side === 'out';
  const i = out ? node.outs.length : node.ins.length;
  return { x: out ? node.x + node.w : node.x, y: node.y + HEADER + ROW * (i + 0.5) };
}

/** Geometry, in the layout's own units. */
export const HEADER = 34;
export const ROW = 16;
export const PAD = 8;
export const CHAR = 6.5;
export const MIN_WIDTH = 96;
export const GAP_X = 72;
export const GAP_Y = 24;
const SWEEPS = 4;
/** Feedback lanes run this far apart, the first GAP_Y / 2 below the lowest box. */
export const LANE = 6;

/** Where an export pseudo-node's pins go: a composite's frame. */
export const INPUTS = '@inputs';
export const OUTPUTS = '@outputs';

/**
 * The key of the level a box opens into: a slot's inside (ADR 0037,
 * editable insides: its own composite, or `/slot`, its read-only inside in
 * `graph.insides`), else its module (a composite's name).
 */
export function insideKey(node) {
  return node?.level ?? node?.module;
}

/** `signal`, `events` or `vector`, from a port's `shape`. */
export function shapeName(shape) {
  if (shape && typeof shape === 'object' && 'vector' in shape) return 'vector';
  return shape === 'events' ? 'events' : 'signal';
}

/** An endpoint as written, without the top level's leading `/`. */
function endpoint(text) {
  const plain = String(text ?? '').replace(/^\//, '');
  const dot = plain.lastIndexOf('.');
  return dot < 0 ? { node: plain, port: '' } : { node: plain.slice(0, dot), port: plain.slice(dot + 1) };
}

function pin(port, direction) {
  return { name: port.name, shape: shapeName(port.shape), kind: port.kind ?? null, rate: port.rate ?? null, range: port.range ?? null, direction };
}

/**
 * A frame pin of a slot's inside (ADR 0037, editable insides): its role
 * (`input`, `output`, `parameter`), the export it is wired by (`export`,
 * when it differs from the slot's port name) and whether it is wired inside
 * at all (`unmatched`), for the export ops that wire it.
 */
function slotFramePin(port, direction, role) {
  return { ...pin(port, direction), role, export: port.export ?? null, unmatched: Boolean(port.unmatched) };
}

/**
 * The nodes of a level, before placement. `factsOf(module)` (M2.13-2) gives
 * a composite's export facts (ui/lib/schematic-groups.js `exportFacts`):
 * its box's pins are then ordered in groups, kept as `groups`.
 */
function nodesOf(level, factsOf = null) {
  const nodes = [];
  // A composite's own frame orders its pins as its box outside does (M2.13-2): by group, in export order.
  const own = factsOf ? exportFacts(level) : null;
  const framePins = (pins) => (own ? orderedPins(pins, own) : pins);
  // ADR 0037, editable insides: a slot's inside, its frame the slot's interface, carries each pin's role.
  const slotInside = typeof level.slot === 'string';
  const inputs = [...(level.inputs ?? []).map((p) => ({ ...p, role: 'input' })), ...(level.parameters ?? []).map((p) => ({ ...p, shape: 'signal', role: 'parameter' }))];
  const framePin = (p, direction) => (slotInside ? slotFramePin(p, direction, p.role ?? 'output') : pin(p, direction));
  if (inputs.length) {
    nodes.push({ id: INPUTS, name: 'inputs', module: '', kind: 'input', composite: false, ins: [], outs: framePins(inputs.map((p) => framePin(p, 'out'))) });
  }
  for (const instance of level.instances ?? []) {
    const ports = instance.ports ?? [];
    // ADR 0037 §7: a slot's box shows its interface's pins, loaded or not, and what it holds under its name.
    const slot = slotOf(instance);
    // ADR 0037, editable insides: a slot with an inside opens like a composite, into `level`.
    const inside = insideOf(slot)?.level ?? null;
    nodes.push({
      id: instance.name,
      name: instance.name,
      module: instance.module,
      kind: 'instance',
      composite: Boolean(instance.composite) || Boolean(inside),
      ...(inside ? { level: inside } : {}),
      slot,
      // What the box says under its name: the module, or a slot's content ("empty: lib.voice").
      subtitle: slot ? slotLabel(slot).subtitle : instance.module,
      // As `graph` gives them, for the inspector.
      parameters: instance.parameters ?? [],
      ins: [
        ...ports.filter((p) => p.direction === 'in').map((p) => pin(p, 'in')),
        ...(instance.parameters ?? []).map((p) => ({ ...pin({ ...p, shape: 'signal' }, 'in'), parameter: true })),
      ],
      outs: ports.filter((p) => p.direction === 'out').map((p) => pin(p, 'out')),
    });
    const facts = (instance.composite || inside) && factsOf ? factsOf(inside ?? instance.module) : null;
    if (facts) {
      const node = nodes.at(-1);
      node.ins = orderedPins(node.ins, facts);
      node.outs = orderedPins(node.outs, facts);
      node.groups = { ins: portGroups(node.ins, facts), outs: portGroups(node.outs, facts) };
    }
  }
  if ((level.outputs ?? []).length) {
    nodes.push({ id: OUTPUTS, name: 'outputs', module: '', kind: 'output', composite: false, ins: framePins(level.outputs.map((p) => framePin({ ...p, role: 'output' }, 'in'))), outs: [] });
  }
  for (const node of nodes) {
    const longestIn = Math.max(0, ...node.ins.map((p) => p.name.length));
    const longestOut = Math.max(0, ...node.outs.map((p) => p.name.length));
    const under = (node.subtitle ?? node.module).length;
    node.w = Math.ceil(Math.max(MIN_WIDTH, CHAR * Math.max(node.name.length, node.module.length, under) + 2 * PAD, CHAR * (longestIn + longestOut) + 4 * PAD));
    // A slot's box keeps a row below its pins for the "+" pins that add a port (ADR 0037 §7), drawn in Edit patch.
    const extra = node.slot ? 1 : 0;
    node.h = HEADER + ROW * (Math.max(1, node.ins.length, node.outs.length) + extra) + PAD;
  }
  return nodes;
}

/** The level's edges: its connections, and a composite's exports to and from its frame. */
function edgesOf(level, byId) {
  const edges = [];
  // Only between pins that are drawn.
  const add = (from, to, wire) => {
    if (!byId.get(from.node)?.outs.some((p) => p.name === from.port)) return;
    if (!byId.get(to.node)?.ins.some((p) => p.name === to.port)) return;
    edges.push({ from, to, ...wire });
  };
  for (const c of level.connections ?? []) {
    const from = endpoint(c.from);
    const source = byId.get(from.node)?.outs.find((p) => p.name === from.port);
    add(from, endpoint(c.to), {
      kind: 'connection',
      shape: source?.shape ?? 'signal',
      converters: c.converters ?? [],
      scale: c.scale ?? null,
      label: `${c.from} → ${c.to}`,
      written: { from: c.from, to: c.to },
    });
  }
  // ADR 0037, editable insides: in a slot's inside an export wire says what an export op names (its role and export).
  const slotInside = typeof level.slot === 'string';
  const exportOf = (e, role) => (slotInside ? { role, export: e.export ?? e.name } : {});
  for (const [list, role] of [[level.inputs, 'input'], [level.parameters, 'parameter']]) {
    for (const e of list ?? []) {
      for (const target of e.to ?? []) {
        add({ node: INPUTS, port: e.name }, endpoint(target), { kind: 'export', shape: shapeName(e.shape ?? 'signal'), converters: [], scale: null, label: `${e.name} → ${target}`, written: { from: e.name, to: target, ...exportOf(e, role) } });
      }
    }
  }
  for (const e of level.outputs ?? []) {
    if (!e.from) continue;
    add(endpoint(e.from), { node: OUTPUTS, port: e.name }, { kind: 'export', shape: shapeName(e.shape), converters: [], scale: null, label: `${e.from} → ${e.name}`, written: { from: e.from, to: e.name, ...exportOf(e, 'output') } });
  }
  return edges;
}

/** Back edges of a depth-first search in node order, by edge index. */
function backEdges(nodes, edges) {
  const out = new Map(nodes.map((n) => [n.id, []]));
  edges.forEach((e, i) => out.get(e.from.node).push(i));
  const state = new Map();
  const back = new Set();
  for (const root of nodes) {
    if (state.has(root.id)) continue;
    const stack = [[root.id, 0]];
    state.set(root.id, 'open');
    while (stack.length) {
      const top = stack[stack.length - 1];
      const list = out.get(top[0]);
      if (top[1] >= list.length) {
        state.set(top[0], 'done');
        stack.pop();
        continue;
      }
      const i = list[top[1]++];
      const next = edges[i].to.node;
      if (state.get(next) === 'open') back.add(i);
      else if (!state.has(next)) {
        state.set(next, 'open');
        stack.push([next, 0]);
      }
    }
  }
  return back;
}

/** Longest-path layers over the edges with back edges reversed. */
function layers(nodes, edges, back) {
  const preds = new Map(nodes.map((n) => [n.id, new Set()]));
  edges.forEach((e, i) => {
    const [a, b] = back.has(i) ? [e.to.node, e.from.node] : [e.from.node, e.to.node];
    if (a !== b) preds.get(b).add(a);
  });
  const layer = new Map();
  const visit = (id, seen) => {
    if (layer.has(id)) return layer.get(id);
    if (seen.has(id)) return 0;
    seen.add(id);
    let l = 0;
    for (const p of preds.get(id)) l = Math.max(l, visit(p, seen) + 1);
    layer.set(id, l);
    return l;
  };
  for (const n of nodes) visit(n.id, new Set());
  // The frame's pins: inputs first, outputs last.
  const last = most(nodes.filter((n) => n.kind === 'instance').map((n) => layer.get(n.id)), 0) + 1;
  for (const n of nodes) {
    if (n.kind === 'input') layer.set(n.id, 0);
    if (n.kind === 'output') layer.set(n.id, last);
  }
  if (nodes.some((n) => n.kind === 'input')) {
    for (const n of nodes) if (n.kind === 'instance') layer.set(n.id, Math.max(1, layer.get(n.id)));
  }
  return layer;
}

/** The largest of `values` and `from` (Math.max without spreading them). */
function most(values, from) {
  let m = from;
  for (const v of values) if (v > m) m = v;
  return m;
}
/** The smallest of `values` and `from`. */
function least(values, from) {
  let m = from;
  for (const v of values) if (v < m) m = v;
  return m;
}

/** Each layer's nodes in barycentre order: a few sweeps down and up. */
function order(nodes, edges, layer) {
  const count = most(layer.values(), 0) + 1;
  const rows = Array.from({ length: count }, () => []);
  for (const n of nodes) rows[layer.get(n.id)].push(n.id);
  const index = new Map();
  const number = () => rows.forEach((row) => row.forEach((id, i) => index.set(id, i)));
  number();
  // Each node's neighbours in the layer above and below, in edge order,
  // gathered once (not per sweep: a large project has thousands of edges).
  const above = new Map(nodes.map((n) => [n.id, []]));
  const beneath = new Map(nodes.map((n) => [n.id, []]));
  for (const e of edges) {
    if (layer.get(e.from.node) === layer.get(e.to.node) - 1) {
      above.get(e.to.node)?.push(e.from.node);
      beneath.get(e.from.node)?.push(e.to.node);
    }
  }
  const neighbours = (id, below) => (below ? above : beneath).get(id) ?? [];
  const sweep = (r, below) => {
    const keyed = rows[r].map((id, i) => {
      const near = neighbours(id, below);
      const key = near.length ? near.reduce((s, o) => s + index.get(o), 0) / near.length : i;
      return { id, key, i };
    });
    keyed.sort((a, b) => a.key - b.key || a.i - b.i);
    rows[r] = keyed.map((k) => k.id);
    rows[r].forEach((id, i) => index.set(id, i));
  };
  for (let s = 0; s < SWEEPS; s++) {
    for (let r = 1; r < count; r++) sweep(r, true);
    for (let r = count - 2; r >= 0; r--) sweep(r, false);
  }
  return rows;
}

function overlaps(a, b) {
  return a.x < b.x + b.w + PAD && b.x < a.x + a.w + PAD && a.y < b.y + b.h + PAD && b.y < a.y + a.h + PAD;
}

const NO_PAD = Object.freeze({ left: 0, right: 0, top: 0, bottom: 0 });

/** A node's box with what its panel controls take around it (`pads`, `controlPads`'). */
function footprint(node, pads) {
  const p = pads.get(node.id) ?? NO_PAD;
  return { x: node.x - p.left, y: node.y - p.top, w: node.w + p.left + p.right, h: node.h + p.top + p.bottom };
}

/** A pin's height in its box, from the box's top. */
function pinOffset(node, name, out) {
  const list = out ? node.outs : node.ins;
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  return HEADER + ROW * (i + 0.5);
}

function anchor(node, name, out) {
  if (node.kind === 'control') return controlAnchor(node, name, out);
  const list = out ? node.outs : node.ins;
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  return { x: out ? node.x + node.w : node.x, y: node.y + HEADER + ROW * (i + 0.5) };
}

/** The y of feedback lane `lane` (1, 2, …) below boxes reaching down to `bottom`. */
export function laneY(bottom, lane) {
  return bottom + GAP_Y / 2 + lane * LANE;
}

/**
 * The layout of `level` with the stored `positions` (`{instance: {x, y}}`)
 * of its instances: `{nodes, wires, bounds}`. Nodes carry their box (`x`,
 * `y`, `w`, `h`), `layer`, `pinned` and their pins (`ins` left, `outs`
 * right, each with its `x`, `y`); wires their `curve` and its polyline
 * `points` (ui/lib/schematic-wire.js), `shape`, `converters`, `dashed` (a
 * converter changes the rate), `feedback` and, for a feedback wire, its
 * `lane` below `bottom` (the lowest box's bottom edge).
 */
export function layout(level, positions = {}, { factsOf = null, controls = null, faces = null, faceSize = null } = {}) {
  const nodes = nodesOf(level ?? {}, factsOf);
  const byId = new Map(nodes.map((n) => [n.id, n]));
  // A slot box with a part shows it, its face, under its title (ui/lib/schematic-face.js): the box grows to hold it.
  for (const node of nodes) {
    const face = node.kind === 'instance' ? faces?.boxes?.get(node.name) : null;
    if (face) placeFace(node, face, faceSize?.(face) ?? null);
  }
  const edges = edgesOf(level ?? {}, byId);
  const back = backEdges(nodes, edges);
  const layer = layers(nodes, edges, back);
  const rows = order(nodes, edges, layer);
  // A panel's controls (ui/lib/schematic-controls.js): placed beside their hosts, which keep room for them.
  // A slot's part's controls on its interface are its face's, not nodes (2026-10-30).
  const panel = controls?.length ? controlNodes(controls.filter((c) => !c.part), byId, pinOffset) : [];
  const pads = controlPads(panel, byId);
  let x = 0;
  for (const row of rows) {
    let y = 0;
    let left = 0;
    for (const id of row) left = Math.max(left, (pads.get(id) ?? NO_PAD).left);
    let width = 0;
    for (const id of row) {
      const node = byId.get(id);
      const pad = pads.get(id) ?? NO_PAD;
      node.layer = layer.get(id);
      node.x = x + left;
      node.y = y + pad.top;
      y += node.h + pad.top + pad.bottom + GAP_Y;
      width = Math.max(width, node.w + pad.right);
    }
    x += left + width + GAP_X;
  }
  // Stored positions first, then every other node, in layer order, moved
  // down past whatever it overlaps.
  const placed = [];
  for (const node of nodes) {
    const stored = node.kind === 'instance' ? positions?.[node.name] : undefined;
    node.pinned = Boolean(stored && Number.isFinite(stored.x) && Number.isFinite(stored.y));
    if (node.pinned) {
      node.x = stored.x;
      node.y = stored.y;
      placed.push(node);
    }
  }
  for (const row of rows) {
    for (const id of row) {
      const node = byId.get(id);
      if (node.pinned) continue;
      for (let moved = true; moved; ) {
        moved = false;
        for (const other of placed) {
          const a = footprint(node, pads);
          const b = footprint(other, pads);
          if (overlaps(a, b)) {
            node.y = b.y + b.h + GAP_Y + (node.y - a.y);
            moved = true;
          }
        }
      }
      placed.push(node);
    }
  }
  for (const control of panel) {
    const host = byId.get(control.host);
    control.x = host.x + control.dx;
    control.y = host.y + control.dy;
    control.layer = host.layer;
    control.pinned = host.pinned;
    nodes.push(control);
    byId.set(control.id, control);
  }
  for (const node of nodes) {
    node.ins = node.ins.map((p) => ({ ...p, ...anchor(node, p.name, false) }));
    node.outs = node.outs.map((p) => ({ ...p, ...anchor(node, p.name, true) }));
  }
  const bottom = levelBottom(nodes);
  let lane = 0;
  const all = panel.length ? controlEdges(edges, panel) : edges;
  const wires = all.map((e, i) => {
    const a = anchor(byId.get(e.from.node), e.from.port, true);
    const b = anchor(byId.get(e.to.node), e.to.port, false);
    const feedback = back.has(i) || b.x <= a.x;
    if (feedback) lane += 1;
    const under = feedback ? laneY(bottom, lane) : null;
    const { curve, points, box } = wireGeometry(a, b, { feedback, under });
    return {
      id: i,
      from: e.from,
      to: e.to,
      written: e.written,
      kind: e.kind,
      shape: e.shape,
      converters: e.converters,
      scale: e.scale,
      dashed: e.converters.length > 0,
      feedback,
      reversed: back.has(i),
      label: e.label,
      // Drawn into a panel control's input (ui/lib/schematic-controls.js): the endpoint as written.
      ...(e.target ? { target: e.target } : {}),
      lane: feedback ? lane : 0,
      under,
      curve,
      points,
      box,
    };
  });
  // A pin knows whether a wire of this level reaches it (drawn filled).
  // By side: a panel control's input and output have the same name.
  const ends = { outs: new Set(all.map((e) => `${e.from.node}\n${e.from.port}`)), ins: new Set(all.map((e) => `${e.to.node}\n${e.to.port}`)) };
  for (const node of nodes) {
    for (const side of ['ins', 'outs']) node[side] = node[side].map((p) => ({ ...p, connected: ends[side].has(`${node.id}\n${p.name}`) }));
  }
  // A wire's extent is its curve's box (a short one swings out past its pins).
  const xs = [...nodes.flatMap((n) => [n.x, n.x + n.w]), ...wires.flatMap((w) => [w.box.x0, w.box.x1])];
  const ys = [...nodes.flatMap((n) => [n.y, n.y + n.h]), ...wires.flatMap((w) => [w.box.y0, w.box.y1])];
  // Loops, not Math.min(...xs): a large project's points exceed an
  // engine's argument limit.
  // A slot's inside, opened, has the slot's face at its top, above its nodes (the frame's "panel header", 2026-10-30).
  const face = faces?.frame ? frameFace(faces.frame, faceSize?.(faces.frame) ?? null, { x0: least(xs, Infinity), x1: most(xs, -Infinity), y0: least(ys, Infinity) }) : null;
  if (face) {
    xs.push(face.x, face.x + face.w);
    ys.push(face.y, face.y + face.h);
  }
  const [x0, x1, y0, y1] = [least(xs, Infinity), most(xs, -Infinity), least(ys, Infinity), most(ys, -Infinity)];
  const bounds = xs.length ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : { x: 0, y: 0, w: MIN_WIDTH, h: HEADER };
  return { nodes, wires, bounds, bottom, face };
}

/**
 * Where a box at `node` (`{x, y}`) goes when dragged by `travel` screen
 * pixels at scale `k`: in whole layout units, as `schematic.move` stores it.
 */
export function dragPosition(node, travel, k) {
  return { x: Math.round(node.x + travel.x / k), y: Math.round(node.y + travel.y / k) };
}

/**
 * The lowest box's bottom edge (never above 0), with node `id`, if given,
 * at `to` (or, `id` a Map, each node it holds at its position): what
 * feedback lanes run below, in `layout` and in a drag alike.
 */
export function levelBottom(nodes, id = null, to = null) {
  const moves = id instanceof Map ? id : new Map(id === null ? [] : [[id, to]]);
  let bottom = 0;
  for (const n of nodes) {
    const y = (moves.get(n.id)?.y ?? n.y) + n.h;
    if (y > bottom) bottom = y;
  }
  return bottom;
}

/**
 * The wires of `drawn` (a `layout`) that change while its node `id` is
 * dragged to `to` (`{x, y}`, its box's corner), every other node staying
 * where it is: `[{wire, a, b, feedback, under, key}]`, each routed as
 * `layout` would route it with the node stored there (`routed` gives its
 * geometry; `key` is the same string for the same route). The wires
 * attached to the node (in or out), and the feedback wires whose lane's y
 * moves (the lowest box moved, or a wire attached became or stopped being
 * feedback). Released there, the next layout draws exactly these, unless
 * the node now overlaps one that is not stored (see docs/ui-controls.md
 * "Schematic"). `id` may instead be a Map of node id → `{x, y}` (and `to`
 * left out): a group dragged together, every node of it at its position.
 */
export function movedWires(drawn, id, to) {
  const asked = id instanceof Map ? id : new Map([[id, to]]);
  const moves = new Map();
  for (const node of drawn.nodes) {
    const at = asked.get(node.id);
    if (at) moves.set(node.id, { dx: at.x - node.x, dy: at.y - node.y, at });
  }
  if (!moves.size) return [];
  const bottom = levelBottom(drawn.nodes, new Map([...moves].map(([key, m]) => [key, m.at])));
  const shift = (p, node) => {
    const m = moves.get(node);
    return m ? { x: p.x + m.dx, y: p.y + m.dy } : p;
  };
  const changed = [];
  let lane = 0;
  for (const wire of drawn.wires) {
    const attached = moves.has(wire.from.node) || moves.has(wire.to.node);
    const a = shift(wire.curve.start, wire.from.node);
    const b = shift(wire.curve.segs.at(-1).to, wire.to.node);
    const feedback = wire.reversed || b.x <= a.x;
    if (feedback) lane += 1;
    const under = feedback ? laneY(bottom, lane) : null;
    if (!attached && under === wire.under) continue;
    changed.push({ wire, a, b, feedback, under, key: `${a.x},${a.y},${b.x},${b.y},${under}` });
  }
  return changed;
}

/** The geometry of a route `movedWires` gives: `{curve, points, box}`. */
export function routed(route) {
  return wireGeometry(route.a, route.b, { feedback: route.feedback, under: route.under ?? 0 });
}

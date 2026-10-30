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
import { VIA_ROW, VIA_W, controlAnchor, controlEdges, controlNodes, controlPads } from './schematic-controls.js';
import { frameFace, placeFace } from './schematic-face.js';
import { CLEARANCE, PU, STUB, TRACK, balance, near, placeNear, ring, sugiyama } from './board-layout.js';
import { Maze, STEP } from './board-maze.js';
import { formRibbons, placeRibbonLabels, ribbonName } from './board-ribbon.js';
import { LANE as LANE_PITCH, laneWires } from './board-lanes.js';
import { stretchCorners } from './board-stretch.js';
import { LABEL_STUB, NET_DESTS, measure, orthGeometry, routeWires, simplify, zCorners } from './board-route.js';

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
  const at = node.pinAt?.get(`${out ? 'out' : 'in'}\n${name}`);
  if (at) return at.dy;
  if (!out && node.topOrder?.has(name)) return 0;
  const list = out ? node.outs : node.ins.filter((p) => !node.topOrder?.has(p.name));
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  return HEADER + ROW * (i + 0.5);
}

function anchor(node, name, out) {
  if (node.kind === 'control') return controlAnchor(node, name, out);
  // A package's pin where its die's interface pin falls (ADR 0047 §7–8, `shapePackage`).
  const at = node.pinAt?.get(`${out ? 'out' : 'in'}\n${name}`);
  if (at) return at.side ? { x: node.x + at.dx, y: node.y + at.dy, side: at.side } : { x: node.x + at.dx, y: node.y + at.dy };
  // A parameter on a chip's top edge (ADR 0047 §7, the board over a plate).
  if (!out && node.topOrder?.has(name)) return { x: node.x + TOP_PAD + node.topOrder.get(name) * TOP_PITCH, y: node.y, side: 'top' };
  const list = out ? node.outs : node.ins.filter((p) => !node.topOrder?.has(p.name));
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
export function layout(level, positions = {}, { factsOf = null, controls = null, faces = null, faceSize = null, board = null } = {}) {
  // ADR 0047 §5–6: the board (the top level under the plate): vias, anchored and layered chips, orthogonal wiring.
  if (board) return boardLevel(level, positions, { factsOf, controls, faces, faceSize, board });
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
    const stored = storedAt(node, positions);
    node.pinned = Boolean(stored);
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
 * The area of package `node` its die is drawn in (ADR 0047 §8): its body
 * below its header, inside its edges. A slot's face is not drawn while its
 * die is (it lifts), so the die has the whole body.
 */
export function dieArea(node) {
  return { x: node.x + PAD, y: node.y + HEADER, w: Math.max(1, node.w - 2 * PAD), h: Math.max(1, node.h - HEADER - PAD) };
}

/**
 * The transform of die `die` (an inside's layout, `bounds` its rectangle)
 * into package `node`'s rectangle (ADR 0047 §8, the owner's verdict on
 * 9e81dc8: a microscope): uniform, the die filling it, centred. `{k, x, y}`,
 * die → the package's level.
 */
export function dieLink(node, die) {
  const R = die.bounds;
  const k = Math.min(node.w / Math.max(R.w, 1e-9), node.h / Math.max(R.h, 1e-9));
  return { k, x: node.x + (node.w - R.w * k) / 2 - R.x * k, y: node.y + (node.h - R.h * k) / 2 - R.y * k };
}

/** A package's pins keep at least this far apart (layout units; its die spreads them at least half an even share of an edge apart). */
export const PACKAGE_PITCH = 8;

/**
 * Package `node` given its die's shape (ADR 0047 §7, the owner's verdict on
 * 9e81dc8): the aspect of its die (`die.bounds`), about the area it had
 * (more when its pins or title need it: PACKAGE_PITCH between pins, its
 * title's width), its width in whole plate units and its height the width
 * over that aspect, rounded; and its pins where its die's interface pins
 * fall when the die is scaled into it (`dieLink`): inputs on its left edge,
 * outputs on its right, parameters on its top (`pinAt`, `topOrder`), one
 * geometry. A slot with a face keeps its size. Deterministic.
 */
export function shapePackage(node, die) {
  node.die = true;
  if (node.face || !(die?.bounds?.w > 0) || !(die?.bounds?.h > 0)) return node;
  const a = die.bounds.w / die.bounds.h;
  const frame = (id) => die.nodes.find((n) => n.id === id);
  const lefts = (frame(INPUTS)?.outs ?? []).filter((p) => node.ins.some((q) => q.name === p.name));
  const tops = (frame(PARAMS)?.outs ?? []).filter((p) => node.ins.some((q) => q.name === p.name));
  const rights = (frame(OUTPUTS)?.ins ?? []).filter((p) => node.outs.some((q) => q.name === p.name));
  const area = node.w * node.h;
  let w = Math.sqrt(area * a);
  let h = w / a;
  const title = CHAR * Math.max(node.name.length, (node.subtitle ?? node.module).length) + 2 * PAD;
  const wMin = Math.max(MIN_WIDTH, title, PACKAGE_PITCH * 2 * (tops.length + 1));
  const hMin = Math.max(HEADER + PAD, PACKAGE_PITCH * 2 * (Math.max(lefts.length, rights.length) + 1));
  const grow = Math.max(1, wMin / w, hMin / h);
  w = Math.ceil((w * grow) / PU) * PU;
  h = Math.max(PU, Math.round(w / a));
  node.w = w;
  node.h = h;
  const t = dieLink({ x: 0, y: 0, w, h }, die);
  // On eighths of a unit: a connector's pins (ADR 0047 phase 5) lie closer than a unit on a small package.
  const map = (p) => ({ x: Math.round((p.x * t.k + t.x) * 8) / 8, y: Math.round((p.y * t.k + t.y) * 8) / 8 });
  const at = new Map();
  for (const p of lefts) at.set(`in
${p.name}`, { dx: 0, dy: map(p).y });
  for (const p of rights) at.set(`out
${p.name}`, { dx: w, dy: map(p).y });
  for (const p of tops) at.set(`in
${p.name}`, { dx: map(p).x, dy: 0, side: 'top' });
  node.pinAt = at;
  // Its die's connector (ADR 0047 phase 5): the same pin group on the package's top edge, labelled with its count.
  const strip = frame(PARAMS);
  if (strip?.connector) {
    const xs = tops.map((p) => map(p).x);
    node.connector = { dx0: Math.min(...xs) - 3, dx1: Math.max(...xs) + 3, count: tops.length, label: `params ×${tops.length}` };
  }
  const top = [...tops].sort((p, q) => p.x - q.x).map((p) => p.name);
  if (top.length) node.topOrder = new Map(top.map((name, i) => [name, i]));
  return node;
}

/** A die's parameters this many or more are one connector (ADR 0047 phase 5), their pins this far apart (die units), its outline this far past them. */
export const CONNECTOR_MIN = 4;
export const CONNECTOR_PITCH = 6;
const CONNECTOR_PAD = 12;

/** A die's parameters' strip (ADR 0047 §8): the node on its top edge. */
export const PARAMS = '@params';
/** A die's edges lie this far (layout units) outside what it holds, where no interface pin sets them. */
export const DIE_MARGIN = 32;
/** Between a die's parameters' strip and its highest chip: room for the drops, at least. */
const PARAM_GAP = 48;

/**
 * A die's frame (ADR 0047 §8, the coordinator's design after 63bbc55):
 * its parameters' pins moved off `@inputs` onto a strip `@params` (not
 * laid out with the chips: placed on the top edge after them), the chips'
 * parameters they alone set put on the chips' top edges (`topOrder`, as
 * the plate's chips have), and `@inputs` and `@outputs` zero wide (their
 * pins on the die's left and right edges). `{params, edges}`: the strip
 * (or null) and the export edges from it, taken out of the layered layout.
 */
function dieFrame(level, nodes, byId, edges) {
  const names = new Set((level.parameters ?? []).map((p) => p.name));
  const inputs = byId.get(INPUTS);
  for (const id of [INPUTS, OUTPUTS]) {
    const n = byId.get(id);
    if (n) Object.assign(n, { w: 0, edge: true });
  }
  if (!inputs || !names.size) return { params: null, edges: [] };
  const pins = inputs.outs.filter((p) => names.has(p.name));
  if (!pins.length) return { params: null, edges: [] };
  inputs.outs = inputs.outs.filter((p) => !names.has(p.name));
  const params = { id: PARAMS, name: 'parameters', module: '', kind: 'input', composite: false, ins: [], outs: pins.map((p) => ({ ...p, side: 'bottom' })), w: 0, h: 0, edge: true };
  byId.set(PARAMS, params);
  const taken = edges.filter((e) => e.from.node === INPUTS && names.has(e.from.port));
  for (const e of taken) e.from = { node: PARAMS, port: e.from.port };
  // An input frame left with no pin is not drawn.
  if (!inputs.outs.length) {
    nodes.splice(nodes.indexOf(inputs), 1);
    byId.delete(INPUTS);
  }
  // Each chip's parameters the strip alone sets: on its top edge, in the strip's order.
  const order = new Map(pins.map((p, i) => [p.name, i]));
  const driven = new Set(edges.filter((e) => !taken.includes(e)).map((e) => `${e.to.node}\n${e.to.port}`));
  const tops = new Map();
  for (const e of taken) {
    const node = byId.get(e.to.node);
    if (!node || node.kind !== 'instance' || node.face || driven.has(`${e.to.node}\n${e.to.port}`)) continue;
    if (!tops.has(node)) tops.set(node, new Map());
    const own = tops.get(node);
    own.set(e.to.port, Math.min(own.get(e.to.port) ?? Infinity, order.get(e.from.port)));
  }
  for (const [node, own] of tops) {
    const keyed = [...own].sort((a, b) => a[1] - b[1] || node.ins.findIndex((p) => p.name === a[0]) - node.ins.findIndex((p) => p.name === b[0]));
    node.topOrder = new Map(keyed.map(([name], i) => [name, i]));
    const side = node.ins.filter((p) => !node.topOrder.has(p.name));
    const longest = (list) => Math.max(0, ...list.map((p) => p.name.length));
    node.w = Math.max(node.w, Math.ceil(CHAR * (longest(side) + longest(node.outs)) + 4 * PAD), 2 * TOP_PAD + (keyed.length - 1) * TOP_PITCH);
    node.h = HEADER + ROW * Math.max(1, side.length, node.outs.length) + PAD;
  }
  return { params, edges: taken };
}

/**
 * A die's rectangle and its interface (ADR 0047 §8, the owner's verdict on
 * 9e81dc8: a microscope, the die scaled into its package): the rectangle
 * holds its chips and their wires with DIE_MARGIN round (more above for
 * the parameters' drops), on whole grid steps; its inputs' pins spread
 * evenly down its left edge in their layout's order, its outputs' down its
 * right, its parameters' along its top edge in `order` (the package's pin
 * order, where the level around gives one) else over the pins they set.
 * Every wire from or to them is then routed by the maze within the
 * rectangle, around the chips and every wire drawn; one with no way is
 * drawn as labels. Replaces those wires in `wires`, appends the
 * parameters' wires and the strip. `{rect}`.
 */
/** Values near `want` (in its order), kept in order at least `gap` apart and within (`lo`, `hi`): pushed right, then back left from `hi`; whole units. */
function settle(want, lo, hi, gap) {
  const n = want.length;
  const out = [];
  for (let i = 0; i < n; i++) out.push(Math.max(want[i], lo + gap * (i + 1), i ? out[i - 1] + gap : -Infinity));
  for (let i = n - 1; i >= 0; i--) out[i] = Math.min(out[i], hi - gap * (n - i), i + 1 < n ? out[i + 1] - gap : Infinity);
  return out.map((v) => Math.round(v));
}

function dieWires({ die, nodes, byId, edges, boxes, routes, labels, wires, order = null }) {
  const chips = nodes.filter((n) => n.kind === 'instance');
  const frameEdge = (e) => e.from.node === INPUTS || e.to.node === OUTPUTS;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const n of chips) {
    x0 = Math.min(x0, n.x);
    y0 = Math.min(y0, n.y);
    x1 = Math.max(x1, n.x + n.w);
    y1 = Math.max(y1, n.y + n.h);
  }
  wires.forEach((w, i) => {
    if (frameEdge(edges[i]) || routes[i]?.labelled) return;
    x0 = Math.min(x0, w.box.x0);
    y0 = Math.min(y0, w.box.y0);
    x1 = Math.max(x1, w.box.x1);
    y1 = Math.max(y1, w.box.y1);
  });
  if (!Number.isFinite(x0)) [x0, y0, x1, y1] = [0, 0, MIN_WIDTH, HEADER];
  const { params } = die;
  const pEdges = die.edges;
  const inputs = byId.get(INPUTS);
  const outputs = byId.get(OUTPUTS);
  const snap = (v, up) => (up ? Math.ceil(v / STEP) : Math.floor(v / STEP)) * STEP;
  const side = (n) => Math.max(DIE_MARGIN, 2 * STUB + TRACK * (n + 1));
  const left = snap(x0 - side(inputs?.outs.length ?? 0), false);
  const right = snap(x1 + side(outputs?.ins.length ?? 0), true);
  // A connector's wires leave it as one ribbon (ADR 0047 phase 5): room for its comb, not a track per drop.
  const connected = params && params.outs.length >= CONNECTOR_MIN;
  const top = snap(y0 - (params ? PARAM_GAP + (connected ? 2 * combDepth(params.outs.length) : TRACK * pEdges.length) : DIE_MARGIN), false);
  const bottom = snap(y1 + DIE_MARGIN, true);
  const rect = { x: left, y: top, w: right - left, h: bottom - top };
  // The interface along the edges, each pin as near where its wires want it as order and room allow: in order,
  // at least half an even share of the edge apart (`settle`), on whole units.
  const spread = (want, a, b) => settle(want, a, b, (b - a) / (2 * (want.length + 1)));
  if (inputs) {
    const sorted = [...inputs.outs].sort((p, q) => p.y - q.y);
    const ys = spread(sorted.map((p) => p.y), top, bottom);
    const at = new Map(sorted.map((p, i) => [p.name, ys[i]]));
    Object.assign(inputs, { x: left, y: top, w: 0, h: bottom - top, outs: inputs.outs.map((p) => ({ ...p, x: left, y: at.get(p.name), side: 'right' })) });
  }
  if (outputs) {
    const sorted = [...outputs.ins].sort((p, q) => p.y - q.y);
    const ys = spread(sorted.map((p) => p.y), top, bottom);
    const at = new Map(sorted.map((p, i) => [p.name, ys[i]]));
    Object.assign(outputs, { x: right, y: top, w: 0, h: bottom - top, ins: outputs.ins.map((p) => ({ ...p, x: right, y: at.get(p.name), side: 'left' })) });
  }
  if (params) {
    const want = params.outs.map((p, i) => {
      const xs = pEdges.filter((e) => e.from.port === p.name).map((e) => anchor(byId.get(e.to.node), e.to.port, false).x);
      return { p, i, x: xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : x0, o: order?.get(p.name) };
    });
    // The package's order wins (its top pins follow its vias); else over what they set.
    want.sort((a, b) => (a.o !== undefined && b.o !== undefined ? a.o - b.o : 0) || (a.o === undefined) - (b.o === undefined) || a.x - b.x || a.i - b.i);
    // ADR 0047 phase 5 (the owner: "ribbons can go into the chips"): CONNECTOR_MIN parameters or more are one connector, packed
    // CONNECTOR_PITCH apart in their order over the middle of what they set, the ribbon from the plate entering there as one.
    let xs;
    if (want.length >= CONNECTOR_MIN) {
      const span = (want.length - 1) * CONNECTOR_PITCH;
      const mid = want.reduce((s, w) => s + w.x, 0) / want.length;
      const first = Math.round(Math.min(right - STEP - span, Math.max(left + STEP, mid - span / 2)) / STEP) * STEP;
      xs = want.map((_, i) => first + i * CONNECTOR_PITCH);
      params.connector = { x0: xs[0] - CONNECTOR_PAD, x1: xs.at(-1) + CONNECTOR_PAD, count: want.length };
    } else xs = spread(want.map((w) => w.x), left, right);
    const at = new Map(want.map((w, i) => [w.p.name, xs[i]]));
    params.outs = params.outs.map((p) => ({ ...p, x: at.get(p.name), y: top, side: 'bottom' }));
    Object.assign(params, { x: left, y: top, w: right - left, h: 0, layer: null, pinned: false });
    nodes.push(params);
  }
  // Every wire of the interface: the maze within the rectangle, around the chips and the wires drawn.
  const asked = [];
  edges.forEach((e, i) => {
    if (frameEdge(e)) asked.push({ e, id: i });
  });
  pEdges.forEach((e, k) => asked.push({ e, id: edges.length + k }));
  const pinOf = (id, port, out) => (out ? byId.get(id)?.outs : byId.get(id)?.ins)?.find((p) => p.name === port) ?? anchor(byId.get(id), port, out);
  for (const q of asked) {
    q.a = q.e.from.node === PARAMS ? params.outs.find((p) => p.name === q.e.from.port) : pinOf(q.e.from.node, q.e.from.port, true);
    q.b = pinOf(q.e.to.node, q.e.to.port, false);
    if (q.e.to.node !== OUTPUTS) q.b = { ...q.b, ...anchor(byId.get(q.e.to.node), q.e.to.port, false) };
    if (q.e.from.node !== INPUTS && q.e.from.node !== PARAMS) q.a = { ...q.a, ...anchor(byId.get(q.e.from.node), q.e.from.port, true) };
  }
  const segments = [];
  wires.forEach((w, i) => {
    if (frameEdge(edges[i]) || routes[i]?.labelled) return;
    for (let k = 1; k < w.corners.length; k++) segments.push(Object.assign([w.corners[k - 1], w.corners[k]], { net: `${w.from.node}\n${w.from.port}` }));
  });
  const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'left' })), ...n.outs.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'right' }))]);
  const area = { x0: rect.x, x1: rect.x + rect.w, y0: rect.y, y1: rect.y + rect.h };
  const maze = new Maze(boxes, asked.flatMap((q) => [q.a, q.b]), segments, area, pins);
  const order2 = asked.map((q, j) => ({ q, j, d: Math.abs(q.a.x - q.b.x) + Math.abs(q.a.y - q.b.y) }));
  order2.sort((p, q) => p.d - q.d || p.j - q.j);
  const found = new Map();
  for (const { q } of order2) {
    const net = `${q.e.from.node}\n${q.e.from.port}`;
    // The wires into the same pin (a parameter also driven from the flow) it may join there.
    const share = edges.filter((o) => o.to.node === q.e.to.node && o.to.port === q.e.to.port).map((o) => `${o.from.node}\n${o.from.port}`);
    // A connector's wires are one bus (ADR 0047 phase 5): they share their way out of it, drawn as one ribbon in lanes (ui/lib/board-lanes.js).
    const bus = q.e.from.node === PARAMS && params?.connector ? 'bus:params' : null;
    found.set(q.id, maze.route(q.a, q.b, net, { bus, from: q.a.side ?? 'right', into: q.b.side ?? 'left', share, budget: 2 * DIE_STATES }));
  }
  const kept = labels.filter((l) => !asked.some((q) => q.id === l.wire));
  labels.length = 0;
  labels.push(...kept);
  const sourced = new Set(labels.filter((l) => l.side === 'out').map((l) => l.net));
  for (const q of asked) {
    const { a, b, e, id } = q;
    const corners = found.get(id);
    let r;
    if (corners) r = { corners: simplify(corners), labelled: false };
    else {
      const net = `${e.from.node}\n${e.from.port}`;
      const text = String(e.written?.from ?? e.from.port).replace(/^\//, '');
      const into = b.side === 'top' ? { x: b.x, y: b.y - LABEL_STUB } : b.side === 'left' ? { x: b.x - LABEL_STUB, y: b.y } : { x: b.x - LABEL_STUB, y: b.y };
      labels.push({ net, text: `◂ ${text}`, side: 'in', at: into, corners: [into, b], wire: id });
      if (!sourced.has(net)) {
        sourced.add(net);
        const end = a.side === 'bottom' ? { x: a.x, y: a.y + LABEL_STUB } : { x: a.x + LABEL_STUB, y: a.y };
        labels.push({ net, text: `▸ ${text}`, side: 'out', at: end, corners: [a, end], wire: id });
      }
      r = { corners: [into, b], labelled: true, whole: zCorners(a, b) };
    }
    const wire = boardWire(id, e, { feedback: false, lane: 0, under: null, a, b }, r);
    if (e.from.node === PARAMS && params?.connector) wire.bus = 'bus:params';
    if (id < edges.length) wires[id] = wire;
    else wires.push(wire);
  }
  return { rect };
}

/** A node's stored position (`ui/schematic.toml`), or null. */
function storedAt(node, positions) {
  const stored = node.kind === 'instance' ? positions?.[node.name] : undefined;
  return stored && Number.isFinite(stored.x) && Number.isFinite(stored.y) ? stored : null;
}

/** A pin's row on its side of `node` as a fraction in [0, 1): what crossing minimisation orders by. */
function pinFraction(node, name, out) {
  const at = node.pinAt?.get(`${out ? 'out' : 'in'}\n${name}`);
  if (at) return Math.min(0.999, Math.max(0, at.dy / Math.max(1, node.h)));
  const list = out ? node.outs : node.ins;
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  return (i + 0.5) / Math.max(1, list.length);
}

const rectOf = (n) => ({ x: n.x, y: n.y, w: n.w, h: n.h });

/** Between the plate and the board's first column: at least this, and room for the legs' tracks. */
export const PLATE_GAP = 64;
/** A source chip feeding this many chips or more is a global net's (the transport's clock): its nets are labels. */
export const GLOBAL_FANOUT = 3;
/** The maze's margin around everything drawn. */
const MAZE_MARGIN = 64;
/** A die's maze searches this many states for a wire the channels found no way for (ADR 0047 phase 3: every inner net drawn). */
const DIE_STATES = 12000;

/**
 * The legs of the plate's vias (`panel`, placed): `[{id, control, pin,
 * from, to, a, b, chip, writes}]`. A writer's leg runs from its pad's
 * output to the parameter or input it binds; a reader's from the output it
 * reads to its pad's input. A nested address, or a reader of an input,
 * has none.
 */
function legsOf(panel, byId) {
  const legs = [];
  for (const c of panel) {
    for (const pin of c.binds) {
      if (!pin.role) continue;
      const chip = byId.get(pin.instance);
      if (!chip) continue;
      if (pin.writes) legs.push({ control: c, pin, chip: chip.id, from: { node: c.id, port: pin.name }, to: { node: chip.id, port: pin.port }, writes: true });
      else if (pin.role === 'out') legs.push({ control: c, pin, chip: chip.id, from: { node: chip.id, port: pin.port }, to: { node: c.id, port: pin.name }, writes: false });
    }
  }
  return legs;
}

/**
 * The pins of each chip ordered as its neighbours lie (ADR 0047 §6, the
 * owner's verdict: fewer crossings): a side's ports by the mean height of
 * what they connect to, its parameters likewise (a via's leg counts), the
 * unconnected after them in their order. Not a chip with port groups, a
 * slot or a face (their order is their interface's).
 */
function orderPins(nodes, byId, edges, legs, factsOf = null) {
  const ys = new Map();
  const add = (node, side, port, y) => {
    const key = `${node}\n${side}\n${port}`;
    if (!ys.has(key)) ys.set(key, []);
    ys.get(key).push(y);
  };
  const centre = (n) => n.y + n.h / 2;
  for (const e of edges) {
    const a = byId.get(e.from.node);
    const b = byId.get(e.to.node);
    if (!a || !b) continue;
    add(e.from.node, 'outs', e.from.port, centre(b));
    add(e.to.node, 'ins', e.to.port, centre(a));
  }
  for (const l of legs) {
    const r = l.control.control.rect;
    add(l.chip, l.writes ? 'ins' : 'outs', l.pin.port, r.y + r.h / 2);
  }
  for (const node of nodes) {
    if (node.kind !== 'instance' || node.slot || node.face) continue;
    // A chip with port groups keeps its order within each kind (its interface's): only the two kinds may swap.
    const grouped = Boolean(node.groups);
    for (const side of ['ins', 'outs']) {
      const keyed = node[side].map((p, i) => {
        const list = ys.get(`${node.id}\n${side}\n${p.name}`);
        return { p, i, key: list ? list.reduce((s, v) => s + v, 0) / list.length : Infinity, kind: p.parameter ? 1 : 0 };
      });
      // Ports and parameters stay two groups; the group whose wires come from higher up is on top.
      const mean = (k) => {
        const v = keyed.filter((x) => x.kind === k && Number.isFinite(x.key)).map((x) => x.key);
        return v.length ? v.reduce((s, x) => s + x, 0) / v.length : Infinity;
      };
      const first = mean(1) < mean(0) ? 1 : 0;
      keyed.sort((u, v) => (u.kind === first ? 0 : 1) - (v.kind === first ? 0 : 1) || (grouped ? 0 : u.key - v.key) || u.i - v.i);
      node[side] = keyed.map((k) => k.p);
    }
    if (grouped && factsOf) {
      const facts = factsOf(insideKey(node));
      if (facts) node.groups = { ins: portGroups(node.ins, facts), outs: portGroups(node.outs, facts) };
    }
  }
}

/**
 * The board (ADR 0047 §5–6, after the owner's verdict of 2026-10-30: one
 * arrangement, beautiful and clear): level `level` (the top level) with
 * `board` (`{vias, obstacles, plate}`: `vias` a Map of a panel control's
 * index → its rectangle on the plate in layout units, whole plate units;
 * `obstacles` every control's rectangle; `plate` the plate's `{w, h}` or
 * null). Every chip is laid out by the layered layout, the signal flowing
 * left to right in columns right of the plate; a chip its vias bind wants
 * the height of their middle, so it lies in the row of its controls, in
 * flow order. Each via is a small pad at its control's middle. Wires
 * between chips run in the channels between the columns (`routeWires`),
 * the vias' legs through the gutters between the plate's controls and the
 * channels (`Maze`), never under a control or a chip; only a net with
 * NET_DESTS destinations or more, a global net and a wire that finds no
 * way are labels. A stored position wins, moved off a control or chip it
 * would cover. The result has the shape `layout` gives, each wire `orth`
 * with its `ends`, and `board`: `{labels, metrics, plate, crossings}`.
 */
function boardLevel(level, positions, { factsOf, controls, faces, faceSize, board }) {
  const nodes = nodesOf(level ?? {}, factsOf);
  const byId = new Map(nodes.map((n) => [n.id, n]));
  for (const node of nodes) {
    const face = node.kind === 'instance' ? faces?.boxes?.get(node.name) : null;
    if (face) placeFace(node, face, faceSize?.(face) ?? null);
  }
  const all0 = edgesOf(level ?? {}, byId);
  // A die (ADR 0047 §8, the coordinator's design after 63bbc55): its parameters a strip on its top edge, its
  // inputs and outputs zero-wide columns at its left and right edges (`dieFrame`).
  const die = board.die ? dieFrame(level ?? {}, nodes, byId, all0) : null;
  const edges = die ? all0.filter((e) => !die.edges.includes(e)) : all0;
  const back = backEdges(nodes, edges);
  const ranks = layers(nodes, edges, back);
  const vias = board.vias instanceof Map ? board.vias : new Map();
  // The vias: a small pad at its control's middle; the control's whole rectangle stays an obstacle.
  const panel = (controls?.length ? controlNodes(controls.filter((c) => !c.part), byId, pinOffset) : []).filter((c) => vias.has(c.control.index));
  for (const c of panel) {
    const v = vias.get(c.control.index);
    const rows = Math.max(1, c.ins.length, c.outs.length);
    c.w = Math.max(VIA_W, VIA_ROW * rows + 8);
    c.h = 12;
    c.x = Math.round(v.x + v.w / 2 - c.w / 2);
    c.y = Math.round(v.y + v.h / 2 - c.h / 2);
    c.control = { ...c.control, rect: v };
    c.via = true;
    c.pinned = true;
    c.layer = null;
  }
  const controlRects = [...(board.obstacles ?? []), ...panel.map((c) => c.control.rect)];
  const legs = legsOf(panel, byId);
  // ADR 0047 §7–8: a chip with an inside (`board.dieOf`, its die) is a package of its die's shape, its pins its die's.
  const widen = () => {
    for (const node of nodes) {
      const die = node.composite && node.kind === 'instance' ? (board.dieOf?.(node) ?? null) : null;
      if (die) shapePackage(node, die);
    }
  };
  // Over a plate: the chips under their controls (the coordinator's design after the owner's verdict).
  if (panel.length) {
    const { wires, labels, bottom, sections } = plateBoard({ nodes, byId, edges, back, ranks, panel, legs, controlRects, board, positions, factsOf, widen });
    const done = finishBoard(nodes, wires, labels, bottom, board, 0, { obstacles: controlRects });
    done.board.sections = sections;
    return done;
  }
  // The plate's right edge: the board's columns start past it and the legs' tracks.
  let plateRight = board.plate?.w ?? 0;
  for (const r of controlRects) plateRight = Math.max(plateRight, r.x + r.w);
  widen();
  const buses = new Set(legs.map((l) => l.chip));
  const plateArea = Boolean(board.plate || controlRects.length);
  const x0 = plateArea ? plateRight + Math.max(PLATE_GAP, 2 * STUB + 2 * TRACK * (buses.size + 1)) : 0;
  // A chip with vias wants the height of their middle.
  const anchors = new Map();
  for (const chip of buses) {
    const own = legs.filter((l) => l.chip === chip).map((l) => l.control.control.rect);
    const node = byId.get(chip);
    const mid = own.reduce((s, r) => s + r.y + r.h / 2, 0) / own.length;
    anchors.set(chip, Math.round(mid - node.h / 2));
  }
  const sEdges = [];
  edges.forEach((e, i) => {
    if (e.from.node === e.to.node) return;
    const reversed = back.has(i);
    const [u, uPort, uOut, v, vPort, vOut] = reversed ? [e.to.node, e.to.port, false, e.from.node, e.from.port, true] : [e.from.node, e.from.port, true, e.to.node, e.to.port, false];
    const un = byId.get(u);
    const vn = byId.get(v);
    sEdges.push({ key: i, from: u, to: v, net: `${e.from.node}\n${e.from.port}`, fromFrac: pinFraction(un, uPort, uOut), toFrac: pinFraction(vn, vPort, vOut), fromOff: pinOffset(un, uPort, uOut), toOff: pinOffset(vn, vPort, vOut) });
  });
  let layer = new Map(nodes.map((n) => [n.id, ranks.get(n.id)]));
  layer = balance(
    nodes.map((n) => n.id),
    sEdges,
    layer,
    (id) => byId.get(id).kind !== 'instance',
  );
  const kept = [...new Set(layer.values())].sort((a, b) => a - b);
  const renumber = new Map(kept.map((l, i) => [l, i]));
  for (const [id, l] of layer) layer.set(id, renumber.get(l));
  // A die's interface (ADR 0047 phase 3): its outputs' frame pins in the last column, at the package's right edge.
  if (board.die && layer.has(OUTPUTS)) {
    const last = Math.max(...layer.values());
    const fed = sEdges.some((e) => e.from === OUTPUTS);
    // A column of its own when another chip lies in the last one.
    if (!fed) layer.set(OUTPUTS, [...layer].some(([id, l]) => l === last && id !== OUTPUTS) ? last + 1 : last);
  }
  // Each channel keeps room for the legs' buses entering the column after it.
  const extra = new Map();
  for (const chip of buses) {
    const l = layer.get(chip);
    extra.set(l - 1, (extra.get(l - 1) ?? 0) + 2 * TRACK);
  }
  const lay = sugiyama(nodes, sEdges, layer, { anchors, extra });
  for (const node of nodes) {
    node.layer = layer.get(node.id);
    const stored = storedAt(node, positions);
    node.pinned = Boolean(stored);
    if (stored) {
      node.x = stored.x;
      node.y = stored.y;
    } else {
      node.x = x0 + lay.x.get(node.id);
      node.y = lay.y.get(node.id);
    }
  }
  // No chip lies over a control of the plate or another chip, whatever placed it (a stored position
  // from before the board): it moves to the nearest free spot of the spiral from where it is.
  const moved = new Set();
  const placed = [];
  for (const row of lay.rows) {
    for (const id of row) {
      const node = byId.get(id);
      if (!node) continue;
      const over = (o) => near(node, o, CLEARANCE - 1);
      if (controlRects.some(over) || placed.some(over)) {
        const at = placeNear(node, node, [...controlRects, ...placed]);
        if (at) {
          node.x = at.x;
          node.y = at.y;
          moved.add(node.id);
        }
      }
      placed.push(rectOf(node));
    }
  }
  orderPins(nodes, byId, edges, legs, factsOf);
  for (const c of panel) {
    nodes.push(c);
    byId.set(c.id, c);
  }
  for (const node of nodes) {
    node.ins = node.ins.map((p) => ({ ...p, ...anchor(node, p.name, false) }));
    node.outs = node.outs.map((p) => ({ ...p, ...anchor(node, p.name, true) }));
  }
  const bottom = levelBottom(nodes);
  // A source feeding GLOBAL_FANOUT chips or more (the transport's clock): its nets are global, drawn as labels.
  const fed = new Map();
  const hasInput = new Set(edges.map((e) => e.to.node));
  for (const e of edges) {
    if (!fed.has(e.from.node)) fed.set(e.from.node, new Set());
    fed.get(e.from.node).add(e.to.node);
  }
  const global = new Set();
  for (const e of edges) if (!hasInput.has(e.from.node) && (fed.get(e.from.node)?.size ?? 0) >= GLOBAL_FANOUT) global.add(`${e.from.node}\n${e.from.port}`);
  let lane = 0;
  const asked = edges.map((e, i) => {
    const a = anchor(byId.get(e.from.node), e.from.port, true);
    const b = anchor(byId.get(e.to.node), e.to.port, false);
    const feedback = back.has(i);
    if (feedback) lane += 1;
    const chain = lay.chains.get(i);
    const still = (id) => !moved.has(id) && !byId.get(id).pinned;
    const straight = Boolean(chain?.length) && !feedback && still(e.from.node) && still(e.to.node);
    return {
      id: i,
      net: `${e.from.node}\n${e.from.port}`,
      from: e.from.node,
      to: e.to.node,
      a,
      b,
      feedback,
      under: feedback ? laneY(bottom, lane) : null,
      lane: feedback ? lane : 0,
      span: Math.abs(layer.get(e.to.node) - layer.get(e.from.node)),
      global: global.has(`${e.from.node}\n${e.from.port}`),
      // A die's interface wires are the maze's, from its edges (`dieWires`): the channels leave them.
      never: Boolean(die) && (e.from.node === INPUTS || e.to.node === OUTPUTS),
      stations: straight ? chain.map((d) => ({ x0: x0 + lay.columns[d.layer].x, x1: x0 + lay.columns[d.layer].x + lay.columns[d.layer].w, y: d.y })) : null,
      edge: e,
    };
  });
  const chips = nodes.filter((n) => n.kind !== 'control');
  const boxes = [...chips.map(({ id, x, y, w, h }) => ({ id, x, y, w, h })), ...controlRects.map((r, k) => ({ id: `@plate:${k}`, ...r }))];
  const nameOf = (w) => String(w.edge.written?.from ?? `${w.edge.from.node}.${w.edge.from.port}`).replace(/^\//, '');
  // A die (ADR 0047 phase 3) draws every net: a wire with no way in the channels is searched for by the maze, around everything drawn.
  const fallback = board.die
    ? (taken) => {
        const xsB = boxes.flatMap((b) => [b.x, b.x + b.w]);
        const ysB = boxes.flatMap((b) => [b.y, b.y + b.h, laneY(bottom, lane + 1)]);
        const area = { x0: least(xsB, 0) - MAZE_MARGIN, x1: most(xsB, 0) + MAZE_MARGIN, y0: least(ysB, 0) - MAZE_MARGIN, y1: most(ysB, 0) + MAZE_MARGIN };
        const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, out: false })), ...n.outs.map((p) => ({ x: p.x, y: p.y, out: true }))]);
        const maze = new Maze(boxes, asked.flatMap((w) => [w.a, w.b]), taken, area, pins);
        return {
          route: (w) => maze.route(w.a, w.b, w.net, { budget: DIE_STATES }),
          take: (corners, net) => {
            const id = maze.netId(net);
            for (let k = 1; k < corners.length; k++) maze.takeSegment(corners[k - 1], corners[k], id);
          },
        };
      }
    : null;
  const { routes, labels } = routeWires(boxes, asked, { nameOf, fanout: false, fallback });
  // The legs, through the plate's gutters and the channels, around everything already drawn.
  const drawnSegments = [];
  routes.forEach((r, i) => {
    for (let k = 1; k < r.corners.length; k++) drawnSegments.push(Object.assign([r.corners[k - 1], r.corners[k]], { net: asked[i].net }));
  });
  for (const l of labels) drawnSegments.push(Object.assign([l.corners[0], l.corners[1]], { net: l.net }));
  const legEnds = legs.map((l) => ({ a: anchor(byId.get(l.from.node), l.from.port, true), b: anchor(byId.get(l.to.node), l.to.port, false) }));
  const legRoutes = [];
  if (legs.length) {
    const all = [...boxes, ...panel.map(({ id, x, y, w, h }) => ({ id, x, y, w, h }))];
    const xsAll = all.flatMap((b) => [b.x, b.x + b.w]);
    const ysAll = all.flatMap((b) => [b.y, b.y + b.h]);
    const bounds = { x0: least(xsAll, 0) - MAZE_MARGIN, x1: most(xsAll, 0) + MAZE_MARGIN, y0: least(ysAll, 0) - MAZE_MARGIN, y1: most(ysAll, 0) + MAZE_MARGIN };
    // Every pin of a chip or a pad keeps its approach for its own wires.
    const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, out: false })), ...n.outs.map((p) => ({ x: p.x, y: p.y, out: true }))]);
    const maze = new Maze(all, legEnds.flatMap((e) => [e.a, e.b]), drawnSegments, bounds, pins);
    // Chip by chip in source order, each chip's legs nearest first: a bus forms from the first.
    const order = legs.map((l, i) => ({ l, i, d: Math.abs(legEnds[i].a.x - legEnds[i].b.x) + Math.abs(legEnds[i].a.y - legEnds[i].b.y) }));
    const chipIndex = new Map(nodes.map((n, i) => [n.id, i]));
    order.sort((p, q) => chipIndex.get(p.l.chip) - chipIndex.get(q.l.chip) || p.d - q.d || p.i - q.i);
    for (const { l, i } of order) {
      const own = [l.control.control.rect, rectOf(l.control)];
      // The wires at its chip's pin (into the parameter it sets, out of the output it reads) it may join.
      const share = l.writes ? edges.filter((e) => e.to.node === l.chip && e.to.port === l.pin.port).map((e) => `${e.from.node}\n${e.from.port}`) : [`${l.chip}\n${l.pin.port}`];
      legRoutes[i] = maze.route(legEnds[i].a, legEnds[i].b, `leg:${l.control.id}:${l.pin.name}`, { bus: `bus:${l.chip}`, own, share });
    }
  }
  const wires = asked.map((w, i) => {
    const e = w.edge;
    const r = routes[i];
    return boardWire(i, e, w, r);
  });
  if (die) {
    const done = dieWires({ die, nodes, byId, edges, boxes, routes, labels, wires, order: board.paramOrder ?? null });
    const finished = finishBoard(nodes, wires, labels, bottom, board, lay.crossings, { obstacles: controlRects, area: { x0: done.rect.x, x1: done.rect.x + done.rect.w, y0: done.rect.y, y1: done.rect.y + done.rect.h } });
    finished.bounds = done.rect;
    finished.board.die = done.rect;
    return finished;
  }
  legs.forEach((l, k) => {
    const id = edges.length + k;
    const corners = legRoutes[k];
    const label = `${l.control.name} ⇢ ${l.pin.address}`;
    const e = {
      from: l.from,
      to: l.to,
      kind: 'binding',
      shape: l.pin.shape,
      converters: [],
      scale: null,
      label,
      written: { from: `${l.control.id}.${l.pin.name}`, to: l.pin.address, binding: true },
    };
    const ends = legEnds[k];
    let r;
    if (corners) r = { corners, labelled: false };
    else {
      // No way through the plate: a label at its chip's pin (its via names it on the plate).
      const at = l.writes ? { x: ends.b.x - LABEL_STUB, y: ends.b.y } : { x: ends.a.x + LABEL_STUB, y: ends.a.y };
      const stub = l.writes ? [at, ends.b] : [ends.a, at];
      r = { corners: stub, labelled: true, whole: zCorners(ends.a, ends.b) };
      labels.push({ net: `leg:${l.control.id}:${l.pin.name}`, text: `${l.writes ? '◂' : '▸'} ${l.control.name}`, side: l.writes ? 'in' : 'out', at, corners: stub, wire: id });
    }
    wires.push(boardWire(id, e, { feedback: false, lane: 0, under: null, a: ends.a, b: ends.b }, r));
  });
  return finishBoard(nodes, wires, labels, bottom, board, lay.crossings, { obstacles: controlRects });
}

/** A board's measures, its pins' `connected`, its bounds: what `layout` returns. */
function finishBoard(nodes, wires, labels, bottom, board, crossings, { obstacles = [], area = null } = {}) {
  // ADR 0047 phase 4: a wire that found no way within the layout's budget is searched for again, around everything drawn; a net label only where none exists.
  // ADR 0047 phase 5 (the owner: "wires on every level, no labelled invisible nets"): a wire the layout left as a label is routed now.
  rescueLabelled(nodes, wires, labels, obstacles, area);
  // ADR 0047 phase 5: the wires into (or out of) a connector meet it as one comb.
  combConnectors(nodes, wires, obstacles);
  const ribbons = boardRibbons(nodes, wires, obstacles, labels);
  const metrics = boardMeasures(wires, labels, ribbons);
  const ends = { outs: new Set(wires.map((w) => `${w.from.node}\n${w.from.port}`)), ins: new Set(wires.map((w) => `${w.to.node}\n${w.to.port}`)) };
  for (const node of nodes) {
    for (const side of ['ins', 'outs']) node[side] = node[side].map((p) => ({ ...p, connected: ends[side].has(`${node.id}\n${p.name}`) }));
  }
  // A label's text reaches about CHAR per character from its stub.
  const xs = [...nodes.flatMap((n) => [n.x, n.x + n.w]), ...wires.flatMap((w) => [w.box.x0, w.box.x1]), ...labels.map((l) => (l.side === 'in' ? l.at.x - CHAR * l.text.length : l.at.x + CHAR * l.text.length))];
  const ys = [...nodes.flatMap((n) => [n.y, n.y + n.h]), ...wires.flatMap((w) => [w.box.y0, w.box.y1])];
  if (board.plate) {
    xs.push(0, board.plate.w);
    ys.push(0, board.plate.h);
  }
  const [bx0, bx1, by0, by1] = [least(xs, Infinity), most(xs, -Infinity), least(ys, Infinity), most(ys, -Infinity)];
  const bounds = xs.length ? { x: bx0, y: by0, w: bx1 - bx0, h: by1 - by0 } : { x: 0, y: 0, w: MIN_WIDTH, h: HEADER };
  return { nodes, wires, bounds, bottom, face: null, board: { labels, metrics, ribbons, plate: board.plate ?? null, crossings, obstacles, area } };
}

/**
 * The ribbons of a board's `wires` among `nodes` and the plate's
 * `obstacles` (ADR 0047 phase 4, ui/lib/board-ribbon.js): each member's
 * drawn geometry set (`display`, `curve`, `points`, `box`), its route
 * (`corners`) unchanged. Returns the ribbons.
 */
export function boardRibbons(nodes, wires, obstacles = [], labels = []) {
  const boxes = [...nodes.filter((n) => n.w > 0 && n.h > 0), ...obstacles];
  const all = wires.filter((w) => !w.labelled).map((w) => ({ id: w.id, corners: w.corners, net: mazeNet(w).net, wire: w }));
  const nameOf = (m) => (m.wire.kind === 'binding' ? String(m.wire.label).split(' ⇢ ')[0] : m.wire.from.node === PARAMS ? 'params' : m.wire.from.port);
  // The group whose ribbons share one label: a chip's legs, a die's connector.
  const groupOf = (net) => (net.startsWith('leg:') ? `legs:${net.split(':').at(-2)}` : net.startsWith(`${PARAMS}\n`) ? 'params' : null);
  // ADR 0047 phase 5: nets sharing a way (a chip's legs, a die's parameters, a wire run along another) each in a lane, a ribbon where three or more share it.
  const lanes = laneWires(all, { nameOf, isLeg: (net) => net.startsWith('leg:@control:'), ribbonName, groupOf });
  // Then side by side on their own tracks (phase 4), as the lanes draw them (a lane is never packed again: `RIBBON_NEAR`).
  const side = formRibbons(
    all.map((w) => ({ ...w, corners: lanes.display.get(w.id) ?? w.corners })),
    boxes,
    { nameOf, endsOf: (m) => [m.wire.from.node, m.wire.to.node] },
  );
  // A lane's band never lies over a box (its lanes still do where they enter it).
  const meets = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
  const chips = nodes.filter((n) => n.w > 0 && n.h > 0);
  for (const r of side.ribbons) {
    const groups = new Set(r.members.map((m) => groupOf(m.net)));
    r.group = groups.size === 1 ? [...groups][0] : null;
  }
  const ribbons = [...lanes.ribbons.filter((r) => !chips.some((b) => meets(r.band, b))), ...side.ribbons];
  const display = new Map([...lanes.display, ...side.display]);
  // Their labels off every box, a via's name and the net labels (`placeRibbonLabels`).
  const texts = [...boxes.map((b) => ({ x: b.x - 2, y: b.y - (b.via ? 10 : 2), w: b.w + 4, h: b.h + (b.via ? 12 : 4) })), ...labels.map((l) => ({ x: l.side === 'in' ? l.at.x - CHAR * l.text.length : l.at.x, y: l.at.y - 9, w: CHAR * l.text.length, h: 12 }))];
  placeRibbonLabels(ribbons, texts);
  for (const w of wires) {
    const shown = display.get(w.id);
    if (shown) {
      w.display = shown;
      Object.assign(w, orthGeometry(shown));
    } else if (w.display) {
      delete w.display;
      Object.assign(w, orthGeometry(w.corners));
    }
  }
  return ribbons;
}

/** A board's measures (ADR 0047 §6.6): of its routes, a leg's bus one net (its ribbons, `board.ribbons`, are counted apart). */
export function boardMeasures(wires, labels, ribbons = []) {
  const m = measure(
    // A connector's ribbon (ADR 0047 phase 5) is one net, as a chip's legs are: its lanes keep its members apart.
    wires.map((w) => ({ net: w.kind === 'binding' ? `bus:${w.from.node.startsWith('@control:') ? w.to.node : w.from.node}` : w.from.node === PARAMS && w.bus ? 'bus:params' : `${w.from.node}\n${w.from.port}` })),
    wires.map((w) => ({ corners: w.corners })),
    labels,
  );
  return m;
}


/** A connector's comb runs this far from its edge (layout units) past half its ribbon's width (its lanes LANE apart). */
export const COMB = 16;
/** How far from its edge the comb of a connector of `count` pins runs. */
export const combDepth = (count) => COMB + (count * LANE_PITCH) / 2;

/**
 * The wires of a connector combed (ADR 0047 phase 5, the owner: "ribbons
 * can go into the chips"): each wire into a package's connector pin (out
 * of a die's strip's) runs, from where its route last meets the comb's line
 * COMB outside the edge, along that line to above its pin and straight in,
 * so all of them reach the connector as one ribbon, each peeling off at its
 * pin (lanes, ui/lib/board-lanes.js). Kept as routed where the comb's line
 * would pass over a box or the route never meets it. Deterministic.
 */
function combConnectors(nodes, wires, obstacles) {
  const boxes = [...nodes.filter((n) => n.w > 0 && n.h > 0), ...obstacles];
  const clearH = (y, x0, x1, own) => !boxes.some((b) => b !== own && y > b.y - 2 && y < b.y + b.h + 2 && Math.max(x0, x1) > b.x - 2 && Math.min(x0, x1) < b.x + b.w + 2);
  // Where corners `c` last meet the horizontal line y (scanning from its end), as an index and point, or null.
  const lastMeet = (c, y) => {
    for (let k = c.length - 1; k > 0; k--) {
      const p = c[k - 1];
      const q = c[k];
      if (p.x === q.x && Math.min(p.y, q.y) <= y && Math.max(p.y, q.y) >= y) return { k, at: { x: p.x, y } };
      if (p.y === q.y && p.y === y) return { k, at: { x: q.x, y } };
    }
    return null;
  };
  for (const node of nodes) {
    const strip = node.id === PARAMS && node.connector;
    if (!node.connector || (node.kind !== 'instance' && !strip)) continue;
    const count = node.connector.count;
    const y = strip ? node.y + combDepth(count) : node.y - combDepth(count);
    for (const w of wires) {
      if (w.labelled) continue;
      const into = !strip && w.to.node === node.id && node.ins.some((p) => p.name === w.to.port && p.side === 'top');
      const outOf = strip && w.from.node === PARAMS;
      if (!into && !outOf) continue;
      const c = outOf ? [...w.corners].reverse() : w.corners;
      const pin = c.at(-1);
      const meet = lastMeet(c.slice(0, -1).concat([{ x: c.at(-2)?.x ?? pin.x, y: pin.y }]), y);
      if (!meet || !clearH(y, meet.at.x, pin.x, strip ? null : node)) continue;
      const combed = simplify([...c.slice(0, meet.k), meet.at, { x: pin.x, y }, pin]);
      const corners = outOf ? combed.reverse() : combed;
      Object.assign(w, { corners, ...orthGeometry(corners) });
    }
  }
}

/** A labelled wire's second search (ADR 0047 phase 4) may visit this many states. */
const RESCUE_STATES = 8000;

/** The net a board's wire belongs to for the maze: a leg's own, in its chip's bus; else its source pin's. */
export function mazeNet(w) {
  if (w.kind === 'binding') {
    const chip = String(w.from.node).startsWith('@control:') ? w.to.node : w.from.node;
    return { net: `leg:${w.from.node}:${w.from.port}:${w.to.node}:${w.to.port}`, bus: `bus:${chip}` };
  }
  return { net: `${w.from.node}\n${w.from.port}`, bus: w.bus ?? null };
}

/**
 * The labelled wires of a board searched for again (ADR 0047 phase 4, the
 * owner's "no labels where a wire can go"): the maze over every box, the
 * plate's `obstacles` and every wire already drawn, with RESCUE_STATES, the
 * shortest first (ties by id); within `area` (a die's rectangle) when given.
 * A wire found is drawn whole and its labels go. Deterministic.
 */
function rescueLabelled(nodes, wires, labels, obstacles, area) {
  const lost = wires.filter((w) => w.labelled && w.ends);
  if (!lost.length) return;
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const boxes = [...nodes.filter((n) => n.w > 0 && n.h > 0).map(({ id, x, y, w, h }) => ({ id, x, y, w, h })), ...obstacles];
  const segments = [];
  for (const w of wires) {
    if (w.labelled) continue;
    const { net } = mazeNet(w);
    for (let k = 1; k < w.corners.length; k++) segments.push(Object.assign([w.corners[k - 1], w.corners[k]], { net }));
  }
  for (const l of labels) if (!lost.some((w) => w.id === l.wire)) segments.push(Object.assign([l.corners[0], l.corners[1]], { net: l.net }));
  const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'left' })), ...n.outs.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'right' }))]);
  const xs = boxes.flatMap((b) => [b.x, b.x + b.w]);
  const ys = boxes.flatMap((b) => [b.y, b.y + b.h]);
  const bounds = area ?? { x0: least(xs, 0) - MAZE_MARGIN, x1: most(xs, 0) + MAZE_MARGIN, y0: least(ys, 0) - MAZE_MARGIN, y1: most(ys, 0) + MAZE_MARGIN };
  const pinOf = (w, out) => {
    const node = byId.get(out ? w.from.node : w.to.node);
    const port = out ? w.from.port : w.to.port;
    return (out ? node?.outs : node?.ins)?.find((p) => p.name === port) ?? (out ? w.ends.a : w.ends.b);
  };
  const ends = lost.map((w) => ({ w, a: pinOf(w, true), b: pinOf(w, false) }));
  const maze = new Maze(boxes, ends.flatMap((e) => [e.a, e.b]), segments, bounds, pins);
  ends.sort((p, q) => Math.abs(p.a.x - p.b.x) + Math.abs(p.a.y - p.b.y) - (Math.abs(q.a.x - q.b.x) + Math.abs(q.a.y - q.b.y)) || p.w.id - q.w.id);
  const found = new Set();
  for (const { w, a, b } of ends) {
    const { net, bus } = mazeNet(w);
    const own = [byId.get(w.from.node), byId.get(w.to.node)].filter((n) => n?.kind === 'control').flatMap((n) => [n.control?.rect, rectOf(n)].filter(Boolean));
    const share = wires.filter((o) => o !== w && !o.labelled && o.to.node === w.to.node && o.to.port === w.to.port).map((o) => mazeNet(o).net);
    // The layout's own search found no way within its budget: a second one over the whole level, with every wire drawn in it (never along another net, never turning where one passes: no false junction).
    // ADR 0047 phase 5 (wires on every level, no label): a relaxed search, which may run along another net (drawn in a lane beside
    // it, ui/lib/board-lanes.js) at a high cost; a far one with a larger budget.
    const ask = { bus, own, share, from: a.side ?? 'right', into: b.side ?? 'left', relaxed: true };
    // Past every budget, the plain orthogonal way (never seen on the example projects): a wire still, never a label.
    const corners = maze.route(a, b, net, { ...ask, budget: RESCUE_STATES }) ?? maze.route(a, b, net, { ...ask, budget: 8 * RESCUE_STATES }) ?? zCorners(a, b);
    const clean = simplify(corners);
    const vertical = clean.find((p, k) => k > 0 && clean[k - 1].x === p.x && clean[k - 1].y !== p.y);
    Object.assign(w, { corners: clean, labelled: false, trunkX: vertical?.x ?? null, ...orthGeometry(clean) });
    delete w.whole;
    found.add(w.id);
  }
  // No net is drawn as a label (ADR 0047 phase 5: "wires on every level, no labelled invisible nets").
  labels.length = 0;
}

/** A chip's top pins (its parameters, ADR 0047 §7) are this far apart… */
export const TOP_PITCH = 12;
/** …and the first this far in from its left edge. */
export const TOP_PAD = 16;
/** Between a card's controls and a chip below them: room for the comb's collector. */
export const COMB_GAP = 24;
/** …and room for its connector's ribbon (ADR 0047 phase 5): its lanes and the comb's line, when it has one. */
const combGap = (node) => (node.connector ? Math.max(COMB_GAP, Math.ceil((2 * combDepth(node.connector.count) + COMB) / PU) * PU) : COMB_GAP);
/** A chip keeps this far from a control (a track or two around it)… */
export const CONTROL_ROOM = 16;
/** …and this far from another chip (a channel for the wires between them). */
export const CHIP_ROOM = 32;
/** A chip keeps this far from a card's edge inside it. */
const CARD_PAD = 8;

/**
 * The parameters of each chip a via binds on its top edge (ADR 0047 §7,
 * the coordinator's design of 2026-10-30): ordered by their via's x, then
 * y (so a card's comb of legs never crosses itself), a parameter no via
 * binds after them in its order; the chip as wide as they need at
 * TOP_PITCH, as tall as its ports need.
 */
function topPins(nodes, legs, edges = []) {
  const at = new Map();
  for (const l of legs) {
    if (!l.writes) continue;
    const r = l.control.control.rect;
    const key = `${l.chip}\n${l.pin.port}`;
    const prev = at.get(key);
    if (!prev || r.x < prev.x || (r.x === prev.x && r.y < prev.y)) at.set(key, { x: r.x + r.w / 2, y: r.y });
  }
  for (const node of nodes) {
    if (node.kind !== 'instance' || node.face) continue;
    // A parameter a wire drives (and no via sets) stays an input on its left side: its wire comes from the flow, not from above.
    const driven = new Set(edges.filter((e) => e.to.node === node.id).map((e) => e.to.port));
    const params = node.ins.filter((p) => p.parameter && (at.has(`${node.id}\n${p.name}`) || !driven.has(p.name)));
    if (!params.length || !params.some((p) => at.has(`${node.id}\n${p.name}`))) continue;
    const keyed = params.map((p, i) => ({ p, i, v: at.get(`${node.id}\n${p.name}`) ?? null }));
    keyed.sort((a, b) => (a.v ? 0 : 1) - (b.v ? 0 : 1) || (a.v && b.v ? a.v.x - b.v.x || a.v.y - b.v.y : 0) || a.i - b.i);
    node.topOrder = new Map(keyed.map((k, i) => [k.p.name, i]));
    const sides = Math.max(1, node.ins.length - params.length, node.outs.length);
    // As wide as its title, its side pins' names and its top pins need (the parameters' names are no longer beside it).
    const longest = (list) => Math.max(0, ...list.map((p) => p.name.length));
    const under = (node.subtitle ?? node.module).length;
    node.w = Math.ceil(Math.max(MIN_WIDTH, CHAR * Math.max(node.name.length, node.module.length, under) + 2 * PAD, CHAR * (longest(node.ins.filter((p) => !params.includes(p))) + longest(node.outs)) + 4 * PAD, 2 * TOP_PAD + (params.length - 1) * TOP_PITCH));
    node.h = HEADER + ROW * (sides + (node.slot ? 1 : 0)) + PAD;
  }
}

/** Chips side by side at one height (within PU), three or more: rows, each left to right. */
function rowsOf(chips, edges = []) {
  const rows = [];
  for (const c of [...chips].sort((a, b) => a.y - b.y || a.x - b.x)) {
    const row = rows.find((r) => Math.abs(r[0].y - c.y) <= PU);
    if (row) row.push(c);
    else rows.push([c]);
  }
  // A chip feeding three or more of its row (the sequencer) is the row's source, not one of it.
  return rows
    .map((r) => r.filter((c) => new Set(edges.filter((e) => e.from.node === c.id && r.some((o) => o.id === e.to.node && o !== c)).map((e) => e.to.node)).size < 3))
    .filter((r) => r.length >= 3)
    .map((r) => r.sort((a, b) => a.x - b.x));
}

/**
 * The ordered buses of the board's rows (the coordinator's polish of
 * 2026-10-30), by construction never crossing themselves: a chip left of
 * a row feeding several of its chips sends its wires along tracks under
 * the row, the nearest chip's on top, each dropping from its pin farther
 * right the nearer its chip and rising into its chip's left pins (the
 * upper pin's riser farther left); the row's outputs into chips at its
 * right end (its mixers) run on tracks below those, the leftmost deepest,
 * rising into the mixer the nearer its edge the deeper. Each wire `i` →
 * its corners; a wire whose way meets a box is left to the maze.
 */
function rowBuses(rows, edges, byId, boxes) {
  const out = new Map();
  const clear = (corners, ends) => {
    for (let k = 1; k < corners.length; k++) {
      const [p, q] = [corners[k - 1], corners[k]];
      const seg = { x: Math.min(p.x, q.x), y: Math.min(p.y, q.y), w: Math.abs(q.x - p.x) || 0.01, h: Math.abs(q.y - p.y) || 0.01 };
      for (const b of boxes) {
        if (ends.has(b.id) && (k === 1 || k === corners.length - 1)) continue;
        if (seg.x < b.x + b.w && b.x < seg.x + seg.w && seg.y < b.y + b.h && b.y < seg.y + seg.h) return false;
      }
    }
    return true;
  };
  const pin = (id, port, out2) => anchor(byId.get(id), port, out2);
  for (const row of rows) {
    const inRow = new Set(row.map((n) => n.id));
    const rowBottom = Math.max(...row.map((n) => n.y + n.h));
    const left = Math.min(...row.map((n) => n.x));
    // The sources left of the row feeding three or more of its chips.
    const sources = new Map();
    edges.forEach((e, i) => {
      const s = byId.get(e.from.node);
      if (!inRow.has(e.to.node) || inRow.has(e.from.node) || !s || s.x + s.w > left) return;
      if (!sources.has(s.id)) sources.set(s.id, []);
      sources.get(s.id).push(i);
    });
    let depth = rowBottom + 2 * TRACK;
    for (const [sid, list] of sources) {
      const targets = new Set(list.map((i) => edges[i].to.node));
      if (targets.size < 3) continue;
      const src = byId.get(sid);
      // Nets (one source pin) into each chip, the nearest chip first, then the upper pin.
      const nets = new Map();
      for (const i of list) {
        const e = edges[i];
        const key = `${e.from.port}\n${e.to.node}`;
        if (!nets.has(key)) nets.set(key, { port: e.from.port, chip: e.to.node, wires: [] });
        nets.get(key).wires.push(i);
      }
      const ordered = [...nets.values()].map((n) => ({ ...n, x: byId.get(n.chip).x, y: Math.min(...n.wires.map((i) => pin(n.chip, edges[i].to.port, false).y)) }));
      ordered.sort((a, b) => a.x - b.x || a.y - b.y);
      // The row's first chip, right beside its source, is joined straight (the maze's): the bus serves the others.
      const first = ordered[0]?.chip;
      if (ordered.length > 1 && byId.get(first).x - (src.x + src.w) < 4 * STUB) ordered.splice(0, ordered.filter((n) => n.chip === first).length);
      // The drops share the room between the source and the first riser: TRACK apart, closer when it is narrow.
      const room = Math.min(...row.map((n) => n.x)) - STUB / 2 - (src.x + src.w + STUB / 2);
      const pitch = Math.max(3, Math.min(TRACK, Math.floor(room / (ordered.length + 1))));
      const top = Math.max(depth, src.y + src.h + 2 * TRACK);
      const perChip = new Map();
      for (const n of ordered) perChip.set(n.chip, [...(perChip.get(n.chip) ?? []), n]);
      ordered.forEach((n, k) => {
        const trackY = top + k * TRACK;
        const a = pin(sid, n.port, true);
        const dropX = src.x + src.w + STUB / 2 + (ordered.length - k) * pitch;
        const mine = perChip.get(n.chip);
        const riserX = byId.get(n.chip).x - STUB - (mine.length - 1 - mine.indexOf(n)) * TRACK;
        for (const i of n.wires) {
          const b = pin(n.chip, edges[i].to.port, false);
          const corners = simplify([a, { x: dropX, y: a.y }, { x: dropX, y: trackY }, { x: riserX, y: trackY }, { x: riserX, y: b.y }, b]);
          if (clear(corners, new Set([sid, n.chip]))) out.set(i, corners);
        }
      });
      depth = top + ordered.length * TRACK + TRACK;
    }
    // The row's outputs into its mixers (chips right of it): the leftmost deepest.
    const right = Math.max(...row.map((n) => n.x + n.w));
    const outsByMixer = new Map();
    edges.forEach((e, i) => {
      const m = byId.get(e.to.node);
      if (!inRow.has(e.from.node) || !m || m.x < right) return;
      if (!outsByMixer.has(m.id)) outsByMixer.set(m.id, []);
      outsByMixer.get(m.id).push(i);
    });
    const outs = [...outsByMixer.values()].flat().sort((p, q) => byId.get(edges[q].from.node).x - byId.get(edges[p].from.node).x || p - q);
    // Shallow (right voices) first: k grows with depth.
    outs.forEach((i, k) => {
      const e = edges[i];
      const a = pin(e.from.node, e.from.port, true);
      const b = pin(e.to.node, e.to.port, false);
      const trackY = depth + k * TRACK;
      const dropX = a.x + STUB;
      // The deeper, the nearer the mixer's edge it rises.
      const sameMixer = outs.filter((j) => edges[j].to.node === e.to.node);
      const riserX = b.x - STUB - (sameMixer.length - 1 - sameMixer.indexOf(i)) * TRACK;
      const corners = simplify([a, { x: dropX, y: a.y }, { x: dropX, y: trackY }, { x: riserX, y: trackY }, { x: riserX, y: b.y }, b]);
      if (clear(corners, new Set([e.from.node, e.to.node]))) out.set(i, corners);
    });
  }
  return out;
}

/** A card's margins beside the plate are at most this wide. */
const MARGIN_W = 960;
/** A section reaches this far past what lies in it. */
const SECTION_PAD = 16;

/**
 * Chip `node` at the free spot nearest `target` (whole plate units, a
 * fixed spiral) that lies wholly in one of `windows` (`[{x, y, w, h}]`),
 * `free(box)` saying whether a spot is free: `{x, y}`, or null.
 */
function placeIn(node, target, windows, free) {
  const x0 = Math.round(target.x / PU) * PU;
  const y0 = Math.round(target.y / PU) * PU;
  windows = windows.filter((w) => w.w >= node.w && w.h >= node.h);
  if (!windows.length) return null;
  const fits = (b) => windows.some((w) => b.x >= w.x && b.y >= w.y && b.x + b.w <= w.x + w.w && b.y + b.h <= w.y + w.h);
  // A row moves more than a column: the cost dx² + 4·dy², the cheapest free spot of the rings up to twice the first's.
  let best = null;
  // No farther than the windows reach.
  let last = 0;
  for (const w of windows) last = Math.max(last, Math.abs(w.x - x0), Math.abs(w.x + w.w - x0), Math.abs(w.y - y0), Math.abs(w.y + w.h - y0));
  // The spiral steps two plate units: a spot a unit off is no better, and the search is four times shorter.
  const step = 2 * PU;
  last = Math.min(80, Math.ceil(last / step));
  for (let r = 0; r <= last; r++) {
    // A ring's cheapest cell costs r² (along x): past the best found, nothing nearer is left (the result is the same, sooner).
    if (best && r * r >= best.cost) break;
    for (const [dx, dy] of ring(r)) {
      const cost = dx * dx + 4 * dy * dy;
      if (best && cost >= best.cost) continue;
      const box = { x: x0 + dx * step, y: y0 + dy * step, w: node.w, h: node.h };
      if (!fits(box) || !free(box)) continue;
      best = { cost, x: box.x, y: box.y };
      last = Math.min(last, 2 * r + 1);
    }
  }
  return best && { x: best.x, y: best.y };
}

/** The card (`{x, y, w, h}`) holding the middle of `box`, or null. */
function cardOf(cards, box) {
  const cx = box.x + box.w / 2;
  const cy = box.y + box.h / 2;
  return cards.find((c) => cx >= c.x && cx <= c.x + c.w && cy >= c.y && cy <= c.y + c.h) ?? null;
}

/**
 * Chip `node` placed under its vias (`rects`, the controls' rectangles):
 * first inside their card below their bounding box, then just below the
 * card, each time centred under their middle and pushed sideways only as
 * far as it must; else the nearest free spot of the spiral. `free(box)`
 * says whether a spot is free. `{x, y}`.
 */
function underVias(node, rects, cards, free, controlRects = rects) {
  const x0 = Math.min(...rects.map((r) => r.x));
  const x1 = Math.max(...rects.map((r) => r.x + r.w));
  const y1 = Math.max(...rects.map((r) => r.y + r.h));
  const cx = rects.reduce((s, r) => s + r.x + r.w / 2, 0) / rects.length;
  const card = cardOf(cards, { x: x0, y: y1 - 1, w: x1 - x0, h: 1 });
  const snap = (v) => Math.round(v / PU) * PU;
  const scan = (yFrom, yTo, xLo, xHi, reachX) => {
    for (let y = snap(yFrom); y <= yTo; y += PU) {
      for (let k = 0; k <= reachX; k++) {
        for (const sign of k ? [1, -1] : [1]) {
          const x = snap(cx - node.w / 2) + sign * k * PU;
          if (x < xLo || x + node.w > xHi) continue;
          if (free({ x, y, w: node.w, h: node.h })) return { x, y };
        }
      }
    }
    return null;
  };
  if (card) {
    // (a) In its card, below all its card's controls (the card's chips in one row), pushed sideways a little at most.
    let floor = y1;
    for (const r of controlRects) if (cardOf([card], r) && r.y + r.h > floor) floor = r.y + r.h;
    // The nearest free spot to its place in the row (right under its vias), in a window: beside the row (past the plate's edge too) before a row lower.
    const x0w = snap(cx - node.w / 2);
    const y0w = snap(floor + combGap(node));
    const cands = [];
    for (let dy = 0; dy <= 40; dy++) {
      if (y0w + dy * PU + node.h > card.y + card.h - CARD_PAD) break;
      for (let dx = -40; dx <= 40; dx++) cands.push([dx * dx + 4 * dy * dy, dy, dx]);
    }
    cands.sort((p, q) => p[0] - q[0] || p[1] - q[1] || p[2] - q[2]);
    for (const [, dy, dx] of cands) {
      const at = { x: x0w + dx * PU, y: y0w + dy * PU };
      if (free({ ...at, w: node.w, h: node.h })) return at;
    }
    // (b) Just below its card.
    const below = scan(card.y + card.h + combGap(node) / 2, card.y + card.h + 640, -Infinity, Infinity, 24);
    if (below) return below;
  } else {
    const below = scan(y1 + combGap(node), y1 + 640, -Infinity, Infinity, 24);
    if (below) return below;
  }
  return null;
}

/**
 * The board over a plate (ADR 0047 §5–6, the coordinator's design after
 * the owner's verdict of 2026-10-30): each chip its vias bind goes
 * directly under its own controls (`underVias`), its parameters on its top
 * edge (`topPins`); each other chip by flow among its neighbours (the
 * middle of those placed, in the order of the layers), at the nearest free
 * spot. Its vias' legs drop through the gap below their row to a collector
 * into the chip's top pins (a comb, by the maze router), the chips' wires
 * run between and around them (the maze router, ordered shortest first);
 * nothing under a control or a chip. Only a fan-out of NET_DESTS or more,
 * a global net or a wire with no way are labels.
 */
function plateBoard({ nodes, byId, edges, back, ranks, panel, legs, controlRects, board, positions, factsOf, widen = () => {} }) {
  topPins(nodes, legs, edges);
  widen();
  const cards = (board.cards ?? []).filter((c) => c.w > 0 && c.h > 0);
  const placed = [];
  // A chip keeps room around it for the wires: from a control CONTROL_ROOM, from another chip CHIP_ROOM.
  // The controls in buckets of BUCKET units, so a spot asks only those near it (the same answer, sooner: the spiral asks thousands).
  const BUCKET = 128;
  const buckets = new Map();
  controlRects.forEach((o, i) => {
    for (let bx = Math.floor((o.x - CONTROL_ROOM) / BUCKET); bx <= Math.floor((o.x + o.w + CONTROL_ROOM) / BUCKET); bx++) {
      for (let by = Math.floor((o.y - CONTROL_ROOM) / BUCKET); by <= Math.floor((o.y + o.h + CONTROL_ROOM) / BUCKET); by++) {
        const key = `${bx},${by}`;
        if (!buckets.has(key)) buckets.set(key, []);
        buckets.get(key).push(i);
      }
    }
  });
  const nearControl = (box) => {
    for (let bx = Math.floor(box.x / BUCKET); bx <= Math.floor((box.x + box.w) / BUCKET); bx++) {
      for (let by = Math.floor(box.y / BUCKET); by <= Math.floor((box.y + box.h) / BUCKET); by++) {
        for (const i of buckets.get(`${bx},${by}`) ?? []) if (near(box, controlRects[i], CONTROL_ROOM)) return true;
      }
    }
    return false;
  };
  const free = (box) => !nearControl(box) && !placed.some((o) => near(box, o, CHIP_ROOM));
  // The same rooms for the spiral (which keeps CLEARANCE): the obstacles grown by the difference.
  const grow = (r, d) => ({ x: r.x - d, y: r.y - d, w: r.w + 2 * d, h: r.h + 2 * d });
  const roomy = () => [...controlRects.map((r) => grow(r, CONTROL_ROOM - CLEARANCE)), ...placed.map((r) => grow(r, CHIP_ROOM - CLEARANCE))];
  const rectsOf = new Map();
  for (const l of legs) {
    if (!rectsOf.has(l.chip)) rectsOf.set(l.chip, []);
    rectsOf.get(l.chip).push(l.control.control.rect);
  }
  const plateW = board.plate?.w ?? Math.max(0, ...controlRects.map((r) => r.x + r.w));
  const plateH = board.plate?.h ?? Math.max(0, ...controlRects.map((r) => r.y + r.h));
  const done = new Set();
  // Stored positions first (moved off a control or chip they would cover).
  for (const node of nodes) {
    const stored = storedAt(node, positions);
    node.pinned = Boolean(stored);
    if (!stored) continue;
    let at = { x: stored.x, y: stored.y };
    if (!free({ ...at, w: node.w, h: node.h })) at = placeNear(node, at, roomy()) ?? at;
    Object.assign(node, at);
    placed.push(rectOf(node));
    done.add(node.id);
  }
  // ADR 0047, the owner's verdict on 7a62595: each card is a section of the board; its chips (those its
  // controls bind, and those whose flow neighbours mostly are its) lie in it, or in its margin right of the
  // plate at its height when the card has no room; only wires between cards leave a card.
  const neighbours = new Map(nodes.map((n) => [n.id, []]));
  for (const e of edges) {
    neighbours.get(e.from.node)?.push({ id: e.to.node, after: true });
    neighbours.get(e.to.node)?.push({ id: e.from.node, after: false });
  }
  const cardIndex = new Map();
  for (const [id, rects] of rectsOf) {
    const votes = new Map();
    for (const r of rects) {
      const c = cards.indexOf(cardOf(cards, r));
      if (c >= 0) votes.set(c, (votes.get(c) ?? 0) + 1);
    }
    const best = [...votes].sort((a, b) => b[1] - a[1] || a[0] - b[0])[0];
    if (best) cardIndex.set(id, best[0]);
  }
  // The end of the flow (a chain of single links ending at a chip with no output: bus → dc → limiter → out) takes its end's card.
  const srcs = new Map(nodes.map((n) => [n.id, new Set()]));
  const snks = new Map(nodes.map((n) => [n.id, new Set()]));
  for (const e of edges) {
    if (e.from.node === e.to.node) continue;
    srcs.get(e.to.node)?.add(e.from.node);
    snks.get(e.from.node)?.add(e.to.node);
  }
  const chains = [];
  for (const end of nodes) {
    if (end.kind !== 'instance' || snks.get(end.id).size || !srcs.get(end.id).size) continue;
    const chain = [end];
    for (let cur = end; srcs.get(cur.id).size === 1; ) {
      const prev = byId.get([...srcs.get(cur.id)][0]);
      if (!prev || snks.get(prev.id).size !== 1 || chain.includes(prev)) break;
      chain.unshift(prev);
      cur = prev;
    }
    if (chain.length < 3) continue;
    chains.push(chain);
    const c = chain.map((n) => cardIndex.get(n.id)).find((v) => v !== undefined);
    if (c !== undefined) for (const n of chain) if (!cardIndex.has(n.id)) cardIndex.set(n.id, c);
  }
  // Every other chip: the card most of its neighbours have (its sinks' on a tie), in the order of the layers, twice.
  const byRank = [...nodes].sort((a, b) => ranks.get(a.id) - ranks.get(b.id) || nodes.indexOf(a) - nodes.indexOf(b));
  for (let pass = 0; pass < 3; pass++) {
    for (const node of byRank) {
      if (cardIndex.has(node.id)) continue;
      const votes = new Map();
      for (const n of neighbours.get(node.id)) {
        const c = cardIndex.get(n.id);
        if (c !== undefined) votes.set(c, (votes.get(c) ?? 0) + (n.after ? 1.01 : 1));
      }
      const best = [...votes].sort((a, b) => b[1] - a[1] || a[0] - b[0])[0];
      if (best) cardIndex.set(node.id, best[0]);
    }
  }
  // A card's windows: its rectangle, then its margin right of the plate at its height.
  const margin = Math.max(plateW, ...controlRects.map((r) => r.x + r.w)) + PLATE_GAP;
  const windowsOf = (c) => {
    if (c === undefined) return null;
    const card = cards[c];
    return [
      { x: card.x + CARD_PAD, y: card.y + CARD_PAD, w: card.w - 2 * CARD_PAD, h: card.h - 2 * CARD_PAD },
      { x: margin, y: card.y + CARD_PAD, w: MARGIN_W, h: card.h - 2 * CARD_PAD },
      { x: -MARGIN_W - PLATE_GAP, y: card.y + CARD_PAD, w: MARGIN_W, h: card.h - 2 * CARD_PAD },
    ];
  };
  const put = (node, target, sides = 3, minX = -Infinity) => {
    const windows = windowsOf(cardIndex.get(node.id))
      ?.slice(0, sides)
      .map((w) => (w.x >= minX ? w : { ...w, x: minX, w: w.x + w.w - minX }))
      .filter((w) => w.w > 0);
    // In its card if it fits anywhere there; else in its margins beside the plate at its card's height.
    const at = (windows && (placeIn(node, target, windows.slice(0, 1), free) ?? placeIn(node, target, windows.slice(1), free))) ?? placeNear(node, target, roomy()) ?? target;
    Object.assign(node, at);
    placed.push(rectOf(node));
    done.add(node.id);
  };
  // Chips under their controls, those with the most vias first (they need the most room), ties in source order:
  // below every control of their card (its chips in one row), centred under their vias.
  const anchored = nodes.filter((n) => rectsOf.has(n.id) && !done.has(n.id));
  anchored.sort((a, b) => rectsOf.get(b.id).length - rectsOf.get(a.id).length || nodes.indexOf(a) - nodes.indexOf(b));
  for (const node of anchored) {
    const rects = rectsOf.get(node.id);
    const cx = rects.reduce((s, r) => s + r.x + r.w / 2, 0) / rects.length;
    const card = cards[cardIndex.get(node.id)];
    let floor = Math.max(...rects.map((r) => r.y + r.h));
    if (card) for (const r of controlRects) if (cardOf([card], r) && r.y + r.h > floor) floor = r.y + r.h;
    put(node, { x: cx - node.w / 2, y: floor + combGap(node) });
  }
  // A row's bus (a sequencer's triggers under its voices) keeps its way free: no other chip there.
  for (const row of rowsOf(nodes.filter((n) => done.has(n.id) && rectsOf.has(n.id) && n.kind === 'instance'), edges)) {
    const inRow = new Set(row.map((n) => n.id));
    const left = Math.min(...row.map((n) => n.x));
    for (const src of nodes.filter((n) => done.has(n.id) && !inRow.has(n.id) && n.x + n.w <= left)) {
      const fed = new Set(edges.filter((e) => e.from.node === src.id && inRow.has(e.to.node)).map((e) => e.to.node));
      if (fed.size < 3) continue;
      const nets = edges.filter((e) => e.from.node === src.id && inRow.has(e.to.node)).length;
      const top = Math.min(src.y, ...row.map((n) => n.y));
      const bottom = Math.max(src.y + src.h, ...row.map((n) => n.y + n.h)) + 2 * TRACK + (nets + 2) * TRACK;
      const zones = [
        { x: src.x + src.w, y: top, w: left - src.x - src.w, h: bottom - top },
        { x: src.x, y: Math.max(src.y + src.h, ...row.map((n) => n.y + n.h)), w: Math.max(...row.map((n) => n.x + n.w)) - src.x, h: bottom - Math.max(src.y + src.h, ...row.map((n) => n.y + n.h)) },
      ];
      // A chip already there (placed after the row's) moves out of the way.
      const inWay = nodes.filter((n) => done.has(n.id) && !inRow.has(n.id) && n !== src && !n.pinned && zones.some((z) => near(n, z, 0)));
      for (const n of inWay) {
        const k = placed.findIndex((r) => r.x === n.x && r.y === n.y && r.w === n.w && r.h === n.h);
        if (k >= 0) placed.splice(k, 1);
      }
      placed.push(...zones);
      for (const n of inWay) {
        const rects = rectsOf.get(n.id) ?? [];
        const cx = rects.length ? rects.reduce((sum, r) => sum + r.x + r.w / 2, 0) / rects.length : n.x + n.w / 2;
        put(n, { x: cx - n.w / 2, y: n.y });
      }
    }
  }
  // The others by flow, in the order of the layers: beside what they feed when that is placed, else below what feeds them.
  const rest = byRank.filter((n) => !done.has(n.id));
  // From the end of the flow back first: a chip whose every sink is placed goes just left of them.
  for (const node of [...rest].reverse()) {
    if (done.has(node.id) || chains.some((c) => c.includes(node))) continue;
    const sinks = neighbours.get(node.id).filter((n) => n.after);
    if (!sinks.length || !sinks.every((n) => done.has(n.id))) continue;
    const ns = sinks.map((n) => byId.get(n.id));
    put(node, { x: Math.min(...ns.map((n) => n.x)) - 2 * STUB - CHIP_ROOM - node.w, y: ns.reduce((sum, n) => sum + n.y + n.h / 2, 0) / ns.length - node.h / 2 });
  }
  for (let pass = 0; pass < 2; pass++) {
    for (const node of rest) {
      if (done.has(node.id) || chains.some((c) => c.includes(node))) continue;
      const near1 = neighbours.get(node.id).map((n) => ({ ...n, node: byId.get(n.id) })).filter((n) => done.has(n.id));
      if (!near1.length && pass === 0) continue;
      let at;
      if (near1.length) {
        const cx = near1.reduce((s, n) => s + n.node.x + n.node.w / 2, 0) / near1.length;
        const cy = near1.reduce((s, n) => s + n.node.y + n.node.h / 2, 0) / near1.length;
        const lo = Math.max(-Infinity, ...near1.filter((n) => !n.after).map((n) => n.node.x + n.node.w + 2 * STUB));
        const hi = Math.min(Infinity, ...near1.filter((n) => n.after).map((n) => n.node.x - 2 * STUB - node.w));
        let x = cx - node.w / 2;
        if (Number.isFinite(lo)) x = Math.max(x, lo);
        if (Number.isFinite(hi)) x = Math.min(x, hi);
        const sinks = near1.filter((n) => n.after);
        const sources = near1.filter((n) => !n.after);
        if (sinks.length) at = { x, y: sinks.reduce((s, n) => s + n.node.y + n.node.h / 2, 0) / sinks.length - node.h / 2 };
        else if (sources.length > 1) at = { x: cx - node.w / 2, y: Math.max(...sources.map((n) => n.node.y + n.node.h)) + CHIP_ROOM + 12 * TRACK };
        else at = { x, y: cy - node.h / 2 };
      } else at = { x: margin, y: plateH / 2 };
      put(node, at);
    }
  }
  // The chains: one row, left to right, from their first placed control chip's place (in their card, else beside the plate).
  for (const chain of chains) {
    const anchoredIn = chain.filter((n) => done.has(n.id));
    for (let k = placed.length - 1; k >= 0; k--) {
      if (chain.some((n) => done.has(n.id) && n.x === placed[k].x && n.y === placed[k].y && n.w === placed[k].w && n.h === placed[k].h)) placed.splice(k, 1);
    }
    const card = cards[cardIndex.get(chain[0].id)];
    // In its card where its controls' chips are, else beside the plate on the right: never left of it (the flow runs right).
    const inCard = anchoredIn.filter((n) => n.x >= 0 && n.x + n.w <= plateW);
    let x = inCard.length ? Math.min(...inCard.map((n) => n.x)) : margin;
    const y = anchoredIn.length ? Math.max(...anchoredIn.map((n) => n.y)) : card ? card.y + CARD_PAD : plateH;
    let minX = -Infinity;
    for (const node of chain) {
      put(node, { x, y }, 2, minX);
      x = node.x + node.w + CHIP_ROOM + 2 * STUB;
      minX = x - 2 * STUB;
    }
  }
  // The board's sections: each card with what lies in it (ADR 0047: drawn on the Board, its schematic inside).
  const sections = cards.map((card, c) => {
    let [x0, y0, x1, y1] = [card.x, card.y, card.x + card.w, card.y + card.h];
    for (const node of nodes) {
      if (cardIndex.get(node.id) !== c) continue;
      x0 = Math.min(x0, node.x - SECTION_PAD);
      y0 = Math.min(y0, node.y - SECTION_PAD);
      x1 = Math.max(x1, node.x + node.w + SECTION_PAD);
      y1 = Math.max(y1, node.y + node.h + SECTION_PAD);
    }
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0, title: card.title ?? '', card: { x: card.x, y: card.y, w: card.w, h: card.h } };
  });
  for (const node of nodes) node.layer = ranks.get(node.id);
  // Each chip's side pins in the order of where their other ends lie (twice: the other ends move too).
  for (let pass = 0; pass < 2; pass++) {
    for (const node of nodes) {
      if (node.kind !== 'instance' || node.groups || node.slot || node.face) continue;
      const key = (side, p) => {
        const ends = edges
          .filter((e) => (side === 'ins' ? e.to.node === node.id && e.to.port === p.name : e.from.node === node.id && e.from.port === p.name))
          .map((e) => (side === 'ins' ? anchor(byId.get(e.from.node), e.from.port, true) : anchor(byId.get(e.to.node), e.to.port, false)));
        // Outputs by how far right their ends lie (the nearest on top: the farther run deeper), then by height; inputs by their sources' height.
        return ends.length ? ends.reduce((s, q) => s + (side === 'outs' ? Math.round(q.x / 64) * 1e4 + q.y : q.y + q.x * 0.01), 0) / ends.length : Infinity;
      };
      for (const side of ['ins', 'outs']) {
        const top = node[side].filter((p) => side === 'ins' && node.topOrder?.has(p.name));
        const rest = node[side].filter((p) => !top.includes(p));
        const keyed = rest.map((p, i) => ({ p, i, k: key(side, p) }));
        keyed.sort((a, b) => a.k - b.k || a.i - b.i);
        node[side] = [...keyed.map((k) => k.p), ...top];
      }
    }
  }
  for (const c of panel) {
    nodes.push(c);
    byId.set(c.id, c);
  }
  for (const node of nodes) {
    node.ins = node.ins.map((p) => ({ ...p, ...anchor(node, p.name, false) }));
    node.outs = node.outs.map((p) => ({ ...p, ...anchor(node, p.name, true) }));
  }
  const bottom = levelBottom(nodes);
  // A source feeding GLOBAL_FANOUT chips or more (the transport's clock): its nets are global, drawn as labels.
  const fed = new Map();
  const hasInput = new Set(edges.map((e) => e.to.node));
  for (const e of edges) {
    if (!fed.has(e.from.node)) fed.set(e.from.node, new Set());
    fed.get(e.from.node).add(e.to.node);
  }
  const nets = new Map();
  for (const e of edges) nets.set(`${e.from.node}\n${e.from.port}`, (nets.get(`${e.from.node}\n${e.from.port}`) ?? 0) + 1);
  // ADR 0047 phase 4–5: a fan-out and a global net are wires as any other (routed last, the longest), drawn as ribbons where they run together.
  const late = (e) => {
    const net = `${e.from.node}\n${e.from.port}`;
    return nets.get(net) >= NET_DESTS || (!hasInput.has(e.from.node) && (fed.get(e.from.node)?.size ?? 0) >= GLOBAL_FANOUT);
  };
  const sectionOf = (id) => (cardIndex.has(id) ? sections[cardIndex.get(id)] : null);
  // The maze over everything: chips, controls, pads; every pin's approach its own.
  const chips = nodes.filter((n) => n.kind !== 'control');
  const all = [...chips.map(({ id, x, y, w, h }) => ({ id, x, y, w, h })), ...controlRects.map((r, k) => ({ id: `@plate:${k}`, ...r })), ...panel.map(({ id, x, y, w, h }) => ({ id, x, y, w, h }))];
  const pinsAll = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'left' })), ...n.outs.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'right' }))]);
  const xsAll = all.flatMap((b) => [b.x, b.x + b.w]);
  const ysAll = all.flatMap((b) => [b.y, b.y + b.h]);
  const bounds = { x0: least(xsAll, 0) - MAZE_MARGIN, x1: most(xsAll, 0) + MAZE_MARGIN, y0: least(ysAll, 0) - MAZE_MARGIN, y1: most(ysAll, 0) + MAZE_MARGIN };
  const maze = new Maze(all, [], [], bounds, pinsAll);
  const pinOf = (node, port, out) => (out ? node.outs : node.ins).find((p) => p.name === port);
  const labels = [];
  const wires = [];
  const routeOf = new Map();
  // The legs first (the combs: short, under their own controls), chip by chip, each chip's in its pins' order.
  const legOrder = legs.map((l, i) => ({ l, i }));
  legOrder.sort((p, q) => nodes.indexOf(byId.get(p.l.chip)) - nodes.indexOf(byId.get(q.l.chip)) || (byId.get(p.l.chip).topOrder?.get(p.l.pin.port) ?? 0) - (byId.get(q.l.chip).topOrder?.get(q.l.pin.port) ?? 0) || p.i - q.i);
  for (const { l, i } of legOrder) {
    const a = pinOf(byId.get(l.from.node), l.from.port, true);
    const b = pinOf(byId.get(l.to.node), l.to.port, false);
    const own = [l.control.control.rect, rectOf(l.control)];
    const share = l.writes ? edges.filter((e) => e.to.node === l.chip && e.to.port === l.pin.port).map((e) => `${e.from.node}\n${e.from.port}`) : [`${l.chip}\n${l.pin.port}`];
    routeOf.set(`leg${i}`, maze.route(a, b, `leg:${l.control.id}:${l.pin.name}`, { bus: `bus:${l.chip}`, own, share, from: a.side ?? 'right', into: b.side ?? 'left', within: sectionOf(l.chip) }));
  }
  // Rows: three or more chips of vias side by side (a drum machine's voices).
  const rows = rowsOf(nodes.filter((n) => n.kind === 'instance' && rectsOf.has(n.id)), edges);
  // The rows' buses (ADR 0047, the coordinator's polish): a sequencer's triggers into a row, the row's outputs into its mixers, ordered so neither crosses itself.
  for (const [i, corners] of rowBuses(rows, edges, byId, all)) {
    routeOf.set(`edge${i}`, corners);
    const e = edges[i];
    const id = maze.netId(`${e.from.node}\n${e.from.port}`);
    for (let k = 1; k < corners.length; k++) maze.takeSegment(corners[k - 1], corners[k], id);
  }
  // Then the chips' wires, shortest first; a fan-out or global net as labels.
  const order = edges.map((e, i) => {
    const a = pinOf(byId.get(e.from.node), e.from.port, true);
    const b = pinOf(byId.get(e.to.node), e.to.port, false);
    return { e, i, a, b, d: Math.abs(a.x - b.x) + Math.abs(a.y - b.y) };
  });
  const labelled = () => false;
  order.sort((p, q) => late(p.e) - late(q.e) || p.d - q.d || p.i - q.i);
  for (const { e, i, a, b } of order) {
    if (labelled(e) || routeOf.has(`edge${i}`)) continue;
    // The legs at its own pins (a meter reading its source, a control setting its target) it may join there.
    const share = legs.filter((l) => (!l.writes && l.chip === e.from.node && l.pin.port === e.from.port) || (l.writes && l.chip === e.to.node && l.pin.port === e.to.port)).map((l) => `leg:${l.control.id}:${l.pin.name}`);
    // A wire inside one card stays in its section; only a wire between cards leaves one.
    const within = cardIndex.get(e.from.node) === cardIndex.get(e.to.node) ? sectionOf(e.from.node) : null;
    routeOf.set(`edge${i}`, maze.route(a, b, `${e.from.node}\n${e.from.port}`, { from: a.side ?? 'right', into: b.side ?? 'left', share, within }));
  }
  const sourced = new Set();
  const labelFor = (id, net, text, a, b, intoSide, outSide) => {
    const into = { left: [-LABEL_STUB, 0], top: [0, -LABEL_STUB], bottom: [0, LABEL_STUB], right: [LABEL_STUB, 0] }[intoSide];
    const at = { x: b.x + into[0], y: b.y + into[1] };
    labels.push({ net, text: `◂ ${text}`, side: 'in', at, corners: [at, b], wire: id });
    if (!sourced.has(net)) {
      sourced.add(net);
      const out = { left: [-LABEL_STUB, 0], top: [0, -LABEL_STUB], bottom: [0, LABEL_STUB], right: [LABEL_STUB, 0] }[outSide];
      const end = { x: a.x + out[0], y: a.y + out[1] };
      labels.push({ net, text: `▸ ${text}`, side: 'out', at: end, corners: [a, end], wire: id });
    }
    return [at, b];
  };
  edges.forEach((e, i) => {
    const a = pinOf(byId.get(e.from.node), e.from.port, true);
    const b = pinOf(byId.get(e.to.node), e.to.port, false);
    const corners = routeOf.get(`edge${i}`) ?? null;
    const net = `${e.from.node}\n${e.from.port}`;
    const r = corners ? { corners, labelled: false } : { corners: labelFor(i, net, String(e.written?.from ?? `${e.from.node}.${e.from.port}`).replace(/^\//, ''), a, b, b.side ?? 'left', a.side ?? 'right'), labelled: true, whole: zCorners(a, b) };
    wires.push(boardWire(i, e, { feedback: back.has(i), lane: 0, under: null, a, b }, r));
  });
  legs.forEach((l, k) => {
    const id = edges.length + k;
    const a = pinOf(byId.get(l.from.node), l.from.port, true);
    const b = pinOf(byId.get(l.to.node), l.to.port, false);
    const e = { from: l.from, to: l.to, kind: 'binding', shape: l.pin.shape, converters: [], scale: null, label: `${l.control.name} ⇢ ${l.pin.address}`, written: { from: `${l.control.id}.${l.pin.name}`, to: l.pin.address, binding: true } };
    const corners = routeOf.get(`leg${k}`) ?? null;
    const r = corners ? { corners, labelled: false } : { corners: labelFor(id, `leg:${l.control.id}:${l.pin.name}`, l.control.name, a, b, b.side ?? 'left', a.side ?? 'right'), labelled: true, whole: zCorners(a, b) };
    wires.push(boardWire(id, e, { feedback: false, lane: 0, under: null, a, b }, r));
  });
  return { wires, labels, bottom, sections };
}

/** A board's wire: its edge `e`, its routing ask `w`, its route `r`. */
function boardWire(id, e, w, r) {
  const vertical = r.corners.find((p, k) => k > 0 && r.corners[k - 1].x === p.x && r.corners[k - 1].y !== p.y);
  const wire = {
    id,
    from: e.from,
    to: e.to,
    written: e.written,
    kind: e.kind,
    shape: e.shape,
    converters: e.converters,
    scale: e.scale,
    dashed: e.converters.length > 0,
    feedback: w.feedback,
    reversed: w.feedback,
    label: e.label,
    lane: w.lane,
    under: w.under,
    orth: true,
    ends: { a: { x: w.a.x, y: w.a.y }, b: { x: w.b.x, y: w.b.y } },
    trunkX: vertical?.x ?? null,
    labelled: Boolean(r.labelled),
    corners: r.corners,
    ...orthGeometry(r.corners),
  };
  // Drawn whole while traced or selected (the same id: its element is built again).
  if (r.labelled) wire.whole = { ...wire, labelled: false, whole: true, corners: r.whole, ...orthGeometry(r.whole) };
  return wire;
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
    // ADR 0047 phase 4: a board's wire is no feedback loop of the curved layout's lanes: one not attached stays as it is
    // (the owner: "only the moved element's own wires may change"); one attached keeps its route, stretched at its moved end.
    if (wire.orth) {
      if (!attached) continue;
      const da = moves.get(wire.from.node) ?? null;
      const db = moves.get(wire.to.node) ?? null;
      const corners = stretchCorners(wire.corners, da, db);
      const a = corners[0];
      const b = corners.at(-1);
      changed.push({ wire, a, b, feedback: false, under: null, orth: true, corners, key: `${a.x},${a.y},${b.x},${b.y}` });
      continue;
    }
    const a = shift(wire.ends?.a ?? wire.curve.start, wire.from.node);
    const b = shift(wire.ends?.b ?? wire.curve.segs.at(-1).to, wire.to.node);
    const feedback = wire.reversed || b.x <= a.x;
    if (feedback) lane += 1;
    const under = feedback ? laneY(bottom, lane) : null;
    if (!attached && under === wire.under) continue;
    changed.push({ wire, a, b, feedback, under, orth: Boolean(wire.orth), key: `${a.x},${a.y},${b.x},${b.y},${under}` });
  }
  return changed;
}

/** The geometry of a route `movedWires` gives: `{curve, points, box}`. */
export function routed(route, corner = undefined) {
  // A board's wire (ADR 0047 §6, phase 4) follows a dragged box with its route stretched (`stretchCorners`); the release reroutes only it.
  if (route.orth) return orthGeometry(route.corners ?? zCorners(route.a, route.b, { feedback: route.feedback, under: route.under ?? 0 }), corner);
  return wireGeometry(route.a, route.b, { feedback: route.feedback, under: route.under ?? 0 });
}

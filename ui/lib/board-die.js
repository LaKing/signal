// A die fitted into its package (ADR 0047 §8, phase 3, after the
// coordinator's check of acd08f9): no bond wires and no interface column.
// The inside's chips keep their places (its layout, as stored and edited);
// they are scaled to fill the package's body (`link`: die → package's
// level), and the inside's interface pins are moved onto the package's own
// edges, at the very points of the package's pins: an input on the left
// edge, a parameter set from above on the top edge, an output on the right.
// The wires from and to them are routed again from there, orthogonally, by
// the board's maze around the chips and the die's other wires; one that
// finds no way is drawn as labels, as on every board. Per package (its pins
// differ per instance), cached. Deterministic. No DOM.

import { INPUTS, OUTPUTS, dieArea } from './schematic-layout.js';
import { Maze } from './board-maze.js';
import { LABEL_STUB, orthGeometry, simplify } from './board-route.js';

/** The die's chips keep this far (die units) from the package's body edges, room for the wires from its edge pins. */
export const DIE_MARGIN = 48;
/** The maze searches this many states per wire from an edge pin. */
const EDGE_STATES = 12000;

const fitted = new WeakMap();

const isFrame = (n) => n.id === INPUTS || n.id === OUTPUTS;

/**
 * The die `die` (an inside's board layout) fitted into package `node` (its
 * box in its parent's layout): `{layout, link}`, `link` the transform die →
 * parent (`{k, x, y}`), `layout` the die with its interface on the
 * package's edges (`edge` frame nodes of no width) and those wires routed
 * again. The same object for the same node and die.
 */
export function fitDie(node, die) {
  const cached = fitted.get(node);
  if (cached?.die === die) return cached.result;
  const result = fit(node, die);
  fitted.set(node, { die, result });
  return result;
}

function fit(node, die) {
  const chips = die.nodes.filter((n) => !isFrame(n));
  const frameWire = (w) => w.from.node === INPUTS || w.to.node === OUTPUTS;
  // The extent of what stays: the chips and the wires between them.
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const n of chips) {
    x0 = Math.min(x0, n.x);
    y0 = Math.min(y0, n.y);
    x1 = Math.max(x1, n.x + n.w);
    y1 = Math.max(y1, n.y + n.h);
  }
  for (const w of die.wires) {
    if (frameWire(w) || w.labelled) continue;
    x0 = Math.min(x0, w.box.x0);
    y0 = Math.min(y0, w.box.y0);
    x1 = Math.max(x1, w.box.x1);
    y1 = Math.max(y1, w.box.y1);
  }
  if (!Number.isFinite(x0)) [x0, y0, x1, y1] = [0, 0, 1, 1];
  const area = dieArea(node);
  const ew = x1 - x0 + 2 * DIE_MARGIN;
  const eh = y1 - y0 + 2 * DIE_MARGIN;
  const k = Math.min(area.w / ew, area.h / eh);
  const link = { k, x: area.x + (area.w - ew * k) / 2 - (x0 - DIE_MARGIN) * k, y: area.y + (area.h - eh * k) / 2 - (y0 - DIE_MARGIN) * k };
  const local = (p) => ({ x: (p.x - link.x) / k, y: (p.y - link.y) / k });
  // The package's edges in the die's units.
  const left = local({ x: node.x, y: 0 }).x;
  const right = local({ x: node.x + node.w, y: 0 }).x;
  const top = local({ x: 0, y: node.y }).y;
  const bottom = local({ x: 0, y: node.y + node.h }).y;
  const outer = new Map([...node.ins.map((p) => [p.name, p]), ...node.outs.map((p) => [`out:${p.name}`, p])]);
  // Each interface pin at its package pin (a pin the package does not show: along its edge, in order).
  const place = (pins, out) =>
    pins.map((p, i) => {
      const q = outer.get(out ? `out:${p.name}` : p.name);
      if (!q) {
        const y = top + ((bottom - top) * (i + 1)) / (pins.length + 1);
        return { ...p, x: out ? right : left, y, side: out ? 'left' : 'right' };
      }
      const at = local(q);
      if (q.side === 'top') return { ...p, x: at.x, y: top, side: 'bottom' };
      return { ...p, x: out ? right : left, y: at.y, side: out ? 'left' : 'right' };
    });
  const nodes = die.nodes.map((n) => {
    if (n.id === INPUTS) return { ...n, x: left, y: top, w: 0, h: bottom - top, outs: place(n.outs, false), edge: true };
    if (n.id === OUTPUTS) return { ...n, x: right, y: top, w: 0, h: bottom - top, ins: place(n.ins, true), edge: true };
    return n;
  });
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const pinOf = (id, port, out) => (out ? byId.get(id)?.outs : byId.get(id)?.ins)?.find((p) => p.name === port) ?? null;
  // The maze over the chips, the die's other wires already drawn.
  const boxes = chips.map(({ id, x, y, w, h }) => ({ id, x, y, w, h }));
  const segments = [];
  const netOf = (w) => `${w.from.node}\n${w.from.port}`;
  for (const w of die.wires) {
    if (frameWire(w) || w.labelled) continue;
    for (let i = 1; i < w.corners.length; i++) segments.push(Object.assign([w.corners[i - 1], w.corners[i]], { net: netOf(w) }));
  }
  const asked = die.wires.filter(frameWire).map((w) => ({ w, a: pinOf(w.from.node, w.from.port, true), b: pinOf(w.to.node, w.to.port, false) }));
  const points = asked.flatMap(({ a, b }) => [a, b]).filter(Boolean);
  const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'left' })), ...n.outs.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'right' }))]);
  const bounds = { x0: Math.min(left, x0) - DIE_MARGIN, x1: Math.max(right, x1) + DIE_MARGIN, y0: Math.min(top, y0) - DIE_MARGIN, y1: Math.max(bottom, y1) + DIE_MARGIN };
  const maze = new Maze(boxes, points, segments, bounds, pins);
  const routes = new Map();
  const order = asked.map((q, i) => ({ ...q, i, d: q.a && q.b ? Math.abs(q.a.x - q.b.x) + Math.abs(q.a.y - q.b.y) : Infinity }));
  order.sort((p, q) => p.d - q.d || p.i - q.i);
  for (const { w, a, b } of order) {
    if (!a || !b) continue;
    const corners = maze.route(a, b, netOf(w), { from: a.side ?? 'right', into: b.side ?? 'left', budget: EDGE_STATES });
    if (corners) routes.set(w.id, simplify(corners));
  }
  // The die's labels but those of the wires routed again; a wire from an edge pin with no way, labelled at its ends.
  const again = new Set(asked.map(({ w }) => w.id));
  const labels = die.board.labels.filter((l) => !again.has(l.wire));
  const sourced = new Set(labels.filter((l) => l.side === 'out').map((l) => l.net));
  const wires = die.wires.map((w) => {
    if (!again.has(w.id)) return w;
    const { a, b } = asked.find((q) => q.w.id === w.id);
    if (!a || !b) return w;
    const corners = routes.get(w.id);
    if (corners) return { ...w, labelled: false, whole: undefined, corners, ends: { a: { x: a.x, y: a.y }, b: { x: b.x, y: b.y } }, ...orthGeometry(corners) };
    const at = { x: b.x - LABEL_STUB, y: b.y };
    const net = netOf(w);
    const text = String(w.written?.from ?? `${w.from.node}.${w.from.port}`).replace(/^\//, '');
    labels.push({ net, text: `◂ ${text}`, side: 'in', at, corners: [at, b], wire: w.id, textAt: { x: at.x - 3, y: at.y + 3 } });
    if (!sourced.has(net)) {
      sourced.add(net);
      const end = { x: a.x + LABEL_STUB, y: a.y };
      labels.push({ net, text: `▸ ${text}`, side: 'out', at: end, corners: [a, end], wire: w.id, textAt: { x: end.x + 3, y: end.y + 3 } });
    }
    const stub = [at, b];
    return { ...w, labelled: true, whole: undefined, corners: stub, ends: { a: { x: a.x, y: a.y }, b: { x: b.x, y: b.y } }, ...orthGeometry(stub) };
  });
  const layout = { ...die, nodes, wires, bounds: { x: left, y: top, w: right - left, h: bottom - top }, board: { ...die.board, labels }, fitted: true };
  return { layout, link };
}

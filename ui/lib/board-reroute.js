// A chip moved on a board (ADR 0047 phase 4; the owner: "when I drag an
// element, everything re-routes; only the moved element's own wires may
// change"). While it is dragged, its wires keep their routes, stretched at
// the moved end (`stretchCorners`: the last two corners ride with the pin,
// the corner before them slides along its segment), so nothing jumps; when
// it is let go, only its own wires (and a wire it now lies over) are routed
// again by the maze, around everything else as it is drawn
// (`rerouteMoved`): every other wire keeps its route exactly, every other
// box its place. This is the session's continuity, view state (rule 4): a
// full layout (a reload, a change of structure) is derived from the files
// alone, deterministically. Pure; no DOM.

import { Maze } from './board-maze.js';
import { stretchCorners } from './board-stretch.js';
import { orthGeometry, simplify } from './board-route.js';
export { stretchCorners };
import { boardMeasures, boardRibbons, mazeNet } from './schematic-layout.js';

/** A rerouted wire's search may visit this many states. */
export const REROUTE_STATES = 12000;
/** The maze's margin around a board without a rectangle of its own. */
const MARGIN = 64;

const overlapsBox = (corners, b) => {
  for (let k = 1; k < corners.length; k++) {
    const p = corners[k - 1];
    const q = corners[k];
    const x0 = Math.min(p.x, q.x);
    const x1 = Math.max(p.x, q.x);
    const y0 = Math.min(p.y, q.y);
    const y1 = Math.max(p.y, q.y);
    if (x0 < b.x + b.w && x1 > b.x && y0 < b.y + b.h && y1 > b.y) return true;
  }
  return false;
};

/**
 * Board layout `old` (`layout`'s, `board` set) with the instances `moves`
 * (Map of node id → its new corner `{x, y}`) moved there: a new layout in
 * which every other node keeps its place and every wire neither attached
 * to a moved node nor now passing under one keeps its route (`corners`)
 * exactly; those are routed again by the maze around everything else (the
 * shortest first, ties by id), else (no way within REROUTE_STATES)
 * stretched. A die keeps its rectangle (its package does not change).
 * Ribbons, labels and measures are made again. Deterministic.
 */
export function rerouteMoved(old, moves) {
  const shift = new Map();
  for (const n of old.nodes) {
    const to = moves.get(n.id);
    if (to && (to.x !== n.x || to.y !== n.y)) shift.set(n.id, { dx: to.x - n.x, dy: to.y - n.y });
  }
  if (!shift.size) return old;
  const move = (p, d) => (d ? { ...p, x: p.x + d.dx, y: p.y + d.dy } : p);
  const nodes = old.nodes.map((n) => {
    const d = shift.get(n.id);
    if (!d) return n;
    return { ...n, x: n.x + d.dx, y: n.y + d.dy, pinned: true, ins: n.ins.map((p) => move(p, d)), outs: n.outs.map((p) => move(p, d)) };
  });
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const movedBoxes = nodes.filter((n) => shift.has(n.id) && n.w > 0 && n.h > 0);
  const own = (w) => shift.has(w.from.node) || shift.has(w.to.node);
  const redo = new Set(old.wires.filter((w) => own(w) || (!w.labelled && movedBoxes.some((b) => overlapsBox(w.corners, b)))).map((w) => w.id));
  const keptWires = old.wires.filter((w) => !redo.has(w.id));
  const obstacles = old.board?.obstacles ?? [];
  const boxes = [...nodes.filter((n) => n.w > 0 && n.h > 0).map(({ id, x, y, w, h }) => ({ id, x, y, w, h })), ...obstacles];
  const segments = [];
  for (const w of keptWires) {
    if (w.labelled) continue;
    const { net } = mazeNet(w);
    for (let k = 1; k < w.corners.length; k++) segments.push(Object.assign([w.corners[k - 1], w.corners[k]], { net }));
  }
  const pinOf = (w, out) => {
    const node = byId.get(out ? w.from.node : w.to.node);
    const port = out ? w.from.port : w.to.port;
    const p = (out ? node?.outs : node?.ins)?.find((q) => q.name === port);
    return p ?? move(out ? w.ends.a : w.ends.b, shift.get(out ? w.from.node : w.to.node));
  };
  const asks = old.wires.filter((w) => redo.has(w.id)).map((w) => ({ w, a: pinOf(w, true), b: pinOf(w, false) }));
  const pins = nodes.flatMap((n) => [...n.ins.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'left' })), ...n.outs.map((p) => ({ x: p.x, y: p.y, side: p.side ?? 'right' }))]);
  const rect = old.board?.die ?? null;
  const xs = boxes.flatMap((b) => [b.x, b.x + b.w]);
  const ys = boxes.flatMap((b) => [b.y, b.y + b.h]);
  const bounds = rect ? { x0: rect.x, x1: rect.x + rect.w, y0: rect.y, y1: rect.y + rect.h } : { x0: Math.min(0, ...xs) - MARGIN, x1: Math.max(0, ...xs) + MARGIN, y0: Math.min(0, ...ys) - MARGIN, y1: Math.max(0, ...ys) + MARGIN };
  const maze = new Maze(boxes, asks.flatMap((q) => [q.a, q.b]), segments, bounds, pins);
  asks.sort((p, q) => Math.abs(p.a.x - p.b.x) + Math.abs(p.a.y - p.b.y) - (Math.abs(q.a.x - q.b.x) + Math.abs(q.a.y - q.b.y)) || p.w.id - q.w.id);
  const routes = new Map();
  for (const { w, a, b } of asks) {
    const { net, bus } = mazeNet(w);
    const ownBoxes = [byId.get(w.from.node), byId.get(w.to.node)].filter((n) => n?.kind === 'control').flatMap((n) => [n.control?.rect, { x: n.x, y: n.y, w: n.w, h: n.h }].filter(Boolean));
    const share = old.wires.filter((o) => o !== w && o.to.node === w.to.node && o.to.port === w.to.port).map((o) => mazeNet(o).net);
    const ask = { bus, own: ownBoxes, share, from: a.side ?? 'right', into: b.side ?? 'left', budget: REROUTE_STATES };
    let corners = maze.route(a, b, net, ask);
    if (corners) corners = simplify(corners);
    else corners = stretchCorners(w.corners, shift.get(w.from.node) ?? null, shift.get(w.to.node) ?? null);
    routes.set(w.id, corners);
  }
  const wires = old.wires.map((w) => {
    if (!redo.has(w.id)) return { ...w };
    const corners = routes.get(w.id);
    const vertical = corners.find((p, k) => k > 0 && corners[k - 1].x === p.x && corners[k - 1].y !== p.y);
    const copy = { ...w, corners, labelled: false, trunkX: vertical?.x ?? null, ends: { a: pinOf(w, true), b: pinOf(w, false) }, ...orthGeometry(corners) };
    delete copy.whole;
    delete copy.display;
    return copy;
  });
  // A kept wire keeps its route; its drawn geometry follows its ribbon, made again (`boardRibbons`).
  const labels = [];
  const ribbons = boardRibbons(nodes, wires, obstacles, labels);
  const metrics = boardMeasures(wires, labels, ribbons);
  let bounds2 = old.bounds;
  if (!rect) {
    const bx = [...nodes.flatMap((n) => [n.x, n.x + n.w]), ...wires.flatMap((w) => [w.box.x0, w.box.x1])];
    const by = [...nodes.flatMap((n) => [n.y, n.y + n.h]), ...wires.flatMap((w) => [w.box.y0, w.box.y1])];
    if (old.board?.plate) {
      bx.push(0, old.board.plate.w);
      by.push(0, old.board.plate.h);
    }
    const x0 = Math.min(...bx);
    const y0 = Math.min(...by);
    bounds2 = { x: x0, y: y0, w: Math.max(...bx) - x0, h: Math.max(...by) - y0 };
  }
  return { ...old, nodes, wires, bounds: bounds2, board: { ...old.board, labels, metrics, ribbons }, rerouted: [...redo].sort((a, b) => a - b) };
}

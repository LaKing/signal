// A composite opened in place (M2.13-3, ADR 0030 §2): its box grows into
// a frame that draws its inside at a readable scale where the box is, and
// the outer wires attached to it are routed to the frame's edge, where the
// links across it (`boundaryLinks`, ui/lib/schematic-scene.js) carry them on
// to the inner pins. The frame's scale (`linkScale`) depends on the layouts
// only, never on the view, so a level and its open inside stay one exact
// affine link (`compose`, `uncompose`). A composite opens when its inside
// would be drawn at OPEN_K or more on screen and closes below CLOSE_K; at
// most one is open per level (`chooseOpen`), as grown frames would cover
// each other. Positions are never changed: the opened copy of a level
// (`openedLayout`) is drawn, its other boxes moved out of the frame's way
// (`makeRoom`), the stored layout is what is moved. No DOM.

import { CHAR, GAP_Y, HEADER, PAD, laneY, levelBottom } from './schematic-layout.js';
import { wireGeometry } from './schematic-wire.js';
import { fit, focus, zoomAt } from './schematic-view.js';

/** An inside opens when drawn at this scale on screen or more (Enter's readability, about 0.45 for the studio's synth), and closes below CLOSE_K. */
export const OPEN_K = 0.45;
/** More than a wheel tick (×1.22) below OPEN_K (2026-10-20, the operator's review: at 0.375 one tick out closed it again). */
export const CLOSE_K = OPEN_K / 1.6;
/**
 * Enter, a double-click and a breadcrumb show an inside at this scale at
 * least: a margin above OPEN_K, never on the threshold itself (two wheel
 * ticks out still keep it open, CLOSE_K being well below). Small: at 1.25
 * times OPEN_K the studio's synth no longer fitted the view.
 */
export const READ_K = OPEN_K * 1.05;
/** An open inside is drawn at this scale of its parent's units, */
export const FRAME_K = 0.4;
/** unless its frame would then be more than this many times its box (or it fits its box larger already). */
export const GROW_MAX = 8;

/** The scale at which `bounds` fits inside `node`'s box, below its header (as `inside`). */
function fitScale(node, bounds) {
  const w = Math.max(1, node.w - 2 * PAD);
  const h = Math.max(1, node.h - HEADER - PAD);
  return Math.min(w / Math.max(bounds.w, 1e-9), h / Math.max(bounds.h, 1e-9));
}

const grownOf = new WeakMap();

/**
 * `node` (a composite's box) grown into the frame of its inside of layout
 * `bounds`, where it is (its inside centred on the box's centre): big enough to hold the inside at FRAME_K of
 * its units (at most GROW_MAX times the scale its box holds it at, never
 * smaller than the box), its pins on the frame's edges in their order,
 * spread over its height. `node` itself when it does not grow; a grown node
 * is returned as it is.
 */
export function grownNode(node, bounds) {
  if (node.grown) return node;
  const cached = grownOf.get(node);
  if (cached?.bounds === bounds) return cached.grown;
  const natural = fitScale(node, bounds);
  const k = Math.min(natural * GROW_MAX, Math.max(natural, FRAME_K));
  const w = Math.max(node.w, bounds.w * k + 2 * PAD);
  const h = Math.max(node.h, bounds.h * k + HEADER + PAD);
  let grown = node;
  if (w > node.w || h > node.h) {
    // The inside (below the header) centred on the box's centre, as far as the frame still covers the box.
    const x = node.x + (node.w - w) / 2;
    const y = Math.min(node.y, Math.max(node.y + node.h - h, node.y + node.h / 2 - HEADER - (h - HEADER - PAD) / 2));
    const spread = (h - HEADER - PAD) / Math.max(1, node.h - HEADER - PAD);
    const edge = (p, right) => ({ ...p, x: right ? x + w : x, y: y + HEADER + (p.y - node.y - HEADER) * spread });
    grown = { ...node, x, y, w, h, ins: node.ins.map((p) => edge(p, false)), outs: node.outs.map((p) => edge(p, true)), grown: true };
  }
  grownOf.set(node, { bounds, grown });
  return grown;
}

/** The scale (inside's units → its parent's) of `node`'s inside of layout `bounds` when it is open. */
export function linkScale(node, bounds) {
  return fitScale(grownNode(node, bounds), bounds);
}

/** The scale of the parent's level on screen at which `node` opens. */
export function opensAt(node, bounds) {
  return OPEN_K / linkScale(node, bounds);
}

/** Room (layout units) kept clear beside a frame's edges for its pins' names, drawn outside it: about this many units per character, */
export const ROOM_CHAR = CHAR;
/** and this much more, as between a box's edge and its neighbour (`GAP_Y` above and below). */
export const ROOM_PAD = 3 * PAD;

/** Which side of the interval `[a0, a1)` the interval `[b0, b1)` lies on: -1 before, 1 after, 0 overlapping. */
function side(a0, a1, b0, b1) {
  if (b1 <= a0) return -1;
  if (b0 >= a1) return 1;
  return 0;
}

/**
 * How far each box of `nodes` moves so that `frame` (the box `box` grown)
 * covers none of them nor the room for its pins' names (`room`: `{left,
 * right}` units beside its edges, GAP_Y above and below): `Map(node →
 * {dx, dy})`, only the boxes that move (M2.13-3, the operator's review:
 * "make room for the frame"). Every box moves by one shift per side,
 * along each axis on its own: a box left of `box` (its whole width
 * before `box`'s) by the same `dx` leftward, one right of it by the same
 * rightward, one above or below by the same `dy`, one beside it on an
 * axis not at all on that axis. So a box left of the composite stays left
 * of its frame, the order of the boxes and the gaps between them on each
 * side are kept, and no two boxes come to overlap (the shift is monotone
 * along each axis). The vertical shifts are found first, from the boxes
 * over or under the frame; the horizontal ones from the boxes then beside
 * it (their rows reaching its rows), so a box at a corner moves only as
 * far as it must.
 */
export function makeRoom(nodes, box, frame, room) {
  // Instances only: a level's own frame pins (`inputs`, `outputs`) stay on its frame.
  const others = nodes.filter((n) => n !== box && n.id !== box.id && n.kind === 'instance');
  const sides = new Map(others.map((n) => [n, { x: side(box.x, box.x + box.w, n.x, n.x + n.w), y: side(box.y, box.y + box.h, n.y, n.y + n.h) }]));
  let up = 0;
  let down = 0;
  for (const n of others) {
    const s = sides.get(n);
    if (s.x !== 0) continue;
    if (s.y < 0) up = Math.min(up, frame.y - GAP_Y - (n.y + n.h));
    if (s.y > 0) down = Math.max(down, frame.y + frame.h + GAP_Y - n.y);
  }
  const dyOf = (s) => (s.y < 0 ? up : s.y > 0 ? down : 0);
  let left = 0;
  let right = 0;
  for (const n of others) {
    const s = sides.get(n);
    if (s.x === 0) continue;
    const y = n.y + dyOf(s);
    // Its rows (moved) clear of the frame's (with GAP_Y): it need not move sideways.
    if (y + n.h + GAP_Y <= frame.y || y >= frame.y + frame.h + GAP_Y) continue;
    if (s.x < 0) left = Math.min(left, frame.x - room.left - (n.x + n.w));
    else right = Math.max(right, frame.x + frame.w + room.right - n.x);
  }
  const moves = new Map();
  for (const n of others) {
    const s = sides.get(n);
    const dx = s.x < 0 ? left : s.x > 0 ? right : 0;
    const dy = dyOf(s);
    if (dx || dy) moves.set(n, { dx, dy });
  }
  return moves;
}

/** The room beside frame `grown`'s edges for its pins' names, drawn outside it (`makeRoom`). */
function roomOf(grown) {
  const longest = (pins) => pins.reduce((m, p) => Math.max(m, String(p.name).length), 0);
  return { left: longest(grown.ins) * ROOM_CHAR + ROOM_PAD, right: longest(grown.outs) * ROOM_CHAR + ROOM_PAD };
}

function shifted(node, { dx, dy }) {
  const move = (p) => ({ ...p, x: p.x + dx, y: p.y + dy });
  return { ...node, x: node.x + dx, y: node.y + dy, ins: node.ins.map(move), outs: node.outs.map(move) };
}

const openedOf = new WeakMap();

/**
 * `layout` (a level's) with its composite instance `name` open: its box
 * grown (`grownNode`, inside of layout `bounds`), the boxes around moved
 * out of its way (`makeRoom`), and the wires attached to the frame or to a
 * moved box routed again (`crossing`: attached to the frame). A wire
 * across the frame's edge keeps the direction it had to the box (a wire
 * into its left pins runs left to right into the frame, one from its
 * right pins on to the right; never a loop because the frame grew), every
 * other one is routed as `layout` routes it, feedback lanes again below
 * the lowest box drawn. Every node and wire that does not change is the
 * same object. Its `bounds` are `layout`'s, so nothing around depends on
 * what is open inside (a level drawn in a frame of its own may then
 * reach past that frame by the room made); `opened` names the open
 * instance, `moved` the boxes moved (name → `{dx, dy}`). `layout` is not
 * changed.
 */
export function openedLayout(layout, name, bounds) {
  let byName = openedOf.get(layout);
  const cached = byName?.get(name);
  if (cached?.bounds === bounds) return cached.opened;
  const node = layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
  const grown = node ? grownNode(node, bounds) : null;
  let opened = layout;
  if (grown) {
    const room = grown === node ? new Map() : makeRoom(layout.nodes, node, grown, roomOf(grown));
    const shifts = new Map();
    const nodes = layout.nodes.map((n) => {
      if (n === node) return grown;
      const m = room.get(n);
      if (!m) return n;
      shifts.set(n.id, m);
      return shifted(n, m);
    });
    const bottom = levelBottom(nodes);
    const pin = (list, port, fallback) => {
      const p = list.find((q) => q.name === port);
      return p ? { x: p.x, y: p.y } : fallback;
    };
    const moved = (p, id) => {
      const m = shifts.get(id);
      return m ? { x: p.x + m.dx, y: p.y + m.dy } : p;
    };
    let lane = 0;
    const wires = layout.wires.map((wire) => {
      const out = wire.from.node === node.id;
      const into = wire.to.node === node.id;
      const a = out ? pin(grown.outs, wire.from.port, wire.curve.start) : moved(wire.curve.start, wire.from.node);
      const b = into ? pin(grown.ins, wire.to.port, wire.curve.segs.at(-1).to) : moved(wire.curve.segs.at(-1).to, wire.to.node);
      // Across the frame's edge: as it ran to the box (the operator's review).
      const feedback = out || into ? Boolean(wire.feedback) : wire.reversed || b.x <= a.x;
      if (feedback) lane += 1;
      const under = feedback ? laneY(bottom, lane) : null;
      const shiftedEnd = shifts.has(wire.from.node) || shifts.has(wire.to.node);
      if (!out && !into && !shiftedEnd && under === wire.under) return wire;
      const { curve, points, box } = wireGeometry(a, b, { feedback, under: under ?? 0 });
      return { ...wire, feedback, lane: feedback ? lane : 0, under, curve, points, box, crossing: out || into };
    });
    const names = new Map(layout.nodes.map((n) => [n.id, n.name]));
    opened = { ...layout, nodes, wires, bottom, opened: name, moved: new Map([...shifts].map(([id, m]) => [names.get(id), m])) };
  }
  if (!byName) openedOf.set(layout, (byName = new Map()));
  byName.set(name, { bounds, opened });
  return opened;
}

/**
 * The largest scale on screen of a level laid out as `layout` at which
 * none of its composites is open, nor stays open (below every CLOSE_K):
 * what Fit, a breadcrumb and Escape keep to, so that going out of a level
 * never shows it open again (Infinity when it holds no inside known to
 * `layoutOf`).
 */
export function closedScale(layout, layoutOf) {
  let k = Infinity;
  for (const node of layout?.nodes ?? []) {
    if (!node.composite || node.kind !== 'instance') continue;
    const inner = layoutOf(node.module);
    if (inner) k = Math.min(k, (CLOSE_K / linkScale(node, inner.bounds)) * 0.999);
  }
  return k;
}

/** `view` zoomed out about the screen point `at` until its level, drawn at scale `k` now, is at most `max` (`closedScale`). */
export function closedView(view, k, max, at) {
  return k > max ? zoomAt(view, max / k, at) : view;
}

/** `view` zoomed in about the screen point `at` until its level, drawn at scale `k` now, is at least READ_K: what Enter and a breadcrumb show, readable, the inside overflowing the view if need be. */
export function readableView(view, k, at) {
  return k < READ_K ? zoomAt(view, READ_K / k, at) : view;
}

/** Enter's view of an inside of layout `bounds` (in its own coordinates): fitted (`fit`'s margins after), and readable (`readableView`). */
export function enterView(bounds, viewport, ...margins) {
  const view = fit(bounds, viewport, ...margins);
  return readableView(view, view.k, { x: view.x, y: view.y });
}

/**
 * A breadcrumb's view of the open level `level` (a scene entry, drawn in
 * `view` now) whose layout is `rect` on screen: fitted as `focus` does,
 * readable (`readableView`), and with none of its own composites open
 * (`closedScale`, which wins).
 */
export function crumbView(view, level, rect, viewport, layoutOf, ...margins) {
  const focused = focus(view, rect, viewport, ...margins);
  const target = fit(rect, viewport, ...margins);
  const at = { x: target.x, y: target.y };
  const k = level.transform.k * (focused.k / view.k);
  const readable = readableView(focused, k, at);
  return closedView(readable, k * (readable.k / focused.k), closedScale(level.layout, layoutOf), at);
}

/**
 * The pins left after a zoom by `factor` of the scene `drawn`: a zoom out
 * that takes a pinned composite's inside below CLOSE_K unpins it and what
 * is pinned inside it (`pinned` itself when nothing changes; a zoom in
 * never unpins, auditor-2-unpin-direction).
 */
export function unpinned(pinned, drawn, factor) {
  if (factor >= 1 || !pinned.size) return pinned;
  const closing = drawn.levels.filter((l) => l.path && pinned.has(l.path) && l.transform.k * factor < CLOSE_K).map((l) => l.path);
  if (!closing.length) return pinned;
  return new Set([...pinned].filter((path) => !closing.some((c) => path === c || path.startsWith(`${c}/`))));
}

/**
 * The zoom factor from the scene `drawn` to `view` (as `scene` takes it):
 * every zoom since that scene, however many came before the next frame;
 * null when `view` is in another level's coordinates (a breadcrumb, Enter).
 */
export function zoomSince(drawn, view) {
  return (view.path ?? '') === (drawn.view.path ?? '') ? view.k / drawn.view.k : null;
}

/** `pinned` with `address` pinned instead of any sibling of it (and what is pinned inside those): one open per level. */
export function pinOne(pinned, address) {
  const parent = address.slice(0, address.lastIndexOf('/'));
  const kept = [...pinned].filter((path) => {
    const top = path.startsWith(`${parent}/`) ? `${parent}/${path.slice(parent.length + 1).split('/')[0]}` : null;
    return !top || top === address;
  });
  return new Set([...kept, address]);
}

/** The level edited (M2.13-3, AC3): the innermost open one of the scene `drawn` (`{path, level}`). */
export function editLevel(drawn) {
  const last = drawn?.levels?.at(-1);
  return last ? { path: last.path, level: last.level } : { path: '', level: '' };
}

/** Whether part `hit` (a hit or a selection) may be edited: it is of the innermost open level; the surroundings are only selected and inspected. */
export function editable(drawn, hit) {
  return Boolean(hit) && (hit.path ?? '') === editLevel(drawn).path;
}

/**
 * Why part `hit` (a hit or a selection, at level path `hit.path`) of the
 * scene `drawn` is not edited, or null when it may be (`editable`): it is
 * around the innermost open composite (the operator's review: its edit
 * controls disabled, this their title).
 */
export function aroundReason(drawn, hit) {
  if (!hit || editable(drawn, hit)) return null;
  const where = hit.path ? hit.path : 'the top level';
  return `${where} is around the open ${editLevel(drawn).path}: zoom out of it to edit ${hit.path ? 'there' : 'it'}`;
}

/** A level's nodes in drawing order: the open composite's frame (`opened`) last, over its neighbours. */
export function drawOrder(nodes, opened) {
  if (!opened) return nodes;
  const frame = nodes.filter((n) => n.kind === 'instance' && n.name === opened);
  return frame.length ? [...nodes.filter((n) => !frame.includes(n)), ...frame] : nodes;
}

/** `fit` of a level laid out as `layout` (`fit`'s margins after), its composites closed (`closedScale`): Fit, and `0`. */
export function closedFit(layout, layoutOf, viewport, ...margins) {
  const view = fit(layout.bounds, viewport, ...margins);
  return closedView(view, view.k, closedScale(layout, layoutOf), { x: view.x, y: view.y });
}

function intersects(a, b) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

function onScreen(transform, r) {
  return {
    x: (r.x - (transform.ox ?? 0)) * transform.k + transform.x,
    y: (r.y - (transform.oy ?? 0)) * transform.k + transform.y,
    w: r.w * transform.k,
    h: r.h * transform.k,
  };
}

/**
 * The composite of a level to open: `{node, inner}` (`node` its box in the
 * level's stored `layout`, `inner` its inside's layout), or null. The level
 * at `path` is drawn with `transform` on `screen`; `layoutOf(module)` gives
 * an inside (a library composite's is unknown: never opened). A pinned
 * one (double-clicked) first; else the one `open` held, while its inside is
 * drawn at CLOSE_K or more and its frame is on screen; else one whose
 * inside would be drawn at OPEN_K or more, its box on screen: the one
 * under `at` (the zoom's point), else the nearest to it; but only when
 * `opening` (a zoom, Enter, a breadcrumb: never a pan alone, the
 * operator's review, so panning across a level opens nothing new).
 */
export function chooseOpen({ path, layout, transform, screen, layoutOf, open = new Set(), pinned = new Set(), at, opening = true }) {
  let kept = null;
  let best = null;
  for (const node of layout.nodes) {
    if (!node.composite || node.kind !== 'instance') continue;
    const child = `${path}/${node.name}`;
    const isPinned = pinned.has(child);
    const wasOpen = open.has(child);
    const inner = layoutOf(node.module);
    if (!inner) continue;
    if (isPinned) return { node, inner };
    const k = transform.k * linkScale(node, inner.bounds);
    if (wasOpen && !kept && k >= CLOSE_K && intersects(onScreen(transform, grownNode(node, inner.bounds)), screen)) {
      kept = { node, inner };
      continue;
    }
    if (!opening || k < OPEN_K) continue;
    const r = onScreen(transform, node);
    if (!intersects(r, screen)) continue;
    const dx = Math.max(r.x - at.x, 0, at.x - r.x - r.w);
    const dy = Math.max(r.y - at.y, 0, at.y - r.y - r.h);
    const d = Math.hypot(dx, dy);
    if (!best || d < best.d) best = { node, inner, d };
  }
  if (kept) return kept;
  return best ? { node: best.node, inner: best.inner } : null;
}

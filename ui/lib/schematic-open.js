// A composite opened in place (M2.13-3, ADR 0030 §2): its box grows into
// a frame that draws its inside at a readable scale where the box is, and
// the outer wires attached to it are routed to the frame's edge, where the
// links across it (`boundaryLinks`, ui/lib/schematic-scene.js) carry them on
// to the inner pins. The frame's scale (`linkScale`) depends on the layouts
// only, never on the view, so a level and its open inside stay one exact
// affine link (`compose`, `uncompose`). A composite opens when the person
// opens it (2026-10-22, the owner: a click on its box, a double-click,
// Enter, Open in place, a reveal: it is pinned, `pinOne`) and closes when
// they close it (the × in its frame's header, `closeBox`; Escape, Close, a
// breadcrumb, Fit: unpinned, `unpin`), never because of the view's scale:
// a zoom or a pan only moves the view. At most one is open per level
// (`chooseOpen`, `pinOne`), as grown frames would cover each other.
// Positions are never changed: the opened copy of a level
// (`openedLayout`) is drawn, its other boxes moved out of the frame's way
// (`makeRoom`), the stored layout is what is moved. No DOM.

import { CHAR, GAP_Y, HEADER, PAD, insideKey, laneY, levelBottom } from './schematic-layout.js';
import { LABEL_PX, READ_PX, estimateWidth } from './schematic-labels.js';
import { crosses, wireGeometry } from './schematic-wire.js';
import { fit, focus, zoomAt } from './schematic-view.js';

/**
 * Enter, a double-click and a breadcrumb show an inside at this scale at
 * least (about the readability Enter gives the studio's synth; it was 5 %
 * above the zoom's opening threshold, 0.45, before 2026-10-22). Small: at
 * 0.56 the studio's synth no longer fitted the view.
 */
export const READ_K = 0.4725;
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
 * the lowest box drawn, and a forward one that would pass under the frame
 * around it (`avoid`, kept as the layout's for its bundles: `detour`,
 * ui/lib/schematic-wire.js). Every node and wire that does not change is the
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
    // What passes the frame goes around it (2026-10-22): along a lane between its pins' names and the boxes moved aside, or in the gap above or below it.
    const beside = roomOf(grown);
    const frame = { x: grown.x, y: grown.y, w: grown.w, h: grown.h };
    const avoid = { rect: frame, lanes: { left: frame.x - beside.left + PAD, right: frame.x + frame.w + beside.right - PAD, top: frame.y - GAP_Y / 2, bottom: frame.y + frame.h + GAP_Y / 2 } };
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
      const around = !out && !into && !feedback;
      if (!out && !into && !shiftedEnd && under === wire.under && !(around && crosses(wire.curve, frame))) return wire;
      const { curve, points, box } = wireGeometry(a, b, { feedback, under: under ?? 0, avoid: around ? avoid : null });
      return { ...wire, feedback, lane: feedback ? lane : 0, under, curve, points, box, crossing: out || into };
    });
    const names = new Map(layout.nodes.map((n) => [n.id, n.name]));
    opened = { ...layout, nodes, wires, bottom, opened: name, moved: new Map([...shifts].map(([id, m]) => [names.get(id), m])), avoid };
  }
  if (!byName) openedOf.set(layout, (byName = new Map()));
  byName.set(name, { bounds, opened });
  return opened;
}

/** `view` zoomed in about the screen point `at` until its level, drawn at scale `k` now, is at least READ_K: what Enter and a breadcrumb show, readable, the inside overflowing the view if need be. */
export function readableView(view, k, at) {
  return k < READ_K ? zoomAt(view, READ_K / k, at) : view;
}

/**
 * Enter and a double-click draw neither an open frame (its title, its ×,
 * in its parent's units) nor its inside larger than this scale (2026-10-29,
 * the operator's review: an empty slot's frame, fitted, drew its subtitle
 * about 100 px tall and cut its title off; a box holding one small module
 * filled the screen with it).
 */
export const ENTER_MAX_K = 2;

/** `frame` (an open composite's grown box, in its parent's units) in the units of its inside (`inner`: inside → parent, `inside`'s). */
export function frameRect(frame, inner) {
  return { x: (frame.x - inner.x) / inner.k, y: (frame.y - inner.y) / inner.k, w: frame.w / inner.k, h: frame.h / inner.k };
}

/**
 * Enter's view of an open composite, in its inside's coordinates: its
 * whole frame (`frame`, the grown box, header and × included; `inner` its
 * inside's link, as `chain` gives them) fitted (`fit`'s margins after),
 * neither the frame nor the inside drawn larger than ENTER_MAX_K (a small
 * or empty inside does not blow up), and readable (`readableView`: a wide
 * inside overflows the view rather than drop below READ_K, zoomed about
 * the frame's top edge, so its header stays where the fit put it).
 */
export function enterView(frame, inner, viewport, ...margins) {
  const rect = frameRect(frame, inner);
  const fitted = fit(rect, viewport, ...margins);
  const most = ENTER_MAX_K * Math.min(1, inner.k);
  const view = fitted.k > most ? { ...fitted, k: most } : fitted;
  return readableView(view, view.k, { x: view.x, y: view.y - (rect.h / 2) * view.k });
}

/**
 * A breadcrumb's view of the open level `level` (a scene entry, drawn in
 * `view` now) whose layout is `rect` on screen: fitted as `focus` does,
 * and readable (`readableView`). What is open below it is closed by the
 * breadcrumb itself (`unpin`), not by the view.
 */
export function crumbView(view, level, rect, viewport, ...margins) {
  const focused = focus(view, rect, viewport, ...margins);
  const target = fit(rect, viewport, ...margins);
  return readableView(focused, level.transform.k * (focused.k / view.k), { x: target.x, y: target.y });
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

/** `pinned` without `address` and what is pinned inside it (its × or Escape, Close, Go outside; `pinned` itself when neither is pinned). */
export function unpin(pinned, address) {
  const closing = (path) => path === address || path.startsWith(`${address}/`);
  return [...pinned].some(closing) ? new Set([...pinned].filter((path) => !closing(path))) : pinned;
}

/** The level edited (M2.13-3, AC3): the innermost open one of the scene `drawn` (`{path, level}`). */
export function editLevel(drawn) {
  const last = drawn?.levels?.at(-1);
  return last ? { path: last.path, level: last.level } : { path: '', level: '' };
}

/**
 * Whether part `hit` (an instance, or a pin of its box) is the open frame
 * whose inside is the level edited (`editLevel`): around it, yet what is
 * edited from inside it (2026-10-29: an open slot's own items, Load…,
 * Edit interface… and the like, act on the slot whose inside is shown).
 */
export function shownFrame(drawn, hit) {
  const path = editLevel(drawn).path;
  return Boolean(path) && hit?.node?.kind === 'instance' && (hit.kind === 'instance' || hit.kind === 'port') && `${hit.path ?? ''}/${hit.node.name}` === path;
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
  return `${where} is around the open ${editLevel(drawn).path}: close it (×) to edit ${hit.path ? 'there' : 'it'}`;
}

/** A level's nodes in drawing order: the open composite's frame (`opened`) last, over its neighbours. */
export function drawOrder(nodes, opened) {
  if (!opened) return nodes;
  const frame = nodes.filter((n) => n.kind === 'instance' && n.name === opened);
  return frame.length ? [...nodes.filter((n) => !frame.includes(n)), ...frame] : nodes;
}

/**
 * The composite of the level at `path` (laid out as `layout`) to open:
 * `{node, inner}` (`node` its box in the level's stored layout, `inner`
 * its inside's layout), or null. Only a pinned one opens (2026-10-22: a
 * click, a double-click, Enter, Open in place, a reveal), never one the
 * view's scale or position would pick; `layoutOf(module)` gives an inside
 * (a library composite's is unknown: never opened).
 */
export function chooseOpen({ path, layout, layoutOf, pinned = new Set() }) {
  if (!pinned.size) return null;
  for (const node of layout.nodes) {
    if (!node.composite || node.kind !== 'instance' || !pinned.has(`${path}/${node.name}`)) continue;
    const inner = layoutOf(insideKey(node));
    if (inner) return { node, inner };
  }
  return null;
}

/** The close button's square (the × in an open frame's header, top right; 2026-10-22), in its level's units, */
export const CLOSE_UNITS = 22;
/** its letter's size in those units (scaling with the view as every text does, ADR 0030's 2026-10-20 refinement), */
export const CLOSE_TEXT = 16;
/** and its distance from the frame's right edge. */
export const CLOSE_INSET = 6;
/**
 * Its square is never smaller than this on screen (px; 2026-10-22, the
 * operator's review: about 12 px at Fit, too small to hit): zoomed out it
 * keeps this size, its letter CLOSE_TEXT / CLOSE_UNITS of it (14.5 px).
 */
export const CLOSE_MIN_PX = 20;
/** Where a frame's title starts (layout units from its left edge) and its sizes (`nodeElement`'s name and module). */
const TITLE_X = 8;
const TITLE_TEXT = 12;
const MODULE_TEXT = 10;

/**
 * The square of the × that closes `frame` (an open composite's grown box)
 * in its level drawn at scale `k`, in its level's units: inside the frame
 * at its top right, CLOSE_UNITS (in its header) or CLOSE_MIN_PX on screen,
 * whichever is larger; larger than the header, it reaches below it.
 */
export function closeBox(frame, k = Infinity) {
  const side = Math.max(CLOSE_UNITS, CLOSE_MIN_PX / k);
  const inset = Math.max(3, (HEADER - side) / 2);
  // In the header (CLOSE_INSET from its edges at CLOSE_UNITS), larger nearer the corner.
  return { x: frame.x + frame.w - inset - side, y: frame.y + inset, w: side, h: side };
}

/** The × letter's size (units) in `closeBox`'s square `r`. */
export function closeText(r) {
  return (r.w * CLOSE_TEXT) / CLOSE_UNITS;
}

/**
 * Whether the × of `frame` is drawn (and hit) in its level drawn at scale
 * `k`: its square (`closeBox`) inside the frame and clear of its title
 * (the name and module, while drawn: 12 units on screen READ_PX or more),
 * so it never covers it; else hidden (Escape still closes).
 */
export function closeShown(frame, k) {
  const r = closeBox(frame, k);
  const titled = TITLE_TEXT * k >= READ_PX;
  const title = titled ? TITLE_X + Math.max(estimateWidth(frame.name ?? '') * (TITLE_TEXT / LABEL_PX), estimateWidth(frame.module ?? '') * (MODULE_TEXT / LABEL_PX)) : 0;
  return r.x >= frame.x + Math.max(3, title + 2) && r.y + r.h <= frame.y + frame.h - 3;
}

// The schematic's semantic zoom (M2.9-1, ADR 0026 §2): what is drawn for a
// view. A composite the person opened (pinned: a click, a double-click,
// Enter; `chooseOpen`, ui/lib/schematic-open.js; never the view's scale,
// 2026-10-22) opens in place (M2.13-3): its box grows into a frame and its
// own layout is drawn in it, recursively, one per level. The view is in the
// coordinates of a level (its `path`, the deepest drawn under the screen's
// centre): the levels above it are derived from it upward and the others
// downward from them (M2.9-1b), so a deep view never passes through huge
// numbers, and `scene` returns the view re-rooted at the deepest level
// drawn under the centre now. Only what is on screen is listed (a node or a
// wire smaller than MIN_PX is not), so the flattened plan is never drawn
// whole and parents far outside the screen are culled. Breadcrumbs are
// the open levels, the innermost last. No DOM.

import { HEADER, PAD, dieArea, dieLink, insideKey } from './schematic-layout.js';
import { chooseOpen, grownNode, openedLayout } from './schematic-open.js';
import { compose, screenRect, toScreen, uncompose } from './schematic-view.js';
import { shownOpacity } from './board-zoom.js';

/** Smaller than this on screen (px), a node or a wire is not drawn. */
export const MIN_PX = 2;
/** Nested deeper than this, nothing more opens (a guard; projects are shallower). */
export const MAX_DEPTH = 32;

function intersects(a, b) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** The transform that puts a level whose layout has `bounds` inside `node`'s box, below its header. */
export function inside(node, bounds) {
  // Its body below its header (ADR 0047 §8, a package's die area).
  const area = dieArea(node);
  const k = Math.min(area.w / Math.max(bounds.w, 1e-9), area.h / Math.max(bounds.h, 1e-9));
  return {
    k,
    x: area.x + (area.w - bounds.w * k) / 2 - bounds.x * k,
    y: area.y + (area.h - bounds.h * k) / 2 - bounds.y * k,
  };
}

/**
 * What `node` (an open composite's box) shows of its inside `inner`:
 * `{layout, link}`, the transform inside → parent. A package (ADR 0047 §8,
 * `node.die`) draws its die scaled into its own rectangle, its interface
 * on its pins (`dieLink`); any other box draws its inside in its frame.
 */
export function drawnInside(node, inner) {
  if (node.die) return { layout: inner, link: dieLink(node, inner) };
  return { layout: inner, link: inside(grownNode(node, inner.bounds), inner.bounds) };
}

/**
 * A wire's bounding box in its level's coordinates: its curve's (`box`, a
 * layout's wires), else its points', computed once per layout.
 */
const boxes = new WeakMap();
function wireBox(wire) {
  if (wire.box) return wire.box;
  let box = boxes.get(wire);
  if (!box) {
    box = { x0: Infinity, y0: Infinity, x1: -Infinity, y1: -Infinity };
    for (const p of wire.points) {
      box.x0 = Math.min(box.x0, p.x);
      box.y0 = Math.min(box.y0, p.y);
      box.x1 = Math.max(box.x1, p.x);
      box.y1 = Math.max(box.y1, p.y);
    }
    boxes.set(wire, box);
  }
  return box;
}

/**
 * The open composites from the top level down to the level at `path`, as
 * `[{path, level, layout, node, inner}]` (`node` the instance's box in its
 * parent, grown into its frame, `layout` with the next one open, `inner`
 * its inside's transform), the top level first with no node; null when
 * `path` is not a chain of composites in these layouts.
 */
export function chain(layoutOf, path = '') {
  const top = layoutOf('');
  if (!top) return null;
  const links = [{ path: '', level: '', layout: top, node: null, inner: null }];
  let at = '';
  for (const name of String(path).split('/').filter(Boolean)) {
    const parent = links.at(-1);
    const box = parent.layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
    const drawn = box?.composite ? layoutOf(insideKey(box)) : null;
    if (!drawn) return null;
    parent.layout = openedLayout(parent.layout, name, drawn.bounds);
    const node = grownNode(box, drawn.bounds);
    at = `${at}/${name}`;
    const shown = drawnInside(node, drawn);
    links.push({ path: at, level: insideKey(node), layout: shown.layout, node, inner: shown.link });
  }
  return links;
}

/** The transforms of a chain's levels, derived upward from the last one's `view`. */
function upward(links, view) {
  const transforms = new Array(links.length);
  transforms[links.length - 1] = view;
  for (let i = links.length - 1; i > 0; i--) transforms[i - 1] = uncompose(transforms[i], links[i].inner);
  return transforms;
}

/**
 * The levels to draw: `{levels: [{path, level, layout, transform, nodes,
 * wires, parent, behind}], open, view}`, outermost first (`behind`: a
 * parent of which only the inside of open boxes shows, drawn as nothing).
 * `layoutOf(level)` gives a
 * level's layout (`""`: the top level, else a composite's name); `path` is
 * an open composite instance's address (`""` for the top level); `view` is
 * in the coordinates of the level at `view.path` (the top level when
 * absent or no longer there: a level just closed keeps the view of the
 * levels around it still); `pinned` is the set of paths opened (`open`,
 * returned, those of them drawn). `view` comes back re-rooted at the
 * deepest level drawn under the screen's centre.
 */
export function scene({ layoutOf, view, viewport, pinned = new Set() }) {
  const screen = { x: 0, y: 0, w: viewport.w, h: viewport.h };
  const links = chain(layoutOf, view.path ?? '') ?? chain(layoutOf, '');
  const along = new Map();
  if (links) {
    const transforms = upward(links, view);
    links.forEach((link, i) => along.set(link.path, transforms[i]));
  }
  const levels = [];
  const visit = (path, level, transform, depth, parent, given = null) => {
    // A die is drawn fitted into its package (`drawnInside`); its chips are the stored layout's.
    const stored = given ?? layoutOf(level);
    if (!stored) return;
    const pick = depth < MAX_DEPTH ? chooseOpen({ path, layout: stored, layoutOf, pinned }) : null;
    const drawn = pick ? openedLayout(stored, pick.node.name, pick.inner.bounds) : stored;
    // How much of it is shown (2026-10-31): a die by its text and size on screen (`shownOpacity`), the top level whole.
    const opacity = parent ? shownOpacity(transform.k, screenRect(transform, drawn.bounds), viewport) : 1;
    const entry = { path, level, layout: drawn, stored, transform, opacity, nodes: [], wires: [], parent };
    levels.push(entry);
    for (const node of drawn.nodes) {
      const rect = screenRect(transform, node);
      // A die's interface (ADR 0047 phase 4): its zero-wide columns and zero-high strip are drawn and hit for their pins
      // (the owner: "the crossings at the chip boundary do not get the click"), never culled for their size.
      if (node.edge ? !intersects({ x: rect.x - STROKE_PX, y: rect.y - STROKE_PX, w: rect.w + 2 * STROKE_PX, h: rect.h + 2 * STROKE_PX }, screen) : rect.w < MIN_PX || !intersects(rect, screen)) continue;
      entry.nodes.push(node);
    }
    // The board (ADR 0047) draws every wire at every zoom: zoom never decides which nets are drawn, only the view does
    // (review 2026-10-31): a wire none of whose segments crosses the screen is not drawn, however long it is.
    if (drawn.board) {
      for (const wire of drawn.wires) if (wireOnScreen(wire, transform, screen)) entry.wires.push(wire);
    } else for (const wire of drawn.wires) {
      const box = wireBox(wire);
      const w = (box.x1 - box.x0) * transform.k;
      const h = (box.y1 - box.y0) * transform.k;
      if (Math.max(w, h) < MIN_PX) continue;
      const at = screenRect(transform, { x: box.x0, y: box.y0, w: 0, h: 0 });
      if (intersects({ x: at.x, y: at.y, w: w + 1, h: h + 1 }, screen)) entry.wires.push(wire);
    }
    if (!pick) return;
    const child = `${path}/${pick.node.name}`;
    const shown = drawnInside(pick.node, pick.inner);
    // A level on the view's chain has its transform from the view itself.
    const next = along.get(child) ?? compose(transform, shown.link);
    visit(child, insideKey(pick.node), next, depth + 1, entry, shown.layout);
  };
  visit('', '', along.get('') ?? view, 0, null);
  const drawn = { levels, open: new Set(levels.slice(1).map((l) => l.path)) };
  for (const entry of levels) entry.behind = behind(entry, drawn.open, screen);
  // Re-rooted at the deepest level drawn under the centre.
  const deepest = underCentre(drawn, viewport).at(-1).path;
  const root = levels.find((l) => l.path === deepest) ?? levels[0];
  drawn.view = root ? { ...root.transform, path: root.path } : view;
  return drawn;
}

/**
 * The chip the person's zoom opens (2026-10-31, the owner: "the microscope
 * view should appear by itself while zooming in"), as its address, or null:
 * in the outermost level of the scene `drawn` that has none open on screen,
 * of its chips on screen with an inside (`layoutOf`) whose die would be
 * shown (`shownOpacity` over 0, the very point where zooming out leaves
 * it), the one under the zoom's `anchor` (screen px), else the nearest to
 * it. One per level, as a click pins (`pinOne`); called again after it
 * opens, it finds the chips inside its die (recursion).
 */
export function risen({ drawn, layoutOf, viewport, anchor }) {
  const screen = { x: 0, y: 0, w: viewport.w, h: viewport.h };
  for (const entry of drawn?.levels ?? []) {
    const child = drawn.levels.find((l) => l.parent === entry);
    if (child) {
      const name = child.path.slice(entry.path.length + 1);
      const box = entry.layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
      if (box && intersects(screenRect(entry.transform, box), screen)) continue;
    }
    let best = null;
    for (const node of entry.nodes) {
      if (node.kind !== 'instance' || !node.composite) continue;
      const inner = layoutOf(insideKey(node));
      if (!inner) continue;
      const t = compose(entry.transform, drawnInside(node, inner).link);
      if (!(shownOpacity(t.k, screenRect(t, inner.bounds), viewport) > 0)) continue;
      const r = screenRect(entry.transform, node);
      const dx = Math.max(r.x - anchor.x, 0, anchor.x - (r.x + r.w));
      const dy = Math.max(r.y - anchor.y, 0, anchor.y - (r.y + r.h));
      const d = Math.hypot(dx, dy);
      if (!best || d < best.d) best = { d, path: `${entry.path}/${node.name}` };
    }
    if (best) return best.path;
  }
  return null;
}

/** How far past the screen (px) a stroke may reach. */
const STROKE_PX = 16;

/** Whether the segment `a`–`b` (screen px) meets rectangle `r` (Liang–Barsky). */
function segmentMeets(a, b, r) {
  let t0 = 0;
  let t1 = 1;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  for (const [p, q] of [
    [-dx, a.x - r.x],
    [dx, r.x + r.w - a.x],
    [-dy, a.y - r.y],
    [dy, r.y + r.h - a.y],
  ]) {
    if (p === 0) {
      if (q < 0) return false;
      continue;
    }
    const t = q / p;
    if (p < 0) t0 = Math.max(t0, t);
    else t1 = Math.min(t1, t);
    if (t0 > t1) return false;
  }
  return true;
}

/**
 * Whether a board wire is on screen under `transform`: one of its segments
 * (its points', else its corners') meets the screen grown by STROKE_PX. Its
 * box is tried first; a wire crossing the screen with both ends off it is
 * on screen, one whose box only covers the screen is not.
 */
function wireOnScreen(wire, transform, screen) {
  const grown = { x: screen.x - STROKE_PX, y: screen.y - STROKE_PX, w: screen.w + 2 * STROKE_PX, h: screen.h + 2 * STROKE_PX };
  const points = wire.points?.length ? wire.points : (wire.corners ?? []);
  if (!points.length) return true;
  const box = wireBox(wire);
  const at = screenRect(transform, { x: box.x0, y: box.y0, w: box.x1 - box.x0, h: box.y1 - box.y0 });
  if (!(at.x <= grown.x + grown.w && grown.x <= at.x + at.w && at.y <= grown.y + grown.h && grown.y <= at.y + at.h)) return false;
  if (points.length === 1) return true;
  let a = toScreen(transform, points[0]);
  for (let i = 1; i < points.length; i++) {
    const b = toScreen(transform, points[i]);
    if (segmentMeets(a, b, grown)) return true;
    a = b;
  }
  return false;
}

/**
 * Whether all that level `entry` shows on screen is the inside of open
 * boxes around the screen: their fill, the background's colour (their
 * edges, header, pins and the wires under them are off screen or hidden).
 * Such a parent, however far its edges, is culled: not drawn at all.
 */
function behind(entry, open, screen) {
  return (
    entry.nodes.length > 0 &&
    entry.nodes.every((node) => {
      if (!open.has(`${entry.path}/${node.name}`)) return false;
      const area = screenRect(entry.transform, { x: node.x + PAD, y: node.y + HEADER, w: node.w - 2 * PAD, h: node.h - HEADER - PAD });
      return area.x <= -STROKE_PX && area.y <= -STROKE_PX && area.x + area.w >= screen.w + STROKE_PX && area.y + area.h >= screen.h + STROKE_PX;
    })
  );
}

/**
 * `view` (as `scene` takes it) after the layouts changed from `before` to
 * `after` (`layoutOf` both): kept in the coordinates of the deepest level
 * of its chain whose links to the top level did not change, so what did
 * not move stays where it is on screen.
 */
export function rebase(view, before, after) {
  const old = chain(before, view.path ?? '');
  if (!old) return view;
  const now = chain(after, view.path ?? '');
  let kept = old.length - 1;
  for (let i = 1; i < old.length; i++) {
    const a = old[i].inner;
    const b = now?.[i]?.inner;
    if (!b || a.k !== b.k || a.x !== b.x || a.y !== b.y) {
      kept = i - 1;
      break;
    }
  }
  if (kept === old.length - 1) return view;
  return { ...upward(old, view)[kept], path: old[kept].path };
}

/**
 * `view` (as `scene` takes it, with the layouts `layoutOf`) in the
 * coordinates of the level at `path`, one of its chain's (the view's level
 * or one it is inside); `view` itself when `path` is not on its chain.
 * Handed to `scene` with layouts in which that level changed, it keeps that
 * level where it is on screen, and the levels around it move instead.
 */
export function reroot(view, layoutOf, path) {
  const links = chain(layoutOf, view.path ?? '');
  const i = links ? links.findIndex((l) => l.path === path) : -1;
  if (i < 0) return view;
  return { ...upward(links, view)[i], path };
}

/** Whether instance `path` (an open composite's) is drawn as open in `drawn`. */
export function isOpen(drawn, path) {
  return drawn.open.has(path);
}

/** The breadcrumbs: the top level, then each open level inside it, the innermost last (`[{path, level, name}]`). */
export function breadcrumbs(drawn) {
  return drawn.levels.map((l) => ({ path: l.path, level: l.level, name: l.path ? l.path.split('/').at(-1) : '' }));
}

/** The top level, then each open box under the view's centre, `[{path, level, name}]`: the deepest is the level the view is kept in. */
function underCentre(drawn, viewport) {
  const centre = { x: viewport.w / 2, y: viewport.h / 2, w: 0, h: 0 };
  const crumbs = [{ path: '', level: '', name: '' }];
  let current = drawn.levels[0];
  while (current) {
    const next = drawn.levels.find((l) => {
      if (l.parent !== current) return false;
      const node = current.layout.nodes.find((n) => `${current.path}/${n.name}` === l.path);
      const rect = node && screenRect(current.transform, node);
      return rect && centre.x >= rect.x && centre.x <= rect.x + rect.w && centre.y >= rect.y && centre.y <= rect.y + rect.h;
    });
    if (next) crumbs.push({ path: next.path, level: next.level, name: next.path.split('/').at(-1) });
    current = next;
  }
  return crumbs;
}

/** The screen rectangle of a crumb's level as drawn (its layout's bounds), or null. */
export function levelRect(drawn, path) {
  const level = drawn.levels.find((l) => l.path === path);
  return level ? screenRect(level.transform, level.layout.bounds) : null;
}

/** The screen rectangle of the node at instance address `path` as drawn, or null. */
export function nodeRect(drawn, path) {
  const cut = path.lastIndexOf('/');
  const level = drawn.levels.find((l) => l.path === path.slice(0, cut));
  const node = level?.layout.nodes.find((n) => n.name === path.slice(cut + 1) && n.kind === 'instance');
  return node ? screenRect(level.transform, node) : null;
}

/**
 * The running plan's primitive port behind port `address` of an instance as
 * drawn (`/drums.out` → `/drums/kick/dc.out`), following the composites'
 * exported outputs in `graph`, or (`parameter`) their exported parameters'
 * first target (`/drums.kick_decay` → `/drums/kick/amp_env.decay_ms`), or
 * (`input`) their exported inputs' first target; null if it reaches none.
 */
export function planPort(graph, address, { parameter = false, input = false } = {}) {
  const dot = address.lastIndexOf('.');
  if (dot < 0) return null;
  const segments = address.slice(1, dot).split('/');
  let port = address.slice(dot + 1);
  let level = graph?.top;
  let prefix = '';
  for (let i = 0; i < segments.length; i++) {
    const instance = level?.instances?.find((n) => n.name === segments[i]);
    if (!instance) return null;
    prefix += `/${segments[i]}`;
    if (!instance.composite) return i === segments.length - 1 ? `${prefix}.${port}` : null;
    if (i === segments.length - 1) {
      // An exported output, followed down to its primitive.
      let inner = graph.composites?.[instance.module];
      for (let depth = 0; inner && depth <= MAX_DEPTH; depth++) {
        const from = parameter
          ? inner.parameters?.find((o) => o.name === port)?.to?.[0]
          : input
            ? inner.inputs?.find((o) => o.name === port)?.to?.[0]
            : inner.outputs?.find((o) => o.name === port)?.from;
        if (!from) return null;
        const cut = from.lastIndexOf('.');
        const name = from.slice(0, cut);
        port = from.slice(cut + 1);
        prefix += `/${name}`;
        const next = inner.instances?.find((n) => n.name === name);
        if (!next) return null;
        if (!next.composite) return `${prefix}.${port}`;
        inner = graph.composites?.[next.module];
      }
      return null;
    }
    level = graph.composites?.[instance.module];
  }
  return null;
}

/**
 * The links across an open composite's edge (2026-10-20, the owner's
 * "tracing must be able to go through the composite"): from each pin of
 * its box (`node`, in its parent's layout) to the same port's pin on the
 * frame inside it (`inner`, its own layout), so an outer wire, its box pin
 * and the inner wires are one drawn path. In the inside's coordinates:
 * `[{name, direction, shape, from: {x, y}, to: {x, y}}]`, `from` the
 * upstream end (an input's box pin, an output's frame pin). `anchors`
 * (`groupAnchors` of the box, when its groups are collapsed) moves a
 * grouped box pin to its group's pin.
 */
export function boundaryLinks(node, inner, anchors = null) {
  // A package's die has its interface on the package's own edges (ADR 0047 §8): no link to draw.
  if (!node || !inner || node.die) return [];
  const t = inside(node, inner.bounds);
  const local = (p) => ({ x: (p.x - t.x) / t.k, y: (p.y - t.y) / t.k });
  const boxPin = (p) => local(anchors?.get(p.name) ?? p);
  const links = [];
  const frame = (kind) => inner.nodes.find((n) => n.kind === kind);
  const ins = new Map((frame('input')?.outs ?? []).map((p) => [p.name, p]));
  const outs = new Map((frame('output')?.ins ?? []).map((p) => [p.name, p]));
  for (const p of node.ins) {
    const f = ins.get(p.name);
    if (f) links.push({ name: p.name, direction: 'in', shape: f.shape ?? p.shape, from: boxPin(p), to: { x: f.x, y: f.y } });
  }
  for (const p of node.outs) {
    const f = outs.get(p.name);
    if (f) links.push({ name: p.name, direction: 'out', shape: f.shape ?? p.shape, from: { x: f.x, y: f.y }, to: boxPin(p) });
  }
  return links;
}

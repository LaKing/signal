// The schematic's semantic zoom (M2.9-1, ADR 0026 §2): what is drawn for a
// view. A composite whose box is wider on screen than OPEN_PX (and stays
// open down to CLOSE_PX), or that was double-clicked, opens in place: its
// own layout is drawn scaled into its box, recursively. The view is in the
// coordinates of a level (its `path`, the deepest drawn under the screen's
// centre): the levels above it are derived from it upward and the others
// downward from them (M2.9-1b), so a deep view never passes through huge
// numbers, and `scene` returns the view re-rooted at the deepest level
// drawn under the centre now. Only what is on screen is listed (a node or a
// wire smaller than MIN_PX is not), so the flattened plan is never drawn
// whole and parents far outside the screen are culled. Breadcrumbs follow
// the open boxes under the view's centre. No DOM.

import { HEADER, PAD } from './schematic-layout.js';
import { compose, screenRect, uncompose } from './schematic-view.js';

/** A composite opens past this on-screen width (px), and closes below CLOSE_PX. */
export const OPEN_PX = 360;
export const CLOSE_PX = 300;
/** Smaller than this on screen (px), a node or a wire is not drawn. */
export const MIN_PX = 2;
/** Nested deeper than this, nothing more opens (a guard; projects are shallower). */
export const MAX_DEPTH = 32;

function intersects(a, b) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** The transform that puts a level whose layout has `bounds` inside `node`'s box, below its header. */
export function inside(node, bounds) {
  const area = { x: node.x + PAD, y: node.y + HEADER, w: Math.max(1, node.w - 2 * PAD), h: Math.max(1, node.h - HEADER - PAD) };
  const k = Math.min(area.w / Math.max(bounds.w, 1e-9), area.h / Math.max(bounds.h, 1e-9));
  return {
    k,
    x: area.x + (area.w - bounds.w * k) / 2 - bounds.x * k,
    y: area.y + (area.h - bounds.h * k) / 2 - bounds.y * k,
  };
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
 * parent, `inner` its inside's transform), the top level first with no
 * node; null when `path` is not a chain of composites in these layouts.
 */
export function chain(layoutOf, path = '') {
  const top = layoutOf('');
  if (!top) return null;
  const links = [{ path: '', level: '', layout: top, node: null, inner: null }];
  let at = '';
  for (const name of String(path).split('/').filter(Boolean)) {
    const parent = links.at(-1).layout;
    const node = parent.nodes.find((n) => n.kind === 'instance' && n.name === name);
    const drawn = node?.composite ? layoutOf(node.module) : null;
    if (!drawn) return null;
    at = `${at}/${name}`;
    links.push({ path: at, level: node.module, layout: drawn, node, inner: inside(node, drawn.bounds) });
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
 * absent or no longer there); `open` is the set of paths open in the last
 * scene, `pinned` those double-clicked. `view` comes back re-rooted at the
 * deepest level drawn under the screen's centre.
 */
export function scene({ layoutOf, view, viewport, open = new Set(), pinned = new Set() }) {
  const screen = { x: 0, y: 0, w: viewport.w, h: viewport.h };
  const links = chain(layoutOf, view.path ?? '') ?? chain(layoutOf, '');
  const along = new Map();
  if (links) {
    const transforms = upward(links, view);
    links.forEach((link, i) => along.set(link.path, transforms[i]));
  }
  const levels = [];
  const visit = (path, level, transform, depth, parent) => {
    const drawn = layoutOf(level);
    if (!drawn) return;
    const entry = { path, level, layout: drawn, transform, nodes: [], wires: [], parent };
    levels.push(entry);
    for (const node of drawn.nodes) {
      const rect = screenRect(transform, node);
      if (rect.w < MIN_PX || !intersects(rect, screen)) continue;
      entry.nodes.push(node);
    }
    for (const wire of drawn.wires) {
      const box = wireBox(wire);
      const w = (box.x1 - box.x0) * transform.k;
      const h = (box.y1 - box.y0) * transform.k;
      if (Math.max(w, h) < MIN_PX) continue;
      const at = screenRect(transform, { x: box.x0, y: box.y0, w: 0, h: 0 });
      if (intersects({ x: at.x, y: at.y, w: w + 1, h: h + 1 }, screen)) entry.wires.push(wire);
    }
    if (depth >= MAX_DEPTH) return;
    for (const node of entry.nodes) {
      if (!node.composite) continue;
      const child = `${path}/${node.name}`;
      const width = node.w * transform.k;
      const opens = pinned.has(child) || width >= (open.has(child) ? CLOSE_PX : OPEN_PX);
      const inner = opens ? layoutOf(node.module) : null;
      if (!inner) continue;
      // A level on the view's chain has its transform from the view itself.
      const next = along.get(child) ?? compose(transform, inside(node, inner.bounds));
      visit(child, node.module, next, depth + 1, entry);
    }
  };
  visit('', '', along.get('') ?? view, 0, null);
  const drawn = { levels, open: new Set(levels.slice(1).map((l) => l.path)) };
  for (const entry of levels) entry.behind = behind(entry, drawn.open, screen);
  // Re-rooted at the deepest level drawn under the centre.
  const deepest = breadcrumbs(drawn, viewport).at(-1).path;
  const root = levels.find((l) => l.path === deepest) ?? levels[0];
  drawn.view = root ? { ...root.transform, path: root.path } : view;
  return drawn;
}

/** How far past the screen (px) a stroke may reach. */
const STROKE_PX = 16;

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

/** The breadcrumbs: the top level, then each open box under the view's centre, `[{path, level, name}]`. */
export function breadcrumbs(drawn, viewport) {
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
 * first target (`/drums.kick_decay` → `/drums/kick/amp_env.decay_ms`); null
 * if it reaches none.
 */
export function planPort(graph, address, { parameter = false } = {}) {
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

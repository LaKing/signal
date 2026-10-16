// The schematic's semantic zoom (M2.9-1, ADR 0026 §2): what is drawn for a
// view. The top level's layout is drawn through the view; a composite whose
// box is wider on screen than OPEN_PX (and stays open down to CLOSE_PX), or
// that was double-clicked, opens in place: its own layout is drawn scaled
// into its box, recursively. Only what is on screen is listed, so the
// flattened plan is never drawn whole. Breadcrumbs follow the open boxes
// under the view's centre. No DOM.

import { HEADER, PAD } from './schematic-layout.js';
import { compose, screenRect } from './schematic-view.js';

/** A composite opens past this on-screen width (px), and closes below CLOSE_PX. */
export const OPEN_PX = 360;
export const CLOSE_PX = 300;
/** Smaller than this on screen (px), a node is not drawn. */
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
 * The levels to draw: `[{path, level, layout, transform, nodes, wires,
 * parent}]`, outermost first. `layoutOf(level)` gives a level's layout
 * (`""`: the top level, else a composite's name); `path` is an open
 * composite instance's address (`""` for the top level); `open` is the
 * set of paths open in the last scene, `pinned` those double-clicked.
 */
export function scene({ layoutOf, view, viewport, open = new Set(), pinned = new Set() }) {
  const screen = { x: 0, y: 0, w: viewport.w, h: viewport.h };
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
      const xs = wire.points.map((p) => p.x * transform.k + transform.x);
      const ys = wire.points.map((p) => p.y * transform.k + transform.y);
      const rect = { x: Math.min(...xs), y: Math.min(...ys), w: Math.max(...xs) - Math.min(...xs) + 1, h: Math.max(...ys) - Math.min(...ys) + 1 };
      if (intersects(rect, screen)) entry.wires.push(wire);
    }
    if (depth >= MAX_DEPTH) return;
    for (const node of entry.nodes) {
      if (!node.composite) continue;
      const child = `${path}/${node.name}`;
      const width = node.w * transform.k;
      const opens = pinned.has(child) || width >= (open.has(child) ? CLOSE_PX : OPEN_PX);
      const inner = opens ? layoutOf(node.module) : null;
      if (inner) visit(child, node.module, compose(transform, inside(node, inner.bounds)), depth + 1, entry);
    }
  };
  visit('', '', view, 0, null);
  return { levels, open: new Set(levels.slice(1).map((l) => l.path)) };
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

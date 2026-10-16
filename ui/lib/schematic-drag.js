// A box being dragged on the schematic, and what follows it (M2.9-1; the
// owner's "while moving parts around the cables should follow",
// 2026-10-16), or a group of boxes of one level selected together and
// dragged by one of them (2026-10-17): the boxes themselves, the wires that change with it
// (`movedWires`), and an open composite's inside, in every drawn instance
// of the level (a composite's layout is its type's, so each open instance
// of it shows the drag). Only attributes of elements already drawn change
// (a box's and a level's `transform`, a wire's `d`), each only when its
// value does, so a redraw with nothing moved writes nothing; `restore`
// puts every one back. Elements are anything with `getAttribute`,
// `setAttribute` and `removeAttribute`: no DOM here.

import { movedWires, routed } from './schematic-layout.js';
import { bakedPath, inBounds } from './schematic-draw.js';
import { matrix } from './schematic-view.js';

export class DragFollow {
  /**
   * The instance `name` of level `level` (`""` the top level, else a
   * composite's name); `name` may be a list of instances of the level, the
   * first the one dragged, the others moved by as much.
   */
  constructor(level, name) {
    this.level = level;
    this.names = Array.isArray(name) ? [...name] : [name];
    this.name = this.names[0];
    /** Wire elements written → `{was, key}`: their own `d`, and the route written. */
    this.wires = new Map();
    /** Box elements given a transform. */
    this.boxes = new Set();
    /** Groups of levels inside moved → their drawn group (its `placed`). */
    this.inner = new Map();
    this.cache = null;
    /** The group's nodes in the layout last applied: `{layout, node, nodes, names}`. */
    this.group = null;
  }

  /**
   * The box at `to` (`{x, y}`, layout units) in `layout` (the level's, as
   * now laid out), the group's other boxes moved by as much, in each of
   * `levels` (the scene's, `{path, level, transform}`) drawn in `groups` (path → `{g, baked, placed, wireCache,
   * nodeCache}`, as <signal-schematic> keeps them). Returns how many
   * attributes it wrote.
   */
  apply({ layout, levels, groups, to }) {
    // The group's nodes, found once per layout (a group of thousands: no search per frame).
    if (this.group?.layout !== layout) {
      const byName = new Map((layout?.nodes ?? []).filter((n) => n.kind === 'instance').map((n) => [n.name, n]));
      this.group = { layout, node: byName.get(this.name), nodes: this.names.map((name) => byName.get(name)).filter(Boolean), names: new Set(this.names) };
    }
    const { node, nodes, names } = this.group;
    if (!node) return 0;
    const dx = to.x - node.x;
    const dy = to.y - node.y;
    const cache = this.cache;
    if (!cache || cache.layout !== layout || cache.x !== to.x || cache.y !== to.y) {
      const moves = new Map(nodes.map((n) => [n.id, { x: n.x + dx, y: n.y + dy }]));
      this.cache = { layout, x: to.x, y: to.y, moved: movedWires(layout, moves), geometry: new Map() };
    }
    const { moved, geometry } = this.cache;
    let writes = 0;
    const set = (element, name, value) => {
      if (element.getAttribute(name) === value) return;
      element.setAttribute(name, value);
      writes++;
    };
    const wires = new Set();
    const boxes = new Set();
    const inner = new Set();
    for (const entry of levels) {
      if (entry.level !== this.level) continue;
      const group = groups.get(entry.path);
      if (!group?.baked) continue;
      const m = group.baked.m;
      for (const each of nodes) {
        const box = group.nodeCache.get(each.id)?.el;
        if (!box) continue;
        set(box, 'transform', `translate(${dx * m} ${dy * m})`);
        boxes.add(box);
      }
      for (const route of moved) {
        const element = group.wireCache.get(route.wire.id)?.el;
        if (!element) continue;
        wires.add(element);
        const had = this.wires.get(element);
        if (had?.key === route.key) continue;
        if (!geometry.has(route)) geometry.set(route, routed(route));
        const { points, curve } = geometry.get(route);
        this.wires.set(element, { was: had ? had.was : element.getAttribute('d'), key: route.key });
        element.setAttribute('d', bakedPath(group.baked, points, curve));
        writes++;
      }
      // The levels inside them, moved on screen as the boxes are; one that
      // would leave SVG's bounds (a zoom since) stays where it is drawn.
      const prefix = `${entry.path}/`;
      for (const [path, drawn] of groups) {
        if (!path.startsWith(prefix) || !drawn.placed) continue;
        const child = path.slice(prefix.length).split('/')[0];
        if (!names.has(child)) continue;
        const placed = innerOffset(drawn.placed, { x: dx, y: dy }, entry.transform.k);
        set(drawn.g, 'transform', matrix(inBounds(placed) ? placed : drawn.placed));
        this.inner.set(drawn.g, drawn);
        inner.add(drawn.g);
      }
    }
    // What no longer follows goes back.
    for (const [element, { was }] of this.wires) {
      if (wires.has(element)) continue;
      element.setAttribute('d', was);
      this.wires.delete(element);
      writes++;
    }
    for (const box of this.boxes) {
      if (boxes.has(box)) continue;
      box.removeAttribute('transform');
      writes++;
    }
    this.boxes = boxes;
    for (const [g, drawn] of this.inner) {
      if (inner.has(g)) continue;
      set(g, 'transform', matrix(drawn.placed));
      this.inner.delete(g);
    }
    return writes;
  }

  /** Everything back where it was drawn (a cancelled drag). */
  restore() {
    for (const [element, { was }] of this.wires) element.setAttribute('d', was);
    for (const box of this.boxes) box.removeAttribute('transform');
    for (const [g, drawn] of this.inner) g.setAttribute('transform', matrix(drawn.placed));
    this.wires.clear();
    this.boxes.clear();
    this.inner.clear();
    this.cache = null;
    this.group = null;
  }
}

/** A level's SVG matrix `placed` moved by `offset` layout units of a parent drawn at scale `k`. */
export function innerOffset(placed, offset, k) {
  return { ...placed, x: placed.x + offset.x * k, y: placed.y + offset.y * k };
}

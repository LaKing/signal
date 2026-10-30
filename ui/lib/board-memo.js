// When a board level is laid out again (ADR 0047 phase 4; the owner: "when
// I click an element and drag it, the wiring under it gets redrawn"): the
// layout depends on the level's structure and the stored positions only,
// so a `graph` whose structure is the same (a parameter's value, the
// selection, a hover, a meter: none of them is in the key) keeps the very
// layout object drawn, and one in which only some instances' stored
// positions changed (a drop) moves those and reroutes only their wires
// (`rerouteMoved`). Anything else is a full layout. Pure; no DOM.

import { rerouteMoved } from './board-reroute.js';

/** Fields of a level that are values, not structure: never in its key. */
const VALUES = new Set(['default', 'value', 'values', 'current']);

/** The key of a level's structure (`graph`'s level, as `levelData` gives it): its JSON without values. */
export function structureKey(level) {
  return JSON.stringify(level ?? null, (k, v) => (VALUES.has(k) ? undefined : v));
}

/** What a level laid out around a die depends on of it: its rectangle and its interface pins. */
export function dieSignature(die) {
  if (!die) return 'none';
  const frames = die.nodes.filter((n) => n.kind !== 'instance' && n.kind !== 'control');
  return JSON.stringify([die.bounds, frames.map((n) => [n.id, n.ins.map((p) => [p.name, p.x, p.y]), n.outs.map((p) => [p.name, p.x, p.y])])]);
}

/**
 * The instances whose stored position differs between `before` and `after`
 * (`{name: {x, y}}`), as a Map of their node id in `layout` → the new
 * position; null when a position went (a reset: a full layout) or names an
 * instance not in the layout.
 */
export function positionMoves(before, after, layout) {
  const byName = new Map(layout.nodes.filter((n) => n.kind === 'instance').map((n) => [n.name, n]));
  const moves = new Map();
  for (const name of new Set([...Object.keys(before ?? {}), ...Object.keys(after ?? {})])) {
    const a = before?.[name];
    const b = after?.[name];
    if (a && b && a.x === b.x && a.y === b.y) continue;
    if (!b) return null;
    const node = byName.get(name);
    if (!node) return null;
    if (node.x !== b.x || node.y !== b.y) moves.set(node.id, { x: b.x, y: b.y });
  }
  return moves;
}

/**
 * A level's layout given what it was last laid out from (`prev`: `{key,
 * stored, layout, dies}` or null) and now (`key`, `stored`), `full()` the
 * full layout, `diesNow(prev.dies)` whether the dies it was laid around
 * still have their signatures. `{layout, how}`: `how` 'kept' (the same
 * object), 'rerouted' (only the moved instances' wires) or 'full'.
 */
export function relayout(prev, key, stored, full, diesNow = () => true) {
  if (!prev || prev.key !== key || !prev.layout || !diesNow(prev.dies)) return { layout: full(), how: 'full' };
  const moves = positionMoves(prev.stored, stored, prev.layout);
  if (moves === null) return { layout: full(), how: 'full' };
  if (!moves.size) return { layout: prev.layout, how: 'kept' };
  if (!prev.layout.board) return { layout: full(), how: 'full' };
  return { layout: rerouteMoved(prev.layout, moves), how: 'rerouted' };
}

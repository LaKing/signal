// What a click or a hover on the schematic is over (M2.9-1): a port, an
// instance box, a wire or the background, in a scene as `scene()` gives it.
// The innermost level drawn is tried first; within a level a port beats its
// box and a box beats a wire. Tolerances are screen pixels, so they hold at
// every zoom, and a wire is hit on its curve, not its flattened points. No
// DOM.

import { INPUTS, OUTPUTS } from './schematic-layout.js';
import { toLocal } from './schematic-view.js';
import { curveDistance, distanceToSegment } from './schematic-wire.js';

/** A port is hit within this radius (px). */
export const PORT_PX = 6;
/** A wire is hit within this distance (px). */
export const WIRE_PX = 4;

/** Whether `wire` passes within `limit` (level units) of `p`: its curve when it has one, else its polyline. */
function nearWire(wire, p, limit) {
  if (wire.curve) return curveDistance(wire.curve, p, limit) <= limit;
  for (let s = 1; s < wire.points.length; s++) if (distanceToSegment(p, wire.points[s - 1], wire.points[s]) <= limit) return true;
  return false;
}

/** The address of a written endpoint (`kick.out`, `/a.out`) at level `path`. */
export function endpointAddress(path, written) {
  return `${path}/${String(written).replace(/^\//, '')}`;
}

/**
 * What screen point `point` is over: `{kind: 'port' | 'instance' | 'wire' |
 * 'background', path, level, node?, port?, wire?, address?}`. A port's and
 * an instance's `address` is the concrete one (`/drums/kick.out`); a wire
 * gives `from` and `to`.
 */
export function hitTest(drawn, point) {
  for (let i = drawn.levels.length - 1; i >= 0; i--) {
    const entry = drawn.levels[i];
    const local = toLocal(entry.transform, point);
    const scale = entry.transform.k;
    const base = { path: entry.path, level: entry.level };
    for (const node of entry.nodes) {
      if (node.kind !== 'instance') continue;
      for (const port of [...node.ins, ...node.outs]) {
        if (Math.hypot(port.x - local.x, port.y - local.y) * scale <= PORT_PX) {
          const address = `${entry.path}/${node.name}.${port.name}`;
          return { kind: 'port', ...base, node, port, address };
        }
      }
    }
    for (const node of entry.nodes) {
      if (node.kind !== 'instance') continue;
      if (local.x >= node.x && local.x <= node.x + node.w && local.y >= node.y && local.y <= node.y + node.h) {
        // An open composite's inside was tried first; its box still counts.
        return { kind: 'instance', ...base, node, address: `${entry.path}/${node.name}` };
      }
    }
    for (const wire of entry.wires) {
      if (nearWire(wire, local, WIRE_PX / scale)) {
        return {
          kind: 'wire',
          ...base,
          wire,
          // An export's end on the frame is the composite instance's port.
          from: wire.from.node === INPUTS ? `${entry.path}.${wire.from.port}` : endpointAddress(entry.path, wire.written.from),
          to: wire.to.node === OUTPUTS ? `${entry.path}.${wire.to.port}` : endpointAddress(entry.path, wire.written.to),
        };
      }
    }
  }
  return { kind: 'background', path: '', level: '' };
}

/**
 * `selection` (a hit) found again in the levels as now laid out
 * (`layoutOf(level)`, after a new `graph`): the same instance, port or
 * connection by name, with its new node, pin or wire; null when it is no
 * longer there. Its whole path is followed first, from the top level: every
 * ancestor instance must still exist and still be a composite whose level
 * leads to the selection's (a composite type used elsewhere proves
 * nothing about this instance).
 */
export function reselect(selection, layoutOf) {
  if (!selection || selection.kind === 'background') return null;
  let level = '';
  for (const name of String(selection.path ?? '').split('/').filter(Boolean)) {
    const ancestor = layoutOf(level)?.nodes.find((n) => n.kind === 'instance' && n.name === name);
    if (!ancestor?.composite) return null;
    level = ancestor.module;
  }
  if (level !== selection.level) return null;
  const drawn = layoutOf(level);
  if (!drawn) return null;
  if (selection.kind === 'wire') {
    const { from, to } = selection.wire.written;
    const wire = drawn.wires.find((w) => w.written.from === from && w.written.to === to);
    return wire ? { ...selection, wire } : null;
  }
  const node = drawn.nodes.find((n) => n.kind === 'instance' && n.name === selection.node.name);
  if (!node) return null;
  if (selection.kind === 'instance') return { ...selection, node };
  const port = [...node.ins, ...node.outs].find((p) => p.name === selection.port.name && p.direction === selection.port.direction);
  return port ? { ...selection, node, port } : null;
}

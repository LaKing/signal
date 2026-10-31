// What a click or a hover on the schematic is over (M2.9-1): a port, an
// instance box, a wire or the background, in a scene as `scene()` gives it.
// The innermost level drawn is tried first; within a level a port beats its
// box and a box beats a wire, and of the ports within reach the nearest
// (2026-10-17: pins are meant to be easy to hit; the reach shrinks with a
// box small on screen). Tolerances are screen pixels, so they hold at
// every zoom, and a wire is hit on its curve, not its flattened points. No
// DOM. M2.13-2: a level drawn with bundles (`entry.bundles`, ui/lib/
// schematic-bundle.js) is hit on what is drawn, the topmost first: a solo
// wire above its bundle, a bundle as a `bundle` hit carrying its wires'.
// ADR 0047 §8 (phase 3): a die is hit once it is drawn (its text at its
// threshold on screen); there is no frame and no × (Escape zooms out).

import { nearBundle } from './schematic-bundle.js';
import { groupAnchors } from './schematic-groups.js';
import { INPUTS, OUTPUTS } from './schematic-layout.js';
import { belowThreshold } from './board-zoom.js';
import { controlHit } from './schematic-controls.js';
import { toLocal } from './schematic-view.js';
import { curveDistance, distanceToSegment } from './schematic-wire.js';

/**
 * A port is hit within this radius (px): three times its drawn radius at
 * scale 1; never more than a quarter of its box's height on screen, so a
 * box zoomed out is not all pins.
 */
export const PORT_PX = 9;
/** A wire is hit within this distance (px). */
export const WIRE_PX = 4;
/**
 * ADR 0047 phase 4 (the owner: "not every element is selectable"; phase 5:
 * "only if I hit exactly the middle"): a pin is hit within at least this
 * (px) however small its box on screen, and a pin
 * of a box with no height (a die's parameters' strip) or no width (its
 * interface's columns) within PORT_PX.
 */
export const MIN_PORT_PX = 8;

/** A side pin's name runs this far into its box (layout units: its gap, its 9-unit text per character), its leg this far out, its row this high each way. */
const PIN_LABEL_GAP = 6;
const PIN_LABEL_CHAR = 5.4;
const PIN_LEG = 5;
const PIN_ROW_HALF = 7;

/** How far (level units) `wire` passes from `p`: a board's on its drawn corners (its turns are rounded to a few px on screen), else its curve, else its polyline; Infinity beyond `limit`. */
function wireDistance(wire, p, limit) {
  const corners = wire.orth ? (wire.display ?? wire.corners) : null;
  if (corners) {
    let d = Infinity;
    for (let s = 1; s < corners.length; s++) d = Math.min(d, distanceToSegment(p, corners[s - 1], corners[s]));
    return d <= limit ? d : Infinity;
  }
  if (wire.curve) {
    const d = curveDistance(wire.curve, p, limit);
    return d <= limit ? d : Infinity;
  }
  let d = Infinity;
  for (let s = 1; s < wire.points.length; s++) d = Math.min(d, distanceToSegment(p, wire.points[s - 1], wire.points[s]));
  return d <= limit ? d : Infinity;
}

function nearWire(wire, p, limit) {
  return wireDistance(wire, p, limit) <= limit;
}

/** A pin's reach (px) on box `node` drawn at scale `scale`. */
export function portReach(node, scale) {
  if (!(node.h > 0) || !(node.w > 0)) return PORT_PX;
  return Math.min(PORT_PX, Math.max(MIN_PORT_PX, (node.h * scale) / 4));
}

/**
 * The hit of a board's leg (a plate control's binding, ADR 0047 §5): no
 * connection of the project, so the pin it reaches on its chip (what it
 * sets, or reads), whose net it is.
 */
function legHit(entry, wire) {
  const chipEnd = String(wire.from.node).startsWith('@control:') ? { id: wire.to.node, port: wire.to.port, out: false } : { id: wire.from.node, port: wire.from.port, out: true };
  const node = entry.layout.nodes.find((n) => n.id === chipEnd.id);
  const port = (chipEnd.out ? node?.outs : node?.ins)?.find((p) => p.name === chipEnd.port);
  return node && port ? portHit(entry, node, port) : null;
}

/** The address of a written endpoint (`kick.out`, `/a.out`) at level `path`. */
export function endpointAddress(path, written) {
  return `${path}/${String(written).replace(/^\//, '')}`;
}

/** The hit of instance `node` drawn at level `entry` (`{path, level}`). */
export function instanceHit(entry, node) {
  return { kind: 'instance', path: entry.path, level: entry.level, node, address: `${entry.path}/${node.name}` };
}

/**
 * The hit of pin `port` of instance `node` at level `entry`; of a
 * composite's frame (`node` its inputs or outputs, ADR 0030 §1) the
 * composite instance's port, which is what the frame's pin stands for
 * (`/drums.trigger` at level `/drums`).
 */
export function portHit(entry, node, port) {
  const address = node.kind === 'instance' ? `${entry.path}/${node.name}.${port.name}` : `${entry.path}.${port.name}`;
  return { kind: 'port', path: entry.path, level: entry.level, node, port, address };
}

/** The hit of `wire` (a layout's) at level `entry`: its ends' addresses as `from` and `to`. */
export function wireHit(entry, wire) {
  return {
    kind: 'wire',
    path: entry.path,
    level: entry.level,
    wire,
    // An export's end on the frame is the composite instance's port.
    from: wire.from.node === INPUTS ? `${entry.path}.${wire.from.port}` : endpointAddress(entry.path, wire.written.from),
    to: wire.to.node === OUTPUTS ? `${entry.path}.${wire.to.port}` : endpointAddress(entry.path, wire.written.to),
  };
}

/**
 * The hit of a collapsed port group's pin (M2.13-2, ui/lib/schematic-groups.js)
 * of box `node` at level `entry`: never one member, all of them as `ports`
 * (their hits), the box's address and the group's `label`. Not a patch end.
 */
export function groupHit(entry, node, group) {
  const at = new Map([...node.ins, ...node.outs].map((p) => [p.name, p]));
  // Each member knows its group (`group`, its label), so that a selection of them is shown and traced as the group.
  const ports = group.pins
    .map((p) => portHit(entry, node, at.get(p.name)))
    .filter((h) => h.port)
    .map((h) => ({ ...h, group: group.label }));
  return { kind: 'group', path: entry.path, level: entry.level, node, group, label: group.label, ports, address: `${entry.path}/${node.name}` };
}

/** The hit of `bundle` (ui/lib/schematic-bundle.js) at level `entry`: its wires' hits as `wires`. */
export function bundleHit(entry, bundle) {
  return { kind: 'bundle', path: entry.path, level: entry.level, bundle, wires: bundle.wires.map((w) => wireHit(entry, w)) };
}

/**
 * What screen point `point` is over: `{kind: 'port' | 'instance' | 'wire' |
 * 'bundle' | 'group' | 'background', path, level, node?, port?,
 * wire?, bundle?, address?}` (a collapsed port group: `groupHit`). A port's and an instance's `address` is the concrete one
 * (`/drums/kick.out`); a wire gives `from` and `to`; a bundle its wires'
 * hits as `wires`.
 */
export function hitTest(drawn, point) {
  for (let i = drawn.levels.length - 1; i >= 0; i--) {
    const entry = drawn.levels[i];
    // A die not drawn yet (ADR 0047 §8: its text under its threshold on screen) is not hit: its package is.
    if (entry.parent && belowThreshold(entry.transform.k)) continue;
    const local = toLocal(entry.transform, point);
    const scale = entry.transform.k;
    // An open composite's frame (M2.13-3) lies over its neighbours: inside it, only its own pins and box are hit.
    const frame = entry.layout?.opened ? entry.nodes.find((n) => n.kind === 'instance' && n.name === entry.layout.opened) : null;
    const covered = frame && local.x >= frame.x && local.x <= frame.x + frame.w && local.y >= frame.y && local.y <= frame.y + frame.h;
    const nodes = covered ? [frame] : entry.nodes;
    let best = null;
    for (const node of nodes) {
      // A composite's frame pins too (ADR 0030 §1: a trace starts there); its boxes are not hit.
      // A panel control is hit whole, its pins with it (ui/lib/schematic-controls.js).
      if (node.kind === 'control' || (node.kind !== 'instance' && !entry.path)) continue;
      // A box small on screen (zoomed out) keeps its body: its pins reach less, never under MIN_PORT_PX (`portReach`).
      const reach = portReach(node, scale);
      // M2.13-2: a collapsed box (`entry.collapsed`) is hit as drawn: a group of several by its one pin, its members' rows not at all.
      const anchors = entry.collapsed && node.groups ? groupAnchors(node) : null;
      const seen = new Set();
      for (const port of [...node.ins, ...node.outs]) {
        const anchor = anchors?.get(port.name);
        if (anchor) {
          if (seen.has(anchor.group)) continue;
          seen.add(anchor.group);
          const d = Math.hypot(anchor.x - local.x, anchor.y - local.y) * scale;
          if (d <= reach && (!best || d < best.d)) best = { d, node, port, anchor };
          continue;
        }
        const d = Math.hypot(port.x - local.x, port.y - local.y) * scale;
        if (d <= reach && (!best || d < best.d)) best = { d, node, port };
      }
    }
    // ADR 0047 phase 5 (the owner: "a click selects the composite; the whole solid area should be the pin's"): on a package's
    // edge its pins win over its box: anywhere on its connector's pill the nearest of its pins (by x), anywhere on a side pin's
    // leg and name the pin.
    if (!best && entry.layout?.board) {
      const pad = MIN_PORT_PX / scale;
      for (const node of nodes) {
        if (node.kind !== 'instance' || best) continue;
        if (node.connector && local.x >= node.x + node.connector.dx0 - pad && local.x <= node.x + node.connector.dx1 + pad && Math.abs(local.y - node.y) <= 3 + pad) {
          for (const port of node.ins) {
            if (port.side !== 'top') continue;
            const d = Math.abs(port.x - local.x);
            if (!best || d < best.d) best = { d, node, port };
          }
          continue;
        }
        if (entry.collapsed && node.groups) continue;
        for (const port of [...node.ins, ...node.outs]) {
          if (port.side === 'top') continue;
          const out = port.direction === 'out';
          const name = PIN_LABEL_GAP + String(port.name ?? '').length * PIN_LABEL_CHAR;
          const [x0, x1] = out ? [port.x - name, port.x + PIN_LEG + pad] : [port.x - PIN_LEG - pad, port.x + name];
          if (local.x >= x0 && local.x <= x1 && Math.abs(local.y - port.y) <= PIN_ROW_HALF) {
            best = { d: 0, node, port };
            break;
          }
        }
      }
    }
    if (best?.anchor) return groupHit(entry, best.node, best.anchor.group);
    if (best) return portHit(entry, best.node, best.port);
    // A panel control (its pins on its edges, within reach of them too: a via's on its bottom edge as well, ADR 0047 phase 4).
    for (const node of nodes) {
      if (node.kind !== 'control') continue;
      const reach = portReach(node, scale) / scale;
      if (local.x >= node.x - reach && local.x <= node.x + node.w + reach && local.y >= node.y - reach && local.y <= node.y + node.h + reach) return controlHit(entry, node);
    }
    for (const node of nodes) {
      if (node.kind !== 'instance') continue;
      // An open composite's inside was tried first; its box still counts.
      if (local.x >= node.x && local.x <= node.x + node.w && local.y >= node.y && local.y <= node.y + node.h) return instanceHit(entry, node);
    }
    // A board (ADR 0047 phase 4): every wire drawn is hit, the nearest within reach (a ribbon's members a few units apart), a leg as its chip's pin.
    if (entry.layout?.board) {
      let near = null;
      const list = entry.bundles ?? entry.wires;
      for (let j = list.length - 1; j >= 0; j--) {
        const item = list[j];
        const wires = item.kind === 'bundle' ? item.wires : [item];
        for (const wire of wires) {
          const d = wireDistance(wire, local, WIRE_PX / scale);
          if (d < (near?.d ?? Infinity)) near = { d, wire };
        }
      }
      if (near) {
        if (near.wire.kind === 'binding') {
          const leg = legHit(entry, near.wire);
          if (leg) return leg;
        } else return wireHit(entry, near.wire);
      }
      continue;
    }
    if (entry.bundles) {
      for (let j = entry.bundles.length - 1; j >= 0; j--) {
        const item = entry.bundles[j];
        // A control's binding wire is drawn, never hit: it is no connection of the project.
        if (item.kind === 'binding' || (item.kind === 'bundle' && item.wires.every((w) => w.kind === 'binding'))) continue;
        if (item.kind === 'bundle' ? nearBundle(item, local, WIRE_PX / scale) : nearWire(item, local, WIRE_PX / scale)) {
          return item.kind === 'bundle' ? bundleHit(entry, item) : wireHit(entry, item);
        }
      }
    } else for (const wire of entry.wires) if (wire.kind !== 'binding' && nearWire(wire, local, WIRE_PX / scale)) return wireHit(entry, wire);
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
  // A panel control by its id (its place in the panel).
  if (selection.kind === 'control') {
    const node = drawn.nodes.find((n) => n.kind === 'control' && n.id === selection.node.id);
    return node ? { ...selection, node, address: node.binds[0]?.address ?? '', addresses: node.binds.map((b) => b.address) } : null;
  }
  // A frame's pin by its frame (`@inputs`, `@outputs`).
  const frame = selection.node.kind !== 'instance';
  const node = drawn.nodes.find((n) => (frame ? n.id === selection.node.id : n.kind === 'instance' && n.name === selection.node.name));
  if (!node) return null;
  if (selection.kind === 'instance') return { ...selection, node };
  const port = [...node.ins, ...node.outs].find((p) => p.name === selection.port.name && p.direction === selection.port.direction);
  return port ? { ...selection, node, port } : null;
}

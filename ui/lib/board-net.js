// A pin's net across a package's edge (ADR 0047 phase 4; the owner: "the
// crossings at the chip boundary … do not get the click"): a pin selected
// (a package's, a die's interface pin at the same point, a via's) shows
// the wires of its net, the one outside and its continuation inside
// together. Pure; no DOM.

import { INPUTS, OUTPUTS, PARAMS } from './schematic-layout.js';

/** Whether `wire` reaches pin `port` (`{name, direction}`) of node `id`. */
const reaches = (wire, id, port) => (port.direction === 'out' ? wire.from.node === id && wire.from.port === port.name : wire.to.node === id && wire.to.port === port.name);

/**
 * The wires of the net of a port hit (`{path, node, port}`) among the drawn
 * `levels` (a scene's, `{path, layout, parent}`): `[{path, wire}]`, those
 * at its own pin, and across its package's edge: a package's pin, the die's
 * wires from (an input, a parameter) or to (an output) its interface pin of
 * that name; a die's interface pin, the wires at its package's pin outside.
 */
export function portNet(levels, hit) {
  if (hit?.kind !== 'port' || !hit.node || !hit.port) return [];
  const out = [];
  const entry = levels.find((l) => l.path === hit.path);
  if (!entry) return out;
  for (const w of entry.layout.wires) if (reaches(w, hit.node.id, hit.port)) out.push({ path: entry.path, wire: w });
  const name = hit.port.name;
  if (hit.node.kind === 'instance') {
    // Into its die, when it is drawn.
    const inner = levels.find((l) => l.parent === entry && l.path === `${entry.path}/${hit.node.name}`);
    if (inner) {
      for (const w of inner.layout.wires) {
        const into = hit.port.direction === 'in' && (w.from.node === INPUTS || w.from.node === PARAMS) && w.from.port === name;
        const from = hit.port.direction === 'out' && w.to.node === OUTPUTS && w.to.port === name;
        if (into || from) out.push({ path: inner.path, wire: w });
      }
    }
  } else if (entry.parent && [INPUTS, OUTPUTS, PARAMS].includes(hit.node.id)) {
    // Out to its package's pin.
    const pkg = entry.parent.layout.nodes.find((n) => n.kind === 'instance' && n.name === entry.path.split('/').at(-1));
    if (pkg) {
      const direction = hit.node.id === OUTPUTS ? 'out' : 'in';
      for (const w of entry.parent.layout.wires) if (reaches(w, pkg.id, { name, direction })) out.push({ path: entry.parent.path, wire: w });
    }
  }
  return out;
}

/**
 * A selected wire (a wire hit, `{path, wire}`) followed into the dies drawn
 * (2026-10-31, the owner: "follow a signal all the way into the chip"):
 * `[{path, wire}]`, the wires inside each open package at its ends that
 * carry it on from its interface pin of that name (an input or a
 * parameter at its far end, an output at its near end), and so on into
 * the dies drawn inside those, recursively. The wire itself is not listed.
 */
export function wireNet(levels, hit) {
  const out = [];
  const seen = new Set();
  const follow = (entry, wire) => {
    for (const [end, into] of [
      [wire.to, true],
      [wire.from, false],
    ]) {
      const node = entry.layout.nodes.find((n) => n.kind === 'instance' && n.id === end?.node);
      const inner = node && levels.find((l) => l.parent === entry && l.path === `${entry.path}/${node.name}`);
      if (!inner) continue;
      for (const w of inner.layout.wires) {
        const on = into ? (w.from.node === INPUTS || w.from.node === PARAMS) && w.from.port === end.port : w.to.node === OUTPUTS && w.to.port === end.port;
        if (!on || seen.has(w)) continue;
        seen.add(w);
        out.push({ path: inner.path, wire: w });
        follow(inner, w);
      }
    }
  };
  const entry = hit?.kind === 'wire' ? levels.find((l) => l.path === (hit.path ?? '')) : null;
  if (entry && hit.wire) follow(entry, hit.wire);
  return out;
}

/**
 * The junctions of a board's `wires` (the note "Layered placement and
 * channel routing", 2026-10-31: a fan-out is a tree, its branches marked):
 * where the drawn wires of one net (the wires from one output pin, not a
 * plate's legs) meet from three ways or more, `[{x, y, wire}]` (`wire` one
 * of its wires, for its tone), each point once, in the wires' order. A
 * wire drawn in a lane (`display`) is drawn apart from its route: its net
 * has none. Pure.
 */
export function junctions(wires) {
  const nets = new Map();
  for (const w of wires) {
    if (w.kind === 'binding' || w.labelled || !w.corners) continue;
    const key = `${w.from.node}\n${w.from.port}`;
    if (!nets.has(key)) nets.set(key, []);
    nets.get(key).push(w);
  }
  const out = [];
  for (const list of nets.values()) {
    if (list.length < 2 || list.some((w) => w.display)) continue;
    const segs = [];
    for (const w of list) for (let i = 1; i < w.corners.length; i++) segs.push([w.corners[i - 1], w.corners[i]]);
    const seen = new Set();
    for (const w of list) {
      for (let i = 1; i < w.corners.length - 1; i++) {
        const p = w.corners[i];
        const key = `${p.x},${p.y}`;
        if (seen.has(key)) continue;
        seen.add(key);
        // The ways out of p along the net's segments: a segment through it two, one ending at it one.
        const ways = new Set();
        for (const [a, b] of segs) {
          const h = a.y === b.y;
          if (h ? a.y !== p.y || p.x < Math.min(a.x, b.x) || p.x > Math.max(a.x, b.x) : a.x !== p.x || p.y < Math.min(a.y, b.y) || p.y > Math.max(a.y, b.y)) continue;
          for (const q of [a, b]) {
            if (q.x === p.x && q.y === p.y) continue;
            ways.add(h ? (q.x > p.x ? 'E' : 'W') : q.y > p.y ? 'S' : 'N');
          }
        }
        if (ways.size >= 3) out.push({ x: p.x, y: p.y, wire: w });
      }
    }
  }
  return out;
}

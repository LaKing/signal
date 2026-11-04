// A signal's whole way through the project (the owner's, 2026-11-04: "the
// wire followed all the way"): from any port, the net it belongs to across
// every level and layer. A net is the ports joined by connections, at the
// top level and inside each composite instance at any depth, and through
// each composite's exports (an input or a parameter on to its targets
// inside, an output from its source inside), so a selection marks the
// Wiring's cable, the package's pin, the Engine's wires on both sides of a
// chip's edge, a control's leg and its way to the parameters it sets.
// Never through an instance (an input does not join its outputs). Full
// addresses as the person knows them (`/synth.gate`, `/synth/fenv.gate`;
// ui/lib/plane-engine-edit.js `fullAddress`). Pure; no DOM.

import { fullAddress, norm, wireKey } from './plane-engine-edit.js';

/**
 * The links of `graph` (`{top: {instances, connections}, composites}`):
 * `Map(address → [{to, key}])` both ways, `key` the connection's `wireKey`
 * as the Engine draws it (an export's from the chip's pin to its target
 * inside, or from its source inside to the chip's pin). Built once per graph.
 */
export function netLinks(graph) {
  if (!graph) return new Map();
  if (CACHE.has(graph)) return CACHE.get(graph);
  const links = new Map();
  const link = (a, b) => {
    const key = wireKey(a, b);
    for (const [x, y] of [[a, b], [b, a]]) {
      if (!links.has(x)) links.set(x, []);
      links.get(x).push({ to: y, key });
    }
  };
  for (const c of graph.top?.connections ?? []) link(norm(c.from), norm(c.to));
  // Each composite instance, at its address, deeper inside it in turn (a composite never holds itself; a depth guard all the same).
  const walk = (instances, prefix, depth) => {
    if (depth > 16) return;
    for (const inst of instances ?? []) {
      const def = graph.composites?.[inst.module];
      if (!def) continue;
      const at = prefix ? `${prefix}/${inst.name}` : `/${inst.name}`;
      const full = (a) => fullAddress(at, a);
      for (const c of def.connections ?? []) link(full(c.from), full(c.to));
      for (const p of [...(def.inputs ?? []), ...(def.parameters ?? [])]) for (const to of p.to ?? []) link(`${at}.${p.name}`, full(to));
      for (const p of def.outputs ?? []) if (p.from) link(full(p.from), `${at}.${p.name}`);
      walk(def.instances, at, depth + 1);
    }
  };
  walk(graph.top?.instances, '', 0);
  CACHE.set(graph, links);
  return links;
}

const CACHE = new WeakMap();

/**
 * The net of the ports `seeds` (full addresses) in `graph`: `{addresses,
 * wires}`, the ports reached and the `wireKey`s of the links walked. A
 * seed no connection reaches is its net alone.
 */
export function netOf(graph, seeds) {
  const links = netLinks(graph);
  const addresses = new Set();
  const wires = new Set();
  const queue = seeds.filter(Boolean).map(norm);
  while (queue.length) {
    const a = queue.pop();
    if (addresses.has(a)) continue;
    addresses.add(a);
    for (const l of links.get(a) ?? []) {
      wires.add(l.key);
      if (!addresses.has(l.to)) queue.push(l.to);
    }
  }
  return { addresses, wires };
}

/**
 * The ports a selection (ui/controls/plane.js `selectionOf`'s) starts its net
 * from: a wire's or a cable's two ends, a pin's or a port's address, a leg's
 * parameter, an automation's source and target. A card or an instrument has
 * none.
 */
export function selectionSeeds(selection) {
  if (!selection) return [];
  if (selection.kind === 'engine') {
    const item = selection.item ?? {};
    if (item.kind === 'e-wire' || item.kind === 'e-edge') return [item.from, item.to];
    if (item.kind === 'e-pin' || item.kind === 'e-bind') return [item.address];
    return [];
  }
  if (selection.kind === 'cable' || selection.kind === 'auto') return [selection.from, selection.to];
  if (selection.kind === 'port' || selection.kind === 'leg') return [selection.address];
  return [];
}

/** A selection's net in `graph` (`netOf`'s), or null where it has none. */
export function selectionNet(graph, selection) {
  const seeds = selectionSeeds(selection).filter(Boolean);
  return seeds.length && graph ? netOf(graph, seeds) : null;
}

// Ports in groups (M2.13-2, ADR 0030 §3; the owner's "on the outside of the
// composite you can't see which of the 20 inputs is what"): a composite's
// box orders its pins by group, each group by the composite's export order
// (`order` on `graph`'s exports, the file's), a group being the export's
// `group = "…"` (its table form) or else its name's prefix (`osc1_*`, `f_*`,
// `a_*`; ui/lib/schematic-labels.js `prefixOf`), groups in the order of
// their first export. At small scale (below PITCH_PX a row, where a label
// a pin no longer fits) a group of several pins collapses to one pin at its
// middle, labelled with its name and count, and the wires to it join it as
// a bundle. Pure; no DOM.

import { ROW } from './schematic-layout.js';
import { PITCH_PX, prefixOf } from './schematic-labels.js';

/**
 * A box's groups collapse below this many px a row on screen: a label's
 * pitch (ui/lib/schematic-labels.js), so that a pin drawn on its own always
 * has room for its name, and below it a group's pin is labelled instead.
 */
export const GROUP_PX = PITCH_PX;

/** Whether a box's groups are collapsed at scale `k`. */
export function collapsedAt(k) {
  return ROW * k < GROUP_PX;
}

/**
 * The export facts of composite `data` (`graph.composites[module]`) by pin
 * name: `Map(name → {rank, group})`, `rank` its place among the box's pins
 * of its side as the file writes them (inputs, then parameters; outputs).
 */
export function exportFacts(data) {
  const facts = new Map();
  const add = (list, base) => {
    for (const e of list ?? []) facts.set(e.name, { rank: base + (Number.isFinite(e.order) ? e.order : 0), group: e.group ?? null });
  };
  add(data?.inputs, 0);
  add(data?.parameters, 1e6);
  add(data?.outputs, 0);
  return facts;
}

/**
 * Pins `pins` (`{name}`, a side of a box, in the layout's order) as groups,
 * in order: `[{key, label, pins}]`. A pin's group is its export's `group`,
 * else its prefix, else its own name; groups by their first pin's rank,
 * pins within by rank (the file's order; the given order where unknown).
 */
export function portGroups(pins, facts = new Map()) {
  const rank = (p, i) => facts.get(p.name)?.rank ?? 2e6 + i;
  const ranked = pins.map((p, i) => ({ p, r: rank(p, i) })).sort((a, b) => a.r - b.r);
  const groups = new Map();
  for (const { p, r } of ranked) {
    const explicit = facts.get(p.name)?.group ?? null;
    const prefix = prefixOf(p.name);
    const key = explicit !== null ? `group ${explicit}` : prefix !== null ? `prefix ${prefix}` : `pin ${p.name}`;
    if (!groups.has(key)) groups.set(key, { key, label: explicit ?? (prefix !== null ? `${prefix}*` : p.name), pins: [], first: r });
    groups.get(key).pins.push(p);
  }
  // A prefix with one pin is that pin's name.
  return [...groups.values()]
    .sort((a, b) => a.first - b.first)
    .map(({ key, label, pins: members }) => ({ key, label: key.startsWith('prefix ') && members.length === 1 ? members[0].name : label, pins: members }));
}

/** `pins` reordered by `portGroups`: what a composite box lays its pins out in. */
export function orderedPins(pins, facts) {
  return portGroups(pins, facts).flatMap((g) => g.pins);
}

/**
 * A collapsed box's pin anchors: `Map(pin name → {x, y, group})` for the
 * pins of groups of more than one (each at its group's middle row), given
 * `node`'s laid-out pins and `node.groups` (`{ins, outs}`, `portGroups`').
 */
export function groupAnchors(node) {
  const anchors = new Map();
  for (const side of ['ins', 'outs']) {
    const at = new Map(node[side].map((p) => [p.name, p]));
    for (const g of node.groups?.[side] ?? []) {
      if (g.pins.length < 2) continue;
      const first = at.get(g.pins[0].name);
      const last = at.get(g.pins.at(-1).name);
      if (!first || !last) continue;
      const point = { x: first.x, y: (first.y + last.y) / 2, group: g };
      for (const p of g.pins) anchors.set(p.name, point);
    }
  }
  return anchors;
}

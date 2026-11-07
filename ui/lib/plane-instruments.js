// The instruments of the Signal Project plane (ADR 0051 §4–5) and the
// Stacks' derived boundaries. In phase 1 an instrument is a card of the
// panel listed in a Stack (the migration form, `card:<id>`), or a top-level
// slot with a part (the full form). A card's engine is the top-level
// instances its controls bind, assigned by ADR 0050 §2's rules (by
// controls, then by flow); a device slot (`std.audio_out`, role `main`,
// ADR 0038 §3) is never an instrument's: it is a role endpoint, drawn at
// the Setup level. A Stack's interface is derived, never written: one edge
// pin per instrument port that a connection joins to something outside the
// Stack, inputs on the left, outputs on the right, grouped per instrument.
// Presentation only: nothing here changes a plan (ADR 0051 §14). Pure;
// arrays in source order, ties broken by source order, a fixed iteration
// bound (ADR 0047 §6's determinism). No DOM.

import { CARD_PREFIX, cardOfEntry, entryOf, isCardEntry } from './plane-file.js';

/** The module types that are device slots (role endpoints), with their role (ADR 0038 §3: `std.audio_out` is `main`). */
export const DEVICE_SLOTS = Object.freeze({ 'std.audio_out': 'main' });

/** The flow rule's passes at most (ADR 0050 §2.4's fixed bound). */
export const FLOW_PASSES = 32;
/** A sink neighbour weighs a hair more than a source (ADR 0050 §2.4). */
export const SINK_WEIGHT = 1.001;

/** The top-level instance an address names (`/synth.cutoff`, `/rack/fx.out` → `synth`, `rack`). */
export function instanceOf(address) {
  const text = String(address ?? '').replace(/^\//, '');
  const cut = text.search(/[./]/);
  return cut === -1 ? text : text.slice(0, cut);
}

/** The port name of a top-level port address (`/limiter.out` → `out`). */
export function portOf(address) {
  const text = String(address ?? '');
  return text.slice(text.lastIndexOf('.') + 1);
}

/** Whether a top-level instance (`graph.top.instances[i]`) is a device slot, and its role: `graph`'s `device.role` (ADR 0038 §3, a `device = { play, role }` slot), else `std.audio_out`'s `main`. */
export function roleOf(instance) {
  return instance?.device?.role ?? DEVICE_SLOTS[instance?.module] ?? null;
}

/** Whether a top-level instance is a slot with a part (ADR 0046: `slot.part.file`). */
export function hasPart(instance) {
  return Boolean(instance?.slot?.part?.file);
}

/**
 * Every top-level instance assigned to an instrument key (`card:<id>` or a
 * slot's name), or to none: `Map(name → key | null)`, in source order.
 * `instances`, `connections`: `graph.top`'s; `cards`: `panelCards`', keyed
 * by their `entry` (`withEntries`: a slot's card is the slot's).
 * The rules, in order (ADR 0050 §2): a device slot is no instrument's; a
 * slot with a part is its own; by controls, the card most of its bound
 * controls sit in (a tie: the earlier card); by flow, a chain of single
 * links goes to its end's instrument, any other to the one most of its
 * neighbours belong to, sinks weighing a hair more, repeated until nothing
 * changes. What is left is in no instrument.
 */
export function assignInstruments({ instances = [], connections = [], cards = [] } = {}) {
  const owner = new Map(instances.map((i) => [i.name, null]));
  const fixed = new Set();
  for (const i of instances) {
    if (roleOf(i)) fixed.add(i.name);
    else if (hasPart(i)) {
      owner.set(i.name, i.name);
      fixed.add(i.name);
    }
  }
  // By controls.
  const counts = new Map();
  cards.forEach((card, order) => {
    for (const address of card.binds) {
      const name = instanceOf(address);
      if (!owner.has(name) || fixed.has(name)) continue;
      if (!counts.has(name)) counts.set(name, new Map());
      const tally = counts.get(name);
      const key = entryOf(card);
      tally.set(key, { n: (tally.get(key)?.n ?? 0) + 1, order });
    }
  });
  for (const [name, tally] of counts) {
    let best = null;
    for (const [key, { n, order }] of tally) if (!best || n > best.n || (n === best.n && order < best.order)) best = { key, n, order };
    owner.set(name, best.key);
    fixed.add(name);
  }
  // By flow: the instances' downstream and upstream neighbours (other instances, each once, in connection order).
  const downstream = new Map(instances.map((i) => [i.name, []]));
  const upstream = new Map(instances.map((i) => [i.name, []]));
  for (const c of connections) {
    const a = instanceOf(c.from);
    const b = instanceOf(c.to);
    if (a === b || !owner.has(a) || !owner.has(b)) continue;
    if (!downstream.get(a).includes(b)) downstream.get(a).push(b);
    if (!upstream.get(b).includes(a)) upstream.get(b).push(a);
  }
  const free = (name) => !fixed.has(name) && !roleOf(instances.find((i) => i.name === name));
  /** A link single at both ends: `name`'s only downstream neighbour, whose only upstream neighbour it is; or null. */
  const single = (name) => {
    const next = downstream.get(name);
    return next.length === 1 && upstream.get(next[0]).length === 1 ? next[0] : null;
  };
  /** The end of a chain of single links from `name`: its instrument, or null. */
  const chainEnd = (name) => {
    const seen = new Set([name]);
    let at = name;
    while (single(at)) {
      at = single(at);
      if (seen.has(at)) return null;
      seen.add(at);
      if (owner.get(at)) return owner.get(at);
    }
    return null;
  };
  for (let pass = 0; pass < FLOW_PASSES; pass++) {
    let changed = false;
    for (const { name } of instances) {
      if (!free(name)) continue;
      let key = single(name) ? chainEnd(name) : null;
      if (!key) {
        const weight = new Map();
        const add = (other, w) => {
          const k = owner.get(other);
          if (k) weight.set(k, (weight.get(k) ?? 0) + w);
        };
        for (const other of upstream.get(name)) add(other, 1);
        for (const other of downstream.get(name)) add(other, SINK_WEIGHT);
        let best = null;
        for (const [k, w] of weight) if (!best || w > best.w + 1e-12) best = { k, w };
        key = best?.k ?? null;
      }
      if (key !== owner.get(name)) {
        owner.set(name, key);
        changed = true;
      }
    }
    if (!changed) break;
  }
  return owner;
}

/**
 * Which instrument entries of `plane` are known (`cards`' ids and the
 * top-level slots with a part), and every instance's Stack:
 * `{stackOf: Map(name → stack name | null), unstacked: [names]}`. A device
 * slot is in no Stack and never unstacked (it is a role endpoint).
 */
export function stackMembers(plane, owner, instances = []) {
  const stackOfEntry = new Map();
  for (const stack of plane?.stacks ?? []) for (const entry of stack.instruments) if (!stackOfEntry.has(entry)) stackOfEntry.set(entry, stack.name);
  const stackOf = new Map();
  const unstacked = [];
  for (const i of instances) {
    if (roleOf(i)) {
      stackOf.set(i.name, null);
      continue;
    }
    const key = owner.get(i.name) ?? null;
    const stack = key ? (stackOfEntry.get(key) ?? null) : null;
    stackOf.set(i.name, stack);
    if (!stack) unstacked.push(i.name);
  }
  return { stackOf, unstacked };
}

/**
 * A Stack's edge pins (ADR 0051 §4, "The boundary"): one per instrument
 * port that a connection joins to something outside the Stack (another
 * Stack, a role endpoint, an unstacked instance). `[{side: 'in' | 'out',
 * instrument, address, label, outside: [addresses]}]`, inputs then
 * outputs, each grouped by instrument in the Stack's order and, within one,
 * in connection order. `label` is the port's name, `instance.port` where
 * two pins of an instrument share it.
 */
export function edgePins(stack, owner, stackOf, connections = []) {
  const order = new Map(stack.instruments.map((e, i) => [e, i]));
  const inside = (address) => stackOf.get(instanceOf(address)) === stack.name;
  const pins = new Map();
  let seq = 0;
  const pin = (side, address, other) => {
    const key = `${side} ${address}`;
    if (!pins.has(key)) pins.set(key, { side, instrument: owner.get(instanceOf(address)), address, label: portOf(address), outside: [], seq: seq++ });
    const p = pins.get(key);
    if (!p.outside.includes(other)) p.outside.push(other);
  };
  for (const c of connections) {
    const a = inside(c.from);
    const b = inside(c.to);
    if (a && !b) pin('out', c.from, c.to);
    else if (b && !a) pin('in', c.to, c.from);
  }
  const list = [...pins.values()].sort((p, q) => (p.side === q.side ? 0 : p.side === 'in' ? -1 : 1) || (order.get(p.instrument) ?? 1e9) - (order.get(q.instrument) ?? 1e9) || p.seq - q.seq);
  // Two pins of one instrument and side sharing a port name: both say their instance.
  for (const p of list) {
    if (list.some((q) => q !== p && q.side === p.side && q.instrument === p.instrument && portOf(q.address) === portOf(p.address))) p.label = String(p.address).replace(/^\//, '');
  }
  return list.map(({ seq: _seq, ...p }) => p);
}

/** The instrument entry's shown name: its card's heading, else a card's id or a slot's name. */
export function instrumentTitle(entry, cards = []) {
  const card = cardOfEntry(entry, cards);
  return card?.heading || (isCardEntry(entry) ? entry.slice(CARD_PREFIX.length) : entry);
}

/**
 * The role endpoints of the top level (ADR 0051 §2; ADR 0038 §3): each
 * device slot with its role, its layout (`stereo` for two inputs, `mono`
 * for one, else `<n> ch`) and its input ports in order. `[{name, role,
 * layout, ports: [addresses]}]`.
 */
export function roleEndpoints(instances = []) {
  const out = [];
  for (const i of instances) {
    const role = roleOf(i);
    // A capture slot plays nothing: no endpoint (it showed as "0 ch, not heard", the owner's, 2026-11-07).
    if (!role || (i.device && !i.device.play)) continue;
    const ports = (i.ports ?? []).filter((p) => p.direction === 'in' && (p.shape ?? 'signal') === 'signal').map((p) => p.address);
    const layout = i.device?.play ?? (ports.length === 2 ? 'stereo' : ports.length === 1 ? 'mono' : `${ports.length} ch`);
    out.push({ name: i.name, role, layout, ports });
  }
  return out;
}

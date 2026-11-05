// Selection and editing at the Engine layer of the Signal Project plane
// (ADR 0051 §8; step 3 of the owner's Engine rebuild, 2026-11-03): what a
// point of an Engine drawing hits, with full addresses (an inner instance
// of chip `/synth` is `/synth/fenv`, its pins `/synth/fenv.gate`, the
// chip's own input `/synth.gate`); the selection's keys, which mark every
// drawn element of the same thing; and the `patch` ops an edit writes, as
// the standalone schematic's editor writes them (ui/controls/schematic-editor.js,
// ui/lib/schematic-patch.js): the instrument's level is the project's top
// level (`""`), a chip's inside is its composite's level (the composite's
// name: the edit changes the project's composite definition, every instance
// of it), and a wire between a chip's own pin and its inside is an export of
// the composite, which the schematic edits only in a slot's own inside, so
// it is not edited here either. The server is the judge of the rest. Pure;
// no DOM.

import { hitItem } from './plane-hit.js';
import { roleOf } from './plane-instruments.js';
import { addConnection, addInstance, connectTargets, removeConnection, removeInstance, written } from './schematic-patch.js';
import { exportOp } from './slot.js';

/** An address with the top level's leading `/` (a level's own `a.b` → `/a.b`; an export end `@inputs.x` as it is). */
export function norm(address) {
  const text = String(address ?? '');
  return text.startsWith('/') || text.startsWith('@') ? text : `/${text}`;
}

/**
 * An address as the person knows it, at a level whose chip has address
 * `prefix` (`""`: the instrument's level, the project's top): inside the
 * chip `/synth`, `fenv.gate` is `/synth/fenv.gate` and its input
 * `@inputs.gate` is `/synth.gate`.
 */
export function fullAddress(prefix, address) {
  const text = String(address ?? '');
  if (!prefix) return norm(text);
  if (text.startsWith('@inputs.') || text.startsWith('@outputs.')) return `${prefix}.${text.slice(text.indexOf('.') + 1)}`;
  return `${prefix}/${text.replace(/^\//, '')}`;
}

/** A wire's end on the level's own frame (a chip's input or output: an export of its composite). */
function onFrame(address) {
  const text = String(address ?? '');
  return text.startsWith('@inputs.') || text.startsWith('@outputs.');
}

/** The key every drawn element of a connection carries (its two full ends). */
export function wireKey(from, to) {
  return `wire ${norm(from)} ${norm(to)}`;
}

export function cardKey(address) {
  return `card ${norm(address)}`;
}

export function pinKey(side, address) {
  return `pin ${side} ${norm(address)}`;
}

/** The key of a leg of the control signal into parameter `address` (ui/lib/plane-engine.js `layLevel`'s `binds`). */
export function bindKey(address) {
  return `bind ${norm(address)}`;
}

/**
 * The hit items of one laid-out level (`layLevel`'s, plane units, Stack-
 * relative), in the order they are drawn: wires, cards, pins, and at the
 * instrument's level the edge dots. `entry` the instrument, `level` the
 * level's patch level, kept as `patchLevel` (`""` the top, a composite's name inside a chip),
 * `prefix` the chip's address (`""` at the instrument's level), `chip` the
 * chip's id (as `drawLevel` names it; null at the instrument's level).
 * Every item has `kind` (`e-wire`, `e-card`, `e-pin`, `e-edge`), `id`
 * (unique across the Stack), its geometry as ui/lib/plane-hit.js reads it,
 * `key` (the drawn elements', `wireKey`, `cardKey`, `pinKey`), its full
 * address(es) and what an op names at its level.
 */
export function engineItems(laid, { entry, level = '', prefix = '', chip = null } = {}) {
  const scope = chip ?? entry;
  const items = [];
  const cardById = new Map((laid?.cards ?? []).map((c) => [c.id, c]));
  const chipOf = (c) => (c.composite ? (chip ? `${chip}/${c.id}` : c.id) : null);
  for (const w of laid?.wires ?? []) {
    const from = fullAddress(prefix, w.from);
    const to = fullAddress(prefix, w.to);
    items.push({ kind: 'e-wire', id: `${scope}|${w.id}`, key: wireKey(from, to), points: w.points, entry, patchLevel: level, prefix, chip, from, to, written: { from: written(w.from), to: written(w.to) }, edge: w.edge ?? null, frame: Boolean(prefix) && (onFrame(w.from) || onFrame(w.to)) });
  }
  for (const c of laid?.cards ?? []) {
    const address = `${prefix}/${c.name}`;
    items.push({ kind: 'e-card', id: `${scope}|${c.id}`, key: cardKey(address), x: c.x, y: c.y, w: c.w, h: c.h, body: c.body, entry, patchLevel: level, prefix, chip, name: c.name, module: c.module, composite: c.composite, address, chipId: chipOf(c) });
  }
  for (const p of laid?.pins ?? []) {
    const card = cardById.get(p.card);
    const name = card?.name ?? String(p.address).replace(/^\//, '').split('.')[0];
    const address = fullAddress(prefix, p.address);
    items.push({ kind: 'e-pin', id: `${scope}|${p.id}`, key: pinKey(p.side, address), x: p.x, y: p.y, entry, patchLevel: level, prefix, chip, card: name, port: p.name, side: p.side, parameter: p.parameter, wired: Boolean(p.wired), address, local: `${name}.${p.name}`, cardChip: card ? chipOf(card) : null, module: card?.module ?? null });
  }
  // The control signal's legs into the parameters (`layLevel`'s `binds`): each selectable, by the parameter it sets.
  for (const b of laid?.binds ?? []) {
    const address = fullAddress(prefix, b.address);
    items.push({ kind: 'e-bind', id: `${scope}|${b.id}`, key: bindKey(address), points: b.points, entry, patchLevel: level, prefix, chip, address, local: b.address, export: b.export ?? null });
  }
  // Inside a chip the edge dots are its card's own pins (the level around's items).
  if (!prefix) {
    // An output leaving to several places: one dot (one id) for its connections.
    const shared = new Map();
    for (const e of laid?.edges ?? []) shared.set(e.id, (shared.get(e.id) ?? 0) + 1);
    for (const e of laid?.edges ?? []) {
      const [from, to] = e.side === 'in' ? [e.from, e.address] : [e.address, e.to];
      items.push({ kind: 'e-edge', id: `${scope}|${e.id}`, key: wireKey(from, to), x: e.x, y: e.y, entry, patchLevel: level, prefix, chip, from: norm(from), to: norm(to), written: { from: written(from), to: written(to) }, edge: e.side, frame: false, shared: shared.get(e.id) > 1 });
    }
  }
  return items;
}

/**
 * What a press at plane point `p` (Stack-relative) hits in an Engine
 * drawing (`items`, `engineItems`' of the instrument's level and of the
 * revealed chips' insides, outer first), the view at scale `k`: an item,
 * else empty space. The body of a revealed chip (`revealed`: chip ids)
 * where nothing of its inside is is that chip's empty space (`{kind:
 * 'e-space', patchLevel: its composite, prefix: its address, chip}`); an
 * instrument's rectangle (`rects`: `{entry, x, y, w, h}`) where nothing is,
 * the instrument's (`patchLevel` `""`). Null outside every instrument.
 */
export function engineTarget(items, p, k, { revealed = new Set(), rects = [] } = {}) {
  const hit = hitItem(items, p, k)?.item ?? null;
  const b = hit?.body;
  if (hit?.kind === 'e-card' && hit.chipId && revealed.has(hit.chipId) && b && p.x >= b.x && p.x <= b.x + b.w && p.y >= b.y && p.y <= b.y + b.h) {
    return { kind: 'e-space', entry: hit.entry, patchLevel: hit.module, prefix: hit.address, chip: hit.chipId, name: hit.name };
  }
  if (hit) return hit;
  const r = rects.find((x) => p.x >= x.x && p.x <= x.x + x.w && p.y >= x.y && p.y <= x.y + x.h);
  return r ? { kind: 'e-space', entry: r.entry, patchLevel: '', prefix: '', chip: null, name: null } : null;
}

/** The kinds a click selects (empty space clears). */
export const ENGINE_SELECTABLE = new Set(['e-wire', 'e-card', 'e-pin', 'e-edge', 'e-bind']);

/** A hit as the selection keeps it (view state): the item, and the keys of what it marks. */
export function engineSelection(hit) {
  if (!hit || !ENGINE_SELECTABLE.has(hit.kind)) return null;
  return { kind: 'engine', what: hit.kind, keys: [hit.key], item: hit };
}

/**
 * The element keys a selection marks at the Engine: its own; a Wiring cable
 * the Engine's drawing of the same connection, a Wiring port both its pins.
 */
export function selectionKeys(selection, net = null) {
  const keys = new Set();
  if (!selection) return keys;
  if (selection.kind === 'engine') for (const key of selection.keys) keys.add(key);
  if (selection.kind === 'cable') keys.add(wireKey(selection.from, selection.to));
  if (selection.kind === 'port') keys.add(pinKey('in', selection.address)).add(pinKey('out', selection.address));
  // Its whole net (ui/lib/plane-net.js `netOf`): the wires walked, the ports' pins, the control signal's legs into them.
  for (const key of net?.wires ?? []) keys.add(key);
  for (const address of net?.addresses ?? []) keys.add(pinKey('in', address)).add(pinKey('out', address)).add(bindKey(address));
  return keys;
}

/** The subject of the app's Show in items (ui/app/lib/show-in.js `subjectOf`) for an Engine hit, or null. */
export function engineSubject(hit) {
  if (hit?.kind === 'e-card') return { kind: 'instance', address: hit.address };
  if (hit?.kind === 'e-pin') return { kind: 'port', address: hit.address };
  if ((hit?.kind === 'e-wire' || hit?.kind === 'e-edge') && !hit.frame) return { kind: 'wire', from: hit.from, to: hit.to };
  return null;
}

/** Why the schematic leaves a chip's exports alone (the operator's reading of ui/controls/schematic-editor.js `starts`). */
function exportsReason(module) {
  return `that is ${module}’s export (a chip’s own pin wired to its inside), which is edited only in a slot’s own inside, as in the schematic`;
}

/**
 * The `patch` that connects pins `a` and `b` (`engineItems`' `e-pin`s, in
 * either order: an output and an input or a driven parameter of one level):
 * `{level, ops, what}`, or `{error}`.
 */
export function connectOps(a, b) {
  if (a?.kind !== 'e-pin' || b?.kind !== 'e-pin') return { error: 'Not connected: drop the wire on a pin' };
  // A chip's own pin and a pin inside that chip: an export of its composite (from inside, its input is a source, its output a sink).
  const frame = a.cardChip && a.cardChip === b.chip ? a : b.cardChip && b.cardChip === a.chip ? b : null;
  if (frame) return exportConnect(frame, frame === a ? b : a);
  if (a.chip !== b.chip) return { error: `Not connected: ${a.address} and ${b.address} are at different levels (connect two cards of one level)` };
  if (a.side === b.side) return { error: `Not connected: ${a.address} and ${b.address} are both ${a.side === 'out' ? 'outputs' : 'inputs'}` };
  const [out, into] = a.side === 'out' ? [a, b] : [b, a];
  return { level: out.patchLevel, ops: [addConnection(out.local, into.local)], what: `${out.address} → ${into.address}` };
}

/**
 * The `patch` that moves one end of wire `wire` (an `e-wire`) onto pin `to`
 * (an `e-pin` of the same level): `end` the end that moves, `'from'` (onto
 * an output) or `'to'` (onto an input or a driven parameter); the old
 * connection removed and the new one added in one patch (one undo).
 * `{level, ops, what}`, or `{error}`.
 */
export function rewireOps(wire, end, to, def = null) {
  if (wire?.kind !== 'e-wire') return { error: 'Not moved: grab a wire' };
  if (wire.frame) return exportMove(wire, end, to, def);
  if (to?.kind !== 'e-pin') return { error: 'Not moved: drop the wire’s end on a pin' };
  if (to.cardChip && to.cardChip === wire.chip) return { error: `Not moved: ${exportsReason(to.module)}` };
  if ((to.chip ?? null) !== (wire.chip ?? null)) return { error: `Not moved: ${to.address} is at another level than the wire` };
  const want = end === 'from' ? 'out' : 'in';
  if (to.side !== want) return { error: `Not moved: the wire’s ${end === 'from' ? 'source' : 'end'} goes on ${want === 'out' ? 'an output' : 'an input or a parameter'}` };
  const [from, into] = end === 'from' ? [to.local, wire.written.to] : [wire.written.from, to.local];
  if (written(from) === written(wire.written.from) && written(into) === written(wire.written.to)) return { error: 'Not moved: it is already there' };
  const [shownFrom, shownTo] = end === 'from' ? [to.address, wire.to] : [wire.from, to.address];
  return { level: wire.patchLevel, ops: [removeConnection(wire.written.from, wire.written.to), addConnection(from, into)], what: `${wire.from} → ${wire.to} moved: ${shownFrom} → ${shownTo}` };
}

/**
 * A chip's own pin `frame` wired to pin `inner` inside it (the owner's, 2026-11-04: every wire rewired by hand): the
 * composite's export of the frame pin's name reaches the inner pin too (`export.connect` at the composite's level, as the
 * slot editor writes it): an input or a parameter on to an inner input or parameter, an output from an inner output.
 */
function exportConnect(frame, inner) {
  const role = frame.side === 'out' ? 'output' : frame.parameter ? 'parameter' : 'input';
  if ((role === 'output') !== (inner.side === 'out')) return { error: `Not connected: ${frame.address} ${role === 'output' ? 'leaves from an output inside' : 'goes on to an input or a parameter inside'}` };
  return { level: inner.patchLevel, ops: [exportOp(true, role, frame.port, inner.local)], what: `${frame.address} → ${inner.address} (${frame.module}’s export)` };
}

/**
 * An export's wire inside a chip with one end moved onto `to` (one patch): its inner end onto another pin inside (the
 * export's old port let go, the new one taken), or its end on the chip's own pin onto another of the chip's pins (the
 * inner port handed to that pin's export, `exportHandOver`).
 */
function exportMove(wire, end, to, def) {
  const into = String(wire.written.from).startsWith('@inputs.');
  const name = String(into ? wire.written.from : wire.written.to).replace(/^@(inputs|outputs)\./, '');
  const role = !into ? 'output' : def?.parameters?.some((p) => p.name === name) ? 'parameter' : 'input';
  const old = written(into ? wire.written.to : wire.written.from);
  if ((into && end !== 'to') || (!into && end !== 'from')) return exportHandOver({ chip: wire.chip, level: wire.patchLevel, prefix: wire.prefix, role, name, port: old }, to);
  if (to?.kind !== 'e-pin' || (to.chip ?? null) !== (wire.chip ?? null)) return { error: 'Not moved: drop the wire’s end on a pin inside the same chip' };
  if (to.side !== (into ? 'in' : 'out')) return { error: `Not moved: ${into ? 'an input’s export goes on to an input or a parameter' : 'an output’s export leaves from an output'}` };
  if (old === written(to.local)) return { error: 'Not moved: it is already there' };
  return { level: wire.patchLevel, ops: exportSwap(role, name, old, to.local), what: `${wire.from} → ${wire.to} moved to ${to.address} (${wire.patchLevel}’s export)` };
}

/**
 * Inner port `port` of the export `role` `name` (of the composite at `level`, the inside of chip `chip` at `prefix`)
 * handed to the export of the chip's own pin `to` (an `e-pin` of the level around, on that chip's card): the new export
 * reaches it first, then the old one lets go (an output names one port: it lets go first). The old export goes with its
 * last port. `{level, ops, what}`, or `{error}`.
 */
function exportHandOver({ chip, level, prefix, role, name, port }, to) {
  if (to?.kind !== 'e-pin' || !to.cardChip || to.cardChip !== chip) return { error: 'Not moved: drop the chip’s end of the wire on another of the chip’s own pins' };
  if ((to.side === 'out') !== (role === 'output')) return { error: `Not moved: ${role === 'output' ? 'an output’s export goes to an output of the chip' : 'an input’s or a parameter’s export comes from an input or a parameter of the chip'}` };
  const now = to.side === 'out' ? 'output' : to.parameter ? 'parameter' : 'input';
  if (now === role && to.port === name) return { error: 'Not moved: it is already there' };
  const ops = role === 'output' ? [exportOp(false, role, name, port), exportOp(true, now, to.port, port)] : [exportOp(true, now, to.port, port), exportOp(false, role, name, port)];
  return { level, ops, what: `${fullAddress(prefix, port)} now ${role === 'output' ? 'leaves at' : 'comes from'} ${to.address} instead of ${prefix}.${name} (${level}’s export)` };
}

/**
 * A control signal's leg inside a chip (an `e-bind` with a prefix) moved onto inner parameter `port`
 * (`instance.parameter` at the chip's level): the composite's parameter export `bind.export` reaches it instead; its end
 * on the chip's own pin onto another of the chip's pins (`port` that pin's `e-pin`): that pin's export reaches it instead.
 */
export function bindMoveOps(bind, port) {
  if (bind?.kind !== 'e-bind' || !bind.prefix || !bind.export) return { error: 'Not moved: grab a leg inside a chip' };
  // The leg's end on the chip's own pin onto another of its pins: the inner parameter handed to that pin's export.
  if (port?.kind === 'e-pin') return exportHandOver({ chip: bind.chip, level: bind.patchLevel, prefix: bind.prefix, role: 'parameter', name: bind.export, port: written(bind.local) }, port);
  const old = written(bind.local);
  if (old === written(port)) return { error: 'Not moved: it is already there' };
  return { level: bind.patchLevel, ops: exportSwap('parameter', bind.export, old, port), what: `${bind.prefix}.${bind.export} now reaches ${fullAddress(bind.prefix, port)}` };
}

/**
 * An export's port `old` swapped for `port`: an input's or a parameter's new port taken first, then the old one let go,
 * so that the export, its range and its other attributes stay (it would go with its last port); an output names one port,
 * so it lets go first.
 */
function exportSwap(role, name, old, port) {
  return role === 'output' ? [exportOp(false, role, name, old), exportOp(true, role, name, port)] : [exportOp(true, role, name, port), exportOp(false, role, name, old)];
}

/** The `patch` that removes `item` (a wire, an edge dot's connection, a card): `{level, ops, what}`, or `{error}`. */
export function removeOps(item, def = null) {
  if (item?.kind === 'e-card') return { level: item.patchLevel, ops: [removeInstance(item.name)], what: `${item.address} removed` };
  if (item?.kind === 'e-wire' || item?.kind === 'e-edge') {
    if (item.frame && item.kind === 'e-wire') {
      // An export's wire inside a chip: the inner port let go of the export (the export goes with its last port).
      const into = String(item.written.from).startsWith('@inputs.');
      const name = String(into ? item.written.from : item.written.to).replace(/^@(inputs|outputs)\./, '');
      const role = !into ? 'output' : def?.parameters?.some((p) => p.name === name) ? 'parameter' : 'input';
      return { level: item.patchLevel, ops: [exportOp(false, role, name, written(into ? item.written.to : item.written.from))], what: `${item.from} → ${item.to} removed (${item.patchLevel}’s export)` };
    }
    if (item.frame) return { error: `Not removed: ${exportsReason(item.patchLevel)}` };
    if (item.shared) return { error: `Not removed: the dot at ${item.from} stands for several connections; select the wire to remove` };
    return { level: item.patchLevel, ops: [removeConnection(item.written.from, item.written.to)], what: `${item.from} → ${item.to} removed` };
  }
  return { error: 'Select a wire or a card to remove it' };
}

/** The `patch` that adds `module` named `name` at empty space `target` (`engineTarget`'s `e-space`): `{level, ops, what}`. No position: the Engine lays it out. */
export function addOps(target, module, name) {
  return { level: target.patchLevel, ops: [addInstance(name, module)], what: `${target.prefix}/${name} (${module}) added` };
}

/**
 * A wire from pin `pin` dropped on card `card` (one level's): what it may
 * connect to there, `[{text, from, to}]` (the level's own names; `from` an
 * output, `to` an input or a parameter): an output to the card's inputs and
 * parameters of its shape that it does not drive yet (`connectTargets`); an
 * input or a parameter to the card's outputs of its shape. `levelGraph`:
 * the level as `graph` gives it.
 */
export function dropTargets(levelGraph, pin, card) {
  if (pin?.kind !== 'e-pin' || card?.kind !== 'e-card' || pin.chip !== card.chip) return [];
  const instances = levelGraph?.instances ?? [];
  const own = instances.find((i) => i.name === pin.card);
  const shapeOf = (shape) => (shape && typeof shape === 'object' ? 'vector' : (shape ?? 'signal'));
  if (pin.side === 'out') {
    const port = own?.ports?.find((p) => p.name === pin.port && p.direction === 'out');
    return connectTargets(levelGraph, { instance: pin.card, port: pin.port, shape: shapeOf(port?.shape) })
      .filter((t) => t.instance === card.name)
      .map((t) => ({ text: `${t.port}${t.parameter ? ' (parameter)' : ''}`, from: pin.local, to: `${t.instance}.${t.port}` }));
  }
  const target = own?.ports?.find((p) => p.name === pin.port && p.direction === 'in');
  const wanted = pin.parameter ? 'signal' : shapeOf(target?.shape);
  const other = instances.find((i) => i.name === card.name);
  return (other?.ports ?? []).filter((p) => p.direction === 'out' && shapeOf(p.shape) === wanted).map((p) => ({ text: p.name, from: `${card.name}.${p.name}`, to: pin.local }));
}

/**
 * Escape from inside chips (`chips`: the view's, outermost first): out one
 * chip level, to the parent chip's body (`{chip, part: 'body'}`), or from
 * the outermost to its instrument whole (`{instrument: chip}`); null when
 * not inside a chip.
 */
export function outStep(chips) {
  if (!chips?.length) return null;
  return chips.length > 1 ? { chip: chips[chips.length - 2], part: 'body' } : { instrument: chips[0] };
}

/**
 * The instances added from an instrument's Engine and not joined to any
 * instrument yet (an instance with no control and no connection is in
 * none, ui/lib/plane-instruments.js `assignInstruments`): `owner` with each
 * of `placed` (`Map(name → {entry, seen})`, view state of this page) put in
 * its entry while the assignment leaves it in none; the names to forget
 * (joined an instrument, a device slot, an entry no longer known, or gone
 * from the graph after it was seen there; one not seen yet is kept: its
 * add's graph push is still to come), and those seen now.
 * `{owner, forget, seen}`; `owner` a new Map.
 */
export function placedOwners(owner, placed, instances = [], entries = new Set()) {
  const out = new Map(owner);
  const forget = [];
  const seen = [];
  for (const [name, { entry, seen: before = false }] of placed ?? []) {
    const instance = instances.find((i) => i.name === name);
    if (!instance) {
      // Not in the graph yet (the patch's graph push follows): kept; gone after it came (undone, removed): forgotten.
      if (before) forget.push(name);
      continue;
    }
    if (out.get(name) || roleOf(instance) || !entries.has(entry)) {
      forget.push(name);
      continue;
    }
    seen.push(name);
    out.set(name, entry);
  }
  return { owner: out, forget, seen };
}

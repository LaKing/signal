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
    items.push({ kind: 'e-pin', id: `${scope}|${p.id}`, key: pinKey(p.side, address), x: p.x, y: p.y, entry, patchLevel: level, prefix, chip, card: name, port: p.name, side: p.side, parameter: p.parameter, address, local: `${name}.${p.name}`, cardChip: card ? chipOf(card) : null, module: card?.module ?? null });
  }
  // Inside a chip the edge dots are its card's own pins (the level around's items).
  if (!prefix) {
    for (const e of laid?.edges ?? []) {
      const [from, to] = e.side === 'in' ? [e.from, e.address] : [e.address, e.to];
      items.push({ kind: 'e-edge', id: `${scope}|${e.id}`, key: wireKey(from, to), x: e.x, y: e.y, entry, patchLevel: level, prefix, chip, from: norm(from), to: norm(to), written: { from: written(from), to: written(to) }, edge: e.side, frame: false });
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
export const ENGINE_SELECTABLE = new Set(['e-wire', 'e-card', 'e-pin', 'e-edge']);

/** A hit as the selection keeps it (view state): the item, and the keys of what it marks. */
export function engineSelection(hit) {
  if (!hit || !ENGINE_SELECTABLE.has(hit.kind)) return null;
  return { kind: 'engine', what: hit.kind, keys: [hit.key], item: hit };
}

/**
 * The element keys a selection marks at the Engine: its own; a Wiring cable
 * the Engine's drawing of the same connection, a Wiring port both its pins.
 */
export function selectionKeys(selection) {
  if (!selection) return new Set();
  if (selection.kind === 'engine') return new Set(selection.keys);
  if (selection.kind === 'cable') return new Set([wireKey(selection.from, selection.to)]);
  if (selection.kind === 'port') return new Set([pinKey('in', selection.address), pinKey('out', selection.address)]);
  return new Set();
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
  if (frame) return { error: `Not connected: ${exportsReason(frame.module)}` };
  if (a.chip !== b.chip) return { error: `Not connected: ${a.address} and ${b.address} are at different levels (connect two cards of one level)` };
  if (a.side === b.side) return { error: `Not connected: ${a.address} and ${b.address} are both ${a.side === 'out' ? 'outputs' : 'inputs'}` };
  const [out, into] = a.side === 'out' ? [a, b] : [b, a];
  return { level: out.patchLevel, ops: [addConnection(out.local, into.local)], what: `${out.address} → ${into.address}` };
}

/** The `patch` that removes `item` (a wire, an edge dot's connection, a card): `{level, ops, what}`, or `{error}`. */
export function removeOps(item) {
  if (item?.kind === 'e-card') return { level: item.patchLevel, ops: [removeInstance(item.name)], what: `${item.address} removed` };
  if (item?.kind === 'e-wire' || item?.kind === 'e-edge') {
    if (item.frame) return { error: `Not removed: ${exportsReason(item.patchLevel)}` };
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
 * of `placed` (`Map(name → entry)`, view state of this page) put in its
 * entry while the assignment leaves it in none, and the names to forget
 * (joined an instrument, a device slot, or an entry no longer known; one
 * not in the graph is kept: its add's graph push is still to come).
 * `{owner, forget}`; `owner` a new Map.
 */
export function placedOwners(owner, placed, instances = [], entries = new Set()) {
  const out = new Map(owner);
  const forget = [];
  for (const [name, entry] of placed ?? []) {
    const instance = instances.find((i) => i.name === name);
    // Not in the graph (yet: the patch's graph push follows; or again: an undo of its removal): kept.
    if (!instance) continue;
    if (out.get(name) || roleOf(instance) || !entries.has(entry)) {
      forget.push(name);
      continue;
    }
    out.set(name, entry);
  }
  return { owner: out, forget };
}

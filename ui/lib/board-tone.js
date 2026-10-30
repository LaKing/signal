// A net's colour, one everywhere (ADR 0047 phase 4; the owner: "the
// colouring is not consistent: it turns from brownish to blue"). A wire is
// coloured by its tone: a leg of the plate (a control's binding) amber,
// any other wire by its kind (`signal`, `events`, `vector`). Inside a die
// the wires from its interface (its inputs' frame, its parameters' strip)
// carry the tone of the wire that reaches the package's pin outside, so a
// knob's amber leg stays amber into the die and on to the chip it sets, at
// every depth; a pin is the tone of its wire. Selection draws over in the
// same tone. View only: computed per drawn level from its parent's tones.
// Pure; no DOM.

import { INPUTS, PARAMS } from './schematic-layout.js';

/** The tones a wire is drawn in (each a CSS class beside `wire`). */
export const TONES = Object.freeze(['signal', 'events', 'vector', 'binding']);

/** The tone a wire has of its own: a binding's, else its kind. */
export function ownTone(wire) {
  return wire.kind === 'binding' ? 'binding' : (wire.shape ?? 'signal');
}

/**
 * The tones of `layout`'s wires (a Map of wire id → tone), `inherited` a
 * Map of interface port name → the tone outside (`packageTones`).
 */
export function wireTones(layout, inherited = null) {
  const tones = new Map();
  for (const w of layout?.wires ?? []) {
    const frame = w.from.node === INPUTS || w.from.node === PARAMS;
    tones.set(w.id, (frame && inherited?.get(w.from.port)) || ownTone(w));
  }
  return tones;
}

/**
 * The tones outside package `node` (its level's `layout`, `tones` its
 * wires'): per input pin, the tone of the wire reaching it (the first by
 * id); its outputs keep their own.
 */
export function packageTones(layout, tones, node) {
  const out = new Map();
  const wires = (layout?.wires ?? []).filter((w) => w.to.node === node.id).sort((a, b) => a.id - b.id);
  for (const w of wires) if (!out.has(w.to.port)) out.set(w.to.port, tones.get(w.id) ?? ownTone(w));
  return out;
}

/** The key of a pin's tone: its node, its side, its name. */
export const pinKey = (nodeId, out, name) => `${nodeId}\n${out ? 'out' : 'in'}\n${name}`;

/** The tones of `layout`'s pins that a wire reaches (a Map of `pinKey` → tone, the first wire by id). */
export function pinTones(layout, tones) {
  const out = new Map();
  for (const w of [...(layout?.wires ?? [])].sort((a, b) => a.id - b.id)) {
    const t = tones.get(w.id) ?? ownTone(w);
    const from = pinKey(w.from.node, true, w.from.port);
    const to = pinKey(w.to.node, false, w.to.port);
    if (!out.has(from)) out.set(from, t);
    if (!out.has(to)) out.set(to, t);
  }
  return out;
}

/**
 * The tones of every level of a scene (`levels`, outermost first, each
 * `{path, layout, parent}`): sets each entry's `tones` (wires) and
 * `pinTones`, and `toneKey`, the same text for the same inherited tones.
 */
export function sceneTones(levels) {
  for (const entry of levels) {
    let inherited = null;
    if (entry.parent?.tones) {
      const name = entry.path.split('/').at(-1);
      const node = entry.parent.layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
      if (node) inherited = packageTones(entry.parent.layout, entry.parent.tones, node);
    }
    entry.tones = wireTones(entry.layout, inherited);
    entry.pinTones = pinTones(entry.layout, entry.tones);
    entry.toneKey = inherited ? [...inherited].map(([k, v]) => `${k}=${v}`).join(',') : '';
  }
  return levels;
}

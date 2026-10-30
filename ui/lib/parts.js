// A slot's own panel part (ADR 0046): `ui/parts/<slot>.html`, a panel
// fragment whose binds are relative to its slot, its root (`/synth`):
// `.<name>` a port or parameter of the slot's interface, `/<path>.<port>`
// a port inside its content. Rooted, they are the ordinary absolute
// addresses (`/synth.cutoff`, `/synth/vcf.cutoff`) every control, stream
// and check takes. The open slot card (ui/controls/slot.js) renders its
// part rooted; the panel editor writes it as written; the schematic draws
// its controls inside the slot's box (ui/lib/schematic-controls.js). No DOM.

import { insideOf, levelData, slotOf } from './slot.js';

/** The attributes that name addresses, rooted with the part. */
export const ADDRESS_ATTRIBUTES = Object.freeze(['bind', 'position-bind', 'step-bind', 'audition']);

const NAME = /^[a-z][a-z0-9_]*$/;

/**
 * Whether a slot card (`<signal-slot>`) under `parent` is the open slot
 * card (ADR 0046 §5): a card of the host's grid (`div.signal-cards`); else
 * (in a row) the compact card.
 */
export function opensAsCard(parent) {
  return Boolean(parent?.classList?.contains('signal-cards'));
}

/** The part's file of the slot whose part's key is `key` (`synth`, `rack.fx`), relative to `ui/`. */
export function partFile(key) {
  return `parts/${key}.html`;
}

/** The slot of a part file (`parts/synth.html` → `synth`), or null. */
export function partSlot(file) {
  const match = /^parts\/([a-z][a-z0-9_]*)\.html$/.exec(file ?? '');
  return match ? match[1] : null;
}

/**
 * `address` as a part writes it, rooted at `root` (`/synth`): `.cutoff` →
 * `/synth.cutoff`, `/vcf.cutoff` → `/synth/vcf.cutoff`; null for anything
 * else (outside the slot: the host's panel binds it).
 */
export function rootAddress(address, root) {
  const a = String(address ?? '');
  if (a.startsWith('.')) return NAME.test(a.slice(1)) ? `${root}${a}` : null;
  if (a.startsWith('/')) {
    const dot = a.lastIndexOf('.');
    if (dot <= 1 || dot === a.length - 1) return null;
    const path = a.slice(1, dot).split('/');
    return path.every((s) => NAME.test(s)) ? `${root}${a}` : null;
  }
  return null;
}

/** The inverse of `rootAddress` for slot `root`: `/synth.cutoff` → `.cutoff`, `/synth/vcf.cutoff` → `/vcf.cutoff`; null outside it. */
export function relativeAddress(address, root) {
  const a = String(address ?? '');
  if (!a.startsWith(root)) return null;
  const rest = a.slice(root.length);
  if (rest.startsWith('.')) return NAME.test(rest.slice(1)) ? rest : null;
  if (!rest.startsWith('/')) return null;
  return rootAddress(rest, '') === rest ? rest : null;
}

/** A `bind`'s addresses (space or comma separated, as the controls read them). */
export function bindList(bind) {
  return String(bind ?? '')
    .split(/[\s,]+/)
    .filter(Boolean);
}

/**
 * Attribute `name`'s `value` of a part's control as the page renders it,
 * rooted at `root`: an address attribute's every address rooted (one that
 * cannot be is kept as written, so the check names it); `audition="none"`
 * and every other attribute as written.
 */
export function rootedAttribute(name, value, root) {
  if (value === undefined || value === null || !ADDRESS_ATTRIBUTES.includes(name)) return value;
  if (name === 'audition' && value.trim() === 'none') return value;
  return bindList(value)
    .map((a) => rootAddress(a, root) ?? a)
    .join(' ');
}

/** A panel tree (ui/lib/panel-tree.js) with every control's addresses rooted at `root`. */
export function rootTree(tree, root) {
  const walk = (node) => {
    if (node.kind === 'control') {
      const attributes = {};
      for (const [k, v] of Object.entries(node.attributes)) attributes[k] = rootedAttribute(k, v, root);
      return { ...node, attributes };
    }
    return node.children ? { ...node, children: node.children.map(walk) } : { ...node };
  };
  return walk(tree);
}

/**
 * The panel editor's palette (`paletteEntries`, ui/lib/panel-edit.js) for a
 * part of slot `root`: the entries inside the slot, their addresses
 * relative (what the part writes).
 */
export function partPalette(entries, root) {
  const found = [];
  for (const entry of entries ?? []) {
    const relative = relativeAddress(entry.address, root);
    if (relative) found.push({ ...entry, address: relative, label: relative, absolute: entry.address });
  }
  return found;
}

/**
 * The tree a part starts from before its first write: one card titled
 * after the slot, with a knob on each interface parameter (`.p`), as the
 * slot card shows them (`card`: `slotCard`'s, ui/lib/slot.js).
 */
export function emptyPartTree(card) {
  let next = 1;
  const id = () => `n${next++}`;
  const knobs = (card?.parameters ?? []).map((p) => ({ id: id(), kind: 'control', tag: 'signal-knob', attributes: { bind: `.${p.name}`, label: p.name } }));
  const children = [{ id: id(), kind: 'h2', classes: [], text: card?.name ?? 'part' }];
  if (knobs.length) children.push({ id: id(), kind: 'row', classes: [], children: knobs });
  const grid = { id: id(), kind: 'cards', classes: [], children: [{ id: id(), kind: 'card', classes: [], children }] };
  return { kind: 'panel', next, children: [grid] };
}

/**
 * The panel controls the schematic draws at `level` of `graph` (ADR 0046
 * §6): at the top level the main panel's (`main`, `panelControls`'), then
 * at any level each slot box's part's (its controls on the interface go
 * inside the box); at a slot's inside, opened (its own composite or its
 * read-only preview, `graph.insides`), the part's controls on the interface
 * as the frame's (`frame`) and those on inner addresses beside their inner
 * instances, their binds as written (`/vcf.cutoff`, `partOf` the slot).
 * Null when there are none.
 */
export function levelControls(graph, level, main = []) {
  const found = level === '' ? [...(main ?? [])] : [];
  const levels = [['', graph?.top], ...Object.entries(graph?.composites ?? {})];
  for (const instance of levelData(graph, level)?.instances ?? []) {
    const part = slotOf(instance)?.part;
    if (part) found.push(...partControls(part, instance.name, 0, partKey(level, instance.name)));
  }
  for (const [name, data] of levels) {
    for (const instance of data?.instances ?? []) {
      const slot = slotOf(instance);
      if (!slot?.part || insideOf(slot)?.level !== level) continue;
      for (const control of partControls(slot.part, instance.name, 0, partKey(name, instance.name))) {
        if (control.relative.every((r) => r.startsWith('.'))) found.push({ ...control, frame: true });
        else if (control.relative.every((r) => r.startsWith('/'))) found.push({ ...control, part: null, partOf: instance.name, binds: control.relative });
      }
    }
  }
  return found.length ? found : null;
}

/** The key of the part of slot `slot` at `level`: `synth`, or `rack.fx` in composite `rack`'s file (ADR 0046 §2). */
export function partKey(level, slot) {
  return level ? `${level}.${slot}` : slot;
}

/**
 * A part's controls as the schematic reads them (`graph`'s `slot.part`,
 * ADR 0046 §6, of slot `slot`, its part's key `key`): as `panelControls`
 * gives a panel's (ui/lib/schematic-controls.js), their binds rooted, each
 * with `part` (the slot), `partKey` (its file's key) and `relative` (its
 * binds as written).
 */
export function partControls(part, slot, start = 0, key = slot) {
  const controls = [];
  const seen = new Map();
  for (const c of part?.controls ?? []) {
    const type = String(c.tag ?? '').replace(/^signal-/, '');
    if (['panel', 'slot', 'piano-roll', 'step-grid'].includes(type)) continue;
    const relative = bindList(c.bind);
    if (!relative.length) continue;
    const binds = relative.map((a) => rootAddress(a, `/${slot}`)).filter(Boolean);
    if (!binds.length) continue;
    const same = `${c.tag}\n${c.bind}`;
    const occurrence = seen.get(same) ?? 0;
    seen.set(same, occurrence + 1);
    controls.push({
      index: start + controls.length,
      tag: c.tag,
      type,
      attributes: c.attributes ?? {},
      binds,
      label: c.label || '',
      occurrence,
      part: slot,
      partKey: key,
      relative,
    });
  }
  return controls;
}

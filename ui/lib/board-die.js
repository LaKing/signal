// A die opened over its package (ADR 0047 §8, the coordinator's design
// after 63bbc55). The inside is laid out at its natural size and aspect
// (`layout`'s `board.die`: flow columns, its inputs and outputs on its left
// and right edges, its parameters on its top edge) and drawn over the
// board around it as an opaque section: its rectangle grows from the
// package's (`t` 0) to its own size, one unit per its parent's unit (`t`
// 1), centred on the package and kept within its parent's bounds. Nothing
// around it moves; the outer wires stay at the small package's pins,
// faded, and each edge pin of the die says by a short stub what enters or
// leaves there (`edgeStubs`). Pure; no DOM.

import { INPUTS, OUTPUTS, PARAMS } from './schematic-layout.js';

/** An edge pin's stub reaches this far (die units) out of the die. */
export const STUB_OUT = 24;
/** A stub names at most this many of the nets at its pin, then "…". */
const NAMES = 2;

const lerp = (a, b, t) => a + (b - a) * t;

/**
 * Where die `die` (an inside's layout, its `bounds` its rectangle) is drawn
 * over package `node`, open by `t` (0 to 1), within `room` (the parent
 * level's bounds, or null): `{layout, link, rect}`, `rect` its rectangle in
 * the parent's units, `link` the transform die → parent (uniform, `rect`
 * filled, centred).
 */
export function dieView(node, die, t = 1, room = null) {
  const R = die.bounds;
  let x = node.x + node.w / 2 - R.w / 2;
  let y = node.y + node.h / 2 - R.h / 2;
  if (room) {
    x = R.w <= room.w ? Math.min(Math.max(x, room.x), room.x + room.w - R.w) : room.x;
    y = R.h <= room.h ? Math.min(Math.max(y, room.y), room.y + room.h - R.h) : room.y;
  }
  const u = Math.min(1, Math.max(0, t));
  const rect = { x: lerp(node.x, x, u), y: lerp(node.y, y, u), w: lerp(node.w, R.w, u), h: lerp(node.h, R.h, u) };
  const k = Math.min(rect.w / Math.max(R.w, 1e-9), rect.h / Math.max(R.h, 1e-9));
  const link = { k, x: rect.x + (rect.w - R.w * k) / 2 - R.x * k, y: rect.y + (rect.h - R.h * k) / 2 - R.y * k };
  return { layout: die, link, rect };
}

/**
 * The stubs at the die's edge pins (in its units): each input, parameter
 * and output pin with what the level around connects to the package's pin
 * of that name (`parent`, its layout): `[{corners, at, text, side, shape}]`,
 * `side` the edge (`left`, `top`, `right`). A pin connected to nothing
 * outside has none.
 */
export function edgeStubs(node, parent, die) {
  const out = [];
  const names = (list) => {
    const unique = [...new Set(list)];
    return unique.length > NAMES ? `${unique.slice(0, NAMES).join(', ')}, …` : unique.join(', ');
  };
  const into = (port) =>
    parent.wires.filter((w) => w.to.node === node.id && w.to.port === port).map((w) => (w.kind === 'binding' ? String(w.label).split(' ⇢')[0] : String(w.written?.from ?? `${w.from.node}.${w.from.port}`).replace(/^\//, '')));
  const from = (port) => parent.wires.filter((w) => w.from.node === node.id && w.from.port === port).map((w) => String(w.written?.to ?? `${w.to.node}.${w.to.port}`).replace(/^\//, ''));
  const frame = (id) => die.nodes.find((n) => n.id === id);
  for (const p of frame(INPUTS)?.outs ?? []) {
    const text = names(into(p.name));
    if (!text) continue;
    const at = { x: p.x - STUB_OUT, y: p.y };
    out.push({ corners: [at, { x: p.x, y: p.y }], at, text, side: 'left', shape: p.shape ?? 'signal' });
  }
  for (const p of frame(PARAMS)?.outs ?? []) {
    const text = names(into(p.name));
    if (!text) continue;
    const at = { x: p.x, y: p.y - STUB_OUT };
    out.push({ corners: [at, { x: p.x, y: p.y }], at, text, side: 'top', shape: p.shape ?? 'signal' });
  }
  for (const p of frame(OUTPUTS)?.ins ?? []) {
    const text = names(from(p.name));
    if (!text) continue;
    const at = { x: p.x + STUB_OUT, y: p.y };
    out.push({ corners: [{ x: p.x, y: p.y }, at], at, text, side: 'right', shape: p.shape ?? 'signal' });
  }
  return out;
}

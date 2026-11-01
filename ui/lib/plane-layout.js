// The Signal Project plane's geometry (ADR 0051 §3–4), in plane units (1
// CSS px of a front panel at its own size; a pu is 8): each Stack a
// vertical column of its instruments' front panels at the Stack's width,
// a fixed rail between them, a header with its name; its edge pins; the
// peripherals beside the Stacks (devices that receive from Stacks to their
// right, those that only send to their left, in inventory order); the
// Setup-level cables. A function of what is written and measured only (a
// front panel's natural size, measured once per text), never of the view:
// no rectangle moves with the zoom (ADR 0051 test 4). Pure; no DOM.

import { PU } from './plate.js';
import { RAIL_PU } from './plane-file.js';

/** The Stack's header (its name, a click focuses it), in pu: room for its text at TEXT.head's cap. */
export const HEADER_PU = 8;
/** Between Stacks laid out side by side, in pu. */
export const STACK_GAP_PU = 24;
/** Between the Stacks and the peripherals: room for the Setup cables, in pu. */
export const CABLE_GAP_PU = 56;
/** A peripheral box: its width, its title's height, a port's pitch and a channel's, in pu (room for the text at its caps, TEXT). */
export const BOX_PU = Object.freeze({ w: 80, title: 14, port: 12, channel: 3, pad: 3 });
/** The list of other devices (`splitBoxes`' `others`): its head, each small box, the gap between them and the inner pad, in pu. */
export const OTHERS_PU = Object.freeze({ head: 9, box: 12, gap: 2, pad: 2 });
/** Between peripheral boxes, in pu. */
export const BOX_GAP_PU = 6;
/** An edge pin's pitch within its instrument, in pu. */
export const PIN_PITCH_PU = 3;
/** The tray of unstacked instances: its width, and a line's pitch, in pu. */
export const TRAY_PU = 56;
export const TRAY_LINE_PU = 6;

/**
 * The Setup level's text (the operator's review of phase 1: readable at the
 * default Setup fit). Each kind of text has a size in plane units at the
 * Stack level and above (`base`: it scales with the zoom there, as the
 * schematic's text does), a smallest size on screen (`min`, CSS px) it
 * keeps while zooming out, and the most it may grow to in plane units
 * (`cap`: the room the layout gives it, so nothing overlaps), past which it
 * scales down with the zoom again. The layout never moves with the zoom:
 * only these sizes do, continuously (`textSize`).
 */
export const TEXT = Object.freeze({
  /** A device's name, the list's head. */
  title: Object.freeze({ base: 34, min: 11, cap: 46 }),
  /** "AUDIO DEVICE", a note. */
  kind: Object.freeze({ base: 20, min: 9, cap: 36 }),
  /** A port's name, an edge pin's label, a line of the tray. */
  port: Object.freeze({ base: 28, min: 9.5, cap: 40 }),
  /** A role on its port ("main · stereo"). */
  role: Object.freeze({ base: 24, min: 9.5, cap: 38 }),
  /** A small box's name in the list of other devices. */
  small: Object.freeze({ base: 26, min: 11, cap: 44 }),
  /** The Stack's header: its name. */
  head: Object.freeze({ base: 18, min: 11, cap: 44 }),
  /** The Stack's header: its instrument count. */
  count: Object.freeze({ base: 13, min: 9.5, cap: 38 }),
});

/** A text of kind `rule` (TEXT's) at view scale `k`: its size in plane units, `min / k` clamped to `[base, cap]`; continuous and never rising as `k` grows. */
export function textSize(rule, k) {
  const wanted = k > 0 ? rule.min / k : rule.cap;
  return Math.min(rule.cap, Math.max(rule.base, wanted));
}

/** Every TEXT kind's size at scale `k`, in plane units: `{title, kind, …}`. */
export function textSizes(k) {
  return Object.fromEntries(Object.entries(TEXT).map(([name, rule]) => [name, textSize(rule, k)]));
}

/** How many characters of a text of kind `rule` fit `width` plane units at its cap (an average glyph of 0.58 em): a name cut to it never outgrows its box, whatever the zoom. */
export function capChars(rule, width) {
  return Math.max(1, Math.floor(width / (rule.cap * 0.58)));
}

/**
 * One Stack's instruments laid out (ADR 0051 §4, "Equal width"): `stack`
 * (`{width}` in pu), `sizes` (each instrument's front panel as measured,
 * `{entry, w, h, x}` in plane units: its natural width and height, and a
 * placed card's x, or null). Each instrument's rectangle is the Stack's
 * width; a narrower panel is centred between blank side plates (at its
 * place's x when it fits there), a wider one scaled down whole (`scale` <
 * 1, `wide` true: a warning). A rail of RAIL_PU separates instruments and
 * edges. `{w, h, top, rects: [{entry, x, y, w, h, panel: {x, y, w, h, scale}, wide}]}`,
 * relative to the Stack's top-left; `top` is where the first instrument's
 * rail starts (below the header).
 */
export function stackLayout(stack, sizes) {
  const w = stack.width * PU;
  const rail = RAIL_PU * PU;
  const top = HEADER_PU * PU;
  let y = top + rail;
  const rects = [];
  for (const s of sizes) {
    const natural = Math.max(1, s.w || w);
    const scale = natural > w ? w / natural : 1;
    const pw = natural * scale;
    const ph = Math.max(PU, (s.h || PU) * scale);
    const placed = Number.isFinite(s.x) && s.x >= 0 && s.x + pw <= w ? s.x : (w - pw) / 2;
    // Heights vary, like rack units: whole pu, never under the panel.
    const h = Math.ceil(ph / PU - 1e-9) * PU;
    rects.push({ entry: s.entry, x: 0, y, w, h, panel: { x: placed, y, w: pw, h: ph, scale }, wide: scale < 1 });
    y += h + rail;
  }
  return { w, h: y, top, rects };
}

/** The y (plane units, Stack-relative) of each edge pin of a Stack: grouped per instrument, from its rectangle's top, PIN_PITCH_PU apart. */
export function pinPlaces(pins, rects) {
  const byEntry = new Map(rects.map((r) => [r.entry, r]));
  const seen = new Map();
  const fallback = rects.length ? rects[rects.length - 1] : { y: HEADER_PU * PU, h: 0 };
  return pins.map((pin) => {
    const rect = byEntry.get(pin.instrument) ?? fallback;
    const key = `${pin.side} ${pin.instrument}`;
    const n = seen.get(key) ?? 0;
    seen.set(key, n + 1);
    return { ...pin, y: rect.y + (2 + n * PIN_PITCH_PU) * PU };
  });
}

/** A peripheral box's size (plane units): its title, then a row per port of its taller edge. */
export function boxSize(box) {
  const rows = Math.max(1, box.inputs.length, box.outputs.length);
  return { w: BOX_PU.w * PU, h: (BOX_PU.title + rows * BOX_PU.port + BOX_PU.pad) * PU };
}

/**
 * A box's ports placed (plane units, box-relative): each port's row, and
 * each channel's pin on the box's edge (inputs on the left at x 0, outputs
 * on the right at its width). `{inputs: [{...port, y, pins: [{channel, y}]}], outputs}`.
 */
export function boxPorts(box) {
  const { w } = boxSize(box);
  const place = (ports, x) =>
    ports.map((p, i) => {
      const y = (BOX_PU.title + i * BOX_PU.port) * PU;
      // A row of the rest of many channels spreads its pins over the row; a port of one or two at the channel pitch.
      const pitch = p.channels.length > 2 ? ((BOX_PU.port - 2) * PU) / (p.channels.length - 1) : BOX_PU.channel * PU;
      const first = y + (BOX_PU.port * PU - (p.channels.length - 1) * pitch) / 2;
      return { ...p, x, y, h: BOX_PU.port * PU, pins: p.channels.map((channel, j) => ({ channel, y: first + j * pitch })) };
    });
  return { inputs: place(box.inputs, 0), outputs: place(box.outputs, w) };
}

/**
 * The plane laid out: `stacks` (`[{name, at, size: {w, h}}]`, `at` in pu
 * or null), `boxes` (`peripherals`' boxes, inventory order), `tray` (the
 * unstacked instances' count). Stacks with an `at` stand there; the others
 * left to right from the origin, top-aligned, STACK_GAP_PU apart (after
 * the placed ones' right edge). Receiving devices stand CABLE_GAP_PU to the
 * right of the Stacks, sending-only ones as far to their left, each column
 * from the Stacks' top, BOX_GAP_PU apart. The tray stands below the
 * sending column, left of the Stacks; the list of other devices (`others`:
 * `{count, expanded}`) below the receiving column. `{stacks: Map(name →
 * rect), boxes: Map(key → rect), tray: rect | null, others: rect | null,
 * bounds}`.
 */
export function planeLayout({ stacks = [], boxes = [], tray = 0, others = { count: 0, expanded: false } } = {}) {
  const placed = new Map();
  let right = 0;
  for (const s of stacks) {
    if (!Array.isArray(s.at)) continue;
    const r = { x: s.at[0] * PU, y: s.at[1] * PU, w: s.size.w, h: s.size.h };
    placed.set(s.name, r);
    right = Math.max(right, r.x + r.w + STACK_GAP_PU * PU);
  }
  let x = placed.size ? right : 0;
  for (const s of stacks) {
    if (placed.has(s.name)) continue;
    placed.set(s.name, { x, y: 0, w: s.size.w, h: s.size.h });
    x += s.size.w + STACK_GAP_PU * PU;
  }
  const rects = [...placed.values()];
  const minX = rects.length ? Math.min(...rects.map((r) => r.x)) : 0;
  const maxX = rects.length ? Math.max(...rects.map((r) => r.x + r.w)) : 0;
  const minY = rects.length ? Math.min(...rects.map((r) => r.y)) : 0;
  const boxRects = new Map();
  let ry = minY;
  let ly = minY;
  for (const box of boxes) {
    const size = boxSize(box);
    if (box.side === 'right') {
      boxRects.set(box.key, { x: maxX + CABLE_GAP_PU * PU, y: ry, ...size });
      ry += size.h + BOX_GAP_PU * PU;
    } else {
      boxRects.set(box.key, { x: minX - CABLE_GAP_PU * PU - size.w, y: ly, ...size });
      ly += size.h + BOX_GAP_PU * PU;
    }
  }
  const trayRect = tray > 0 ? { x: minX - CABLE_GAP_PU * PU - TRAY_PU * PU, y: ly, w: TRAY_PU * PU, h: (BOX_PU.title + tray * TRAY_LINE_PU + BOX_PU.pad) * PU } : null;
  // The other devices: one list below the receiving devices (where most of them would stand).
  const othersRect = others.count > 0 ? { x: maxX + CABLE_GAP_PU * PU, y: ry, ...othersSize(others) } : null;
  const all = [...rects, ...boxRects.values(), ...(trayRect ? [trayRect] : []), ...(othersRect ? [othersRect] : [])];
  const bounds = all.length
    ? (() => {
        const x0 = Math.min(...all.map((r) => r.x));
        const y0 = Math.min(...all.map((r) => r.y));
        const x1 = Math.max(...all.map((r) => r.x + r.w));
        const y1 = Math.max(...all.map((r) => r.y + r.h));
        return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
      })()
    : { x: 0, y: 0, w: STACK_GAP_PU * PU, h: STACK_GAP_PU * PU };
  return { stacks: placed, boxes: boxRects, tray: trayRect, others: othersRect, bounds };
}

/** The list of other devices' size (plane units): its head alone, collapsed; expanded, a small box per device below it. */
export function othersSize({ count = 0, expanded = false } = {}) {
  const rows = expanded ? count : 0;
  const h = OTHERS_PU.head + (rows ? rows * OTHERS_PU.box + (rows - 1) * OTHERS_PU.gap + OTHERS_PU.pad : 0);
  return { w: BOX_PU.w * PU, h: h * PU };
}

/** The small boxes of the expanded list (plane units, list-relative): `[{x, y, w, h}]`, one per device, in order. */
export function othersBoxes({ count = 0, expanded = false } = {}) {
  if (!expanded) return [];
  const w = (BOX_PU.w - 2 * OTHERS_PU.pad) * PU;
  return Array.from({ length: count }, (_, i) => ({ x: OTHERS_PU.pad * PU, y: (OTHERS_PU.head + i * (OTHERS_PU.box + OTHERS_PU.gap)) * PU, w, h: OTHERS_PU.box * PU }));
}

/**
 * A Setup-level cable's path (plane units): from a Stack's right-edge pin
 * `a` to a device's input pin `b`, a horizontal S-curve (its tangents
 * horizontal at both ends; to a device on the left it loops round). `lane`
 * spreads cables leaving one Stack apart where they bend.
 */
export function cablePath(a, b, lane = 0) {
  const reach = Math.max(PU * 6, Math.abs(b.x - a.x) / 2 + lane * PU);
  const c1 = { x: a.x + reach, y: a.y };
  const c2 = { x: b.x - reach, y: b.y };
  const f = (n) => Math.round(n * 100) / 100;
  return `M${f(a.x)} ${f(a.y)} C${f(c1.x)} ${f(c1.y)} ${f(c2.x)} ${f(c2.y)} ${f(b.x)} ${f(b.y)}`;
}

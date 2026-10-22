// The port labels of composites' boxes and of the frame inside a composite
// (M2.13-2, ADR 0030 §1; the owner's "on the outside of the composite you
// can't see which of the 20 inputs is what … labels on the edge of the
// composite too"): always readable, at a fixed size on screen whatever the
// zoom, and never on each other. (2026-10-20, the owner's "everything should
// zoom nicely": the control now places them in each level's own units with
// `levelLabels`, where they scale with the drawing like its boxes; what they
// say goes by size, `labelDetail` in ui/lib/schematic-groups.js, and a text
// too small to read is hidden, never kept at a size on screen. The screen
// placement below is what it is built on.) Where they would collide they are grouped,
// never overlapped and never left out: a box's side shows every pin, else
// its runs of one name prefix (`osc1_* (3)`), else those runs merged
// (`osc1_*, osc2_* (6)`), else a count (`31 inputs`); a box whose sides
// still collide shows one summary (`synth: 31 in · 1 out`), and boxes whose
// summaries collide share one (`synth, echo +2: 98 ports`). One pass over
// every label on screen resolves collisions between boxes the same way.
// Widths are measured (`setMeasure`: the control measures its font once
// per text; else a per-character table estimates them), so a label's text
// always fits it. A box's labels and its summary stay inside the box: a
// label too long is cut short (`shown`, ending in `…`; `text` keeps the
// whole), a summary wraps; a frame's labels go on their pins' other side
// where they would leave the viewport. Also the
// notes beside a boundary's pins: where a frame's pin connects outside
// (`/melody.hz → hz`) and where a box's pin goes inside (`hz → 2:
// glide.in, hz_prev.in`), one connection away. A note shows the signal's
// path (2026-10-21, the owner's "show the signal's path"): its arrow points
// the way the signal flows, the source first, so the pin's name stands
// after an upstream note and before a downstream one. Pure; no DOM.

import { neighbours, pinInfo } from './schematic-trace.js';

/** A label's text size (px), whatever the zoom. */
export const LABEL_PX = 9;
/** A label's height (px): labels of one side are at least this far apart. */
export const PITCH_PX = 11;
/** A label's padding and border, both sides (px; `.plabel`'s 2 px padding and 1 px border each side), and a pixel to spare. */
export const LABEL_PAD_PX = 7;
/** A label starts this far from its pin, into the box. */
export const INSET_PX = 5;
/** At most this many labels on screen. */
export const MAX_LABELS = 400;
/** A box's coarsest level of its own: one summary. */
export const SUMMARY = 5;
/** A collapsed box narrower than this on screen (px) shows its summary, not its groups' labels. */
export const COLLAPSED_PX = 66;
/** The most names a note lists. */
export const NOTE_NAMES = 3;
/**
 * Text smaller than this on screen (px) is not drawn (2026-10-20): drawn in
 * its level's units, a label scales with the drawing and is hidden below
 * it, or replaced by a coarser one (`labelDetail`, ui/lib/schematic-groups.js),
 * never kept at a size on screen.
 */
export const READ_PX = 4;
/** A boundary pin's note (`/melody.hz → hz`) is shown from this text size on screen (px). */
export const NOTE_PX = 7;
/** A bundle's count's text, in its level's units (LABEL_PX the pins'): readable while bundles are drawn. */
export const COUNT_UNITS = 18;
/** The most a summary's text grows, in multiples of a pin label's size: so it reads where pins' names no longer do. */
export const SUMMARY_GROW = 6;
/** A frame's label is at most this wide in its level's units: longer, it is cut short (the whole is its title). */
export const FRAME_LABEL_UNITS = 260;

/**
 * Character advances in em of a system sans-serif (SF, Helvetica), each
 * rounded up: the estimate used until the control measures its own font
 * (`setMeasure`), so a label is never narrower than its text.
 */
const EM = new Map([
  ...[...'il.,:;\'|!'].map((c) => [c, 0.3]),
  ...[...'fjrt()[]-'].map((c) => [c, 0.4]),
  [' ', 0.3],
  ['*', 0.5],
  ['_', 0.55],
  ...[...'0123456789'].map((c) => [c, 0.64]),
  ...[...'mw'].map((c) => [c, 0.9]),
  ...[...'MW@'].map((c) => [c, 1]),
  ...[...'←→…·'].map((c) => [c, 1]),
]);
const LOWER_EM = 0.6;
const UPPER_EM = 0.72;

/** The estimated width (px) of `text` at LABEL_PX. */
export function estimateWidth(text) {
  let em = 0;
  for (const c of text) em += EM.get(c) ?? (c >= 'A' && c <= 'Z' ? UPPER_EM : c.charCodeAt(0) > 0x7f ? 1 : LOWER_EM);
  return em * LABEL_PX;
}

let measureText = estimateWidth;
const measured = new Map();

/**
 * Labels measured by `measure(text)` (px of `text` at LABEL_PX in the
 * labels' font: the control's canvas `measureText`, once per text), or by
 * the estimate again (null). Each text is measured once and remembered.
 */
export function setMeasure(measure) {
  measureText = measure ?? estimateWidth;
  measured.clear();
}

/** The width (px) of `text` at LABEL_PX, as measured. */
export function measureWidth(text) {
  let w = measured.get(text);
  if (w === undefined) {
    if (measured.size > 20000) measured.clear();
    w = measureText(text);
    measured.set(text, w);
  }
  return w;
}

/** A label's width on screen for `text` (px): its text measured, its padding and border. */
export function textWidth(text) {
  return Math.ceil(measureWidth(text) + LABEL_PAD_PX);
}

/** `text` as shown in a label `width` px wide: whole when it fits, else cut short ending in `…` (only `…` when nothing more fits). */
export function fitText(text, width) {
  if (textWidth(text) <= width) return text;
  let lo = 0;
  let hi = text.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (textWidth(`${text.slice(0, mid).trimEnd()}…`) <= width) lo = mid;
    else hi = mid - 1;
  }
  return `${text.slice(0, lo).trimEnd()}…`;
}

/** The narrowest a label of `text` is cut to: its first two characters and `…` (the whole when shorter). */
export function minWidth(text) {
  return Math.min(textWidth(text), textWidth(`${text.slice(0, 2)}…`));
}

/**
 * What a label `width` px wide shows of `text`: all of it, else a group's
 * names without its count (`osc1_*` of `osc1_* (4)`), else a noted pin's
 * short form (`hz → 2` of `hz → 2: glide.in, hz_prev.in`, `2 → out` of
 * `2: a.out, b.out → out`), else its name `name` alone (a note is never cut
 * short: `hz → 2:…` read as noise, the operator's review), else cut short.
 */
export function shownText(text, width, name = null) {
  if (textWidth(text) <= width) return text;
  const bare = /^(.*\S) \(\d+\)$/.exec(text)?.[1];
  if (bare && textWidth(bare) <= width) return bare;
  if (name && name !== text) {
    // The label without a trace's summary: the name then a downstream note, or an upstream note then the name.
    const head = text.split(' · trace: ')[0];
    const down = head.startsWith(`${name} → `);
    const up = head.endsWith(` → ${name}`);
    if (head === name || down || up) {
      const count = /^(\d+):/.exec(down ? head.slice(name.length + 3) : up ? head : '')?.[1];
      const short = count ? (down ? `${name} → ${count}` : `${count} → ${name}`) : null;
      if (short && textWidth(short) <= width) return short;
      if (textWidth(name) <= width) return name;
    }
  }
  return fitText(text, width);
}

/**
 * `text` wrapped into at most `lines` lines of `width` px (a label's),
 * between words (a count kept with its word, `·` with the word before),
 * each line cut short if it still does not fit.
 */
export function wrapText(text, width, lines) {
  if (lines <= 1 || textWidth(text) <= width) return [fitText(text, width)];
  const words = [];
  for (const w of text.split(' ')) {
    const last = words.at(-1);
    if (last !== undefined && (w === '·' || /^[+]?\d+$/.test(last))) words[words.length - 1] = `${last} ${w}`;
    else words.push(w);
  }
  const out = [];
  for (const w of words) {
    const next = out.length ? `${out.at(-1)} ${w}` : w;
    if (out.length && textWidth(next) <= width) out[out.length - 1] = next;
    else out.push(w);
  }
  if (out.length > lines) out.splice(lines - 1, out.length, out.slice(lines - 1).join(' '));
  return out.map((line) => fitText(line, width));
}

/** The group a pin name belongs to: its text up to the first `_` (`osc1_`), else without trailing digits (`t` of `t1`); null for none. */
export function prefixOf(name) {
  const cut = name.indexOf('_');
  if (cut > 0 && cut < name.length - 1) return name.slice(0, cut + 1);
  const digits = /^(.*\D)\d+$/.exec(name);
  return digits ? digits[1] : null;
}

/** Adjacent pins of one prefix, as `[{keys: [label], pins}]` (a pin alone keeps its name). */
function runsOf(pins, given = null) {
  // M2.13-2: a box's own port groups (ui/lib/schematic-groups.js), its pins laid out in them.
  if (given) {
    const at = new Map(pins.map((p) => [p.name, p]));
    return given
      .map((g) => ({ keys: [g.pins.length > 1 ? g.label : g.pins[0].name], pins: g.pins.map((p) => at.get(p.name)).filter(Boolean) }))
      .filter((g) => g.pins.length);
  }
  const runs = [];
  let key;
  for (const pin of pins) {
    const k = prefixOf(pin.name);
    if (k !== null && runs.length && k === key) runs.at(-1).pins.push(pin);
    else runs.push({ keys: [k], pins: [pin] });
    key = k;
  }
  return runs.map((r) => ({ keys: [r.pins.length > 1 ? `${r.keys[0]}*` : r.pins[0].name], pins: r.pins }));
}

/** The text of a group of pins: its one pin's (with `texts`' note), else its prefixes and count. */
function groupText(group, texts) {
  if (group.pins.length === 1) return texts?.get(group.pins[0].name) ?? group.pins[0].name;
  const keys = group.keys;
  const names = keys.length === 1 ? keys[0] : keys.length === 2 ? `${keys[0]}, ${keys[1]}` : `${keys[0]} … ${keys.at(-1)}`;
  return `${names} (${group.pins.length})`;
}

const centre = (group) => (group.pins[0].y + group.pins.at(-1).y) / 2;

function fits(groups) {
  for (let i = 1; i < groups.length; i++) if (centre(groups[i]) - centre(groups[i - 1]) < PITCH_PX) return false;
  return true;
}

/** Side `pins` (top to bottom, screen y) grouped at level 1 (every pin) … 4 (a count). */
function sideGroups(pins, level, word, given = null) {
  // One pin is its own label at every level (a count would be no shorter).
  if (level <= 1 || pins.length === 1) return pins.map((p) => ({ keys: [p.name], pins: [p] }));
  let groups = runsOf(pins, given);
  if (level === 2) return groups;
  if (level === 3) {
    // The closest neighbours merged first, until the side fits: groups apart enough keep their own labels.
    groups = [...groups];
    while (groups.length > 1 && !fits(groups)) {
      let at = 1;
      for (let i = 2; i < groups.length; i++) if (centre(groups[i]) - centre(groups[i - 1]) < centre(groups[at]) - centre(groups[at - 1])) at = i;
      const [a, b] = [groups[at - 1], groups[at]];
      groups.splice(at - 1, 2, { keys: [...a.keys, ...b.keys], pins: [...a.pins, ...b.pins] });
    }
    // Merged down to one: the count says it better.
    return groups.length === 1 && pins.length > 1 ? sideGroups(pins, 4, word, given) : groups;
  }
  return [{ keys: [], pins, count: `${pins.length} ${pins.length === 1 ? word : `${word}s`}` }];
}

/** The finest level (1–4) at which side `pins` fit. */
function finest(pins, word, given = null, from = 1) {
  for (let level = from; level < 4; level++) if (fits(sideGroups(pins, level, word, given))) return level;
  return 4;
}

export function overlaps(a, b) {
  return a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;
}

function inside(rect, box) {
  return rect.x0 >= box.x - 0.5 && rect.x1 <= box.x + box.w + 0.5 && rect.y0 >= box.y - 0.5 && rect.y1 <= box.y + box.h + 0.5;
}

function labelAt(text, cx, cy, align, pins, extra = {}, width = null) {
  const w = width ?? textWidth(text);
  const x0 = align === 'left' ? cx : align === 'right' ? cx - w : cx - w / 2;
  const label = { text, x0, y0: cy - PITCH_PX / 2, x1: x0 + w, y1: cy + PITCH_PX / 2, align, pins, ...extra };
  // What fits of it: `shown` (the whole `text` is the label's title).
  label.shown = shownText(text, w, pins.length === 1 ? pins[0].name : null);
  return label;
}

/** Label `label` `width` px wide, kept at its pin (its aligned edge), its text cut to fit. */
function narrowed(label, width) {
  if (width >= label.x1 - label.x0) return label;
  const x0 = label.align === 'right' ? label.x1 - width : label.x0;
  return { ...label, x0, x1: x0 + width, shown: shownText(label.text, width, label.pins.length === 1 ? label.pins[0].name : null) };
}

/**
 * Text `text` centred on `rect` (screen px) and never outside it: wrapped
 * into as many lines as its height holds; else the first of its shorter
 * forms `shorter` that fits whole; else the last cut short to three
 * characters or more; else nothing shown (a box too small to read, whose
 * name is not drawn either). The whole `text` is its title.
 */
function centredIn(text, rect, pins, extra = {}, shorter = []) {
  const room = Math.max(1, rect.w - 2);
  const most = Math.max(1, Math.floor(rect.h / PITCH_PX));
  let lines = null;
  for (const form of [text, ...shorter]) {
    // Whole: its lines, put together again, are the form (none cut short).
    const wrapped = wrapText(form, Math.min(textWidth(form), room), most);
    if (wrapped.join(' ') === form) {
      lines = wrapped;
      break;
    }
  }
  if (!lines) {
    const last = [text, ...shorter].at(-1);
    const cut = fitText(last, room);
    lines = [cut.length >= 4 || cut === last ? cut : ''];
  }
  // Nothing shown still holds its box's room: what collides with it is grouped as before.
  const w = lines[0] === '' ? room : Math.min(room, Math.max(...lines.map(textWidth)));
  const cx = rect.x + rect.w / 2;
  const cy = rect.y + rect.h / 2;
  const h = (lines.length * PITCH_PX) / 2;
  return { text, x0: cx - w / 2, y0: cy - h, x1: cx + w / 2, y1: cy + h, align: 'centre', pins, lines: lines.length, shown: lines.join('\n'), ...extra };
}

/**
 * The narrowest label `label` may be cut to: a pin's name whole up to five
 * characters, else its first four and `…` (a guess at a pin is no label);
 * a group's or a count's its first four and `…` as well.
 */
function leastOf(label) {
  if (label.pins.length !== 1) return Math.min(textWidth(label.text), textWidth(`${label.text.slice(0, 4).trimEnd()}…`));
  const name = label.pins[0].name;
  return Math.min(textWidth(label.text), textWidth(name.length <= 5 ? name : `${name.slice(0, 4)}…`));
}

/**
 * The labels of a closed box (`labels`, its two sides') kept inside it:
 * none wider than the box, and where a left and a right one share rows,
 * the longer cut so both fit side by side. Null when one would have to be
 * cut narrower than `leastOf` (the box then shows its summary).
 */
function insideBox(box, labels) {
  const inner = box.rect.w - 2 * INSET_PX;
  const widths = labels.map((l) => Math.min(l.x1 - l.x0, inner));
  const GAP = 2;
  labels.forEach((a, i) => {
    if (a.align !== 'left') return;
    labels.forEach((b, j) => {
      if (b.align !== 'right' || !(a.y0 < b.y1 && b.y0 < a.y1)) return;
      const room = b.x1 - a.x0 - GAP;
      if (widths[i] + widths[j] <= room) return;
      const [short, long] = widths[i] <= widths[j] ? [i, j] : [j, i];
      if (widths[short] <= room / 2) widths[long] = Math.min(widths[long], room - widths[short]);
      else {
        widths[i] = Math.min(widths[i], room / 2);
        widths[j] = Math.min(widths[j], room / 2);
      }
    });
  });
  if (labels.some((l, i) => widths[i] < leastOf(l) - 1e-9)) return null;
  return labels.map((l, i) => narrowed(l, widths[i]));
}

/**
 * Side labels `labels` (one side's, aligned `align`) on the other side of
 * their pins (`xs`, each pin's x): a frame's kept on screen, an open
 * composite's box's named outside its edge. Each keeps its width and its
 * `shown`, so a label is never wider than it was placed: an outward box's
 * labels are therefore placed without its inside's bound (`boxLabels`).
 */
function flipped(labels, xs) {
  return labels.map((l, i) => {
    const w = l.x1 - l.x0;
    const align = l.align === 'left' ? 'right' : 'left';
    const x0 = align === 'left' ? xs[i] + INSET_PX : xs[i] - INSET_PX - w;
    return { ...l, align, x0, x1: x0 + w };
  });
}

/** A label of `text` centred on screen point `at` that labels no pin: a bundle's count (M2.13-2). */
export function labelBox(text, at) {
  return labelAt(text, at.x, at.y, 'centre', [], { count: true });
}

/**
 * Bundles' counts (`[{text, at: [point]}]`, each its places along its
 * bundle in order of preference, screen pixels) added to `labels` (placed
 * labels), none on another and none left out. With `viewport` (`{w, h}`,
 * the labels' area, which clips) every count is kept wholly inside it: a
 * place near an edge is moved in. Each count goes at its first place free;
 * else a row at a time up and down from its first place; else at the first
 * free place in the viewport, row by row; and in a viewport with no room
 * left at all, at its first place, so that it is still seen. Without a
 * viewport, rows up and down until one is free (finitely many labels).
 * Returns the counts' labels.
 */
export function placeCounts(labels, counts, viewport = null) {
  const placed = [...labels];
  const out = [];
  const within = (c) => !viewport || (c.x0 >= 0 && c.y0 >= 0 && c.x1 <= viewport.w && c.y1 <= viewport.h);
  const free = (c) => within(c) && !placed.some((l) => overlaps(l, c));
  // A place moved in from the viewport's edges, so the whole count is inside.
  const at = (text, p) => {
    if (!viewport) return labelBox(text, p);
    const w = textWidth(text);
    return labelBox(text, { x: Math.min(Math.max(p.x, w / 2), viewport.w - w / 2), y: Math.min(Math.max(p.y, PITCH_PX / 2), viewport.h - PITCH_PX / 2) });
  };
  for (const count of counts) {
    let label = count.at.map((p) => at(count.text, p)).find(free) ?? null;
    const first = at(count.text, count.at[0]);
    const cx = (first.x0 + first.x1) / 2;
    const cy = (first.y0 + first.y1) / 2;
    const rows = viewport ? Math.ceil(viewport.h / PITCH_PX) : Infinity;
    for (let step = 1; !label && step <= rows; step++) {
      label = [cy - step * PITCH_PX, cy + step * PITCH_PX].map((y) => labelBox(count.text, { x: cx, y })).find(free) ?? null;
    }
    if (!label && viewport) {
      const w = textWidth(count.text);
      for (let y = PITCH_PX / 2; !label && y + PITCH_PX / 2 <= viewport.h; y += PITCH_PX) {
        for (let x = w / 2; !label && x + w / 2 <= viewport.w; x += w) {
          const candidate = labelBox(count.text, { x, y });
          if (free(candidate)) label = candidate;
        }
      }
    }
    // No room anywhere in the viewport: still shown, at its own place.
    label ??= first;
    placed.push(label);
    out.push(label);
  }
  return out;
}

/** What box `box` is called in a summary. */
function boxName(box) {
  return box.kind === 'input' ? 'inputs' : box.kind === 'output' ? 'outputs' : box.name;
}

/** The pins of `box` as `{box: index, name}`. */
function pinsOf(box, index) {
  return [...box.ins, ...box.outs].map((p) => ({ box: index, name: p.name }));
}

/**
 * The labels of box `box` (index `index`) at least at level `forced`:
 * `{level, labels}`. Each side at the finer of `forced` and the finest it
 * fits at; a box whose two sides collide, or at level 5, one summary
 * centred on it.
 */
export function boxLabels(box, index, forced = 1, viewport = null) {
  // A collapsed box narrower than COLLAPSED_PX shows its summary: so zooming out goes from groups to the summary once, not back and forth.
  if (forced < SUMMARY && !(box.collapsed && box.kind === 'instance' && box.rect.w < COLLAPSED_PX)) {
    let labels = [];
    let level = 0;
    const sides = [
      [box.ins, 'left', 1, 'input', box.groups?.ins ?? null],
      [box.outs, 'right', -1, 'output', box.groups?.outs ?? null],
    ];
    for (const [pins, align, dir, word, given] of sides) {
      if (!pins.length) continue;
      // A collapsed box (its groups each one pin) labels its groups, not its hidden pins.
      const from = box.collapsed ? 2 : 1;
      const l = Math.max(forced, finest(pins, word, given, from));
      level = Math.max(level, l);
      const side = [];
      const xs = [];
      for (const g of sideGroups(pins, l, word, given)) {
        const pinX = l === 4 ? pins[0].x : g.pins[0].x;
        const text = g.count ?? groupText(g, box.texts);
        // A closed box's note stays on its own half of the box, cut short (the whole text is the title and the tip), never narrower than the pin's name.
        // An open composite's (`outward`) is named outside its edge, where its box's half is no bound (2026-10-22).
        const noted = box.kind === 'instance' && !box.outward && g.pins.length === 1 && text !== g.pins[0].name;
        const whole = box.widest ? Math.min(textWidth(text), box.widest) : null;
        const width = noted ? Math.min(textWidth(text), Math.max(textWidth(g.pins[0].name), box.rect.w / 2 - INSET_PX)) : whole;
        side.push(labelAt(text, pinX + dir * INSET_PX, centre(g), align, g.pins.map((p) => ({ box: index, name: p.name })), {}, width));
        xs.push(pinX);
      }
      // A frame's labels kept on screen: where one in its rows would leave it at a side, on their pins' other side (inside the frame). Its corner at `x`, `y` (a level's units), else 0, 0.
      const [vx, vy] = [viewport?.x ?? 0, viewport?.y ?? 0];
      const off = (lb) => lb.y1 > vy && lb.y0 < vy + (viewport.h ?? Infinity) && (lb.x0 < vx || lb.x1 > vx + viewport.w);
      // An open composite's box (`outward`, 2026-10-20): its inside is drawn in it, so its pins are named outside its edge.
      labels.push(...(box.outward || (box.kind !== 'instance' && viewport && side.some(off)) ? flipped(side, xs) : side));
    }
    if (box.kind === 'instance' && !box.outward) {
      const kept = insideBox(box, labels);
      if (kept) return { level: Math.max(level, 1), labels: kept };
    } else {
      const left = labels.filter((l) => l.align === 'left');
      const right = labels.filter((l) => l.align === 'right');
      if (box.outward || !left.some((a) => right.some((b) => overlaps(a, b)))) return { level: Math.max(level, 1), labels };
    }
  }
  return { level: SUMMARY, labels: [summaryLabel(box, index)] };
}

/**
 * A box's one summary (`synth: 31 in · 1 out`): a closed box's inside it
 * (wrapped, cut short), below its header (`header` px) where there is room
 * for a line; a frame's centred on it.
 */
function summaryLabel(box, index) {
  const r = box.rect;
  const header = box.header ?? 0;
  const body = header > 0 && r.h - header >= PITCH_PX ? { x: r.x, y: r.y + header, w: r.w, h: r.h - header } : r;
  const text = box.kind === 'instance' ? `${box.name}: ${box.ins.length} in · ${box.outs.length} out` : `${boxName(box)}: ${box.ins.length + box.outs.length}`;
  // In a level's units (`grow`, 2026-10-20): as large as its box holds it whole, up to SUMMARY_GROW times a pin's label.
  if (box.grow) {
    for (let f = SUMMARY_GROW; f > 1; f -= 0.5) {
      const area = { x: body.x / f, y: body.y / f, w: body.w / f, h: body.h / f };
      const label = box.kind === 'instance' ? centredIn(text, area, pinsOf(box, index)) : labelAt(text, area.x + area.w / 2, area.y + area.h / 2, 'centre', pinsOf(box, index));
      if (label.shown.split('\n').join(' ') === text && label.x1 - label.x0 <= area.w && label.y1 - label.y0 <= area.h) return scaleLabel(label, f);
    }
  }
  if (box.kind === 'instance') return centredIn(text, body, pinsOf(box, index), {}, [box.name]);
  return labelAt(text, r.x + r.w / 2, r.y + r.h / 2, 'centre', pinsOf(box, index));
}

/** `label` `f` times as large (its coordinates and its text's size, `scale`), from the origin. */
export function scaleLabel(label, f) {
  if (f === 1) return label;
  return { ...label, x0: label.x0 * f, y0: label.y0 * f, x1: label.x1 * f, y1: label.y1 * f, scale: (label.scale ?? 1) * f };
}

/**
 * The port labels of a level's boxes in its own units (2026-10-20, the
 * owner's "everything should zoom nicely"): drawn in the level, they scale
 * with it like its boxes. `boxes` as `placeLabels` takes them, in the
 * level's coordinates (`grow` set: a summary as large as its box holds);
 * `detail` (`labelDetail`'s) the coarsest a box starts at: `pins` every pin
 * named, `groups` its groups (`osc1_* (4)`; a box's groups collapsed, a
 * frame's runs of one group), `summary` one summary. A frame's labels are
 * at most FRAME_LABEL_UNITS wide. With `viewport` (`{x, y, w, h}`, the
 * drawing area in the level's units) a frame's side whose labels would
 * leave it is named on its pins' other side, inside the frame
 * (`frameSides` says when that changes). Pure.
 */
export function levelLabels(boxes, detail = 'pins', viewport = null) {
  const { labels } = placeLabels(levelBoxes(boxes), { cap: Infinity, level: detailLevel(detail), viewport });
  return labels.map((l) => {
    const box = boxes[l.pins[0]?.box];
    return box && box.kind !== 'instance' && !l.scale && l.x1 - l.x0 > FRAME_LABEL_UNITS ? narrowed(l, FRAME_LABEL_UNITS) : l;
  });
}

/** The level (1 every pin … SUMMARY) every box starts at for `labelDetail`'s `detail`. */
function detailLevel(detail) {
  return detail === 'summary' ? SUMMARY : detail === 'groups' ? 2 : 1;
}

/** `boxes` as a level places them: a summary as large as its box holds (`grow`), a frame's label at most FRAME_LABEL_UNITS (`widest`). */
function levelBoxes(boxes) {
  return boxes.map((b) => ({ grow: true, ...(b.kind !== 'instance' ? { widest: FRAME_LABEL_UNITS } : {}), ...b }));
}

/**
 * Which way the labels of the frames among `boxes` (as `levelLabels` takes
 * them) point in `viewport` (`{x, y, w, h}`, the drawing area in the
 * level's units): one letter a label (`l`, `r`), frames apart by `|`. It
 * changes only when a frame's side would leave the viewport or come back
 * (2026-10-22: at deep zoom a frame's inputs' names went past the
 * drawing area's left edge, by the sidebar), so the control places a
 * level's labels again only then, never on every pan.
 */
export function frameSides(boxes, detail, viewport) {
  const level = detailLevel(detail);
  return levelBoxes(boxes)
    .map((b, i) => (b.kind === 'instance' ? '' : boxLabels(b, i, level, viewport).labels.map((l) => l.align[0]).join('')))
    .join('|');
}

/**
 * Bundles' counts (as `placeCounts` takes them, in a level's units) placed
 * among `labels` at COUNT_UNITS (`size` units their text): none on another
 * label, each its `scale`.
 */
export function levelCounts(labels, counts, size = COUNT_UNITS) {
  const f = size / LABEL_PX;
  const down = (l) => ({ ...l, x0: l.x0 / f, y0: l.y0 / f, x1: l.x1 / f, y1: l.y1 / f });
  const placed = placeCounts(labels.map(down), counts.map((c) => ({ ...c, at: c.at.map((p) => ({ x: p.x / f, y: p.y / f })) })));
  return placed.map((l) => scaleLabel(l, f));
}

/** One label for the boxes `members` of `boxes`, centred on them. */
function clusterLabel(boxes, members) {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  let ports = 0;
  for (const i of members) {
    const r = boxes[i].rect;
    x0 = Math.min(x0, r.x);
    y0 = Math.min(y0, r.y);
    x1 = Math.max(x1, r.x + r.w);
    y1 = Math.max(y1, r.y + r.h);
    ports += boxes[i].ins.length + boxes[i].outs.length;
  }
  const names = members.slice(0, 2).map((i) => boxName(boxes[i]));
  const more = members.length > 2 ? ` +${members.length - 2}` : '';
  const text = `${names.join(', ')}${more}: ${ports} ${ports === 1 ? 'port' : 'ports'}`;
  // Within the boxes it names, as a box's summary is within its box.
  return centredIn(text, { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, members.flatMap((i) => pinsOf(boxes[i], i)), { cluster: true }, [`${names.join(', ')}${more}`, names[0]]);
}

/** The first pair of labels of different units that overlap, by the units' order: `{a, b, la, lb}` (unit indexes), or null. */
function firstOverlap(units) {
  const all = [];
  units.forEach((u, i) => {
    for (const label of u.labels) all.push({ label, unit: i });
  });
  all.sort((p, q) => p.label.x0 - q.label.x0);
  let best = null;
  for (let i = 0; i < all.length; i++) {
    const p = all[i];
    for (let j = i + 1; j < all.length && all[j].label.x0 < p.label.x1; j++) {
      const q = all[j];
      if (q.unit === p.unit || !overlaps(p.label, q.label)) continue;
      const [a, b] = p.unit < q.unit ? [p, q] : [q, p];
      if (!best || a.unit < best.a || (a.unit === best.a && b.unit < best.b)) best = { a: a.unit, b: b.unit, la: a.label, lb: b.label };
    }
  }
  return best;
}

/**
 * The labels of `boxes` on screen, none on another: `{labels, iterations}`.
 * A box is `{kind: 'instance' | 'input' | 'output', name, rect: {x, y, w,
 * h}, ins, outs, texts, header?}` in screen pixels (`header` the height of
 * its title, which its summary keeps clear of), its pins `{name, x, y}` top to
 * bottom (`ins` on its left edge, `outs` on its right), `texts` a Map of a
 * pin's full label (its name and notes) where it has one. A label is
 * `{text, shown, x0, y0, x1, y1, align, pins: [{box, name}], lines?,
 * cluster?}` (`shown` what fits of `text`, lines apart by `\n`); every pin
 * is in exactly one label. With `viewport` (`{w, h}`), a frame's labels
 * stay in it. Each box starts at the finest level it fits at
 * by itself; while two labels of different boxes overlap, the pair's box
 * whose label leaves its own box (else the finer, else the later) is made
 * one level coarser, and two summaries (or clusters) become one cluster.
 * Each step coarsens a box (at most five times each) or merges two units,
 * so it ends within 6 × boxes steps, at worst as one cluster. More than
 * `cap` labels: every box is made coarser, and when all are summaries,
 * neighbours in order are merged, until at most `cap` remain. `level`: the
 * coarsest every box starts at (1 every pin … SUMMARY). A box with `grow`
 * has its summary as large as it holds whole (`scale`); one with `outward`
 * (an open composite's) its labels outside its edge.
 */
export function placeLabels(boxes, { cap = MAX_LABELS, viewport = null, level = 1 } = {}) {
  let units = boxes.map((box, i) => ({ members: [i], ...boxLabels(box, i, level, viewport) }));
  let iterations = 0;
  const coarsen = (u) => Object.assign(u, boxLabels(boxes[u.members[0]], u.members[0], u.level + 1, viewport));
  const merge = (a, b) => {
    const members = [...a.members, ...b.members].sort((x, y) => x - y);
    return { members, level: SUMMARY + 1, labels: [clusterLabel(boxes, members)] };
  };
  for (;;) {
    const pair = firstOverlap(units);
    if (!pair) {
      if (units.reduce((n, u) => n + u.labels.length, 0) <= cap) break;
      iterations += 1;
      const fine = units.filter((u) => u.level < SUMMARY);
      if (fine.length) fine.forEach(coarsen);
      else {
        // Every unit one label, still too many: neighbours in order share one.
        const merged = [];
        for (let i = 0; i < units.length; i += 2) merged.push(units[i + 1] ? merge(units[i], units[i + 1]) : units[i]);
        units = merged;
      }
      continue;
    }
    iterations += 1;
    const a = units[pair.a];
    const b = units[pair.b];
    const fine = [
      [a, pair.la],
      [b, pair.lb],
    ].filter(([u]) => u.level < SUMMARY);
    if (!fine.length) {
      units.splice(pair.b, 1);
      units[pair.a] = merge(a, b);
      continue;
    }
    // Prefer the box whose label leaves its own box (it intrudes), then the finer, then the later.
    const leaving = fine.filter(([u, label]) => !inside(label, boxes[u.members[0]].rect));
    const pool = leaving.length ? leaving : fine;
    const [pick] = pool.reduce((best, cur) => (cur[0].level <= best[0].level ? cur : best));
    coarsen(pick);
  }
  return { labels: units.flatMap((u) => u.labels), iterations };
}

/** The address of a neighbour relative to instance `base` (`glide.in` inside `/synth`), else whole. */
function relative(address, base) {
  return base && address.startsWith(`${base}/`) ? address.slice(base.length + 1) : address;
}

/**
 * A note for `neighbours`' answer of a composite pin, the signal's path
 * (2026-10-21, the owner's "show the signal's path"): downstream (the
 * other end is fed by the pin) it starts with the arrow, `→ /echo.in`,
 * `→ 2: glide.in, hz_prev.in` (inside, relative to the composite instance
 * `base`); upstream (the other end feeds the pin) it ends with it,
 * `/melody.hz →`, `2: a.out, b.out →`, `inside a library composite →`,
 * `nothing →`; `… n more` past NOTE_NAMES; null without an answer.
 * `pinText` puts the pin's name on the arrow's other side.
 */
export function noteText(answer, base = '') {
  if (!answer) return null;
  const down = answer.direction === 'down';
  const arrowed = (text) => (down ? `→ ${text}` : `${text} →`);
  if (answer.opaque) return arrowed('inside a library composite');
  const names = answer.addresses.map((a) => relative(a, base));
  const n = names.length;
  if (n === 0) return arrowed('nothing');
  const list = names.slice(0, NOTE_NAMES).join(', ') + (n > NOTE_NAMES ? `, … ${n - NOTE_NAMES} more` : '');
  return arrowed(n === 1 ? list : `${n}: ${list}`);
}

/**
 * Where a frame's pin (`/synth.hz` at level `/synth`) connects outside:
 * `/melody.hz →` (its label `/melody.hz → hz`). None for a parameter
 * nothing outside connects to (2026-10-22: `nothing → cutoff` on 28 of
 * the synth's 31 inputs was noise): it is left to its value, which its
 * hollow ring and the sidebar say. An unconnected port keeps
 * `nothing → hz`, an output `out → nothing`.
 */
export function outsideNote(graph, address, r) {
  const answer = neighbours(graph, address, 'outside', r);
  if (answer && !answer.opaque && answer.addresses.length === 0 && answer.direction === 'up' && pinInfo(graph, address, r)?.parameter) return null;
  return noteText(answer);
}

/** Where a closed composite's pin (`/synth.hz`) goes inside: `→ 2: glide.in, hz_prev.in`. */
export function insideNote(graph, address, r) {
  const base = address.slice(0, address.lastIndexOf('.'));
  return noteText(neighbours(graph, address, 'inside', r), base);
}

/**
 * A pin's label: its name and its note in the signal's direction (an
 * upstream note, ending in `→`, before the name: `/melody.hz → hz`; a
 * downstream one after it: `hz → /echo.in`), and a trace's summary after
 * the whole (never instead of it).
 */
export function pinText(name, note = null, traced = null) {
  const head = !note ? name : note.endsWith('→') ? `${note} ${name}` : `${name} ${note}`;
  return traced ? `${head} · trace: ${traced}` : head;
}

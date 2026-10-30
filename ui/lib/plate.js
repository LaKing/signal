// The front plate (ADR 0047 §1–4, docs/ui-controls.md "Panels"): a panel's
// fixed geometry. A panel file's `<signal-panel plate="W H">` is the plate's
// size and each card's `place="x y w h"` its box, in plate units (`pu`, 8 CSS
// px; 1280 px, the reference width, is 160 pu). The plate is laid out once at
// its own size and scaled whole to the window: the window changes the scale,
// never the arrangement. A panel without places is laid out once at 1280 px
// (the implicit freeze); Freeze layout writes what it measured. Pure: the
// numbers, the rounding, the default panel's packing and the editor's moves.
// No DOM.

/** CSS px per plate unit at the reference size. */
export const PU = 8;
/** The reference plate width in pu (1280 px): an unplaced panel is laid out at it. */
export const REFERENCE_PU = 160;
export const REFERENCE_PX = REFERENCE_PU * PU;
/**
 * A placed card is drawn this many px inside its place on every side, so
 * two cards on neighbouring places keep a gap of one pu between them. An
 * unplaced plate is padded by as much, so a card at its edge freezes at 0.
 */
export const GUTTER = 4;
/** Fitting the width never goes below this scale: under it the plate pans sideways too (ADR 0047 §2, tunable). */
export const MIN_SCALE = 0.5;
/** ⌘+ and ⌘− multiply the scale by this. */
export const ZOOM_STEP = 1.25;
export const ZOOM_MIN = 0.1;
export const ZOOM_MAX = 8;
/** The largest plate side in pu (as the server checks it). */
export const MAX_PLATE = 1000;
/** The smallest card the editor makes by resizing, in pu. */
export const MIN_CARD = Object.freeze({ w: 8, h: 4 });

function numbers(text, n) {
  const parts = String(text ?? '').trim().split(/\s+/);
  if (parts.length !== n || !parts.every((p) => /^\d{1,4}$/.test(p))) return null;
  return parts.map(Number);
}

/** `plate="W H"` → `{w, h}` (each 1 to MAX_PLATE), or null. */
export function parsePlate(text) {
  const n = numbers(text, 2);
  return n && n.every((v) => v >= 1 && v <= MAX_PLATE) ? { w: n[0], h: n[1] } : null;
}

/** `place="x y w h"` → `{x, y, w, h}` (`w`, `h` at least 1), or null. */
export function parsePlace(text) {
  const n = numbers(text, 4);
  return n && n[2] >= 1 && n[3] >= 1 && n.every((v) => v <= MAX_PLATE) ? { x: n[0], y: n[1], w: n[2], h: n[3] } : null;
}

export const plateText = ({ w, h }) => `${w} ${h}`;
export const placeText = ({ x, y, w, h }) => `${x} ${y} ${w} ${h}`;

/** A place's drawn box in plate px (`{x, y, w, h}`): inside it by GUTTER. */
export function drawnBox(place) {
  return { x: place.x * PU + GUTTER, y: place.y * PU + GUTTER, w: place.w * PU - 2 * GUTTER, h: place.h * PU - 2 * GUTTER };
}

/**
 * The place (pu) of a card drawn at `box` (plate px, `{x, y, w, h}`): its
 * box with the gutter around it, rounded outward to whole pu, so the drawn
 * box of the place contains `box` (nothing gets smaller).
 */
export function freezeBox(box) {
  const x0 = Math.max(0, Math.floor((box.x - GUTTER) / PU + 1e-9));
  const y0 = Math.max(0, Math.floor((box.y - GUTTER) / PU + 1e-9));
  const x1 = Math.max(x0 + 1, Math.ceil((box.x + box.w + GUTTER) / PU - 1e-9));
  const y1 = Math.max(y0 + 1, Math.ceil((box.y + box.h + GUTTER) / PU - 1e-9));
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/**
 * Freeze layout (ADR 0047 §2): the places of cards measured at `boxes`
 * (plate px, in the panel's order) and the plate around them, at least
 * `width` pu wide (the reference width). `{plate, places}`.
 */
export function freeze(boxes, { width = REFERENCE_PU } = {}) {
  const places = boxes.map(freezeBox);
  const w = Math.max(width, ...places.map((p) => p.x + p.w));
  const h = Math.max(1, ...places.map((p) => p.y + p.h));
  return { plate: { w, h }, places };
}

/** Whether places `a` and `b` overlap. */
export function overlap(a, b) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** Whether `place` lies on `plate`. */
export function within(place, plate) {
  return place.x >= 0 && place.y >= 0 && place.x + place.w <= plate.w && place.y + place.h <= plate.h;
}

/**
 * Why `place` may not go where it is (the editor's refusal), or null:
 * beyond the plate, or over one of `others`.
 */
export function blocked(place, others, plate) {
  if (!within(place, plate)) return 'it would reach beyond the plate';
  const hit = others.find((o) => o && overlap(place, o));
  return hit ? `it would overlap the card at ${placeText(hit)}` : null;
}

/** A pointer's travel `px` (screen px) at view scale `scale` in whole pu. */
export function snap(px, scale) {
  return Math.round(px / (scale > 0 ? scale : 1) / PU);
}

/** `place` moved by `dx`, `dy` pu, kept on `plate`. */
export function moved(place, dx, dy, plate) {
  const x = Math.min(Math.max(0, place.x + dx), Math.max(0, plate.w - place.w));
  const y = Math.min(Math.max(0, place.y + dy), Math.max(0, plate.h - place.h));
  return { ...place, x, y };
}

/**
 * `place` resized by dragging its corner `corner` (`nw`, `ne`, `sw`, `se`)
 * by `dx`, `dy` pu: the opposite corner stays, at least MIN_CARD, kept on
 * `plate`.
 */
export function resized(place, corner, dx, dy, plate) {
  let x0 = place.x;
  let y0 = place.y;
  let x1 = place.x + place.w;
  let y1 = place.y + place.h;
  if (corner.includes('w')) x0 = Math.min(Math.max(0, x0 + dx), x1 - MIN_CARD.w);
  else x1 = Math.max(Math.min(plate.w, x1 + dx), x0 + MIN_CARD.w);
  if (corner.includes('n')) y0 = Math.min(Math.max(0, y0 + dy), y1 - MIN_CARD.h);
  else y1 = Math.max(Math.min(plate.h, y1 + dy), y0 + MIN_CARD.h);
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/**
 * The view's scale for `mode` (`{kind: 'width'}`: fit the width, never
 * under MIN_SCALE; `{kind: 'whole'}`: the whole plate in the view;
 * `{kind: 'manual', scale}`) of a plate (or crop) `size` (px) in a view of
 * `view` (`{w, h}` px). View state only: it never changes the plate.
 */
export function fitScale(mode, view, size) {
  const sw = view.w > 0 && size.w > 0 ? view.w / size.w : 1;
  if (mode?.kind === 'manual') return clampZoom(mode.scale);
  if (mode?.kind === 'whole') {
    const sh = view.h > 0 && size.h > 0 ? view.h / size.h : sw;
    return clampZoom(Math.min(sw, sh));
  }
  return Math.max(MIN_SCALE, sw);
}

function clampZoom(s) {
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Number.isFinite(s) ? s : 1));
}

/** ⌘+ (`step` 1), ⌘− (-1): the scale `scale` zoomed, as a manual mode. */
export function zoomed(scale, step) {
  return { kind: 'manual', scale: clampZoom(scale * ZOOM_STEP ** step) };
}

/** The bounding box of rectangles `rects` (`{x, y, w, h}`), or null for none. */
export function union(rects) {
  const list = rects.filter((r) => r && r.w > 0 && r.h > 0);
  if (!list.length) return null;
  const x = Math.min(...list.map((r) => r.x));
  const y = Math.min(...list.map((r) => r.y));
  return { x, y, w: Math.max(...list.map((r) => r.x + r.w)) - x, h: Math.max(...list.map((r) => r.y + r.h)) - y };
}

// The generated default panel's packing (ADR 0047 §2): each card's size
// from its controls' fixed cells, the cards on shelves 160 pu wide in their
// order. The cells are the controls' boxes at the theme's default sizes
// (ui/controls/theme.css) with their label and readout, rounded up.

/** A control's cell (CSS px, its label and readout included). */
export const CELLS = Object.freeze({
  'signal-knob': { w: 88, h: 128 },
  'signal-fader': { w: 88, h: 272 },
  'signal-meter': { w: 64, h: 264 },
  'signal-scope': { w: 320, h: 200 },
  'signal-toggle': { w: 88, h: 64 },
  'signal-button': { w: 88, h: 64 },
  'signal-value': { w: 96, h: 48 },
  'signal-xy': { w: 184, h: 208 },
});
/** A slot's card (ADR 0037 §7): a fixed size, its ports and parameters inside. */
export const SLOT_CARD = Object.freeze({ w: 60, h: 40 });
/** A card's padding (px, each side) and its heading's and description's lines with their gaps. */
const PAD_X = 16;
const PAD_Y = 12;
const LINE = 28;
const GAP_X = 16;
const GAP_Y = 12;
const GROUP_GAP = 8;
/** A card is at least this wide (pu): its heading fits. */
export const MIN_CARD_W = 30;

/**
 * A generated card's size in pu: `{w, h}` from its heading (and
 * description, `note`) and its items, each a control's tag or a group
 * (`{group: [tags]}`, side by side), in rows that wrap at the plate's
 * width; `wide` takes the plate's width.
 */
export function cardSize(items, { note = false, wide = false, width = REFERENCE_PU } = {}) {
  const inner = width * PU - 2 * GUTTER - 2 * PAD_X;
  const cell = (item) => {
    if (typeof item === 'string') return CELLS[item] ?? CELLS['signal-knob'];
    const cells = item.group.map((t) => CELLS[t] ?? CELLS['signal-knob']);
    return { w: cells.reduce((s, c) => s + c.w, 0) + GROUP_GAP * Math.max(0, cells.length - 1), h: Math.max(0, ...cells.map((c) => c.h)) };
  };
  const rows = [];
  let row = null;
  for (const c of items.map(cell)) {
    if (!row || row.w + GAP_X + c.w > inner) {
      row = { w: Math.min(c.w, inner), h: c.h };
      rows.push(row);
    } else {
      row.w += GAP_X + c.w;
      row.h = Math.max(row.h, c.h);
    }
  }
  const contentW = Math.max(0, ...rows.map((r) => r.w));
  const contentH = rows.reduce((s, r) => s + r.h, 0) + GAP_Y * Math.max(0, rows.length - 1);
  const px = (v) => Math.ceil((v + 2 * GUTTER) / PU);
  const w = wide ? width : Math.min(width, Math.max(MIN_CARD_W, px(contentW + 2 * PAD_X)));
  const h = px(2 * PAD_Y + LINE + (note ? LINE : 0) + contentH);
  return { w, h };
}

/**
 * Cards of sizes `sizes` (pu) on shelves `width` pu wide, in their order:
 * a card goes right of the one before while it fits, else on a new shelf;
 * the cards of a shelf take its height. `{plate, places}`. Pure: the same
 * sizes give the same plate.
 */
export function pack(sizes, { width = REFERENCE_PU } = {}) {
  const places = [];
  const shelves = [];
  let shelf = null;
  for (const size of sizes) {
    const w = Math.min(width, Math.max(1, size.w));
    const h = Math.max(1, size.h);
    if (!shelf || shelf.x + w > width) {
      shelf = { y: shelf ? shelf.y + shelf.h : 0, x: 0, h: 0, members: [] };
      shelves.push(shelf);
    }
    const place = { x: shelf.x, y: shelf.y, w, h };
    places.push(place);
    shelf.members.push(place);
    shelf.x += w;
    shelf.h = Math.max(shelf.h, h);
  }
  for (const s of shelves) for (const p of s.members) p.h = s.h;
  const last = shelves[shelves.length - 1];
  return { plate: { w: width, h: last ? last.y + last.h : 1 }, places };
}

/**
 * The generated panel's plate (`defaultPanel`'s spec, ui/lib/panel.js):
 * its cards in the order the panel builds them (a card per instance, one
 * per slot unless `slots` is false, the wide output card) packed.
 */
export function defaultPlate(spec, { slots = true } = {}) {
  const sizes = (spec?.groups ?? []).map((g) => cardSize(g.controls.map((c) => c.element), { note: Boolean(g.module) }));
  if (slots) for (const _ of spec?.slots ?? []) sizes.push({ ...SLOT_CARD });
  const output = [...(spec?.meters?.length ? [{ group: spec.meters.map(() => 'signal-meter') }] : []), ...(spec?.scope ? ['signal-scope'] : [])];
  if (output.length) sizes.push(cardSize(output, { wide: true }));
  return pack(sizes);
}

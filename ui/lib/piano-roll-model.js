// <signal-piano-roll>'s model (ADR 0028 §2, §7): the view's geometry
// (beats and keys to pixels and back, zoom about a point, scrolling), snap,
// hit-testing of notes, their ends and velocity bars, and the edits (add,
// move, resize, delete, velocity, loop length, duplicate) with a local undo
// history. No DOM: ui/controls/piano-roll.js draws it and feeds it pointer
// and key input. Notes carry an `id` here only; `pattern()` strips it.

import { SCHEMA_VERSION, compareNotes, normalizePattern, notesOverlap } from './pattern-text.js';
import { keyCommand } from './keys.js';

export const KEYS = 128;
export const MIN_KEY = 0;
export const MAX_KEY = 127;
const BLACK = new Set([1, 3, 6, 8, 10]);
const NAMES = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];

/** Snap choices: label and beats (a beat is a quarter note). */
export const SNAPS = Object.freeze([
  { label: '1/4', beats: 1 },
  { label: '1/8', beats: 1 / 2 },
  { label: '1/16', beats: 1 / 4 },
  { label: '1/32', beats: 1 / 8 },
  { label: '1/8T', beats: 1 / 3 },
  { label: '1/16T', beats: 1 / 6 },
]);

export const ZOOM = Object.freeze({ minBeat: 6, maxBeat: 480, minRow: 6, maxRow: 40 });

/** Whether `key` is a black key. */
export const isBlack = (key) => BLACK.has(((key % 12) + 12) % 12);

/** A key's name, 60 = C4 (the library's convention, ADR 0028 §2). */
export function keyName(key) {
  return `${NAMES[key % 12]}${Math.floor(key / 12) - 1}`;
}

export const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

/** Rounds to a multiple of `grid`, cleaned of float noise (1/3 stays 1/3's float). */
export function snap(beat, grid) {
  if (!(grid > 0)) return beat;
  const n = Math.round(beat / grid + 1e-9);
  return clean(n * grid);
}

/** The multiple of `grid` at or below `beat`. */
export function snapFloor(beat, grid) {
  if (!(grid > 0)) return beat;
  return clean(Math.floor(beat / grid + 1e-9) * grid);
}

/**
 * Removes float noise: a value within 1e-6 of a 1/192-beat multiple becomes
 * exactly n/192, which covers every snap (1/32 and triplets included) and
 * the free 1/64 grid, so a moved note writes 0.75, not 0.7500000000000001.
 */
export function clean(beat) {
  const ticks = beat * 192;
  const r = Math.round(ticks);
  return Math.abs(ticks - r) < 1e-6 ? r / 192 : beat;
}

// ---------------------------------------------------------------- the view

/**
 * The view: pixels per beat, row height, the scroll offsets (pixels of the
 * content left of and above the viewport), and the viewport's size.
 */
export function makeView(overrides = {}) {
  return { beatPx: 48, rowPx: 14, scrollX: 0, scrollY: 0, width: 800, height: 400, ...overrides };
}

export const contentHeight = (view) => KEYS * view.rowPx;
export const beatToX = (beat, view) => beat * view.beatPx - view.scrollX;
export const xToBeat = (x, view) => (x + view.scrollX) / view.beatPx;
/** The top of a key's row: 127 at the top. */
export const keyToY = (key, view) => (MAX_KEY - key) * view.rowPx - view.scrollY;
export const yToKey = (y, view) => clamp(MAX_KEY - Math.floor((y + view.scrollY) / view.rowPx), MIN_KEY, MAX_KEY);

/** A note's rectangle in viewport pixels. */
export function noteRect(note, view) {
  return { x: beatToX(note.start, view), y: keyToY(note.key, view), w: note.length * view.beatPx, h: view.rowPx };
}

/** The view scrolled, kept inside the content (`totalBeats` wide). */
export function clampScroll(view, totalBeats) {
  const maxX = Math.max(0, totalBeats * view.beatPx - view.width);
  const maxY = Math.max(0, contentHeight(view) - view.height);
  return { ...view, scrollX: clamp(view.scrollX, 0, maxX), scrollY: clamp(view.scrollY, 0, maxY) };
}

/** Zoomed by `factor` horizontally about viewport x `at`: the beat under it stays. */
export function zoomX(view, factor, at, totalBeats) {
  const beat = xToBeat(at, view);
  const beatPx = clamp(view.beatPx * factor, ZOOM.minBeat, ZOOM.maxBeat);
  return clampScroll({ ...view, beatPx, scrollX: beat * beatPx - at }, totalBeats);
}

/** Zoomed by `factor` vertically about viewport y `at`: the key under it stays. */
export function zoomY(view, factor, at, totalBeats) {
  const row = (at + view.scrollY) / view.rowPx;
  const rowPx = clamp(view.rowPx * factor, ZOOM.minRow, ZOOM.maxRow);
  return clampScroll({ ...view, rowPx, scrollY: row * rowPx - at }, totalBeats);
}

/** Scrolled so `key` is in the vertical middle. */
export function centreKey(view, key, totalBeats) {
  return clampScroll({ ...view, scrollY: (MAX_KEY - key + 0.5) * view.rowPx - view.height / 2 }, totalBeats);
}

/**
 * The grid's vertical lines in [b0, b1]: `{beat, level}` with level 0 for a
 * bar, 1 a beat, 2 a snap subdivision. A level is left out when its lines
 * would be closer than `minPx`.
 */
export function gridLines(b0, b1, view, { beatsPerBar = 4, grid = 0.25, minPx = 7 } = {}) {
  const lines = [];
  const fine = [grid, 1, beatsPerBar].filter((beats) => beats * view.beatPx >= minPx);
  // Zoomed out past bars: every 2nd, 4th… bar.
  const unit = fine.length ? Math.min(...fine) : beatsPerBar * 2 ** Math.ceil(Math.log2(minPx / (beatsPerBar * view.beatPx)));
  const first = Math.max(0, Math.ceil(b0 / unit - 1e-9));
  for (let i = first; i * unit <= b1 + 1e-9; i++) {
    const beat = clean(i * unit);
    const onBar = Math.abs(beat / beatsPerBar - Math.round(beat / beatsPerBar)) < 1e-9;
    const onBeat = Math.abs(beat - Math.round(beat)) < 1e-9;
    lines.push({ beat, level: onBar ? 0 : onBeat ? 1 : 2 });
  }
  return lines;
}

// ---------------------------------------------------------------- hit-testing

/**
 * The note under viewport point (x, y): `{id, part}` with part `end` within
 * `edgePx` of its right edge (never more than a third of a short note),
 * else `body`; the last drawn (topmost) note wins. Null on empty grid.
 */
export function hitNote(notes, x, y, view, edgePx = 6) {
  const key = yToKey(y, view);
  const beat = xToBeat(x, view);
  for (let i = notes.length - 1; i >= 0; i--) {
    const n = notes[i];
    if (n.key !== key || beat < n.start || beat >= n.start + n.length) continue;
    const right = beatToX(n.start + n.length, view);
    const edge = Math.min(edgePx, (n.length * view.beatPx) / 3);
    return { id: n.id, part: right - x <= edge ? 'end' : 'body' };
  }
  return null;
}

/** The ids of notes touching the beat × key box (inclusive key range). */
export function notesInBox(notes, b0, b1, k0, k1) {
  const [lo, hi] = [Math.min(b0, b1), Math.max(b0, b1)];
  const [klo, khi] = [Math.min(k0, k1), Math.max(k0, k1)];
  return notes.filter((n) => n.key >= klo && n.key <= khi && n.start < hi && n.start + n.length > lo).map((n) => n.id);
}

/**
 * The velocity bar nearest viewport x within `tolPx`: the note id, a
 * selected note preferred among notes starting together, then the higher key.
 */
export function hitVelocity(notes, x, view, selected = new Set(), tolPx = 5) {
  let best = null;
  let bestScore = Infinity;
  for (const n of notes) {
    const d = Math.abs(beatToX(n.start, view) - x);
    if (d > tolPx) continue;
    const score = d * 1000 + (selected.has(n.id) ? 0 : 500) - n.key;
    if (score < bestScore) {
      bestScore = score;
      best = n.id;
    }
  }
  return best;
}

/** A velocity from a y in a lane `height` px tall (top = 1, bottom = 0), rounded to 1/127. */
export function velocityAt(y, height, pad = 4) {
  const v = clamp(1 - (y - pad) / Math.max(1, height - 2 * pad), 0, 1);
  return Math.round(v * 127) / 127;
}

// ---------------------------------------------------------------- the model

const scratch = new DataView(new ArrayBuffer(8));

/** The largest float below a positive finite `x`. */
export function prevFloat(x) {
  if (!(x > 0)) return 0;
  scratch.setFloat64(0, x);
  scratch.setBigUint64(0, scratch.getBigUint64(0) - 1n);
  return scratch.getFloat64(0);
}

/**
 * The length of a note at `start` that ends at `end` without passing it as
 * a float sum: `start + length <= end` holds exactly (a third of a beat's
 * multiples would otherwise end one unit in the last place past it, an
 * overlap by the strict rule). 0 when nothing is left.
 */
export function trimmedLength(start, end) {
  let length = clean(end - start);
  while (length > 0 && start + length > end) length = prevFloat(length);
  return length > 0 ? length : 0;
}

/** Removes the notes overlapping others on their key: an edited note wins, else the later trims the earlier. */
export function resolveOverlaps(notes, edited = new Set()) {
  const byKey = new Map();
  for (const n of notes) {
    if (!byKey.has(n.key)) byKey.set(n.key, []);
    byKey.get(n.key).push({ ...n });
  }
  const out = [];
  for (const list of byKey.values()) {
    list.sort((a, b) => a.start - b.start || (edited.has(a.id) ? 1 : 0) - (edited.has(b.id) ? 1 : 0));
    const kept = [];
    for (const n of list) {
      let drop = false;
      while (kept.length) {
        const prev = kept[kept.length - 1];
        if (!notesOverlap(prev, n)) break;
        const prevEdited = edited.has(prev.id);
        const nEdited = edited.has(n.id);
        if (prevEdited && !nEdited) {
          drop = true; // the edited note covers this start
          break;
        }
        // Trim the earlier note to end where this one starts; gone if nothing is left.
        const length = trimmedLength(prev.start, n.start);
        if (length > 0) {
          prev.length = length;
          break;
        }
        kept.pop();
      }
      if (!drop) kept.push(n);
    }
    out.push(...kept);
  }
  return out.sort(compareNotes);
}

/**
 * The piano roll's editable state: the notes (with ids), the loop length,
 * the selection, the last length drawn, and a local undo history of
 * snapshots. Every edit returns whether anything changed.
 */
export class PianoRollModel {
  constructor(pattern = { schema_version: SCHEMA_VERSION, kind: 'notes', length: 16, notes: [] }) {
    this.nextId = 1;
    this.selected = new Set();
    this.lastLength = null; // the snap's length until a note is drawn or resized
    this.undoStack = [];
    this.redoStack = [];
    this.drag = null;
    this.load(pattern);
  }

  /** Replaces the content (a pattern from outside): the history is kept only as states. */
  load(pattern, { keepHistory = false } = {}) {
    const p = normalizePattern(pattern);
    this.length = p.length;
    this.notes = p.notes.map((n) => ({ ...n, id: this.nextId++ }));
    this.selected = new Set();
    if (!keepHistory) {
      this.undoStack = [];
      this.redoStack = [];
    }
  }

  /** The pattern object (ids stripped, sorted). */
  pattern() {
    return {
      schema_version: SCHEMA_VERSION,
      kind: 'notes',
      length: this.length,
      notes: [...this.notes].sort(compareNotes).map(({ key, start, length, velocity }) => ({ key, start, length, velocity })),
    };
  }

  byId(id) {
    return this.notes.find((n) => n.id === id) ?? null;
  }

  snapshot() {
    return { length: this.length, notes: this.notes.map((n) => ({ ...n })), selected: [...this.selected] };
  }

  restore(s) {
    this.length = s.length;
    this.notes = s.notes.map((n) => ({ ...n }));
    this.selected = new Set(s.selected.filter((id) => this.notes.some((n) => n.id === id)));
  }

  /** Records the state before an edit. */
  remember(before = this.snapshot()) {
    this.undoStack.push(before);
    if (this.undoStack.length > 200) this.undoStack.shift();
    this.redoStack = [];
  }

  get canUndo() {
    return this.undoStack.length > 0;
  }

  get canRedo() {
    return this.redoStack.length > 0;
  }

  undo() {
    if (!this.canUndo) return false;
    this.redoStack.push(this.snapshot());
    this.restore(this.undoStack.pop());
    return true;
  }

  redo() {
    if (!this.canRedo) return false;
    this.undoStack.push(this.snapshot());
    this.restore(this.redoStack.pop());
    return true;
  }

  /** Applies `notes` as an edit of `edited` ids (overlaps resolved in their favour). */
  commitNotes(notes, edited, before) {
    const next = resolveOverlaps(notes, edited);
    if (sameNotes(next, this.notes)) return false;
    this.remember(before);
    this.notes = next;
    this.selected = new Set([...this.selected].filter((id) => next.some((n) => n.id === id)));
    return true;
  }

  /** Adds a note (start snapped down to `grid`) of the last length; selects it. Returns its id. */
  add(beat, key, { grid = 0.25, velocity = 0.8, length = this.lastLength ?? grid } = {}) {
    const start = Math.max(0, snapFloor(beat, grid));
    const note = { id: this.nextId++, key: clamp(Math.round(key), MIN_KEY, MAX_KEY), start, length: Math.max(grid, length), velocity };
    const before = this.snapshot();
    this.selected = new Set([note.id]);
    this.commitNotes([...this.notes, note], new Set([note.id]), before);
    return note.id;
  }

  /** Removes the selected notes (or `ids`). */
  remove(ids = [...this.selected]) {
    const gone = new Set(ids);
    if (!this.notes.some((n) => gone.has(n.id))) return false;
    this.remember();
    this.notes = this.notes.filter((n) => !gone.has(n.id));
    this.selected = new Set([...this.selected].filter((id) => !gone.has(id)));
    return true;
  }

  select(ids, { add = false, toggle = false } = {}) {
    if (!add && !toggle) this.selected = new Set();
    for (const id of ids) {
      if (toggle && this.selected.has(id)) this.selected.delete(id);
      else this.selected.add(id);
    }
  }

  selectAll() {
    this.selected = new Set(this.notes.map((n) => n.id));
  }

  /**
   * Moves the selection by `dBeats` and `dKeys`, kept on the grid's
   * multiples only as far as the caller snapped `dBeats`, clamped so no note
   * leaves [0, ∞) × [0, 127].
   */
  moveSelection(dBeats, dKeys) {
    const moving = this.notes.filter((n) => this.selected.has(n.id));
    if (!moving.length) return false;
    const [db, dk] = limitMove(moving, dBeats, dKeys);
    if (db === 0 && dk === 0) return false;
    const before = this.snapshot();
    const next = this.notes.map((n) => (this.selected.has(n.id) ? { ...n, start: clean(n.start + db), key: n.key + dk } : n));
    return this.commitNotes(next, new Set(this.selected), before);
  }

  /** Sets the selected notes' (or `ids`') velocity. */
  setVelocity(velocity, ids = [...this.selected]) {
    const set = new Set(ids);
    const v = clamp(velocity, 0, 1);
    if (!this.notes.some((n) => set.has(n.id) && n.velocity !== v)) return false;
    this.remember();
    this.notes = this.notes.map((n) => (set.has(n.id) ? { ...n, velocity: v } : n));
    return true;
  }

  /** The loop length in beats (> 0), snapped to `grid`. */
  setLength(beats, grid = 1) {
    const length = Math.max(grid, snap(beats, grid));
    if (length === this.length) return false;
    this.remember();
    this.length = length;
    return true;
  }

  /** Duplicates the selection right after itself; the copies become the selection. */
  duplicate(grid = 0.25) {
    const sel = this.notes.filter((n) => this.selected.has(n.id));
    if (!sel.length) return false;
    const start = Math.min(...sel.map((n) => n.start));
    const end = Math.max(...sel.map((n) => n.start + n.length));
    const span = Math.max(grid, snap(end - start, grid) || grid);
    const copies = sel.map((n) => ({ ...n, id: this.nextId++, start: clean(n.start + span) }));
    const before = this.snapshot();
    this.selected = new Set(copies.map((n) => n.id));
    return this.commitNotes([...this.notes, ...copies], new Set(this.selected), before);
  }

  // ------------------------------------------------ drags (one undo step each)

  /**
   * Starts a drag of `id` (`move` or `resize`) at a beat and key; with `copy`
   * a move drags copies. The selection is what moves; `id` joins it.
   */
  beginDrag(kind, id, beat, key, { copy = false } = {}) {
    if (!this.selected.has(id)) this.selected = new Set([id]);
    const before = this.snapshot();
    let copyOf = null;
    if (copy && kind === 'move') {
      copyOf = new Map();
      const copies = this.notes.filter((n) => this.selected.has(n.id)).map((n) => {
        const copy = { ...n, id: this.nextId++ };
        copyOf.set(n.id, copy.id);
        return copy;
      });
      // The originals stay where they were; the copies are dragged.
      this.notes = [...this.notes, ...copies];
      this.selected = new Set(copies.map((n) => n.id));
      id = copyOf.get(id) ?? id;
    }
    const anchor = this.byId(id) ?? this.notes.find((n) => this.selected.has(n.id));
    this.drag = {
      kind,
      before,
      base: this.notes.map((n) => ({ ...n })),
      anchor: { ...anchor },
      beat,
      key,
      copies: copyOf,
    };
  }

  /** The drag moved to a beat and key: the notes are shown moved (not yet an undo step). */
  dragTo(beat, key, { grid = 0.25, free = false } = {}) {
    const d = this.drag;
    if (!d) return;
    const moving = d.base.filter((n) => this.selected.has(n.id));
    if (d.kind === 'move') {
      // The anchor's start lands on the grid; the others keep their offsets.
      const raw = d.anchor.start + (beat - d.beat);
      const target = free ? Math.max(0, raw) : Math.max(0, snap(raw, grid));
      const [db, dk] = limitMove(moving, target - d.anchor.start, Math.round(key - d.key));
      this.notes = d.base.map((n) => (this.selected.has(n.id) ? { ...n, start: clean(n.start + db), key: n.key + dk } : n));
    } else if (d.kind === 'resize') {
      const rawEnd = d.anchor.start + d.anchor.length + (beat - d.beat);
      const end = free ? rawEnd : snap(rawEnd, grid);
      const delta = end - (d.anchor.start + d.anchor.length);
      const min = free ? 1 / 64 : grid;
      this.notes = d.base.map((n) => (this.selected.has(n.id) ? { ...n, length: clean(Math.max(min, n.length + delta)) } : n));
    }
  }

  /** The drag ended: one undo step when it changed something; a resize sets the last length. */
  endDrag() {
    const d = this.drag;
    if (!d) return false;
    this.drag = null;
    if (d.copies) {
      // Copies dropped where their originals are (an Alt-click): nothing happened.
      const unmoved = [...d.copies].every(([from, to]) => {
        const a = d.before.notes.find((n) => n.id === from);
        const b = this.byId(to);
        return a && b && a.start === b.start && a.key === b.key;
      });
      if (unmoved) {
        this.restore(d.before);
        return false;
      }
    }
    const edited = this.notes.filter((n) => this.selected.has(n.id));
    if (d.kind === 'resize') {
      const a = this.byId(d.anchor.id);
      if (a) this.lastLength = a.length;
    } else if (d.kind === 'move' && edited.length === 1) {
      this.lastLength = edited[0].length;
    }
    const next = resolveOverlaps(this.notes, new Set(this.selected));
    const original = d.before.notes;
    if (sameNotes(next, original)) {
      this.restore(d.before);
      return false;
    }
    this.undoStack.push(d.before);
    this.redoStack = [];
    this.notes = next;
    this.selected = new Set([...this.selected].filter((id) => next.some((n) => n.id === id)));
    return true;
  }

  /** The drag was cancelled (Escape): back to before it. */
  cancelDrag() {
    if (!this.drag) return;
    this.restore(this.drag.before);
    this.drag = null;
  }

  // ------------------------------------------------ velocity drags

  beginVelocity(id) {
    if (!this.selected.has(id)) this.selected = new Set([id]);
    this.drag = { kind: 'velocity', before: this.snapshot(), base: this.notes.map((n) => ({ ...n })), anchor: { ...this.byId(id) } };
  }

  /** Every selected note takes the dragged bar's change, kept in [0, 1]. */
  velocityTo(v) {
    const d = this.drag;
    if (!d || d.kind !== 'velocity') return;
    const delta = v - d.anchor.velocity;
    this.notes = d.base.map((n) => (this.selected.has(n.id) ? { ...n, velocity: Math.round(clamp(n.velocity + delta, 0, 1) * 127) / 127 } : n));
  }
}

/** The move clamped so every note stays at start ≥ 0 and key in 0..127. */
export function limitMove(notes, dBeats, dKeys) {
  if (!notes.length) return [0, 0];
  const minStart = Math.min(...notes.map((n) => n.start));
  const minKey = Math.min(...notes.map((n) => n.key));
  const maxKey = Math.max(...notes.map((n) => n.key));
  const db = Math.max(dBeats, -minStart);
  const dk = clamp(dKeys, MIN_KEY - minKey, MAX_KEY - maxKey);
  return [clean(db), dk];
}

function sameNotes(a, b) {
  if (a.length !== b.length) return false;
  const sa = [...a].sort((x, y) => x.id - y.id);
  const sb = [...b].sort((x, y) => x.id - y.id);
  return sa.every((n, i) => n.id === sb[i].id && n.key === sb[i].key && n.start === sb[i].start && n.length === sb[i].length && n.velocity === sb[i].velocity);
}

/**
 * What a key does to the model, or null when it is not the piano roll's:
 * the `piano_roll` scope's commands (ui/lib/keys.default.js; by default
 * arrows move the selection up/down a semitone, Shift an octave, left/right
 * one snap; Delete removes, ⌘A selects all, ⌘D duplicates, ⌘Z / ⇧⌘Z undo
 * and redo, Escape cancels a drag or clears the selection). `command` is
 * the command's id when the caller looked it up already.
 */
export function rollKey(model, event, grid, command = keyCommand('piano_roll', event)?.id) {
  switch (command) {
    case 'piano_roll.undo':
      return model.undo() || 'handled';
    case 'piano_roll.redo':
      return model.redo() || 'handled';
    case 'piano_roll.select_all':
      model.selectAll();
      return 'selection';
    case 'piano_roll.duplicate':
      return model.duplicate(grid) || 'handled';
    case 'piano_roll.delete':
      return model.remove() || 'handled';
    case 'piano_roll.deselect':
      if (model.drag) model.cancelDrag();
      else model.selected = new Set();
      return 'selection';
    default:
      break;
  }
  const moves = {
    'piano_roll.move_up': [0, 1],
    'piano_roll.move_down': [0, -1],
    'piano_roll.octave_up': [0, 12],
    'piano_roll.octave_down': [0, -12],
    'piano_roll.move_left': [-1, 0],
    'piano_roll.move_right': [1, 0],
  };
  if (moves[command]) {
    const [b, n] = moves[command];
    return model.moveSelection(b * grid, n) || 'handled';
  }
  return null;
}

// <signal-step-grid>'s model (ADR 0028 §2, §7): a `steps` pattern's lanes
// as cells of rest, hit and accent; the click cycle, painting, the accent
// row, the lanes' levels, the step count and length, the layout's geometry
// and hit-testing, and a local undo history. No DOM:
// ui/controls/step-grid.js draws it.

import { ACCENT, HIT, REST, SCHEMA_VERSION, emptyPattern, normalizePattern } from './pattern-text.js';

export const STEP_COUNTS = Object.freeze([8, 12, 16, 24, 32, 48, 64]);
/** Step lengths: label and beats. */
export const STEP_LENGTHS = Object.freeze([
  { label: '1/8', beats: 1 / 2 },
  { label: '1/16', beats: 1 / 4 },
  { label: '1/32', beats: 1 / 8 },
  { label: '1/8T', beats: 1 / 3 },
  { label: '1/16T', beats: 1 / 6 },
]);

/** A click's next state: rest → hit → accent → rest. */
export function cycle(state) {
  return state === REST ? HIT : state === HIT ? ACCENT : REST;
}

/** The colour group of a step (0..3), in fours, as on the classic machines. */
export const groupOf = (step) => Math.floor(step / 4) % 4;

const replaceAt = (s, i, ch) => s.slice(0, i) + ch + s.slice(i + 1);

/** A hit string resized to `steps`: longer repeats it from its start, shorter cuts it. */
export function resizeHits(hits, steps) {
  if (steps <= hits.length) return hits.slice(0, steps);
  if (hits.length === 0) return REST.repeat(steps);
  let out = hits;
  while (out.length < steps) out += hits;
  return out.slice(0, steps);
}

/** Whether a step has an accent in any lane (the accent row's lamp). */
export function accentAt(lanes, step) {
  return lanes.some((lane) => lane.hits[step] === ACCENT);
}

export class StepGridModel {
  constructor(pattern) {
    this.undoStack = [];
    this.redoStack = [];
    this.paint = null;
    this.load(pattern ?? emptyPattern('steps'));
  }

  load(pattern) {
    const p = normalizePattern(pattern);
    this.steps = p.steps;
    this.step = p.step;
    this.lanes = p.lanes;
    this.undoStack = [];
    this.redoStack = [];
    this.paint = null;
    this.levelBefore = null;
  }

  pattern() {
    return normalizePattern({ schema_version: SCHEMA_VERSION, kind: 'steps', steps: this.steps, step: this.step, lanes: this.lanes });
  }

  snapshot() {
    return JSON.stringify({ steps: this.steps, step: this.step, lanes: this.lanes });
  }

  restore(s) {
    Object.assign(this, JSON.parse(s));
  }

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
    this.commitLevel();
    if (!this.canUndo) return false;
    this.redoStack.push(this.snapshot());
    this.restore(this.undoStack.pop());
    return true;
  }

  redo() {
    this.commitLevel();
    if (!this.canRedo) return false;
    this.undoStack.push(this.snapshot());
    this.restore(this.redoStack.pop());
    return true;
  }

  cell(lane, step) {
    return this.lanes[lane]?.hits[step] ?? REST;
  }

  /** Sets one cell without history; whether it changed. */
  put(lane, step, state) {
    const l = this.lanes[lane];
    if (!l || step < 0 || step >= this.steps || l.hits[step] === state) return false;
    this.lanes = this.lanes.map((x, i) => (i === lane ? { ...x, hits: replaceAt(x.hits, step, state) } : x));
    return true;
  }

  /**
   * A press on a cell: it cycles, and a drag from here paints the state it
   * took onto every cell the pointer enters (one undo step for the stroke).
   */
  press(lane, step) {
    this.commitLevel();
    const before = this.snapshot();
    const state = cycle(this.cell(lane, step));
    this.put(lane, step, state);
    this.paint = { before, state, lane: null };
    return state;
  }

  /** The pointer entered a cell during a stroke. */
  enter(lane, step) {
    if (!this.paint) return false;
    return this.put(lane, step, this.paint.state);
  }

  /** The stroke ended: one undo step when it changed anything. */
  release() {
    const p = this.paint;
    this.paint = null;
    if (!p || p.before === this.snapshot()) return false;
    this.remember(p.before);
    return true;
  }

  /**
   * The accent row's press on a step: when any lane accents it, every
   * accent there becomes a hit; else every hit there becomes an accent.
   */
  toggleAccent(step) {
    this.commitLevel();
    const before = this.snapshot();
    const on = accentAt(this.lanes, step);
    let changed = false;
    this.lanes.forEach((lane, i) => {
      const c = lane.hits[step];
      if (on && c === ACCENT) changed = this.put(i, step, HIT) || changed;
      if (!on && c === HIT) changed = this.put(i, step, ACCENT) || changed;
    });
    if (changed) this.remember(before);
    return changed;
  }

  /** A lane's level (0..1), the default 1 when the file gives none. */
  level(lane) {
    return this.lanes[lane]?.level ?? 1;
  }

  /** Sets a lane's level without history (a knob drag); `commitLevel` closes it. */
  setLevel(lane, level) {
    const l = this.lanes[lane];
    if (!l) return false;
    const v = Math.round(Math.min(1, Math.max(0, level)) * 100) / 100;
    if (l.level === v) return false;
    if (!this.levelBefore) this.levelBefore = this.snapshot();
    this.lanes = this.lanes.map((x, i) => (i === lane ? { ...x, level: v } : x));
    return true;
  }

  /** The level drag ended: one undo step. */
  commitLevel() {
    const before = this.levelBefore;
    this.levelBefore = null;
    if (!before || before === this.snapshot()) return false;
    this.remember(before);
    return true;
  }

  /** The level back to the file's default (the key removed). */
  resetLevel(lane) {
    const l = this.lanes[lane];
    if (!l || l.level === undefined) return false;
    this.remember();
    this.lanes = this.lanes.map((x, i) => {
      if (i !== lane) return x;
      const { level, ...rest } = x;
      void level;
      return rest;
    });
    return true;
  }

  /** The step count; lanes repeat or cut (resizeHits). */
  setSteps(steps) {
    const n = Math.max(1, Math.min(256, Math.round(steps)));
    if (n === this.steps) return false;
    this.remember();
    this.steps = n;
    this.lanes = this.lanes.map((l) => ({ ...l, hits: resizeHits(l.hits, n) }));
    return true;
  }

  setStepLength(beats) {
    if (!(beats > 0) || beats === this.step) return false;
    this.remember();
    this.step = beats;
    return true;
  }

  /** Clears a lane (all rests). */
  clearLane(lane) {
    const l = this.lanes[lane];
    if (!l || !/[xX]/.test(l.hits)) return false;
    this.remember();
    this.lanes = this.lanes.map((x, i) => (i === lane ? { ...x, hits: REST.repeat(this.steps) } : x));
    return true;
  }
}

// ---------------------------------------------------------------- layout

/**
 * The grid's geometry in its own units (the SVG's viewBox): a header with
 * the running lights, one row per lane, the accent row at the bottom; the
 * lane's name and level knob on the left. Steps come in groups of four with
 * a gap between groups.
 */
export function gridLayout(steps, lanes, { cell = 34, gap = 6, groupGap = 10, left = 150, top = 34, row = 40, accentRow = 40, pad = 14 } = {}) {
  const groups = Math.ceil(steps / 4);
  const width = left + steps * cell + (steps - 1) * gap + (groups - 1) * (groupGap - gap) + pad;
  const height = top + lanes * row + accentRow + pad;
  return { steps, lanes, cell, gap, groupGap, left, top, row, accentRow, pad, width, height };
}

/** The x of a step's cell. */
export function stepX(layout, step) {
  return layout.left + step * (layout.cell + layout.gap) + Math.floor(step / 4) * (layout.groupGap - layout.gap);
}

/** The y of a lane's row (lane = layout.lanes is the accent row). */
export function laneY(layout, lane) {
  return layout.top + lane * layout.row;
}

/**
 * What is at (x, y): `{area: 'cell', lane, step}` (lane = lanes for the
 * accent row), `{area: 'knob', lane}`, `{area: 'name', lane}`, or null.
 * A point in a gap between cells takes the nearer cell, so a paint stroke
 * never skips one.
 */
export function hitGrid(layout, x, y) {
  if (y < layout.top) return null;
  const lane = Math.floor((y - layout.top) / layout.row);
  if (lane < 0 || lane > layout.lanes) return null;
  if (lane === layout.lanes && y > laneY(layout, lane) + layout.accentRow) return null;
  if (x < layout.left) {
    if (lane >= layout.lanes) return null;
    return { area: x >= layout.left - 46 ? 'knob' : 'name', lane };
  }
  let best = null;
  let bestD = Infinity;
  for (let s = 0; s < layout.steps; s++) {
    const x0 = stepX(layout, s);
    const d = x < x0 ? x0 - x : x > x0 + layout.cell ? x - x0 - layout.cell : 0;
    if (d < bestD) {
      bestD = d;
      best = s;
    }
  }
  if (bestD > layout.groupGap) return null;
  return { area: 'cell', lane, step: best };
}

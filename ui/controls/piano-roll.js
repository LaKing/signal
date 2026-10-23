// <signal-piano-roll>: edits a `notes` pattern (ADR 0028 §2, §7) on a
// canvas: a keyboard strip, a ruler with the loop's end, a bar/beat/snap
// grid, notes coloured by velocity, a velocity lane, and a playhead from
// the `position` property (beats, from a sequencer's `position` output
// through `position-bind`, ui/lib/playhead.js). The geometry, snap, hit-testing, edits
// and undo history are ui/lib/piano-roll-model.js (no DOM); the file text
// is ui/lib/pattern-text.js.
//
// Interface: `.value` (the pattern object), `.text` (its file text), a
// `change` event (bubbling, composed; `detail` the new pattern) after every
// edit, never after setting `.value` or `.text`. With `pattern="<name>"` and
// a connection offering `readPattern`/`writePattern` (ui/lib/pattern-sync.js,
// ADR 0028 §7), it loads that pattern on connect and writes every edit
// (debounced, on the last base).
//
// Pointer: click on empty grid adds a note of the last length (drag from
// empty grid: a rubber band, Shift adds to the selection); drag a note to
// move it (snapped, both axes; Alt drags copies), its right edge to resize;
// Shift/⌘-click toggles a note in the selection; double-click deletes; the
// velocity lane's bars drag the selection's velocities; the loop's end
// drags on the ruler. Wheel scrolls (Shift: horizontal); ⌘/Ctrl+wheel (and a
// pinch) zooms time about the pointer, Alt+wheel zooms the keys. Keys:
// arrows move the selection (↑↓ a semitone, Shift an octave, ←→ one snap),
// Delete removes, ⌘A, ⌘D duplicates, ⌘Z / ⇧⌘Z, Escape, = and - zoom; Enter
// adds a note at the cursor (the last place pressed) and moves past it.
// A double-click on empty grid adds nothing (the first click's note is
// taken back). While a `pattern` is first read, nothing can be edited.

import { canSyncPatterns, PatternSync, shortNotice } from '../lib/pattern-sync.js';
import { playheadAddress, playheadStream } from '../lib/playhead.js';
import { PatternError, parsePattern, serializePattern, validatePattern } from '../lib/pattern-text.js';
import {
  MAX_KEY,
  PianoRollModel,
  SNAPS,
  ZOOM,
  beatToX,
  centreKey,
  clamp,
  clampScroll,
  contentHeight,
  gridLines,
  hitNote,
  hitVelocity,
  isBlack,
  keyName,
  keyToY,
  makeView,
  notesInBox,
  rollKey,
  snap,
  snapFloor,
  velocityAt,
  xToBeat,
  yToKey,
  zoomX,
  zoomY,
} from '../lib/piano-roll-model.js';
import { parseNumber } from '../lib/entry.js';
import { SignalElement, registerWheel, takesWheel, verticalDelta } from './base.js';
import { ariaShortcuts, keyCommand, wheelCommand } from '../lib/keys.js';

const KEYS_W = 64;
const RULER_H = 26;
const VEL_H = 84;
const DRAG_PX = 3;
const FONT = '-apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif';

/** The canvas colours; each can be set as a custom property on the control. */
const COLORS = {
  '--signal-roll-bg': '#1b1e24',
  '--signal-roll-row': '#21252d',
  '--signal-roll-row-black': '#1a1d23',
  '--signal-roll-line': '#262a33',
  '--signal-roll-octave': '#3a404c',
  '--signal-roll-beat': '#30353f',
  '--signal-roll-bar': '#4a5160',
  '--signal-roll-sub': '#252930',
  '--signal-roll-outside': 'rgba(0, 0, 0, 0.32)',
  '--signal-roll-note-hue': '196',
  '--signal-roll-selected': '#ffcf6b',
  '--signal-roll-playhead': '#ffb74d',
  '--signal-roll-loop': '#4fc3f7',
};

export class SignalPianoRoll extends SignalElement {
  static role = 'application';
  static observedAttributes = ['pattern', 'label', 'snap', 'beats-per-bar', 'position', 'position-bind', 'wheel'];
  static resubscribe = ['pattern', 'position-bind'];
  static css = `
:host { display: block; height: var(--signal-piano-roll-height, 460px); min-width: 320px; background: var(--signal-panel, #1f232b); border-radius: 8px; overflow: hidden; }
.roll { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 10px; padding: 6px 10px; border-bottom: 1px solid #111317; background: linear-gradient(#272b33, #22262d); flex: none; flex-wrap: wrap; }
.toolbar .title { font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--signal-fg, #d6dbe4); margin-right: 4px; }
.toolbar label { display: inline-flex; align-items: center; gap: 5px; color: var(--signal-muted, #8a93a3); }
.toolbar select, .toolbar input { font: inherit; color: var(--signal-fg, #d6dbe4); background: #15171c; border: 1px solid #353b46; border-radius: 4px; padding: 2px 4px; height: 22px; box-sizing: border-box; }
.toolbar input { width: 4.5em; font-variant-numeric: tabular-nums; }
.group { display: inline-flex; border: 1px solid #353b46; border-radius: 4px; overflow: hidden; }
.group button { font: inherit; min-width: 24px; height: 20px; padding: 0 6px; color: var(--signal-fg, #d6dbe4); background: #2b3039; border: 0; border-left: 1px solid #353b46; cursor: pointer; }
.group button:first-child { border-left: 0; }
.group button:hover { background: #343a45; }
.group button:disabled { color: #5a6170; cursor: default; background: #2b3039; }
.group button[aria-pressed="true"] { background: #3b4656; color: #fff; }
.spacer { flex: 1; }
.status { color: var(--signal-muted, #8a93a3); font-variant-numeric: tabular-nums; white-space: nowrap; }
.notice { color: var(--signal-unsaved, #ffb74d); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 40ch; }
.notice-full { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
.body { flex: 1; min-height: 0; display: grid; grid-template-columns: ${KEYS_W}px 1fr; grid-template-rows: ${RULER_H}px 1fr ${VEL_H}px; }
:host([no-velocity]) .body { grid-template-rows: ${RULER_H}px 1fr 0; }
.corner { background: #23272e; border-right: 1px solid #111317; border-bottom: 1px solid #111317; display: flex; align-items: center; justify-content: center; color: #6c7484; font-size: 10px; }
.cell { position: relative; min-width: 0; min-height: 0; overflow: hidden; }
canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.vel-label { background: #1d2026; border-right: 1px solid #111317; border-top: 1px solid #111317; color: #6c7484; font-size: 10px; letter-spacing: 0.08em; display: flex; align-items: flex-start; justify-content: center; padding-top: 6px; box-sizing: border-box; }
.vel { border-top: 1px solid #111317; }
.bar { position: absolute; background: rgba(255,255,255,0.18); border-radius: 3px; cursor: default; }
.bar:hover, .bar.active { background: rgba(255,255,255,0.32); }
.hbar { bottom: 2px; height: 6px; }
.vbar { right: 2px; width: 6px; }
`;

  constructor() {
    super();
    this.model = new PianoRollModel();
    this.view = makeView();
    this.positionBeats = null;
    this.gesture = null;
    this.sync = null;
    this.colors = { ...COLORS };
    this.placed = false;
    this.frame = 0;
  }

  // ------------------------------------------------ the interface

  get value() {
    return this.model.pattern();
  }

  set value(pattern) {
    checked(pattern, 'notes');
    this.model.load(pattern);
    // A new pattern is shown from its start, its notes in view.
    this.placed = false;
    if (this.view.width > 0 && this.canvases) this.place();
    this.redraw();
  }

  get text() {
    return serializePattern(this.value);
  }

  set text(text) {
    this.value = parsePattern(text);
  }

  /** The playhead in beats (shown modulo the loop), or null for none. */
  get position() {
    return this.positionBeats;
  }

  set position(beats) {
    const v = beats === null || beats === undefined || beats === '' ? null : Number(beats);
    this.positionBeats = Number.isFinite(v) ? v : null;
    this.requestDraw();
  }

  get grid() {
    const label = this.getAttribute('snap') ?? this.snapLabel ?? '1/16';
    return (SNAPS.find((s) => s.label === label) ?? SNAPS[2]).beats;
  }

  get beatsPerBar() {
    const n = Number(this.getAttribute('beats-per-bar'));
    return Number.isInteger(n) && n > 0 ? n : 4;
  }

  attributeChangedCallback(name, previous, value) {
    if (name === 'position') this.position = value;
    super.attributeChangedCallback(name, previous, value);
  }

  // ------------------------------------------------ DOM

  template() {
    const snaps = SNAPS.map((s) => `<option value="${s.label}">${s.label}</option>`).join('');
    return `<div class="roll" part="roll">
  <div class="toolbar">
    <span class="title"></span>
    <label>Snap <select class="snap" aria-label="Snap">${snaps}</select></label>
    <label>Loop <input class="length" type="text" inputmode="decimal" spellcheck="false" autocomplete="off" aria-label="Loop length in beats" title="Loop length in beats; ↑↓ step by one"> beats</label>
    <span class="group" role="group" aria-label="Zoom time"><button class="zx-out" title="Zoom out (time)">−</button><button class="zx-in" title="Zoom in (time)">+</button></span>
    <span class="group" role="group" aria-label="Zoom keys"><button class="zy-out" title="Smaller keys">▾</button><button class="zy-in" title="Taller keys">▴</button></span>
    <span class="group"><button class="fit" title="Fit the loop">Fit</button></span>
    <span class="group"><button class="undo" title="Undo (⌘Z)">↶</button><button class="redo" title="Redo (⇧⌘Z)">↷</button></span>
    <span class="group"><button class="velocity-toggle" aria-pressed="true" title="Velocity lane">Vel</button></span>
    <span class="spacer"></span>
    <span class="notice" aria-hidden="true"></span><span class="notice-full" role="status"></span>
    <span class="status"></span>
  </div>
  <div class="body">
    <div class="corner"></div>
    <div class="cell"><canvas class="ruler" part="ruler"></canvas></div>
    <div class="cell"><canvas class="keys" part="keys"></canvas></div>
    <div class="cell gridcell"><canvas class="grid" part="grid"></canvas><div class="bar hbar"></div><div class="bar vbar"></div></div>
    <div class="vel-label">VEL</div>
    <div class="cell vel"><canvas class="velocity" part="velocity"></canvas></div>
  </div>
</div>`;
  }

  setup() {
    const $ = (s) => this.$(s);
    this.canvases = { ruler: $('.ruler'), keys: $('.keys'), grid: $('.grid'), velocity: $('.velocity') };
    const snapSelect = $('.snap');
    snapSelect.value = this.getAttribute('snap') ?? '1/16';
    snapSelect.addEventListener('change', () => {
      this.snapLabel = snapSelect.value;
      if (this.hasAttribute('snap')) this.setAttribute('snap', snapSelect.value);
      this.redraw();
    });
    const length = $('.length');
    // Typed as text, not `type="number"`: WebKit's number field drops a
    // decimal comma; `parseNumber` takes `8,5` and `8.5` alike (snapped to
    // whole beats). ↑↓ step by one beat, as the number field did.
    const setLoop = (beats) => {
      if (this.locked) this.redraw();
      else if (beats !== null && beats > 0 && this.model.setLength(beats, 1)) this.changed();
      else this.redraw();
    };
    length.addEventListener('change', () => {
      setLoop(parseNumber(length.value));
      // The loop as it is now (snapped, or kept when the text was no number).
      length.value = String(this.model.length);
    });
    length.addEventListener('keydown', (event) => {
      const step = { ArrowUp: 1, ArrowDown: -1 }[event.key];
      if (step === undefined) return;
      event.preventDefault();
      setLoop(Math.max(1, (parseNumber(length.value) ?? this.model.length) + step));
      length.value = String(this.model.length);
    });
    const zoom = (axis, factor) => {
      this.view = axis === 'x' ? zoomX(this.view, factor, this.view.width / 2, this.totalBeats()) : zoomY(this.view, factor, this.view.height / 2, this.totalBeats());
      this.redraw();
    };
    $('.zx-out').addEventListener('click', () => zoom('x', 1 / 1.25));
    $('.zx-in').addEventListener('click', () => zoom('x', 1.25));
    $('.zy-out').addEventListener('click', () => zoom('y', 1 / 1.2));
    $('.zy-in').addEventListener('click', () => zoom('y', 1.2));
    $('.fit').addEventListener('click', () => this.fit());
    $('.undo').addEventListener('click', () => !this.locked && this.model.undo() && this.changed());
    $('.redo').addEventListener('click', () => !this.locked && this.model.redo() && this.changed());
    $('.velocity-toggle').addEventListener('click', () => {
      this.toggleAttribute('no-velocity');
      this.redraw();
    });
    for (const button of this.shadowRoot.querySelectorAll('button')) {
      button.addEventListener('pointerdown', (event) => event.stopPropagation());
    }
    this.gridEvents();
    this.velocityEvents();
    this.rulerEvents();
    this.scrollbarEvents();
    for (const [name, canvas] of Object.entries(this.canvases)) {
      canvas.addEventListener('wheel', (event) => this.onWheel(event, name), { passive: false });
    }
    registerWheel(this);
    this.resizer = new ResizeObserver(() => this.resized());
    this.resizer.observe($('.gridcell'));
    this.resizer.observe($('.velocity'));
  }

  connectedCallback() {
    super.connectedCallback();
    this.readColors();
    this.resized();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  readColors() {
    if (typeof getComputedStyle !== 'function') return;
    const style = getComputedStyle(this);
    for (const name of Object.keys(COLORS)) {
      const v = style.getPropertyValue(name).trim();
      this.colors[name] = v || COLORS[name];
    }
  }

  resized() {
    const box = this.canvases?.grid.getBoundingClientRect();
    if (!box || box.width === 0) return;
    this.view = clampScroll({ ...this.view, width: box.width, height: box.height }, this.totalBeats());
    if (!this.placed) this.place();
    this.redraw();
  }

  /** The first view of a pattern: the loop fits, the notes (or C4) centred. */
  place() {
    this.placed = true;
    const notes = this.model.notes;
    const keys = notes.map((n) => n.key);
    const lo = keys.length ? Math.min(...keys) : 54;
    const hi = keys.length ? Math.max(...keys) : 66;
    const mid = (lo + hi) / 2;
    // Rows tall enough to read, the notes' range (and a little room) in view when it can be.
    this.view = { ...this.view, rowPx: clamp(Math.floor(this.view.height / (hi - lo + 5)), 12, 18) };
    this.fit(false);
    this.view = centreKey(this.view, Math.round(mid), this.totalBeats());
  }

  fit(draw = true) {
    const beats = Math.max(this.model.length, 1);
    const beatPx = clamp((this.view.width - 8) / beats, ZOOM.minBeat, ZOOM.maxBeat);
    this.view = clampScroll({ ...this.view, beatPx, scrollX: 0 }, this.totalBeats());
    if (draw) this.redraw();
  }

  /** How far the content reaches: the loop, the last note, and a bar more, or the viewport. */
  totalBeats() {
    const last = this.model.notes.reduce((m, n) => Math.max(m, n.start + n.length), 0);
    const bar = this.beatsPerBar;
    const beats = Math.ceil((Math.max(this.model.length, last) + bar) / bar) * bar;
    return Math.max(beats, this.view.width / this.view.beatPx);
  }

  /** Viewport coordinates of a pointer over `canvas`. */
  local(event, canvas) {
    const box = canvas.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top };
  }

  /** Captures the pointer on the part pressed, so its own listeners follow the drag; focuses the control. */
  capture(event) {
    this.focus({ preventScroll: true });
    try {
      event.currentTarget?.setPointerCapture(event.pointerId);
    } catch {
      // A synthetic or already released pointer: the drag still works.
    }
  }

  // ------------------------------------------------ editing

  /** An edit happened: redraw, tell the page, write through the adapter. */
  changed() {
    this.redraw();
    const pattern = this.value;
    let text = null;
    try {
      text = serializePattern(pattern);
    } catch (error) {
      // Never thrown at the page: the edit is shown, said to be invalid and not written.
      this.notice = `not written: ${error.message}`;
    }
    if (text !== null && !this.sync && this.notice?.startsWith('not written')) this.notice = null;
    this.dispatchEvent(new CustomEvent('change', { detail: pattern, bubbles: true, composed: true }));
    if (text !== null) this.sync?.edited(text, pattern);
  }

  /** Waiting for the pattern's first read: no edits until it is shown (ui/lib/pattern-sync.js). */
  get locked() {
    return Boolean(this.sync && !this.sync.loaded);
  }

  gridEvents() {
    const canvas = this.canvases.grid;
    canvas.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || this.locked) return;
      this.capture(event);
      const { x, y } = this.local(event, canvas);
      const v = this.view;
      const hit = hitNote(this.model.notes, x, y, v);
      this.cursor = { beat: snapFloor(xToBeat(x, v), this.grid), key: yToKey(y, v) };
      const toggle = event.shiftKey || event.metaKey || event.ctrlKey;
      if (hit) {
        if (toggle) {
          this.model.select([hit.id], { toggle: true });
          if (!this.model.selected.has(hit.id)) {
            this.gesture = null;
            this.redraw();
            return;
          }
        } else if (!this.model.selected.has(hit.id)) this.model.select([hit.id]);
        this.model.beginDrag(hit.part === 'end' ? 'resize' : 'move', hit.id, xToBeat(x, v), yToKey(y, v), { copy: event.altKey && hit.part !== 'end' });
        this.gesture = { kind: 'drag' };
      } else {
        this.gesture = { kind: 'pending', x, y, add: toggle, base: new Set(toggle ? this.model.selected : []) };
      }
      this.redraw();
    });
    canvas.addEventListener('pointermove', (event) => {
      const { x, y } = this.local(event, canvas);
      const g = this.gesture;
      if (!g) {
        this.hover(x, y);
        return;
      }
      this.autoscroll(x, y);
      const v = this.view;
      if (g.kind === 'drag') {
        this.model.dragTo(xToBeat(x, v), yToKey(y, v), { grid: this.grid, free: event.metaKey || event.ctrlKey });
      } else if (g.kind === 'pending' && Math.hypot(x - g.x, y - g.y) > DRAG_PX) {
        g.kind = 'band';
        g.b0 = xToBeat(g.x, v);
        g.k0 = yToKey(g.y, v);
      }
      if (g.kind === 'band') {
        g.b1 = xToBeat(x, v);
        g.k1 = yToKey(y, v);
        const inside = notesInBox(this.model.notes, g.b0, g.b1, g.k0, g.k1);
        this.model.selected = new Set([...g.base, ...inside]);
      }
      this.redraw();
    });
    const end = (event) => {
      const g = this.gesture;
      this.gesture = null;
      if (!g) return;
      if (g.kind === 'drag') {
        if (this.model.endDrag()) this.changed();
        else this.redraw();
      } else if (g.kind === 'pending' && event?.type === 'pointerup' && !g.add) {
        const v = this.view;
        const id = this.model.add(xToBeat(g.x, v), yToKey(g.y, v), { grid: this.grid, velocity: this.lastVelocity ?? 0.8 });
        this.lastAdd = { id, undo: this.model.undoStack.length };
        this.changed();
      } else this.redraw();
    };
    canvas.addEventListener('pointerup', end);
    canvas.addEventListener('pointercancel', end);
    canvas.addEventListener('dblclick', (event) => {
      const { x, y } = this.local(event, canvas);
      const hit = hitNote(this.model.notes, x, y, this.view);
      if (!hit || this.locked) return;
      const added = this.lastAdd;
      this.lastAdd = null;
      if (added?.id === hit.id && added.undo === this.model.undoStack.length) {
        // A double-click on empty grid: the first click's note is taken back
        // (no history left behind; the file's text again, so nothing is written).
        this.model.undo();
        this.model.redoStack.pop();
        this.changed();
      } else if (this.model.remove([hit.id])) this.changed();
    });
  }

  hover(x, y) {
    const hit = hitNote(this.model.notes, x, y, this.view);
    this.canvases.grid.style.cursor = hit ? (hit.part === 'end' ? 'ew-resize' : 'grab') : 'crosshair';
  }

  /** While dragging outside the grid: scroll towards the pointer. */
  autoscroll(x, y) {
    const v = this.view;
    const dx = x < 0 ? x : x > v.width ? x - v.width : 0;
    const dy = y < 0 ? y : y > v.height ? y - v.height : 0;
    if (dx || dy) this.view = clampScroll({ ...v, scrollX: v.scrollX + dx * 0.5, scrollY: v.scrollY + dy * 0.5 }, this.totalBeats());
  }

  velocityEvents() {
    const canvas = this.canvases.velocity;
    canvas.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || this.locked) return;
      const { x, y } = this.local(event, canvas);
      const id = hitVelocity(this.model.notes, x, this.view, this.model.selected);
      if (id === null) return;
      this.capture(event);
      this.model.beginVelocity(id);
      this.model.velocityTo(velocityAt(y, canvas.clientHeight));
      this.gesture = { kind: 'velocity' };
      this.redraw();
    });
    canvas.addEventListener('pointermove', (event) => {
      const { x, y } = this.local(event, canvas);
      if (this.gesture?.kind !== 'velocity') {
        canvas.style.cursor = hitVelocity(this.model.notes, x, this.view, this.model.selected) === null ? 'default' : 'ns-resize';
        return;
      }
      this.model.velocityTo(velocityAt(y, canvas.clientHeight));
      this.redraw();
    });
    const end = () => {
      if (this.gesture?.kind !== 'velocity') return;
      this.gesture = null;
      const anchor = this.model.drag?.anchor;
      if (this.model.endDrag()) {
        const note = anchor && this.model.byId(anchor.id);
        if (note) this.lastVelocity = note.velocity;
        this.changed();
      } else this.redraw();
    };
    canvas.addEventListener('pointerup', end);
    canvas.addEventListener('pointercancel', end);
  }

  rulerEvents() {
    const canvas = this.canvases.ruler;
    const nearEnd = (x) => Math.abs(beatToX(this.model.length, this.view) - x) <= 6;
    canvas.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || this.locked) return;
      const { x } = this.local(event, canvas);
      if (!nearEnd(x)) return;
      this.capture(event);
      this.gesture = { kind: 'loop', before: this.model.snapshot() };
    });
    canvas.addEventListener('pointermove', (event) => {
      const { x } = this.local(event, canvas);
      if (this.gesture?.kind !== 'loop') {
        canvas.style.cursor = nearEnd(x) ? 'ew-resize' : 'default';
        return;
      }
      this.model.length = Math.max(1, snap(xToBeat(x, this.view), 1));
      this.redraw();
    });
    const end = () => {
      const g = this.gesture;
      if (g?.kind !== 'loop') return;
      this.gesture = null;
      if (this.model.length !== g.before.length) {
        this.model.remember(g.before);
        this.changed();
      }
    };
    canvas.addEventListener('pointerup', end);
    canvas.addEventListener('pointercancel', end);
  }

  scrollbarEvents() {
    for (const [selector, axis] of [['.hbar', 'x'], ['.vbar', 'y']]) {
      const bar = this.$(selector);
      bar.addEventListener('pointerdown', (event) => {
        event.stopPropagation();
        bar.setPointerCapture?.(event.pointerId);
        bar.classList.add('active');
        this.barDrag = { axis, at: axis === 'x' ? event.clientX : event.clientY, scroll: axis === 'x' ? this.view.scrollX : this.view.scrollY };
      });
      bar.addEventListener('pointermove', (event) => {
        const d = this.barDrag;
        if (!d || d.axis !== axis) return;
        const v = this.view;
        const now = axis === 'x' ? event.clientX : event.clientY;
        const content = axis === 'x' ? this.totalBeats() * v.beatPx : contentHeight(v);
        const size = axis === 'x' ? v.width : v.height;
        const scroll = d.scroll + ((now - d.at) * content) / size;
        this.view = clampScroll({ ...v, [axis === 'x' ? 'scrollX' : 'scrollY']: scroll }, this.totalBeats());
        this.redraw();
      });
      const end = () => {
        this.barDrag = null;
        bar.classList.remove('active');
      };
      bar.addEventListener('pointerup', end);
      bar.addEventListener('pointercancel', end);
    }
  }

  /**
   * The wheel scrolls and zooms inside only while the roll owns it (a click
   * or the focus, or the wheel modifier held: ADR 0034); otherwise the page
   * scrolls past it. Inside, it never runs on into the page.
   */
  onWheel(event, where) {
    if (!takesWheel(this, event)) return;
    event.preventDefault();
    const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 200 : 1;
    const dx = event.deltaX * scale;
    const dy = event.deltaY * scale;
    // Shift on a Mac with a mouse turns the wheel's delta into deltaX; a sideways swipe never zooms.
    const delta = verticalDelta(event) * scale;
    const box = this.canvases.grid.getBoundingClientRect();
    const total = this.totalBeats();
    const command = wheelCommand('piano_roll', event)?.id;
    if (command === 'piano_roll.zoom_time') {
      this.view = zoomX(this.view, Math.exp(-delta * 0.0025), clamp(event.clientX - box.left, 0, this.view.width), total);
    } else if (command === 'piano_roll.zoom_keys') {
      this.view = zoomY(this.view, Math.exp(-delta * 0.0025), clamp(event.clientY - box.top, 0, this.view.height), total);
    } else if (command === 'piano_roll.scroll_time' || (command === 'piano_roll.scroll' && (where === 'ruler' || where === 'velocity'))) {
      this.view = clampScroll({ ...this.view, scrollX: this.view.scrollX + (dx || dy) }, total);
    } else if (command === 'piano_roll.scroll') {
      this.view = clampScroll({ ...this.view, scrollX: this.view.scrollX + dx, scrollY: this.view.scrollY + dy }, total);
    } else {
      return;
    }
    this.redraw();
  }

  /** The `piano_roll` scope's keys (ui/lib/keys.default.js); the loop field's keys are its own. */
  onKey(event) {
    const target = event.composedPath?.()[0];
    if (target && (target.tagName === 'INPUT' || target.tagName === 'SELECT')) return false;
    const command = keyCommand('piano_roll', event)?.id;
    if (!command) return false;
    const total = () => this.totalBeats();
    const view = {
      'piano_roll.zoom_in': () => zoomX(this.view, 1.25, this.view.width / 2, total()),
      'piano_roll.zoom_out': () => zoomX(this.view, 1 / 1.25, this.view.width / 2, total()),
      'piano_roll.page_up': () => clampScroll({ ...this.view, scrollY: this.view.scrollY - this.view.height * 0.875 }, total()),
      'piano_roll.page_down': () => clampScroll({ ...this.view, scrollY: this.view.scrollY + this.view.height * 0.875 }, total()),
      'piano_roll.start': () => clampScroll({ ...this.view, scrollX: 0 }, total()),
      'piano_roll.end': () => clampScroll({ ...this.view, scrollX: total() * this.view.beatPx }, total()),
    }[command];
    if (view) {
      this.view = view();
      this.redraw();
      return true;
    }
    if (this.locked) return false;
    if (command === 'piano_roll.add_note') {
      // A note at the cursor (the last pressed place, else the playhead on C4), and the cursor after it.
      const at = this.cursor ?? { beat: snapFloor(this.playhead() ?? 0, this.grid), key: 60 };
      const id = this.model.add(at.beat, at.key, { grid: this.grid, velocity: this.lastVelocity ?? 0.8 });
      const added = this.model.byId(id);
      this.cursor = { beat: added.start + added.length, key: at.key };
      this.changed();
      return true;
    }
    const result = rollKey(this.model, event, this.grid, command);
    if (result === null) return false;
    if (result === true) this.changed();
    else this.redraw();
    return true;
  }

  // ------------------------------------------------ the adapter (ADR 0028 §7)

  streams(connection) {
    this.sync?.close();
    this.sync = null;
    const playhead = this.playheadStreams(connection);
    const name = this.getAttribute('pattern');
    if (!name || !canSyncPatterns(connection)) return playhead;
    let shown = false;
    const sync = new PatternSync({
      name,
      connection,
      kind: 'notes',
      onLoad: (pattern) => {
        // The file's version replaces what is shown: a drag in progress ends
        // here (its pointer moves nothing more), so it cannot put back what
        // the file changed. The history starts again from the file.
        this.model.drag = null;
        this.gesture = null;
        this.lastAdd = null;
        this.model.load(pattern);
        // The file's first version is placed as a new `.value` is (its loop
        // fits, its notes in view), not left in the view of the empty
        // pattern shown while it was read; a later reload keeps the view.
        if (!shown) {
          shown = true;
          this.placed = false;
          if (this.view?.width > 0 && this.canvases) this.place();
        }
        this.redraw();
      },
      onNotice: (text) => {
        this.notice = text;
        this.redraw();
      },
    });
    this.sync = sync;
    const stopStart = sync.start();
    return [
      ...playhead,
      () => {
        stopStart();
        sync.close(); // drains what is pending
        if (this.sync === sync) this.sync = null;
      },
    ];
  }

  /** `position-bind`: the playhead follows that signal output (ui/lib/playhead.js); writes nothing. */
  playheadStreams(connection) {
    this.playheadProblem = null;
    const port = playheadAddress(this.getAttribute('position-bind'));
    if (!port) return [];
    const stop = playheadStream(connection, port, (beats) => (this.position = beats), {
      onProblem: (text) => {
        this.playheadProblem = text;
        this.redraw();
      },
    });
    return [
      () => {
        stop();
        this.position = null;
      },
    ];
  }

  flush() {
    this.sync?.flush();
  }

  // ------------------------------------------------ drawing

  /** Updates the toolbar now and the canvases on the next frame. */
  redraw() {
    this.requestDraw();
  }

  requestDraw() {
    if (this.frame || typeof requestAnimationFrame !== 'function') return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  update() {
    this.requestDraw();
  }

  /** The toolbar's text and state and the ARIA attributes, once per frame. */
  updateToolbar() {
    if (!this.shadowRoot?.firstChild) return;
    const $ = (s) => this.$(s);
    $('.title').textContent = this.getAttribute('label') ?? this.getAttribute('pattern') ?? 'Notes';
    const length = $('.length');
    if (this.shadowRoot.activeElement !== length) length.value = String(this.model.length);
    const snapSelect = $('.snap');
    const label = this.getAttribute('snap') ?? this.snapLabel;
    if (label && snapSelect.value !== label) snapSelect.value = label;
    $('.undo').disabled = !this.model.canUndo;
    $('.redo').disabled = !this.model.canRedo;
    $('.velocity-toggle').setAttribute('aria-pressed', String(!this.hasAttribute('no-velocity')));
    const n = this.model.notes.length;
    const s = this.model.selected.size;
    $('.status').textContent = `${n} note${n === 1 ? '' : 's'}${s ? ` · ${s} selected` : ''}`;
    const notice = this.notice ?? this.playheadProblem ?? '';
    $('.notice').textContent = shortNotice(notice);
    $('.notice').title = notice;
    $('.notice-full').textContent = notice;
    const aria = `${this.getAttribute('label') ?? 'piano roll'}: ${n} notes, loop ${this.model.length} beats${this.locked ? ' (loading)' : ''}`;
    if (this.getAttribute('aria-label') !== aria) this.setAttribute('aria-label', aria);
    // The keymap's, unless the page's author gave its own.
    if (!this.hasAttribute('aria-keyshortcuts') || this.ownAriaKeys) {
      this.setAttribute('aria-keyshortcuts', ariaShortcuts('piano_roll'));
      this.ownAriaKeys = true;
    }
    $('.roll').toggleAttribute('aria-busy', this.locked);
  }

  /** A canvas's 2D context sized to its box at the device's pixel ratio. */
  context(canvas) {
    const dpr = globalThis.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w, h };
  }

  draw() {
    this.updateToolbar();
    if (!this.canvases || this.view.width === 0) return;
    this.drawGrid();
    this.drawKeys();
    this.drawRuler();
    if (!this.hasAttribute('no-velocity')) this.drawVelocity();
    this.placeScrollbars();
  }

  /** The playhead's beat inside the loop, or null. */
  playhead() {
    if (this.positionBeats === null) return null;
    const L = this.model.length;
    return ((this.positionBeats % L) + L) % L;
  }

  noteFill(velocity, selected, alpha = 1) {
    if (selected) return `rgba(255, 207, 107, ${alpha})`;
    const hue = Number(this.colors['--signal-roll-note-hue']) || 196;
    // Low velocities: darker and greyer; high: bright and saturated.
    const s = 25 + 65 * velocity;
    const l = 26 + 36 * velocity;
    return `hsla(${hue - 18 * velocity}, ${s}%, ${l}%, ${alpha})`;
  }

  drawGrid() {
    const { ctx, w, h } = this.context(this.canvases.grid);
    const v = this.view;
    const c = this.colors;
    ctx.fillStyle = c['--signal-roll-bg'];
    ctx.fillRect(0, 0, w, h);
    // Rows: black-key rows darker, a line under every row, a stronger one under each C.
    const top = clamp(yToKey(0, v), 0, MAX_KEY);
    const bottom = clamp(yToKey(h, v), 0, MAX_KEY);
    for (let k = top; k >= bottom; k--) {
      const y = keyToY(k, v);
      ctx.fillStyle = isBlack(k) ? c['--signal-roll-row-black'] : c['--signal-roll-row'];
      ctx.fillRect(0, y, w, v.rowPx);
      ctx.fillStyle = k % 12 === 0 ? c['--signal-roll-octave'] : c['--signal-roll-line'];
      ctx.fillRect(0, Math.round(y + v.rowPx) - 1, w, 1);
    }
    // Vertical lines: bars, beats, snap subdivisions.
    const b0 = xToBeat(0, v);
    const b1 = xToBeat(w, v);
    const colours = [c['--signal-roll-bar'], c['--signal-roll-beat'], c['--signal-roll-sub']];
    for (const line of gridLines(b0, b1, v, { beatsPerBar: this.beatsPerBar, grid: this.grid })) {
      ctx.fillStyle = colours[line.level];
      ctx.fillRect(Math.round(beatToX(line.beat, v)), 0, 1, h);
    }
    // Outside the loop: shaded.
    const loopX = beatToX(this.model.length, v);
    if (loopX < w) {
      ctx.fillStyle = c['--signal-roll-outside'];
      ctx.fillRect(Math.max(0, loopX), 0, w - Math.max(0, loopX), h);
      ctx.fillStyle = c['--signal-roll-loop'];
      ctx.globalAlpha = 0.7;
      ctx.fillRect(Math.round(loopX) - 1, 0, 2, h);
      ctx.globalAlpha = 1;
    }
    // Notes.
    const selected = this.model.selected;
    const r = Math.min(3, v.rowPx / 4);
    ctx.font = `${Math.min(10, v.rowPx - 3)}px ${FONT}`;
    ctx.textBaseline = 'middle';
    const L = this.model.length;
    for (const n of this.model.notes) {
      if (n.key > top || n.key < bottom || n.start > b1 || n.start + n.length < b0) continue;
      const x = beatToX(n.start, v);
      const y = keyToY(n.key, v);
      const nw = Math.max(2, n.length * v.beatPx - 1);
      const nh = v.rowPx - 1;
      const isSel = selected.has(n.id);
      const outside = n.start >= L;
      ctx.fillStyle = this.noteFill(n.velocity, isSel, outside ? 0.35 : 1);
      roundRect(ctx, x + 0.5, y + 0.5, nw, nh - 0.5, r);
      ctx.fill();
      ctx.lineWidth = 1;
      ctx.strokeStyle = isSel ? '#fff4d6' : 'rgba(0, 0, 0, 0.55)';
      ctx.stroke();
      // A highlight along the top edge.
      if (nh >= 8 && nw > 4) {
        ctx.fillStyle = 'rgba(255, 255, 255, 0.18)';
        ctx.fillRect(x + 2, y + 1.5, nw - 3, 1);
      }
      if (nw > 26 && v.rowPx >= 11) {
        ctx.fillStyle = isSel ? '#3a2a00' : 'rgba(5, 15, 25, 0.8)';
        ctx.fillText(keyName(n.key), x + 4, y + v.rowPx / 2 + 0.5, nw - 6);
      }
    }
    // The rubber band.
    const g = this.gesture;
    if (g?.kind === 'band') {
      const x0 = beatToX(Math.min(g.b0, g.b1), v);
      const x1 = beatToX(Math.max(g.b0, g.b1), v);
      const y0 = keyToY(Math.max(g.k0, g.k1), v);
      const y1 = keyToY(Math.min(g.k0, g.k1), v) + v.rowPx;
      ctx.fillStyle = 'rgba(90, 169, 255, 0.12)';
      ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
      ctx.strokeStyle = 'rgba(90, 169, 255, 0.8)';
      ctx.setLineDash([4, 3]);
      ctx.strokeRect(x0 + 0.5, y0 + 0.5, x1 - x0, y1 - y0);
      ctx.setLineDash([]);
    }
    // The playhead.
    const p = this.playhead();
    if (p !== null) {
      const x = Math.round(beatToX(p, v));
      ctx.fillStyle = c['--signal-roll-playhead'];
      ctx.fillRect(x, 0, 1.5, h);
    }
  }

  drawKeys() {
    const { ctx, w, h } = this.context(this.canvases.keys);
    const v = this.view;
    ctx.fillStyle = '#e9e8e3';
    ctx.fillRect(0, 0, w, h);
    const top = Math.min(MAX_KEY, yToKey(0, v) + 1);
    const bottom = Math.max(0, yToKey(h, v) - 1);
    const p = this.playhead();
    const sounding = new Set();
    if (p !== null) for (const n of this.model.notes) if (p >= n.start && p < n.start + n.length) sounding.add(n.key);
    const blackW = Math.round(w * 0.6);
    // White keys: a boundary where two white keys meet (B|C, E|F) or in the middle of a black key.
    ctx.fillStyle = '#b9b8b2';
    for (let k = top; k >= bottom; k--) {
      const y = keyToY(k, v);
      if (sounding.has(k) && !isBlack(k)) {
        const below = isBlack(k - 1) ? v.rowPx / 2 : 0;
        const above = isBlack(k + 1) ? v.rowPx / 2 : 0;
        ctx.fillStyle = 'rgba(79, 195, 247, 0.55)';
        ctx.fillRect(0, y - above, w, v.rowPx + above + below);
        ctx.fillStyle = '#b9b8b2';
      }
      const pc = k % 12;
      if (pc === 0 || pc === 5) ctx.fillRect(0, Math.round(y + v.rowPx) - 1, w, 1);
      if (isBlack(k)) ctx.fillRect(blackW, Math.round(y + v.rowPx / 2), w - blackW, 1);
    }
    // Black keys with a little depth.
    for (let k = top; k >= bottom; k--) {
      if (!isBlack(k)) continue;
      const y = keyToY(k, v);
      const grad = ctx.createLinearGradient(0, 0, blackW, 0);
      grad.addColorStop(0, sounding.has(k) ? '#2d7fa6' : '#16171a');
      grad.addColorStop(0.85, sounding.has(k) ? '#4fc3f7' : '#2c2e33');
      grad.addColorStop(1, sounding.has(k) ? '#3aa0cf' : '#1d1f23');
      ctx.fillStyle = grad;
      roundRect(ctx, 0, y + 0.5, blackW, v.rowPx - 1, [0, 2, 2, 0]);
      ctx.fill();
    }
    // C labels.
    ctx.font = `${Math.min(10, Math.max(8, v.rowPx - 3))}px ${FONT}`;
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'right';
    for (let k = top; k >= bottom; k--) {
      if (k % 12 !== 0) continue;
      const y = keyToY(k, v);
      ctx.fillStyle = k === 60 ? '#1f6f96' : '#6d6c66';
      ctx.fillText(keyName(k), w - 4, y + v.rowPx / 2 + 0.5);
    }
    ctx.textAlign = 'left';
    ctx.fillStyle = '#111317';
    ctx.fillRect(w - 1, 0, 1, h);
  }

  drawRuler() {
    const { ctx, w, h } = this.context(this.canvases.ruler);
    const v = this.view;
    const c = this.colors;
    const grad = ctx.createLinearGradient(0, 0, 0, h);
    grad.addColorStop(0, '#2a2e36');
    grad.addColorStop(1, '#22262d');
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, w, h);
    // The loop: a strip from 0 to its end, with a handle.
    const loopX = beatToX(this.model.length, v);
    const startX = beatToX(0, v);
    ctx.fillStyle = 'rgba(79, 195, 247, 0.22)';
    ctx.fillRect(startX, 0, loopX - startX, 6);
    ctx.fillStyle = c['--signal-roll-loop'];
    ctx.beginPath();
    ctx.moveTo(loopX, 0);
    ctx.lineTo(loopX, 12);
    ctx.lineTo(loopX - 7, 0);
    ctx.closePath();
    ctx.fill();
    // Bars and beats.
    const bpb = this.beatsPerBar;
    ctx.font = `10px ${FONT}`;
    ctx.textBaseline = 'alphabetic';
    const lines = gridLines(xToBeat(0, v), xToBeat(w, v), v, { beatsPerBar: bpb, grid: 1, minPx: 10 });
    const barPx = bpb * v.beatPx;
    const labelEvery = barPx >= 28 ? 1 : 2 ** Math.ceil(Math.log2(28 / barPx));
    for (const line of lines) {
      const x = Math.round(beatToX(line.beat, v));
      if (line.level === 0) {
        ctx.fillStyle = '#7d8696';
        ctx.fillRect(x, 8, 1, h - 8);
        const bar = Math.round(line.beat / bpb);
        if (bar % labelEvery === 0) {
          ctx.fillStyle = '#c9ced8';
          ctx.fillText(String(bar + 1), x + 4, h - 7);
        }
      } else {
        ctx.fillStyle = '#4c5361';
        ctx.fillRect(x, h - 6, 1, 6);
        if (v.beatPx >= 44) {
          ctx.fillStyle = '#6c7484';
          ctx.fillText(`${Math.floor(line.beat / bpb) + 1}.${(Math.round(line.beat) % bpb) + 1}`, x + 3, h - 7);
        }
      }
    }
    const p = this.playhead();
    if (p !== null) {
      const x = beatToX(p, v);
      ctx.fillStyle = c['--signal-roll-playhead'];
      ctx.beginPath();
      ctx.moveTo(x - 5, h - 9);
      ctx.lineTo(x + 5, h - 9);
      ctx.lineTo(x, h);
      ctx.closePath();
      ctx.fill();
    }
    ctx.fillStyle = '#111317';
    ctx.fillRect(0, h - 1, w, 1);
  }

  drawVelocity() {
    const { ctx, w, h } = this.context(this.canvases.velocity);
    const v = this.view;
    ctx.fillStyle = '#1a1d22';
    ctx.fillRect(0, 0, w, h);
    const pad = 4;
    const span = h - 2 * pad;
    ctx.fillStyle = '#252930';
    for (const f of [0.25, 0.5, 0.75]) ctx.fillRect(0, Math.round(pad + (1 - f) * span), w, 1);
    for (const line of gridLines(xToBeat(0, v), xToBeat(w, v), v, { beatsPerBar: this.beatsPerBar, grid: 1 })) {
      if (line.level > 0) continue;
      ctx.fillStyle = '#2c313a';
      ctx.fillRect(Math.round(beatToX(line.beat, v)), 0, 1, h);
    }
    const b0 = xToBeat(-10, v);
    const b1 = xToBeat(w + 10, v);
    const selected = this.model.selected;
    // Unselected first, so the selected bars are on top.
    for (const pass of [false, true]) {
      for (const n of this.model.notes) {
        if (selected.has(n.id) !== pass || n.start < b0 || n.start > b1) continue;
        const x = Math.round(beatToX(n.start, v)) + 0.5;
        const y = pad + (1 - n.velocity) * span;
        const colour = this.noteFill(n.velocity, pass, 1);
        const barW = clamp(n.length * v.beatPx - 2, 2, 10);
        ctx.fillStyle = colour;
        ctx.globalAlpha = 0.35;
        ctx.fillRect(x - 0.5, y, barW, h - pad - y);
        ctx.globalAlpha = 1;
        ctx.fillRect(x - 0.5, y, 2, h - pad - y);
        ctx.beginPath();
        ctx.arc(x + 0.5, y, 3, 0, Math.PI * 2);
        ctx.fill();
        ctx.strokeStyle = pass ? '#fff4d6' : 'rgba(0,0,0,0.6)';
        ctx.stroke();
      }
    }
  }

  placeScrollbars() {
    const v = this.view;
    const hbar = this.$('.hbar');
    const vbar = this.$('.vbar');
    const total = this.totalBeats() * v.beatPx;
    const hFrac = Math.min(1, v.width / total);
    hbar.hidden = hFrac >= 1;
    hbar.style.width = `${Math.max(24, hFrac * (v.width - 12))}px`;
    hbar.style.left = `${(v.scrollX / total) * (v.width - 12)}px`;
    const content = contentHeight(v);
    const vFrac = Math.min(1, v.height / content);
    vbar.hidden = vFrac >= 1;
    vbar.style.height = `${Math.max(24, vFrac * (v.height - 12))}px`;
    vbar.style.top = `${(v.scrollY / content) * (v.height - 12)}px`;
  }
}

/** A rounded rectangle path (radius a number or [tl, tr, br, bl]). */
function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  if (ctx.roundRect) {
    ctx.roundRect(x, y, w, h, r);
    return;
  }
  ctx.rect(x, y, w, h);
}

/** Throws a PatternError unless `pattern` is a valid pattern of `kind`. */
function checked(pattern, kind) {
  if (pattern?.kind !== kind) throw new PatternError(`expected a "${kind}" pattern, not ${JSON.stringify(pattern?.kind)}`);
  const problems = validatePattern(pattern);
  if (problems.length) throw new PatternError(problems[0].message);
}

customElements.get('signal-piano-roll') ?? customElements.define('signal-piano-roll', SignalPianoRoll);

// <signal-step-grid>: edits a `steps` pattern (ADR 0028 §2, §7) as a drum
// machine's step grid in the spirit of the classic analogue rhythm
// machines: a warm grey panel, step buttons in groups of four (red, orange,
// yellow, off-white), each with its lamp, the lanes' names and level knobs
// on the left, an accent row, and a running light from the `step` property
// (a sequencer's `step` output through `step-bind`, ui/lib/playhead.js). The
// cells, cycle, painting, layout and hit-testing are
// ui/lib/step-grid-model.js (no DOM); the file text ui/lib/pattern-text.js.
//
// Interface: `.value` (the pattern object), `.text` (its file text), a
// `change` event (bubbling, composed; `detail` the new pattern) after every
// edit, never after setting `.value` or `.text`. With `pattern="<name>"` and
// a connection offering `readPattern`/`writePattern` (ui/lib/pattern-sync.js,
// ADR 0028 §7), it loads that pattern on connect and writes every edit.
//
// Pointer: a click cycles a step rest → hit → accent → rest, and dragging
// on paints that state; the accent row turns a step's hits into accents
// and back; a level knob drags vertically (Shift fine), takes the wheel,
// and a double-click resets it. Keys: arrows move the cursor, Space or
// Enter cycles its step, Delete rests it, ⌘Z / ⇧⌘Z; left of step 1 the
// cursor is on the lane's knob (↑↓ level, Shift fine, Home, End, Delete
// resets); Page Up / Page Down change the cursor lane's level anywhere.
// While a `pattern` is first read, nothing can be edited.

import { canSyncPatterns, PatternSync, shortNotice } from '../lib/pattern-sync.js';
import { playheadAddress, playheadStream } from '../lib/playhead.js';
import { PatternError, REST, parsePattern, serializePattern, validatePattern } from '../lib/pattern-text.js';
import { STEP_COUNTS, STEP_LENGTHS, StepGridModel, accentAt, gridLayout, groupOf, hitGrid, laneY, stepX } from '../lib/step-grid-model.js';
import { SignalElement, registerWheel, takesWheel, verticalDelta } from './base.js';
import { keyCommand, wheelCommand } from '../lib/keys.js';

const STATE_CLASS = { '.': 'rest', x: 'hit', X: 'accent' };
const escapeText = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
/** A lane's name as printed on the panel: words, upper case. */
const printed = (name) => name.replace(/[_-]+/g, ' ').toUpperCase();

export class SignalStepGrid extends SignalElement {
  static role = 'grid';
  static observedAttributes = ['pattern', 'label', 'step', 'step-bind', 'wheel'];
  static resubscribe = ['pattern', 'step-bind'];
  static css = `
:host { display: block; min-width: 360px; border-radius: 10px; color: #2f2d2a; }
.panel {
  border-radius: 10px; padding: 10px 14px 12px;
  background: linear-gradient(180deg, #d6d1c7 0%, #c9c3b8 55%, #bfb9ad 100%);
  box-shadow: inset 0 1px 0 rgba(255,255,255,0.6), inset 0 -2px 0 rgba(0,0,0,0.12), 0 2px 10px rgba(0,0,0,0.45);
}
.top { display: flex; align-items: center; gap: 14px; flex-wrap: wrap; padding: 0 2px 8px; border-bottom: 2px solid #8f897e; margin-bottom: 4px; }
.title { font: 700 13px/1 "Helvetica Neue", Helvetica, Arial, sans-serif; letter-spacing: 0.14em; text-transform: uppercase; color: #34312d; }
.subtitle { font: 600 9px/1 "Helvetica Neue", Helvetica, Arial, sans-serif; letter-spacing: 0.2em; text-transform: uppercase; color: #6d675d; }
.top label { display: inline-flex; align-items: center; gap: 6px; font: 700 9px/1 "Helvetica Neue", Helvetica, Arial, sans-serif; letter-spacing: 0.14em; text-transform: uppercase; color: #4b4740; }
.seg { display: inline-flex; border-radius: 3px; overflow: hidden; box-shadow: 0 1px 0 rgba(255,255,255,0.5), inset 0 0 0 1px #6f6a61; }
.seg button { font: 700 10px/1 "Helvetica Neue", Helvetica, Arial, sans-serif; min-width: 28px; height: 20px; padding: 0 6px; border: 0; border-left: 1px solid #6f6a61; color: #e8e3d8; background: linear-gradient(#56524c, #45423d); cursor: pointer; }
.seg button:first-child { border-left: 0; }
.seg button[aria-pressed="true"] { background: linear-gradient(#f08a2c, #d9701a); color: #2b1a08; }
.seg button:disabled { color: #8a857c; cursor: default; }
select { font: 700 10px/1 "Helvetica Neue", Helvetica, Arial, sans-serif; height: 20px; color: #e8e3d8; background: #4b4843; border: 1px solid #6f6a61; border-radius: 3px; }
.spacer { flex: 1; }
.notice { font: 11px/1.2 -apple-system, sans-serif; color: #8a2a12; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 40ch; }
.notice-full { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
svg.grid { width: 100%; height: auto; display: block; overflow: visible; touch-action: none; }
.num { font: 700 9px "Helvetica Neue", Helvetica, Arial, sans-serif; fill: #4f4a43; text-anchor: middle; letter-spacing: 0.05em; }
.lane-name { font: 700 10px "Helvetica Neue", Helvetica, Arial, sans-serif; fill: #2f2c28; letter-spacing: 0.12em; dominant-baseline: middle; }
.lane-strip { fill: rgba(255,255,255,0.22); }
.lane-strip.alt { fill: rgba(0,0,0,0.035); }
.rule { fill: #a49e93; }
.cap { stroke: rgba(0,0,0,0.45); stroke-width: 1; }
.cap-shine { fill: rgba(255,255,255,0.28); pointer-events: none; }
.g0 .cap { fill: #d9462e; } .g1 .cap { fill: #ef8b2c; } .g2 .cap { fill: #f2c94e; } .g3 .cap { fill: #ebe5d5; }
.rest.g0 .cap { fill: #8f4a3d; } .rest.g1 .cap { fill: #9c6a3e; } .rest.g2 .cap { fill: #a08b52; } .rest.g3 .cap { fill: #a29d90; }
.rest .cap-shine { fill: rgba(255,255,255,0.1); }
.lamp { fill: #4a1d16; stroke: rgba(0,0,0,0.5); stroke-width: 0.6; }
.hit .lamp { fill: #ff3b1f; filter: url(#glow); }
.accent .lamp { fill: #fff6c8; filter: url(#glow-strong); }
.accent .cap { stroke: #fff8de; stroke-width: 1.6; }
.accent-row .cap { fill: #5b5751; }
.accent-row.on .cap { fill: #6b665f; }
.accent-row .lamp { fill: #3d2a12; }
.accent-row.on .lamp { fill: #ffd34a; filter: url(#glow); }
.run { fill: #43201a; stroke: rgba(0,0,0,0.5); stroke-width: 0.6; }
.run.on { fill: #ff4020; filter: url(#glow-strong); }
.column { fill: rgba(255, 250, 230, 0.0); pointer-events: none; }
.column.on { fill: rgba(255, 250, 230, 0.28); }
.cursor { fill: none; stroke: #1f6fb0; stroke-width: 2; pointer-events: none; }
:host(:not(:focus-visible)) .cursor { display: none; }
.knob-body { fill: url(#knob); stroke: #1e1d1b; stroke-width: 1; }
.knob-track { fill: none; stroke: #8d877c; stroke-width: 2.5; stroke-linecap: round; }
.knob-arc { fill: none; stroke: #e2671f; stroke-width: 2.5; stroke-linecap: round; }
.knob-pointer { stroke: #f4f0e6; stroke-width: 2; stroke-linecap: round; }
.knob-hit { fill: transparent; cursor: ns-resize; }
.cell-hit { fill: transparent; cursor: pointer; }
`;

  constructor() {
    super();
    this.model = new StepGridModel();
    this.runStep = null;
    this.cursor = { lane: 0, step: 0 };
    this.shape = '';
    this.sync = null;
  }

  // ------------------------------------------------ the interface

  get value() {
    return this.model.pattern();
  }

  set value(pattern) {
    checked(pattern, 'steps');
    this.model.load(pattern);
    this.update();
  }

  get text() {
    return serializePattern(this.value);
  }

  set text(text) {
    this.value = parsePattern(text);
  }

  /** The running light's step (0-based, shown modulo the step count), or null. */
  get step() {
    return this.runStep;
  }

  set step(step) {
    const v = step === null || step === undefined || step === '' ? null : Math.floor(Number(step));
    this.runStep = Number.isFinite(v) ? v : null;
    this.updateRun();
  }

  attributeChangedCallback(name, previous, value) {
    if (name === 'step') this.step = value;
    super.attributeChangedCallback(name, previous, value);
  }

  // ------------------------------------------------ DOM

  template() {
    const counts = STEP_COUNTS.map((n) => `<option value="${n}">${n}</option>`).join('');
    const lengths = STEP_LENGTHS.map((s) => `<option value="${s.beats}">${s.label}</option>`).join('');
    return `<div class="panel" part="panel">
  <div class="top">
    <div><div class="title"></div><div class="subtitle">rhythm pattern</div></div>
    <label>Steps <span class="seg steps-seg"><button data-steps="16">16</button><button data-steps="32">32</button></span><select class="steps" aria-label="Steps">${counts}</select></label>
    <label>Step <select class="length" aria-label="Step length">${lengths}</select></label>
    <span class="seg"><button class="undo" title="Undo (⌘Z)">UNDO</button><button class="redo" title="Redo (⇧⌘Z)">REDO</button></span>
    <span class="spacer"></span>
    <span class="notice" aria-hidden="true"></span><span class="notice-full" role="status"></span>
  </div>
  <svg class="grid" part="grid" xmlns="http://www.w3.org/2000/svg"></svg>
</div>`;
  }

  setup() {
    const $ = (s) => this.$(s);
    for (const button of this.shadowRoot.querySelectorAll('button, select')) {
      button.addEventListener('pointerdown', (event) => event.stopPropagation());
    }
    for (const button of this.shadowRoot.querySelectorAll('[data-steps]')) {
      button.addEventListener('click', () => this.edit(() => this.model.setSteps(Number(button.dataset.steps))));
    }
    $('.steps').addEventListener('change', (event) => this.edit(() => this.model.setSteps(Number(event.target.value))));
    $('.length').addEventListener('change', (event) => this.edit(() => this.model.setStepLength(Number(event.target.value))));
    $('.undo').addEventListener('click', () => this.edit(() => this.model.undo()));
    $('.redo').addEventListener('click', () => this.edit(() => this.model.redo()));
    const svg = $('svg.grid');
    svg.addEventListener('pointerdown', (event) => this.down(event));
    svg.addEventListener('pointermove', (event) => this.move(event));
    svg.addEventListener('pointerup', () => this.up());
    svg.addEventListener('pointercancel', () => this.up());
    svg.addEventListener('dblclick', (event) => {
      const hit = this.at(event);
      if (hit?.area === 'knob') this.edit(() => this.model.resetLevel(hit.lane));
    });
    // Only the level knobs use the wheel: over the steps a gesture scrolls the page.
    registerWheel(this, (event) => this.at(event)?.area === 'knob');
    svg.addEventListener(
      'wheel',
      (event) => {
        const hit = this.at(event);
        // Only a level knob takes the wheel, and only while the grid owns it (ADR 0034).
        if (hit?.area !== 'knob' || !takesWheel(this, event)) return;
        event.preventDefault();
        const command = wheelCommand('step_grid', event);
        if (this.locked || command?.id !== 'step_grid.wheel') return;
        const step = (command.fine ? 0.01 : 0.05) * -Math.sign(verticalDelta(event));
        if (this.model.setLevel(hit.lane, this.model.level(hit.lane) + step)) {
          this.update();
          clearTimeout(this.wheelTimer);
          this.wheelTimer = setTimeout(() => this.settleLevel(), 250);
        }
      },
      { passive: false },
    );
  }

  /** Waiting for the pattern's first read: no edits until it is shown (ui/lib/pattern-sync.js). */
  get locked() {
    return Boolean(this.sync && !this.sync.loaded);
  }

  /** A level change from the wheel or keys not yet an undo step: made one now (and written). */
  settleLevel() {
    clearTimeout(this.wheelTimer);
    this.wheelTimer = null;
    if (this.model.commitLevel()) this.changed();
  }

  /** Runs an edit (a level change pending is settled first); `change` when it changed something. */
  edit(apply) {
    if (this.locked) return false;
    this.settleLevel();
    const changed = apply();
    if (changed) this.changed();
    else this.update();
    return changed;
  }

  disconnectedCallback() {
    this.settleLevel();
    super.disconnectedCallback();
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

  /** The grid's coordinates (the viewBox's) under a pointer. */
  at(event) {
    const svg = this.$('svg.grid');
    const box = svg.getBoundingClientRect();
    const L = this.layout;
    if (!L || box.width === 0) return null;
    const x = ((event.clientX - box.left) / box.width) * L.width;
    const y = ((event.clientY - box.top) / box.height) * L.height;
    return hitGrid(L, x, y);
  }

  down(event) {
    if (event.button !== 0 || this.locked) return;
    const hit = this.at(event);
    if (!hit) return;
    this.capture(event);
    this.settleLevel();
    if (hit.area === 'knob') {
      this.gesture = { kind: 'knob', lane: hit.lane, y: event.clientY, level: this.model.level(hit.lane) };
      return;
    }
    if (hit.area !== 'cell') return;
    this.cursor = { lane: Math.min(hit.lane, this.model.lanes.length - 1), step: hit.step };
    if (hit.lane >= this.model.lanes.length) {
      this.edit(() => this.model.toggleAccent(hit.step));
      return;
    }
    this.model.press(hit.lane, hit.step);
    this.gesture = { kind: 'paint' };
    this.update();
  }

  move(event) {
    const g = this.gesture;
    if (!g) return;
    if (g.kind === 'knob') {
      const scale = event.shiftKey ? 0.001 : 0.006;
      if (this.model.setLevel(g.lane, g.level + (g.y - event.clientY) * scale)) this.update();
      return;
    }
    const hit = this.at(event);
    if (hit?.area === 'cell' && hit.lane < this.model.lanes.length && this.model.enter(hit.lane, hit.step)) this.update();
  }

  up() {
    const g = this.gesture;
    this.gesture = null;
    if (!g) return;
    if (g.kind === 'knob') {
      if (this.model.commitLevel()) this.changed();
    } else if (this.model.release()) this.changed();
    else this.update();
  }

  /** The `step_grid` scope's keys (ui/lib/keys.default.js). */
  onKey(event) {
    const target = event.composedPath?.()[0];
    if (target && (target.tagName === 'SELECT' || target.tagName === 'BUTTON')) return false;
    if (this.locked) return false;
    const found = keyCommand('step_grid', event);
    if (!found) return false;
    const command = found.id;
    const m = this.model;
    if (command === 'step_grid.undo' || command === 'step_grid.redo') {
      this.edit(() => (command === 'step_grid.redo' ? m.redo() : m.undo()));
      return true;
    }
    const { lane, step } = this.cursor;
    const onKnob = step < 0;
    // The level knob: the cursor left of step 1 (Up/Down, Shift fine, Home, End, Delete resets);
    // Page Up / Page Down change the cursor lane's level anywhere.
    const levelStep = { 'step_grid.up': 0.05, 'step_grid.down': -0.05, 'step_grid.level_up': 0.1, 'step_grid.level_down': -0.1 }[command];
    if (levelStep !== undefined && (onKnob || command.startsWith('step_grid.level_'))) {
      if (m.setLevel(lane, m.level(lane) + levelStep * (found.fine ? 0.2 : 1))) this.update();
      clearTimeout(this.wheelTimer);
      this.wheelTimer = setTimeout(() => this.settleLevel(), 250);
      return true;
    }
    if (onKnob && (command === 'step_grid.level_min' || command === 'step_grid.level_max')) {
      if (m.setLevel(lane, command === 'step_grid.level_min' ? 0 : 1)) this.update();
      this.settleLevel();
      return true;
    }
    if (onKnob && command === 'step_grid.clear') {
      this.edit(() => m.resetLevel(lane));
      return true;
    }
    const moves = { 'step_grid.up': [-1, 0], 'step_grid.down': [1, 0], 'step_grid.left': [0, -1], 'step_grid.right': [0, 1] };
    if (moves[command]) {
      this.settleLevel();
      const [dl, ds] = moves[command];
      const lanes = Math.max(1, m.lanes.length);
      // Steps wrap; left of step 1 is the lane's level knob.
      const next = step + ds < -1 ? m.steps - 1 : step + ds >= m.steps ? -1 : step + ds;
      this.cursor = { lane: (lane + dl + lanes) % lanes, step: next };
      this.update();
      return true;
    }
    if (onKnob) return false;
    if (command === 'step_grid.cycle') {
      this.edit(() => {
        m.press(lane, step);
        return m.release();
      });
      return true;
    }
    if (command === 'step_grid.clear') {
      this.edit(() => {
        const before = m.snapshot();
        if (!m.put(lane, step, REST)) return false;
        m.remember(before);
        return true;
      });
      return true;
    }
    return false;
  }

  changed() {
    this.update();
    const pattern = this.value;
    let text = null;
    try {
      text = serializePattern(pattern);
      if (!this.sync && this.notice?.startsWith('not written')) this.notice = null;
    } catch (error) {
      // Never thrown at the page: the edit is shown, said to be invalid and not written.
      this.notice = `not written: ${error.message}`;
      this.update();
    }
    this.dispatchEvent(new CustomEvent('change', { detail: pattern, bubbles: true, composed: true }));
    if (text !== null) this.sync?.edited(text, pattern);
  }

  // ------------------------------------------------ the adapter (ADR 0028 §7)

  streams(connection) {
    this.sync?.close();
    this.sync = null;
    const playhead = this.playheadStreams(connection);
    const name = this.getAttribute('pattern');
    if (!name || !canSyncPatterns(connection)) return playhead;
    const sync = new PatternSync({
      name,
      connection,
      kind: 'steps',
      onLoad: (pattern) => {
        // The file's version replaces what is shown: a paint stroke or a
        // level drag in progress ends here, so it cannot put back what the
        // file changed. The history starts again from the file.
        clearTimeout(this.wheelTimer);
        this.wheelTimer = null;
        this.gesture = null;
        this.model.load(pattern);
        this.update();
      },
      onNotice: (text) => {
        this.notice = text;
        this.update();
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

  /** `step-bind`: the running light follows that signal output (ui/lib/playhead.js); writes nothing. */
  playheadStreams(connection) {
    this.playheadProblem = null;
    const port = playheadAddress(this.getAttribute('step-bind'));
    if (!port) return [];
    const stop = playheadStream(connection, port, (step) => (this.step = step), {
      onProblem: (text) => {
        this.playheadProblem = text;
        this.update();
      },
    });
    return [
      () => {
        stop();
        this.step = null;
      },
    ];
  }

  flush() {
    this.sync?.flush();
  }

  // ------------------------------------------------ drawing

  /** Builds the SVG for this step and lane count (once per shape). */
  build() {
    const m = this.model;
    const L = gridLayout(m.steps, m.lanes.length);
    this.layout = L;
    const parts = [];
    parts.push(`<defs>
  <filter id="glow" x="-150%" y="-150%" width="400%" height="400%"><feGaussianBlur stdDeviation="1.6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
  <filter id="glow-strong" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="2.6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
  <radialGradient id="knob" cx="40%" cy="35%" r="70%"><stop offset="0" stop-color="#6a6660"/><stop offset="0.6" stop-color="#3b3936"/><stop offset="1" stop-color="#262523"/></radialGradient>
</defs>`);
    const gridRight = stepX(L, m.steps - 1) + L.cell;
    // Step numbers and the running lights.
    for (let s = 0; s < m.steps; s++) {
      const cx = stepX(L, s) + L.cell / 2;
      parts.push(`<text class="num" x="${cx}" y="12">${s + 1}</text>`);
      parts.push(`<circle class="run" data-run="${s}" cx="${cx}" cy="23" r="3.2"/>`);
    }
    // Lanes.
    m.lanes.forEach((lane, i) => {
      const y = laneY(L, i);
      parts.push(`<rect class="lane-strip${i % 2 ? ' alt' : ''}" x="${L.pad / 2}" y="${y + 2}" width="${gridRight - L.pad / 2 + 6}" height="${L.row - 4}" rx="4"/>`);
      parts.push(`<text class="lane-name" data-name="${i}" x="${L.pad}" y="${y + L.row / 2}">${escapeText(printed(lane.name))}</text>`);
      const kx = L.left - 24;
      const ky = y + L.row / 2;
      parts.push(`<g class="knob" data-knob="${i}"><path class="knob-track" d="${arc(kx, ky, 15, 0, 1)}"/><path class="knob-arc"/><circle class="knob-body" cx="${kx}" cy="${ky}" r="11"/><line class="knob-pointer"/><rect class="knob-hit" x="${kx - 18}" y="${ky - 18}" width="36" height="36"/></g>`);
      parts.push(`<g role="row" aria-label="${escapeText(lane.name)}">`);
      for (let s = 0; s < m.steps; s++) parts.push(this.cellSvg(L, i, s, `g${groupOf(s)}`));
      parts.push('</g>');
    });
    // The accent row.
    const ay = laneY(L, m.lanes.length);
    parts.push(`<rect class="rule" x="${L.pad / 2}" y="${ay + 1}" width="${gridRight - L.pad / 2 + 6}" height="1.5"/>`);
    parts.push(`<text class="lane-name" x="${L.pad}" y="${ay + L.accentRow / 2 + 2}">ACCENT</text>`);
    parts.push('<g role="row" aria-label="accent">');
    for (let s = 0; s < m.steps; s++) parts.push(this.cellSvg(L, m.lanes.length, s, 'accent-row', true));
    parts.push('</g>');
    // Running column and cursor on top.
    parts.push(`<rect class="column" x="0" y="${L.top}" width="${L.cell + 4}" height="${m.lanes.length * L.row + L.accentRow}" rx="4"/>`);
    parts.push(`<rect class="cursor" rx="5"/>`);
    const svg = this.$('svg.grid');
    svg.setAttribute('viewBox', `0 0 ${L.width} ${L.height}`);
    svg.innerHTML = parts.join('');
    this.cells = [...svg.querySelectorAll('[data-cell]')];
    this.shape = shapeOf(m);
  }

  cellSvg(L, lane, step, cls, accentRow = false) {
    const x = stepX(L, step);
    const y = laneY(L, lane) + (accentRow ? 8 : 5);
    const h = (accentRow ? L.accentRow - 14 : L.row - 10);
    const w = L.cell;
    return `<g class="${cls}" data-cell="${lane}:${step}" role="gridcell"><rect class="cap" x="${x}" y="${y}" width="${w}" height="${h}" rx="3.5"/><rect class="cap-shine" x="${x + 2}" y="${y + 1.5}" width="${w - 4}" height="${Math.round(h * 0.32)}" rx="2"/><circle class="lamp" cx="${x + w / 2}" cy="${y + h * 0.62}" r="3.3"/></g>`;
  }

  update() {
    if (!this.shadowRoot?.firstChild) return;
    const m = this.model;
    if (shapeOf(m) !== this.shape) this.build();
    const L = this.layout;
    const $ = (s) => this.$(s);
    $('.title').textContent = this.getAttribute('label') ?? this.getAttribute('pattern') ?? 'Steps';
    for (const b of this.shadowRoot.querySelectorAll('[data-steps]')) b.setAttribute('aria-pressed', String(Number(b.dataset.steps) === m.steps));
    $('.steps').value = String(m.steps);
    const lengthSelect = $('.length');
    lengthSelect.value = String(m.step);
    if (lengthSelect.value !== String(m.step)) {
      // A step length not in the list: shown as its own option.
      const option = document.createElement('option');
      option.value = String(m.step);
      option.textContent = `${m.step} beat`;
      lengthSelect.append(option);
      lengthSelect.value = String(m.step);
    }
    $('.undo').disabled = !m.canUndo;
    $('.redo').disabled = !m.canRedo;
    const notice = this.notice ?? this.playheadProblem ?? '';
    $('.notice').textContent = shortNotice(notice);
    $('.notice').title = notice;
    $('.notice-full').textContent = notice;
    for (const g of this.cells) {
      const [lane, step] = g.dataset.cell.split(':').map(Number);
      let state;
      if (lane < m.lanes.length) {
        state = STATE_CLASS[m.cell(lane, step)];
        g.setAttribute('class', `g${groupOf(step)} ${state}`);
      } else {
        state = accentAt(m.lanes, step) ? 'accent' : 'rest';
        g.setAttribute('class', `accent-row${state === 'accent' ? ' on' : ''}`);
      }
      // A cell's state for assistive technology: selected when it plays, its name saying how.
      const name = lane < m.lanes.length ? m.lanes[lane].name : 'accent';
      g.setAttribute('aria-label', `${name} step ${step + 1}: ${state}`);
      g.setAttribute('aria-selected', String(state !== 'rest'));
      if (lane === this.cursor.lane && step === this.cursor.step) g.setAttribute('aria-current', 'true');
      else g.removeAttribute('aria-current');
    }
    m.lanes.forEach((lane, i) => {
      const knob = this.shadowRoot.querySelector(`[data-knob="${i}"]`);
      if (!knob) return;
      const level = m.level(i);
      const kx = L.left - 24;
      const ky = laneY(L, i) + L.row / 2;
      knob.querySelector('.knob-arc').setAttribute('d', level > 0.001 ? arc(kx, ky, 15, 0, level) : '');
      const a = angle(level);
      const pointer = knob.querySelector('.knob-pointer');
      pointer.setAttribute('x1', kx + Math.cos(a) * 3);
      pointer.setAttribute('y1', ky + Math.sin(a) * 3);
      pointer.setAttribute('x2', kx + Math.cos(a) * 9.5);
      pointer.setAttribute('y2', ky + Math.sin(a) * 9.5);
      knob.setAttribute('aria-label', `${lane.name} level ${Math.round(level * 100)} %`);
    });
    const c = this.$('.cursor');
    const lane = Math.min(this.cursor.lane, Math.max(0, m.lanes.length - 1));
    const step = Math.min(this.cursor.step, m.steps - 1);
    // Left of step 1: the lane's level knob.
    c.setAttribute('x', step < 0 ? L.left - 44 : stepX(L, step) - 3);
    c.setAttribute('y', laneY(L, lane) + 2);
    c.setAttribute('width', step < 0 ? 40 : L.cell + 6);
    c.setAttribute('height', L.row - 4);
    this.setAttribute('aria-label', `${this.getAttribute('label') ?? 'step grid'}: ${m.lanes.length} lanes, ${m.steps} steps`);
    this.updateRun();
  }

  /** Only the running light (cheap: called at a stream's rate). */
  updateRun() {
    if (!this.layout || !this.shadowRoot?.firstChild) return;
    const m = this.model;
    const at = this.runStep === null ? null : ((this.runStep % m.steps) + m.steps) % m.steps;
    if (at === this.shownRun) return;
    this.shownRun = at;
    for (const led of this.shadowRoot.querySelectorAll('[data-run]')) led.setAttribute('class', `run${Number(led.dataset.run) === at ? ' on' : ''}`);
    const column = this.$('.column');
    if (column) {
      column.setAttribute('class', `column${at === null ? '' : ' on'}`);
      if (at !== null) column.setAttribute('x', stepX(this.layout, at) - 2);
    }
  }

}

/** What the SVG is built for: the step count and the lanes' names (any characters). */
function shapeOf(model) {
  return JSON.stringify([model.steps, model.lanes.map((l) => l.name)]);
}

/** The knob's angle for a level: 7 o'clock to 5 o'clock, 300°. */
function angle(level) {
  return ((120 + level * 300) * Math.PI) / 180;
}

/** An arc path on the knob's scale from level a to b. */
function arc(cx, cy, r, a, b) {
  const a0 = angle(a);
  const a1 = angle(b);
  const large = (b - a) * 300 > 180 ? 1 : 0;
  return `M ${cx + Math.cos(a0) * r} ${cy + Math.sin(a0) * r} A ${r} ${r} 0 ${large} 1 ${cx + Math.cos(a1) * r} ${cy + Math.sin(a1) * r}`;
}

/** Throws a PatternError unless `pattern` is a valid pattern of `kind`. */
function checked(pattern, kind) {
  if (pattern?.kind !== kind) throw new PatternError(`expected a "${kind}" pattern, not ${JSON.stringify(pattern?.kind)}`);
  const problems = validatePattern(pattern);
  if (problems.length) throw new PatternError(problems[0].message);
}

customElements.get('signal-step-grid') ?? customElements.define('signal-step-grid', SignalStepGrid);

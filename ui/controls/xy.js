// <signal-xy>: a 2D pad bound to two parameters, with optional spring-back
// to the centre (docs/ui-controls.md). `bind="/x.address /y.address"`.
// Its readout is two boxes, x and y, each taking a typed value.

import { formatValue } from '../lib/format.js';
import { FINE, PAGE_STEP, keyAction } from '../lib/gesture.js';
import { clamp } from '../lib/taper.js';
import { Binding, SignalElement, bindAddresses, bindingEntry, quietly, unsavedMark, unsavedStream } from './base.js';

const SIZE = 160;
const INSET = 8;
const SPAN = SIZE - 2 * INSET;
/** How long the spring takes back to the centre. */
const SPRING_MS = 80;

export class SignalXY extends SignalElement {
  static role = 'group';
  static observedAttributes = ['bind', 'skin', 'min', 'max', 'default', 'taper', 'floor', 'spring', 'label'];
  static css = `
.control { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 4px; }
.art { width: var(--signal-xy-size, 160px); height: var(--signal-xy-size, 160px); cursor: crosshair; }
.pad { fill: var(--signal-panel, #1f232b); stroke: var(--signal-track, #2a2f38); stroke-width: 1; }
.grid { stroke: var(--signal-track, #2a2f38); stroke-width: 1; }
.cross { stroke: var(--signal-accent, #4fc3f7); stroke-width: 1; opacity: 0.5; }
.puck { fill: var(--signal-accent, #4fc3f7); stroke: var(--signal-thumb, #e8ecf2); stroke-width: 2; }
.effective { stroke: none; }
.axes { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
.readout { width: var(--signal-xy-size, 160px); display: flex; }
.readout-x, .readout-y { position: relative; flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.readout-x { text-align: right; }
.readout-y { text-align: left; }
`;

  constructor() {
    super();
    this.newBindings();
    this.dragging = false;
  }

  /** Two bindings sharing one unsaved mark. */
  newBindings() {
    this.x = new Binding(this, null);
    this.y = new Binding(this, null);
    for (const b of [this.x, this.y]) b.onChange = () => this.redrawUnsaved?.();
  }

  bindings() {
    const [x = null, y = null] = bindAddresses(this);
    this.x.address = x;
    this.y.address = y;
  }

  connectedCallback() {
    this.bindings();
    super.connectedCallback();
    if (!this.redrawUnsaved) this.redrawUnsaved = unsavedMark(this, () => [this.x, this.y]);
    if (!this.entries) {
      const y = bindingEntry(this, this.$('.readout-y'), () => this.y);
      const x = bindingEntry(this, this.$('.readout-x'), () => this.x, { next: () => y.open() });
      this.entries = [x, y];
    }
    this.redrawUnsaved();
  }

  rebind() {
    this.flush();
    this.newBindings();
    this.bindings();
    if (this.connection) this.attach(this.connection);
  }

  template() {
    return `<div class="control">
  <div class="face"><svg class="art" viewBox="0 0 ${SIZE} ${SIZE}" part="art">
    <rect class="pad" x="0.5" y="0.5" width="${SIZE - 1}" height="${SIZE - 1}" rx="6"/>
    <line class="grid" x1="${SIZE / 2}" y1="${INSET}" x2="${SIZE / 2}" y2="${SIZE - INSET}"/>
    <line class="grid" x1="${INSET}" y1="${SIZE / 2}" x2="${SIZE - INSET}" y2="${SIZE / 2}"/>
    <line class="cross cx"/><line class="cross cy"/>
    <circle class="effective" r="3"/><circle class="puck" r="7"/>
  </svg></div>
  <div class="skin" hidden></div>
  <div class="label"></div>
  <div class="readout"><span class="readout-x"></span><span class="sep"> · </span><span class="readout-y"></span></div>
  <div class="axes"><div class="axis-x" role="slider" tabindex="-1" aria-label="X"></div><div class="axis-y" role="slider" tabindex="-1" aria-label="Y" aria-orientation="vertical"></div></div>
</div>`;
  }

  /** The pad's x and y positions (0..1, y up) under a pointer. */
  pointerPositions(event) {
    const box = (this.$('.face').hidden ? this.$('.skin') : this.$('.art')).getBoundingClientRect();
    const inset = (INSET / SIZE) * box.width;
    return [
      clamp((event.clientX - box.left - inset) / (box.width - 2 * inset), 0, 1),
      clamp(1 - (event.clientY - box.top - inset) / (box.height - 2 * inset), 0, 1),
    ];
  }

  setup() {
    this.addEventListener('pointerdown', (event) => {
      // A press on the readout is for typing, not on the pad.
      if (event.button !== 0 || event.composedPath?.().includes(this.$('.readout'))) return;
      this.capture(event);
      this.dragging = true;
      this.lastPointer = this.pointerPositions(event);
      if (!event.shiftKey) this.moveBoth(...this.lastPointer, 'ramp');
    });
    this.addEventListener('pointermove', (event) => {
      if (!this.dragging) return;
      const [px, py] = this.pointerPositions(event);
      if (event.shiftKey) {
        // Fine adjust: a tenth of the pointer's movement.
        const [lx, ly] = this.lastPointer;
        this.moveBoth(this.x.position + (px - lx) * FINE, this.y.position + (py - ly) * FINE, 'ramp');
      } else {
        this.moveBoth(px, py, 'ramp');
      }
      this.lastPointer = [px, py];
    });
    const end = () => {
      if (!this.dragging) return;
      this.dragging = false;
      if (this.hasAttribute('spring')) this.springBack();
      else for (const b of [this.x, this.y]) b.setValue(b.value, 'commit');
      this.update();
    };
    this.addEventListener('pointerup', end);
    this.addEventListener('pointercancel', end);
    this.addEventListener('lostpointercapture', end);
    this.addEventListener('dblclick', (event) => {
      // On x's or y's readout (a captured pointer's double-click is the pad's): type that value.
      // (Its readouts tell their own double-click from their presses, which
      // the pad never captures; an open field's double-click is the field's.)
      if (this.entries?.some((entry) => entry.isOpen || (entry.at(event) && entry.open()))) return;
      this.x.reset();
      this.y.reset();
      this.update();
    });
  }

  moveBoth(px, py, how) {
    this.x.moveTo(clamp(px, 0, 1), how);
    this.y.moveTo(clamp(py, 0, 1), how);
    this.update();
  }

  /** Back to the centre of the pad, ramped, and committed there. */
  springBack() {
    for (const b of [this.x, this.y]) {
      b.moveTo(0.5, 'local');
      const connection = this.connection;
      if (connection && b.address) {
        quietly(connection.ramp(b.address, b.base, { ms: SPRING_MS }));
        b.commit(b.base);
      }
    }
    this.update();
  }

  streams(connection) {
    const addresses = [this.x.address, this.y.address].filter(Boolean);
    if (addresses.length === 0) return [];
    return [
      connection.subscribe({ stream: 'parameters', addresses, rate: 30 }, (frame) => {
        const [first, second] = frame.values;
        if (this.x.address) this.x.frame(first);
        if (this.y.address) this.y.frame(this.x.address ? second : first);
        this.update();
      }),
      unsavedStream(connection, () => [this.x, this.y], () => this.redrawUnsaved?.()),
    ];
  }

  described(description) {
    this.x.describe(description);
    this.y.describe(description);
    this.update();
  }

  onKey(event) {
    const fine = { fine: event.shiftKey };
    const key = event.key;
    if (key === 'ArrowLeft' || key === 'ArrowRight') {
      this.x.moveTo(keyAction(key, this.x.position, fine), 'nudge');
    } else if (key === 'ArrowUp' || key === 'ArrowDown') {
      this.y.moveTo(keyAction(key, this.y.position, fine), 'nudge');
    } else if (key === 'PageUp' || key === 'PageDown') {
      const axis = event.altKey ? this.x : this.y;
      const step = (key === 'PageUp' ? PAGE_STEP : -PAGE_STEP) * (event.shiftKey ? FINE : 1);
      axis.moveTo(clamp(axis.position + step, 0, 1), 'nudge');
    } else if (key === 'Home' || key === 'End') {
      this.x.moveTo(key === 'Home' ? 0 : 1, 'nudge');
      this.y.moveTo(key === 'Home' ? 0 : 1, 'nudge');
    } else if (key === 'Delete' || key === 'Backspace') {
      this.x.reset();
      this.y.reset();
    } else if (key === 'Enter') {
      return this.entries?.[0].open() ?? false;
    } else if (key === 'Escape' && this.hasAttribute('spring')) {
      this.springBack();
    } else {
      return false;
    }
    this.update();
    return true;
  }

  flush() {
    this.x.flush();
    this.y.flush();
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const { x, y } = this;
    const label = this.getAttribute('label') ?? '';
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label || `${x.address ?? 'x'} and ${y.address ?? 'y'}`);
    this.setAttribute('aria-keyshortcuts', 'ArrowLeft ArrowRight ArrowUp ArrowDown PageUp PageDown Home End Delete Enter');
    for (const [binding, element] of [[x, this.$('.axis-x')], [y, this.$('.axis-y')]]) {
      const { min, max } = binding.range;
      element.setAttribute('aria-valuemin', String(min));
      element.setAttribute('aria-valuemax', String(max));
      element.setAttribute('aria-valuenow', String(binding.value));
      element.setAttribute('aria-valuetext', binding.ariaText());
    }
    this.$('.label').textContent = label;
    const [xEntry, yEntry] = this.entries ?? [];
    if (!xEntry?.isOpen) this.$('.readout-x').textContent = formatValue(x.value, x.range.unit);
    if (!yEntry?.isOpen) this.$('.readout-y').textContent = formatValue(y.value, y.range.unit);
    if (this.showSkin({ position: x.position, x: x.position, y: y.position })) return;
    const px = INSET + x.position * SPAN;
    const py = INSET + (1 - y.position) * SPAN;
    const puck = this.$('.puck');
    puck.setAttribute('cx', px);
    puck.setAttribute('cy', py);
    const set = (line, x1, y1, x2, y2) => Object.entries({ x1, y1, x2, y2 }).forEach(([k, v]) => line.setAttribute(k, v));
    set(this.$('.cx'), px, INSET, px, SIZE - INSET);
    set(this.$('.cy'), INSET, py, SIZE - INSET, py);
    const effective = this.$('.effective');
    const modulated = x.modulated || y.modulated;
    effective.style.display = modulated ? '' : 'none';
    if (modulated) {
      const ex = Number.isFinite(x.effective) ? x.effectivePosition : x.position;
      const ey = Number.isFinite(y.effective) ? y.effectivePosition : y.position;
      effective.setAttribute('cx', INSET + ex * SPAN);
      effective.setAttribute('cy', INSET + (1 - ey) * SPAN);
    }
  }
}

customElements.get('signal-xy') ?? customElements.define('signal-xy', SignalXY);

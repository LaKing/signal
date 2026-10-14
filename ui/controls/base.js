// What every control shares (docs/ui-controls.md): the connection, the
// `bind` attribute, the `skin` attribute, focus, keys, pointer capture and
// the theme; and, for the controls that edit a value, the binding to one
// parameter (live edits, debounced commits, ARIA values).

import { findParameter, onDefaultConnection } from '../signal-ui.js';
import { formatValue } from '../lib/format.js';
import { dragDelta, keyAction, resetValue, wheelDelta } from '../lib/gesture.js';
import { parseSkin } from '../lib/skin.js';
import { toPosition, toValue } from '../lib/taper.js';
import { loadSkin, mountSkin, showSkin } from './skin-dom.js';

/** Keyboard and wheel edits are committed once this long after the last one. */
export const COMMIT_DELAY_MS = 400;

/** The ramp a drag sends with each move. */
export const RAMP_MS = 5;

/** The dark default theme; every value can be overridden by a custom property. */
export const BASE_CSS = `
:host {
  display: inline-block;
  position: relative;
  box-sizing: border-box;
  outline: none;
  touch-action: none;
  user-select: none;
  -webkit-user-select: none;
  -webkit-tap-highlight-color: transparent;
  color: var(--signal-fg, #d6dbe4);
  font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif);
  border-radius: var(--signal-radius, 6px);
  vertical-align: top;
}
:host(:focus-visible) {
  box-shadow: 0 0 0 2px var(--signal-focus, #5aa9ff);
}
:host([disabled]) { opacity: 0.4; pointer-events: none; }
[hidden] { display: none !important; }
svg { display: block; overflow: visible; }
.label, .readout {
  text-align: center;
  white-space: nowrap;
  color: var(--signal-muted, #8a93a3);
  font-variant-numeric: tabular-nums;
}
.readout { color: var(--signal-fg, #d6dbe4); min-height: 1.3em; }
.skin { display: flex; justify-content: center; }
.track { fill: var(--signal-track, #2a2f38); }
.fill { fill: var(--signal-accent, #4fc3f7); }
.effective { fill: var(--signal-effective, #ffb74d); stroke: var(--signal-effective, #ffb74d); }
.thumb { fill: var(--signal-thumb, #e8ecf2); stroke: var(--signal-bg, #16181d); stroke-width: 1; }
`;

/** A request whose refusal shows in the next frame, not as a console error. */
export function quietly(promise) {
  promise?.catch?.(() => {});
}

/** A numeric attribute, or undefined when absent or not a number. */
export function numberAttribute(element, name) {
  const text = element.getAttribute(name);
  if (text === null || text.trim() === '') return undefined;
  const value = Number(text);
  return Number.isFinite(value) ? value : undefined;
}

/** The addresses in a `bind` attribute (space or comma separated). */
export function bindAddresses(element) {
  return (element.getAttribute('bind') ?? '').split(/[\s,]+/).filter(Boolean);
}

/**
 * A custom element with a shadow root, the page's (or a given) connection,
 * a skin and key handling. Subclasses give `template()`, `update()`, and
 * optionally `streams(connection)`, `described(description)`, `onKey(event)`
 * and `flush()`.
 */
export class SignalElement extends HTMLElement {
  static role = 'group';
  static css = '';
  static observedAttributes = ['bind', 'skin'];
  /** Observed attributes that change a subscription; the others only redraw. */
  static resubscribe = [];

  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this.connection = null;
    this.ownConnection = null;
    this.stops = [];
    this.skin = null;
    this.skinToken = null;
  }

  /** Uses `connection` instead of the page's default one. */
  useConnection(connection) {
    this.ownConnection = connection;
    if (this.isConnected) this.attach(connection);
  }

  $(selector) {
    return this.shadowRoot.querySelector(selector);
  }

  connectedCallback() {
    if (!this.hasAttribute('tabindex')) this.tabIndex = 0;
    if (!this.hasAttribute('role')) this.setAttribute('role', this.constructor.role);
    if (!this.shadowRoot.firstChild) {
      this.shadowRoot.innerHTML = `<style>${BASE_CSS}${this.constructor.css}</style>${this.template()}`;
      this.addEventListener('keydown', (event) => {
        if (this.onKey(event)) {
          event.preventDefault();
          event.stopPropagation();
        }
      });
      this.addEventListener('blur', () => this.flush());
      this.setup();
    }
    this.onPageHide = () => this.flush();
    this.onVisibility = () => {
      if (document.visibilityState === 'hidden') this.flush();
    };
    window.addEventListener('pagehide', this.onPageHide);
    document.addEventListener('visibilitychange', this.onVisibility);
    if (this.ownConnection) this.attach(this.ownConnection);
    else this.stopDefault = onDefaultConnection((connection) => this.attach(connection));
    this.loadSkin();
    this.update();
  }

  disconnectedCallback() {
    this.flush();
    this.detach();
    this.stopDefault?.();
    this.stopDefault = null;
    window.removeEventListener('pagehide', this.onPageHide);
    document.removeEventListener('visibilitychange', this.onVisibility);
  }

  attributeChangedCallback(name, previous, value) {
    if (!this.isConnected || previous === value) return;
    if (name === 'skin') this.loadSkin();
    else if (name === 'bind') this.rebind();
    else if (this.constructor.resubscribe.includes(name) && this.connection) this.attach(this.connection);
    this.update();
  }

  /** `bind` changed: commit what is pending, then follow the new address. */
  rebind() {
    this.flush();
    if (this.connection) this.attach(this.connection);
  }

  attach(connection) {
    this.detach();
    this.connection = connection;
    if (!connection) return;
    this.stops.push(connection.on('describe', (description) => this.described(description)));
    if (connection.description) this.described(connection.description);
    this.stops.push(...this.streams(connection));
    this.update();
  }

  detach() {
    for (const stop of this.stops.splice(0)) stop();
    this.connection = null;
  }

  async loadSkin() {
    const attribute = this.getAttribute('skin');
    const token = {};
    this.skinToken = token;
    let spec = null;
    try {
      spec = parseSkin(attribute);
    } catch (error) {
      console.info(`<${this.localName}> skin="${attribute}": ${error.message}; drawing the default`);
    }
    let skin = null;
    if (spec) {
      try {
        skin = await loadSkin(spec, document.baseURI);
      } catch (error) {
        // From a file, a browser may refuse to fetch the skin (docs/ui-controls.md).
        console.info(`<${this.localName}> skin "${attribute}" did not load (${error.message}); drawing the default`);
      }
    }
    if (token !== this.skinToken) return;
    this.skin = skin;
    const container = this.$('.skin');
    const face = this.$('.face');
    if (container && face) {
      if (skin) mountSkin(container, skin);
      else container.replaceChildren();
      container.hidden = !skin;
      face.hidden = Boolean(skin);
    }
    this.update();
  }

  /** Shows `state` with the skin; false when there is none (draw the default). */
  showSkin(state) {
    if (!this.skin) return false;
    showSkin(this.$('.skin'), this.skin, state);
    return true;
  }

  /** Captures the pointer (mouse, pen or touch) for a drag. */
  capture(event) {
    this.focus({ preventScroll: true });
    try {
      this.setPointerCapture(event.pointerId);
    } catch {
      // A synthetic or already released pointer: the drag still works.
    }
  }

  template() {
    return '';
  }

  setup() {}

  update() {}

  described() {}

  streams() {
    return [];
  }

  onKey() {
    return false;
  }

  flush() {}
}

/**
 * One parameter a control edits: its range (from `describe`, with the
 * element's `min`, `max`, `default`, `unit`, `taper` and `floor` attributes
 * over it), its base and effective values (from a `parameters`
 * subscription), and the edits: a ramp per drag move and a commit on
 * release, or a live `set` per key or wheel step and one commit
 * `COMMIT_DELAY_MS` after the last.
 */
export class Binding {
  constructor(element, address) {
    this.element = element;
    this.address = address;
    this.described = null;
    this.base = NaN;
    this.effective = NaN;
    this.timer = null;
    this.pending = false;
  }

  get range() {
    const e = this.element;
    const d = this.described ?? {};
    return {
      min: numberAttribute(e, 'min') ?? d.min ?? 0,
      max: numberAttribute(e, 'max') ?? d.max ?? 1,
      default: numberAttribute(e, 'default') ?? d.default,
      unit: e.getAttribute('unit') ?? d.unit ?? null,
      taper: e.getAttribute('taper') ?? 'linear',
      floorDb: numberAttribute(e, 'floor'),
    };
  }

  /** The value shown: the base, else the default. */
  get value() {
    return Number.isFinite(this.base) ? this.base : resetValue(this.range);
  }

  get position() {
    return toPosition(this.value, this.range);
  }

  get effectivePosition() {
    return toPosition(this.effective, this.range);
  }

  /** Whether the effective value differs visibly from the base (modulated). */
  get modulated() {
    return Number.isFinite(this.effective) && Math.abs(this.effectivePosition - this.position) > 0.002;
  }

  describe(description) {
    this.described = findParameter(description, this.address);
    if (this.described && !this.pending && !this.element.dragging) {
      const base = this.described.base ?? this.described.default;
      if (Number.isFinite(base)) this.base = base;
    }
  }

  /** A parameter frame's `{base, effective}` (NaN until the runner reported it). */
  frame({ base, effective }) {
    if (!this.pending && !this.element.dragging && Number.isFinite(base)) this.base = base;
    this.effective = effective;
  }

  /**
   * Moves to `position`: `how` is `ramp` (drag), `nudge` (key, wheel),
   * `commit`, or `local` (shown only; the caller sends).
   */
  moveTo(position, how) {
    const value = toValue(position, this.range);
    this.setValue(value, how);
  }

  setValue(value, how) {
    this.base = value;
    const connection = this.element.connection;
    if (how === 'local') this.cancel();
    if (!connection || !this.address || how === 'local') return;
    if (how === 'ramp') {
      quietly(connection.ramp(this.address, value, { ms: RAMP_MS }));
    } else if (how === 'nudge') {
      quietly(connection.set(this.address, value));
      this.pending = true;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.flush(), COMMIT_DELAY_MS);
    } else {
      this.cancel();
      quietly(connection.commit(this.address, value));
    }
  }

  /** Commits now what a debounced edit left pending. */
  flush() {
    if (!this.pending) return;
    this.cancel();
    if (this.element.connection && this.address) quietly(this.element.connection.commit(this.address, this.base));
  }

  cancel() {
    clearTimeout(this.timer);
    this.timer = null;
    this.pending = false;
  }

  reset() {
    this.setValue(resetValue(this.range), 'commit');
  }

  ariaText() {
    const unit = this.range.unit;
    const text = formatValue(this.value, unit);
    return this.modulated ? `${text}, modulated to ${formatValue(this.effective, unit)}` : text;
  }
}

/**
 * A control that edits one parameter (`bind`): ARIA slider values, the
 * keys (arrows, Page, Home, End; Shift fine; Delete resets), the wheel,
 * double-click to reset and drags. Subclasses give `dragStart(event)` and
 * `dragMove(event)` returning a new position.
 */
export class SignalValueElement extends SignalElement {
  static role = 'slider';
  static observedAttributes = ['bind', 'skin', 'min', 'max', 'default', 'unit', 'taper', 'floor', 'label'];

  constructor() {
    super();
    this.binding = new Binding(this, null);
    this.dragging = false;
  }

  rebind() {
    this.binding.flush();
    this.binding = new Binding(this, bindAddresses(this)[0] ?? null);
    if (this.connection) this.attach(this.connection);
  }

  connectedCallback() {
    if (!this.binding.address) this.binding.address = bindAddresses(this)[0] ?? null;
    super.connectedCallback();
  }

  setup() {
    this.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      this.capture(event);
      this.dragging = true;
      this.moved = false;
      this.dragStart(event);
    });
    this.addEventListener('pointermove', (event) => {
      if (!this.dragging) return;
      const position = this.dragMove(event);
      if (position !== null && position !== undefined) {
        this.moved = true;
        this.binding.moveTo(position, 'ramp');
        this.update();
      }
    });
    const end = () => {
      if (!this.dragging) return;
      this.dragging = false;
      this.dragEnd();
      this.update();
    };
    this.addEventListener('pointerup', end);
    this.addEventListener('pointercancel', end);
    this.addEventListener('lostpointercapture', end);
    this.addEventListener('dblclick', () => {
      this.binding.reset();
      this.update();
    });
    this.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
  }

  /** A drag ended: commit where it is (a click without a move commits nothing). */
  dragEnd() {
    if (this.moved) this.binding.setValue(this.binding.value, 'commit');
  }

  onWheel(event) {
    event.preventDefault();
    const delta = wheelDelta(event.deltaY, { fine: event.shiftKey, deltaMode: event.deltaMode });
    this.binding.moveTo(this.binding.position + delta, 'nudge');
    this.update();
  }

  streams(connection) {
    if (!this.binding.address) return [];
    return [connection.subscribe({ stream: 'parameters', addresses: [this.binding.address], rate: 30 }, (frame) => {
      this.binding.frame(frame.values[0]);
      this.update();
    })];
  }

  described(description) {
    this.binding.describe(description);
    this.update();
  }

  onKey(event) {
    if (event.key === 'Delete' || event.key === 'Backspace') {
      this.binding.reset();
      this.update();
      return true;
    }
    const position = keyAction(event.key, this.binding.position, { fine: event.shiftKey });
    if (position === null) return false;
    this.binding.moveTo(position, 'nudge');
    this.update();
    return true;
  }

  flush() {
    this.binding.flush();
  }

  /** The position change for `pixels` of relative drag over `travel` pixels. */
  relative(pixels, event, travel) {
    return this.binding.position + dragDelta(pixels, { fine: event.shiftKey, travel });
  }

  /** Sets the ARIA value attributes (and the label) from the binding. */
  updateAria() {
    const { min, max } = this.binding.range;
    this.setAttribute('aria-valuemin', String(min));
    this.setAttribute('aria-valuemax', String(max));
    this.setAttribute('aria-valuenow', String(this.binding.value));
    this.setAttribute('aria-valuetext', this.binding.ariaText());
    const label = this.getAttribute('label') ?? this.binding.address;
    if (label && !this.hasAttribute('aria-label')) this.setAttribute('aria-label', label);
  }
}

// What every control shares (docs/ui-controls.md): the connection, the
// `bind` attribute, the `skin` attribute, focus, keys, pointer capture and
// the theme; and, for the controls that edit a value, the binding to one
// parameter (live edits, debounced commits, ARIA values, lib/binding.js) and
// its unsaved mark, and the typed value entry of its readout
// (controls/entry-field.js).

import { onDefaultConnection } from '../signal-ui.js';
import { Binding } from '../lib/binding.js';
import { dragDelta, stepAction, wheelDelta } from '../lib/gesture.js';
import { keyCommand, wheelCommand } from '../lib/keys.js';
import { registerWheel, takesWheel, verticalDelta } from './keyboard.js';

export { registerWheel, takesWheel, verticalDelta } from './keyboard.js';
import { parseSkin } from '../lib/skin.js';
import { loadSkin, mountSkin, showSkin } from './skin-dom.js';
import { ENTRY_CSS, ValueEntry, entryDisabled } from './entry-field.js';
import { ClickPair, READOUT_SLACK_PX } from '../lib/entry.js';

export { Binding, COMMIT_DELAY_MS, RAMP_MS, numberAttribute, quietly } from '../lib/binding.js';

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
/* Stable layout (docs/ui-controls.md): a readout is a box of fixed size
   whatever its text, so a changing value never moves anything else. */
.label { overflow: hidden; text-overflow: ellipsis; }
.readout {
  position: relative;
  color: var(--signal-fg, #d6dbe4);
  box-sizing: border-box;
  width: var(--signal-readout-width, 11ch);
  height: 1.3em;
  margin: 0 auto;
  overflow: hidden;
  white-space: pre;
  text-overflow: ellipsis;
}
.skin { display: flex; justify-content: center; }
.track { fill: var(--signal-track, #2a2f38); }
.fill { fill: var(--signal-accent, #4fc3f7); }
.effective { fill: var(--signal-effective, #ffb74d); stroke: var(--signal-effective, #ffb74d); }
.thumb { fill: var(--signal-thumb, #e8ecf2); stroke: var(--signal-bg, #16181d); stroke-width: 1; }
:host([unsaved]) { outline: 1px dashed var(--signal-unsaved, #ffb74d); outline-offset: 2px; }
/* Over what is below the control, never in its flow: showing it moves nothing. */
.unsaved-mark {
  position: absolute;
  z-index: 1;
  top: 100%;
  left: 50%;
  transform: translateX(-50%);
  box-sizing: border-box;
  width: 14em;
  padding: 3px 4px;
  border-radius: 4px;
  background: var(--signal-panel, #1f232b);
  text-align: center;
  white-space: normal;
  font-size: 0.9em;
  color: var(--signal-unsaved, #ffb74d);
}
.unsaved-mark button {
  margin: 3px 2px 0;
  padding: 0 6px;
  font: inherit;
  color: var(--signal-fg, #d6dbe4);
  background: var(--signal-track, #2a2f38);
  border: 1px solid var(--signal-unsaved, #ffb74d);
  border-radius: 3px;
  cursor: pointer;
}
${ENTRY_CSS}`;

/** The addresses in a `bind` attribute (space or comma separated). */
export function bindAddresses(element) {
  return (element.getAttribute('bind') ?? '').split(/[\s,]+/).filter(Boolean);
}

/**
 * The unsaved mark of a control that edits `bindings()` (docs/ui-controls.md
 * "Unsaved"): the `unsaved` attribute, the error text and Retry and Revert,
 * shown while one of them has a refused write. Otherwise it is the control's
 * message place for `element.entryNote` (a typed value's clamp, "Value
 * entry"), without the buttons. Returns the function that redraws it.
 */
export function unsavedMark(element, bindings) {
  const mark = document.createElement('div');
  mark.className = 'unsaved-mark';
  mark.setAttribute('role', 'alert');
  mark.hidden = true;
  const text = document.createElement('span');
  const button = (label, act) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = label;
    // Not a drag of the control, nor its keys.
    b.addEventListener('pointerdown', (event) => event.stopPropagation());
    b.addEventListener('keydown', (event) => event.stopPropagation());
    b.addEventListener('click', (event) => {
      event.stopPropagation();
      for (const binding of bindings()) if (binding.unsaved) act(binding);
    });
    return b;
  };
  const parts = [document.createElement('br'), button('Retry', (b) => b.retry()), button('Revert', (b) => b.revert())];
  mark.append(text, ...parts);
  element.shadowRoot.append(mark);
  return () => {
    const error = bindings().find((b) => b.unsaved)?.unsaved?.error ?? null;
    const note = error ? null : (element.entryNote ?? null);
    element.toggleAttribute('unsaved', Boolean(error));
    mark.hidden = !error && !note;
    for (const part of parts) part.hidden = !error;
    text.textContent = error ? `Unsaved: ${error}` : (note ?? '');
  };
}

/** Each binding's unsaved state from the host's `unsaved` pushes; returns the stop. */
export function unsavedStream(connection, bindings, redraw) {
  const show = (state) => {
    for (const binding of bindings()) binding.fromHost(state);
    redraw();
  };
  if (connection.unsaved) show(connection.unsaved);
  return connection.on('unsaved', show);
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
 * The value entry of `readout` for `binding`, on `element`: a typed value is
 * sent as a drag's release is (a short ramp, then a commit), the clamp
 * reason shown in the control's message place. Off under `no-entry`, and for
 * a parameter that is not a control (`commit` would refuse it).
 */
export function bindingEntry(element, readout, binding, options = {}) {
  return new ValueEntry(readout, {
    spec: () => {
      const range = binding().range;
      return { unit: range.unit, range, taper: range.taper, kind: binding().described?.kind ?? null };
    },
    value: () => binding().value,
    apply: (value) => {
      binding().enter(value);
      element.update();
    },
    note: (text) => {
      element.entryNote = text;
      element.redrawUnsaved?.();
    },
    closed: () => element.update(),
    focus: () => element.focus({ preventScroll: true }),
    enabled: () => !entryDisabled(element) && binding().described?.control !== false,
    ...options,
  });
}

/**
 * A control that edits one parameter (`bind`): ARIA slider values, the
 * keys (arrows, Page, Home, End; Shift fine; Delete resets; Enter types a
 * value), the wheel, double-click to reset (on the readout: type a value)
 * and drags. Subclasses give `dragStart(event)` and `dragMove(event)`
 * returning a new position.
 */
export class SignalValueElement extends SignalElement {
  static role = 'slider';
  static observedAttributes = ['bind', 'skin', 'min', 'max', 'default', 'unit', 'taper', 'floor', 'label'];
  /** Whether it commits, and so shows the unsaved mark. */
  static commits = true;
  /** The readout a typed value replaces. */
  static readout = '.readout';
  /** Whether the readout tells a double-click from its own presses (false: `setup` does). */
  static entryPointer = false;

  constructor() {
    super();
    this.binding = this.newBinding(null);
    this.dragging = false;
  }

  newBinding(address) {
    const binding = new Binding(this, address);
    binding.onChange = () => this.redrawUnsaved?.();
    return binding;
  }

  rebind() {
    this.binding.flush();
    this.binding = this.newBinding(bindAddresses(this)[0] ?? null);
    if (this.connection) this.attach(this.connection);
  }

  connectedCallback() {
    if (!this.binding.address) this.binding.address = bindAddresses(this)[0] ?? null;
    super.connectedCallback();
    if (this.constructor.commits && !this.redrawUnsaved) this.redrawUnsaved = unsavedMark(this, () => [this.binding]);
    // A control without a readout (the toggle) takes nothing typed.
    const readout = this.$(this.constructor.readout);
    // Its presses are the control's (it captures the pointer), so the
    // control tells the double-click (`setup`), unless it has no drag
    // (`<signal-value>`: the readout tells its own).
    if (readout) this.entry ??= bindingEntry(this, readout, () => this.binding, { pointer: this.constructor.entryPointer });
    this.redrawUnsaved?.();
  }

  setup() {
    // A double-click is told from the presses (`ClickPair`), not from
    // `dblclick` alone: the pointer is captured on a press, and what a
    // browser makes of `dblclick` then (its target, whether it comes) is its
    // own. Its second press does not drag: on the readout it types a value
    // (on its release), elsewhere it resets (on a release without a move).
    this.clicks = new ClickPair();
    this.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      const onReadout = Boolean(this.entry?.at(event));
      // A pair seen here: the browser's `dblclick` that follows is not acted on again.
      this.paired = this.clicks.down(event);
      this.second = this.paired ? (onReadout ? 'entry' : 'reset') : null;
      this.capture(event);
      if (this.second === 'entry') return;
      this.dragging = true;
      this.moved = false;
      // A press on the readout drags only once it has moved a little, so a
      // double-click's first press never moves the value.
      this.slack = onReadout ? { x: event.clientX, y: event.clientY } : null;
      if (!this.slack) this.dragStart(event);
    });
    this.addEventListener('pointermove', (event) => {
      if (!this.dragging) return;
      if (this.slack) {
        if (Math.hypot(event.clientX - this.slack.x, event.clientY - this.slack.y) < READOUT_SLACK_PX) return;
        this.slack = null;
        this.clicks.reset();
        this.dragStart(event);
        return;
      }
      const position = this.dragMove(event);
      if (position !== null && position !== undefined) {
        this.moved = true;
        this.clicks.reset();
        this.binding.moveTo(position, 'ramp');
        this.update();
      }
    });
    const end = () => {
      if (!this.dragging) return;
      this.dragging = false;
      this.slack = null;
      this.dragEnd();
      this.update();
    };
    this.addEventListener('pointerup', () => {
      const second = this.second;
      this.second = null;
      const moved = this.dragging && this.moved;
      end();
      if (second === 'entry') this.typeOrReset();
      else if (second === 'reset' && !moved) this.resetValue();
    });
    this.addEventListener('pointercancel', () => {
      this.second = null;
      end();
    });
    this.addEventListener('lostpointercapture', end);
    this.addEventListener('dblclick', (event) => {
      // The pair was seen as presses (above), or an open field's.
      if (this.paired || this.entry?.isOpen) {
        this.paired = false;
        return;
      }
      // No presses came (a double-click made by a script or an assistive
      // tool): on the readout (a captured pointer's double-click is the
      // control's) type a value, elsewhere reset.
      if (this.entry?.at(event)) this.typeOrReset();
      else this.resetValue();
    });
    this.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
    registerWheel(this);
  }

  /** A double-click on the readout: a typed value, or (entry off) a reset. */
  typeOrReset() {
    if (!this.entry?.open()) this.resetValue();
  }

  resetValue() {
    this.binding.reset();
    this.update();
  }

  /** A drag ended: commit where it is (a click without a move commits nothing). */
  dragEnd() {
    if (this.moved) this.binding.setValue(this.binding.value, 'commit');
  }

  /**
   * The wheel changes the value only while the control owns it (a click or
   * the focus, or the wheel modifier held: ADR 0034); otherwise the page
   * scrolls. Shift is finer (`slider.wheel`).
   */
  onWheel(event) {
    if (!takesWheel(this, event)) return;
    event.preventDefault();
    const command = wheelCommand('slider', event);
    if (command?.id !== 'slider.wheel') return;
    const delta = wheelDelta(verticalDelta(event), { fine: command.fine, deltaMode: event.deltaMode });
    if (!delta) return;
    this.binding.moveTo(this.binding.position + delta, 'nudge');
    this.update();
  }

  streams(connection) {
    if (!this.binding.address) return [];
    return [
      connection.subscribe({ stream: 'parameters', addresses: [this.binding.address], rate: 30 }, (frame) => {
        this.binding.frame(frame.values[0]);
        this.update();
      }),
      unsavedStream(connection, () => [this.binding], () => this.redrawUnsaved?.()),
    ];
  }

  described(description) {
    this.binding.describe(description);
    this.update();
  }

  /** The `slider` scope's keys (ui/lib/keys.default.js). */
  onKey(event) {
    const command = keyCommand('slider', event);
    if (!command) return false;
    if (command.id === 'slider.type_value') return this.entry?.open() ?? false;
    if (command.id === 'slider.reset') {
      this.binding.reset();
      this.update();
      return true;
    }
    const position = stepAction(command.id, this.binding.position, { fine: command.fine });
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

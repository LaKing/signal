// <signal-button> (momentary: sends `trigger` to an event input) and
// <signal-toggle> (a parameter at its min or max) (docs/ui-controls.md).

import { SignalElement, SignalValueElement, bindAddresses, quietly } from './base.js';
import { keyCommand } from '../lib/keys.js';

const CSS = `
.control { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 4px; }
.art { width: var(--signal-button-width, 56px); height: var(--signal-button-height, 32px); cursor: pointer; }
.cap { fill: var(--signal-knob, #262b34); stroke: var(--signal-knob-edge, #3a414d); stroke-width: 1.5; transition: fill 60ms; }
.lamp { fill: var(--signal-track, #2a2f38); }
.on .cap { fill: var(--signal-knob-on, #2f3a48); }
.on .lamp { fill: var(--signal-accent, #4fc3f7); }
`;

const ART = `<svg class="art" viewBox="0 0 56 32" part="art"><rect class="cap" x="1" y="1" width="54" height="30" rx="5"/><rect class="lamp" x="18" y="12" width="20" height="8" rx="2"/></svg>`;

export class SignalButton extends SignalElement {
  static role = 'button';
  static observedAttributes = ['bind', 'skin', 'label'];
  static css = CSS;

  constructor() {
    super();
    this.pressed = false;
  }

  template() {
    return `<div class="control"><div class="face">${ART}</div><div class="skin" hidden></div><div class="label"></div></div>`;
  }

  setup() {
    this.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      this.capture(event);
      this.press();
    });
    for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) this.addEventListener(name, () => this.release());
    this.addEventListener('keyup', (event) => {
      if (keyCommand('button', event)?.id === 'button.press') this.release();
    });
    this.addEventListener('blur', () => this.release());
  }

  /** Down: one `trigger` per press (key repeat does not send again). */
  press() {
    if (this.pressed) return;
    this.pressed = true;
    const address = bindAddresses(this)[0];
    if (this.connection && address) quietly(this.connection.trigger(address));
    this.dispatchEvent(new CustomEvent('signal-trigger', { bubbles: true, detail: { address } }));
    this.update();
  }

  release() {
    if (!this.pressed) return;
    this.pressed = false;
    this.update();
  }

  /** The `button` scope's keys (ui/lib/keys.default.js). */
  onKey(event) {
    if (keyCommand('button', event)?.id !== 'button.press') return false;
    if (!event.repeat) this.press();
    return true;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const label = this.getAttribute('label') ?? '';
    this.$('.label').textContent = label;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label || bindAddresses(this)[0] || 'trigger');
    this.setAttribute('aria-pressed', String(this.pressed));
    this.$('.control').classList.toggle('on', this.pressed);
    this.showSkin({ position: this.pressed ? 1 : 0 });
  }
}

export class SignalToggle extends SignalValueElement {
  static role = 'switch';
  static css = CSS;

  template() {
    return `<div class="control"><div class="face">${ART}</div><div class="skin" hidden></div><div class="label"></div></div>`;
  }

  get on() {
    const { min, max } = this.binding.range;
    return this.binding.value >= (min + max) / 2;
  }

  /** Sets min or max; `how` as in `Binding.setValue`. */
  turn(on, how) {
    const { min, max } = this.binding.range;
    this.binding.setValue(on ? max : min, how);
    this.update();
  }

  setup() {
    // A switch flips on click; it neither drags nor takes the wheel.
    this.addEventListener('click', () => {
      this.focus({ preventScroll: true });
      this.turn(!this.on, 'commit');
    });
  }

  /** The `toggle` scope's keys (ui/lib/keys.default.js); Page Up, Page Down, Home and End are the page's. */
  onKey(event) {
    const command = keyCommand('toggle', event)?.id;
    const turns = { 'toggle.flip': !this.on, 'toggle.on': true, 'toggle.off': false };
    if (!(command in turns)) return false;
    if (!event.repeat || command !== 'toggle.flip') this.turn(turns[command], 'nudge');
    return true;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const label = this.getAttribute('label') ?? '';
    this.$('.label').textContent = label;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label || this.binding.address || 'switch');
    this.setAttribute('aria-checked', String(this.on));
    this.$('.control').classList.toggle('on', this.on);
    this.showSkin({ position: this.on ? 1 : 0 });
  }
}

customElements.get('signal-button') ?? customElements.define('signal-button', SignalButton);
customElements.get('signal-toggle') ?? customElements.define('signal-toggle', SignalToggle);

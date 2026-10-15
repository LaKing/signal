// <signal-value>: a parameter's value, formatted with its unit
// (docs/ui-controls.md). Shows the effective value once the runner reported
// it, else the base.

import { formatValue } from '../lib/format.js';
import { clamp } from '../lib/taper.js';
import { SignalValueElement, numberAttribute } from './base.js';

const MAX_DIGITS = 6;

export class SignalValue extends SignalValueElement {
  static role = 'status';
  /** It shows a value and writes none: no unsaved mark. */
  static commits = false;
  static observedAttributes = [...SignalValueElement.observedAttributes, 'digits'];
  static css = `
.value { padding: 2px 6px; font-size: var(--signal-value-size, 13px); font-variant-numeric: tabular-nums; color: var(--signal-fg, #d6dbe4); }
`;

  template() {
    return `<span class="value" part="value"></span>`;
  }

  /** Not a slider: no drags, wheel or reset. */
  setup() {}

  get digits() {
    return this.shownDigits ?? numberAttribute(this, 'digits') ?? null;
  }

  onKey(event) {
    const current = this.digits ?? 3;
    const keys = { ArrowUp: current + 1, ArrowRight: current + 1, ArrowDown: current - 1, ArrowLeft: current - 1, Home: 0, End: MAX_DIGITS };
    if (!(event.key in keys)) return false;
    this.shownDigits = clamp(keys[event.key], 0, MAX_DIGITS);
    this.update();
    return true;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const b = this.binding;
    const value = Number.isFinite(b.effective) ? b.effective : b.value;
    const text = formatValue(value, b.range.unit, { digits: this.digits });
    const element = this.$('.value');
    // Only a changed text is written, so the status region is not re-announced every frame.
    if (element.textContent !== text) element.textContent = text;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', this.getAttribute('label') ?? this.binding.address ?? 'value');
    this.setAttribute('aria-keyshortcuts', 'ArrowUp ArrowDown Home End');
  }
}

customElements.get('signal-value') ?? customElements.define('signal-value', SignalValue);

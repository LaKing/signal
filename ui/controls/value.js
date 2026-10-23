// <signal-value>: a parameter's value, formatted with its unit
// (docs/ui-controls.md). Shows the effective value once the runner reported
// it, else the base. A double-click or Enter types a new base value
// ("Value entry"), committed as the other controls commit.

import { formatValue } from '../lib/format.js';
import { clamp } from '../lib/taper.js';
import { ariaShortcuts, keyCommand } from '../lib/keys.js';
import { SignalValueElement, numberAttribute } from './base.js';

const MAX_DIGITS = 6;

export class SignalValue extends SignalValueElement {
  static role = 'status';
  static readout = '.value';
  /** No drag: the readout tells its own double-click. */
  static entryPointer = true;
  static observedAttributes = [...SignalValueElement.observedAttributes, 'digits'];
  static css = `
.value { position: relative; display: inline-block; box-sizing: border-box; width: var(--signal-value-width, 12ch); padding: 2px 6px; font-size: var(--signal-value-size, 13px); font-variant-numeric: tabular-nums; color: var(--signal-fg, #d6dbe4); text-align: right; white-space: pre; overflow: hidden; text-overflow: ellipsis; }
`;

  template() {
    return `<span class="value" part="value"></span>`;
  }

  /** Not a slider: no drags, wheel or reset. */
  setup() {}

  get digits() {
    return this.shownDigits ?? numberAttribute(this, 'digits') ?? null;
  }

  /** The `value` scope's keys (ui/lib/keys.default.js). */
  onKey(event) {
    const command = keyCommand('value', event)?.id;
    if (command === 'value.type_value') return this.entry?.open() ?? false;
    const current = this.digits ?? 3;
    const digits = { 'value.more_digits': current + 1, 'value.fewer_digits': current - 1, 'value.fewest_digits': 0, 'value.most_digits': MAX_DIGITS };
    if (!(command in digits)) return false;
    this.shownDigits = clamp(digits[command], 0, MAX_DIGITS);
    this.update();
    return true;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const b = this.binding;
    const value = Number.isFinite(b.effective) ? b.effective : b.value;
    const text = formatValue(value, b.range.unit, { digits: this.digits });
    const element = this.$('.value');
    // Only a changed text is written, so the status region is not re-announced
    // every frame; none while a value field is over it.
    if (element.textContent !== text && !this.entry?.isOpen) element.textContent = text;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', this.getAttribute('label') ?? this.binding.address ?? 'value');
    this.setAttribute('aria-keyshortcuts', ariaShortcuts('value'));
  }
}

customElements.get('signal-value') ?? customElements.define('signal-value', SignalValue);

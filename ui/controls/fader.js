// <signal-fader>: a vertical or horizontal fader (docs/ui-controls.md).

import { formatValue } from '../lib/format.js';
import { SignalValueElement } from './base.js';

const LENGTH = 200;
const THICK = 40;
const END = 10; // track inset from each end
const TRAVEL = LENGTH - 2 * END;

export class SignalFader extends SignalValueElement {
  static observedAttributes = [...SignalValueElement.observedAttributes, 'orientation'];
  static css = `
.control { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 4px; }
:host([orientation="horizontal"]) .control { flex-direction: row; }
.art { width: var(--signal-fader-thickness, 40px); height: var(--signal-fader-length, 200px); cursor: ns-resize; }
:host([orientation="horizontal"]) .art { width: var(--signal-fader-length, 200px); height: var(--signal-fader-thickness, 40px); cursor: ew-resize; }
.readout { height: 2.6em; }
`;

  get horizontal() {
    return this.getAttribute('orientation') === 'horizontal';
  }

  template() {
    return `<div class="control">
  <div class="face"><svg class="art" part="art"><rect class="track"/><rect class="fill"/><rect class="effective"/><rect class="thumb" rx="2"/></svg></div>
  <div class="skin" hidden></div>
  <div class="label"></div>
  <div class="readout"></div>
</div>`;
  }

  /** Sets a rect from along-travel and across coordinates. */
  place(rect, alongFrom, alongTo, acrossFrom, acrossTo) {
    const [a0, a1] = [Math.min(alongFrom, alongTo), Math.max(alongFrom, alongTo)];
    const h = this.horizontal;
    rect.setAttribute('x', h ? a0 : acrossFrom);
    rect.setAttribute('y', h ? acrossFrom : a0);
    rect.setAttribute('width', h ? a1 - a0 : acrossTo - acrossFrom);
    rect.setAttribute('height', h ? acrossTo - acrossFrom : a1 - a0);
  }

  /** Along-travel coordinate of a position: up for vertical, right for horizontal. */
  along(position) {
    return this.horizontal ? END + position * TRAVEL : LENGTH - END - position * TRAVEL;
  }

  dragStart(event) {
    this.last = this.horizontal ? event.clientX : event.clientY;
  }

  dragMove(event) {
    const now = this.horizontal ? event.clientX : event.clientY;
    const pixels = this.horizontal ? now - this.last : this.last - now;
    this.last = now;
    const box = (this.skin ? this.$('.skin') : this.$('.art')).getBoundingClientRect();
    const travel = ((this.horizontal ? box.width : box.height) * TRAVEL) / LENGTH;
    return this.relative(pixels, event, travel);
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const b = this.binding;
    const position = b.position;
    this.setAttribute('aria-orientation', this.horizontal ? 'horizontal' : 'vertical');
    this.updateAria();
    this.$('.label').textContent = this.getAttribute('label') ?? '';
    // The effective value on the readout's second line, kept (empty) when not
    // modulated: the readout never changes size. A value field over it keeps it.
    const effectiveText = b.modulated ? `→ ${formatValue(b.effective, b.range.unit)}` : '';
    if (!this.entry?.isOpen) this.$('.readout').textContent = `${formatValue(b.value, b.range.unit)}\n${effectiveText}`;
    if (this.showSkin({ position, text: formatValue(b.value, b.range.unit) })) return;
    const svg = this.$('.art');
    svg.setAttribute('viewBox', this.horizontal ? `0 0 ${LENGTH} ${THICK}` : `0 0 ${THICK} ${LENGTH}`);
    const mid = THICK / 2;
    this.place(this.$('.track'), this.along(0), this.along(1), mid - 2, mid + 2);
    this.place(this.$('.fill'), this.along(0), this.along(position), mid - 2, mid + 2);
    const effective = this.$('.effective');
    effective.style.display = b.modulated ? '' : 'none';
    if (b.modulated) {
      const at = this.along(b.effectivePosition);
      this.place(effective, at - 1.5, at + 1.5, 2, THICK - 2);
    }
    const at = this.along(position);
    this.place(this.$('.thumb'), at - 6, at + 6, mid - 14, mid + 14);
  }
}

customElements.get('signal-fader') ?? customElements.define('signal-fader', SignalFader);

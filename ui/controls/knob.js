// <signal-knob>: a rotary control (docs/ui-controls.md).

import { formatValue } from '../lib/format.js';
import { circularDrag, circularPosition, knobAngle } from '../lib/gesture.js';
import { bipolarOrigin } from '../lib/taper.js';
import { SignalValueElement, numberAttribute } from './base.js';

const C = 32; // centre of the 64 × 64 view box
const R = 24; // arc radius

function point(angle, radius = R) {
  const a = (angle * Math.PI) / 180;
  return `${(C + radius * Math.sin(a)).toFixed(3)} ${(C - radius * Math.cos(a)).toFixed(3)}`;
}

/** An SVG arc between two angles (degrees, 0 at 12 o'clock, clockwise), at `radius`. */
function arc(from, to, radius = R) {
  if (Math.abs(to - from) < 0.01) return '';
  const [a, b] = from < to ? [from, to] : [to, from];
  return `M ${point(a, radius)} A ${radius} ${radius} 0 ${b - a > 180 ? 1 : 0} 1 ${point(b, radius)}`;
}

/** The moving arc's radius (ADR 0051 §6, ADR 0050 §11): outside the knob and its skin's scale. */
const EFFECTIVE_R = R + 6;

export class SignalKnob extends SignalValueElement {
  static observedAttributes = [...SignalValueElement.observedAttributes, 'sweep', 'bipolar', 'drag', 'wheel'];
  static css = `
.control { display: flex; flex-direction: column; align-items: center; gap: 2px; padding: 4px; }
.art { width: var(--signal-knob-size, 64px); height: var(--signal-knob-size, 64px); cursor: ns-resize; }
:host([drag="circular"]) .art { cursor: grab; }
.body { fill: var(--signal-knob, #262b34); stroke: var(--signal-knob-edge, #3a414d); stroke-width: 1.5; }
.ring, .arc { fill: none; stroke-width: 4; stroke-linecap: round; }
.ring { stroke: var(--signal-track, #2a2f38); }
.arc { stroke: var(--signal-accent, #4fc3f7); }
.pointer { stroke: var(--signal-thumb, #e8ecf2); stroke-width: 2.5; stroke-linecap: round; }
.dot { stroke: none; }
.readout { height: 2.6em; }
.dial { position: relative; }
/* A modulated knob (ADR 0051 §6): the effective value as a moving arc from the setting, the pointer staying the setting; over a skin too. */
.mod { position: absolute; left: 50%; top: 0; width: var(--signal-knob-size, 64px); height: var(--signal-knob-size, 64px); transform: translateX(-50%); overflow: visible; pointer-events: none; }
.effective-arc { fill: none; stroke: var(--signal-effective, #ffb74d); stroke-width: 3; stroke-linecap: round; }
`;

  /** The sweep in degrees (10..350, default 270). */
  get sweep() {
    const sweep = numberAttribute(this, 'sweep') ?? 270;
    return Math.min(350, Math.max(10, sweep));
  }

  template() {
    return `<div class="control">
  <div class="dial">
  <div class="face"><svg class="art" viewBox="0 0 64 64" part="art">
    <path class="ring"/><path class="arc"/><circle class="body" cx="32" cy="32" r="17"/>
    <line class="pointer" x1="32" y1="32"/>
  </svg></div>
  <div class="skin" hidden></div>
  <svg class="mod" viewBox="0 0 64 64" aria-hidden="true"><path class="effective-arc"/><circle class="effective dot" r="2.5"/></svg>
  </div>
  <div class="label"></div>
  <div class="readout"></div>
</div>`;
  }

  /** The circular position under the pointer. */
  pointerAngle(event) {
    const box = (this.$('.face').hidden ? this.$('.skin') : this.$('.art')).getBoundingClientRect();
    return circularPosition(event.clientX - (box.left + box.width / 2), event.clientY - (box.top + box.height / 2), this.sweep);
  }

  dragStart(event) {
    this.last = event.clientY;
    this.lastX = event.clientX;
    if (this.getAttribute('drag') === 'circular') this.lastAngle = this.pointerAngle(event);
  }

  dragMove(event) {
    if (this.getAttribute('drag') === 'circular') {
      const angle = this.pointerAngle(event);
      const position = circularDrag(this.binding.position, this.lastAngle, angle, { fine: event.shiftKey });
      this.lastAngle = angle;
      return position;
    }
    // Vertical drag: up increases; sideways movement counts too, so a
    // diagonal drag is not lost.
    const pixels = this.last - event.clientY + (event.clientX - this.lastX);
    this.last = event.clientY;
    this.lastX = event.clientX;
    return this.relative(pixels, event, 200);
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const b = this.binding;
    const sweep = this.sweep;
    const position = b.position;
    const angle = knobAngle(position, sweep);
    this.updateAria();
    this.$('.label').textContent = this.getAttribute('label') ?? '';
    // The effective value on the readout's second line, kept (empty) when not
    // modulated: the readout never changes size. A value field over it keeps it.
    const effectiveText = b.modulated ? `→ ${formatValue(b.effective, b.range.unit)}` : '';
    if (!this.entry?.isOpen) this.$('.readout').textContent = `${formatValue(b.value, b.range.unit)}\n${effectiveText}`;
    // The moving arc and its dot: from the setting to the effective value, while modulated.
    const dot = this.$('.dot');
    const moving = this.$('.effective-arc');
    dot.style.display = b.modulated ? '' : 'none';
    moving.style.display = b.modulated ? '' : 'none';
    if (b.modulated) {
      const to = knobAngle(b.effectivePosition, sweep);
      moving.setAttribute('d', arc(angle, to, EFFECTIVE_R));
      const [x, y] = point(to, EFFECTIVE_R).split(' ');
      dot.setAttribute('cx', x);
      dot.setAttribute('cy', y);
    }
    if (this.showSkin({ position, angle, text: formatValue(b.value, b.range.unit), outOfRange: this.getAttribute('out-of-range') })) return;
    const origin = this.hasAttribute('bipolar') ? knobAngle(bipolarOrigin(b.range), sweep) : -sweep / 2;
    this.$('.ring').setAttribute('d', arc(-sweep / 2, sweep / 2));
    this.$('.arc').setAttribute('d', arc(origin, angle));
    const tip = point(angle, 15).split(' ');
    const pointer = this.$('.pointer');
    pointer.setAttribute('x2', tip[0]);
    pointer.setAttribute('y2', tip[1]);
  }
}

customElements.get('signal-knob') ?? customElements.define('signal-knob', SignalKnob);

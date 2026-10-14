// <signal-scope>: a subscribed signal output on a canvas, in time with a
// trigger level and edge, or X–Y from two outputs (docs/ui-controls.md).

import { formatValue } from '../lib/format.js';
import { clamp } from '../lib/taper.js';
import { alignXY, triggerStart } from '../lib/trigger.js';
import { SignalElement, bindAddresses, numberAttribute } from './base.js';

/** Points shown in time mode, at most: half of what arrived, so a trigger has room. */
const SHOWN = 256;

export class SignalScope extends SignalElement {
  static role = 'group';
  static observedAttributes = ['bind', 'skin', 'mode', 'rate', 'trigger-level', 'trigger-edge', 'label'];
  static resubscribe = ['mode', 'rate'];
  static css = `
:host { display: block; }
.control { display: flex; flex-direction: column; gap: 4px; padding: 4px; }
canvas { width: var(--signal-scope-width, 100%); height: var(--signal-scope-height, 160px); background: var(--signal-panel, #1f232b); border-radius: 4px; display: block; }
:host([mode="xy"]) canvas { width: var(--signal-scope-height, 160px); }
.readout { text-align: left; color: var(--signal-muted, #8a93a3); }
`;

  constructor() {
    super();
    this.blocks = [];
    this.latest = {};
    this.frozen = false;
    this.pending = false;
  }

  get ports() {
    return bindAddresses(this);
  }

  get xy() {
    return (this.getAttribute('mode') ?? (this.ports.length > 1 ? 'xy' : 'time')) === 'xy';
  }

  get level() {
    return numberAttribute(this, 'trigger-level') ?? 0;
  }

  get edge() {
    return this.getAttribute('trigger-edge') === 'falling' ? 'falling' : 'rising';
  }

  template() {
    return `<div class="control"><canvas part="canvas"></canvas><div class="label"></div><div class="readout"></div></div>`;
  }

  streams(connection) {
    this.blocks = [];
    this.latest = {};
    const rate = numberAttribute(this, 'rate') ?? 12000;
    const ports = this.xy ? this.ports.slice(0, 2) : this.ports.slice(0, 1);
    return ports.map((port) =>
      connection.subscribe(
        { stream: 'scope', port, rate },
        (frame) => {
          if (this.frozen) return;
          const block = { firstFrame: frame.firstFrame, decimation: frame.extra, values: frame.values };
          this.latest[port] = block;
          if (port === ports[0]) {
            // Two consecutive blocks for the time display; a gap starts over.
            const previous = this.blocks.at(-1);
            const follows = previous && previous.firstFrame + previous.values.length * previous.decimation === block.firstFrame;
            this.blocks = follows ? [previous, block] : [block];
          }
          this.schedule();
        },
        {
          onSubscribed: () => this.problem(null),
          onError: (error) => this.problem(`${port}: ${error.code ?? error.message}`),
        },
      ),
    );
  }

  problem(text) {
    this.problemText = text;
    this.update();
  }

  /** Draws at the next animation frame, once however many blocks arrive. */
  schedule() {
    if (this.pending) return;
    this.pending = true;
    requestAnimationFrame(() => {
      this.pending = false;
      this.draw();
    });
  }

  onKey(event) {
    const step = event.shiftKey ? 0.001 : 0.01;
    const steps = { ArrowUp: step, ArrowDown: -step, PageUp: 0.1, PageDown: -0.1 };
    if (event.key in steps) this.setLevel(this.level + steps[event.key]);
    else if (event.key === 'Home') this.setLevel(0);
    else if (event.key === 'End') this.setLevel(this.autoLevel());
    else if (event.key === 'e' || event.key === 'E') this.setAttribute('trigger-edge', this.edge === 'rising' ? 'falling' : 'rising');
    else if (event.key === ' ') this.frozen = !this.frozen;
    else return false;
    this.update();
    this.draw();
    return true;
  }

  setLevel(level) {
    this.setAttribute('trigger-level', String(Math.round(clamp(level, -1, 1) * 1000) / 1000));
  }

  /** The middle of the last block's range. */
  autoLevel() {
    const last = this.blocks.at(-1);
    if (!last) return 0;
    let lo = Infinity;
    let hi = -Infinity;
    for (const v of last.values) {
      lo = Math.min(lo, v);
      hi = Math.max(hi, v);
    }
    return (lo + hi) / 2;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const label = this.getAttribute('label') ?? '';
    this.$('.label').textContent = label;
    const what = this.xy ? `X–Y of ${this.ports.slice(0, 2).join(' and ')}` : `${this.ports[0] ?? 'no port'}, trigger ${this.edge} at ${this.level}`;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label ? `${label} scope` : 'scope');
    this.setAttribute('aria-description', what);
    this.setAttribute('aria-keyshortcuts', this.xy ? 'Space' : 'ArrowUp ArrowDown PageUp PageDown Home End E Space');
    const state = this.frozen ? ' · frozen' : '';
    this.$('.readout').textContent = this.problemText ?? (this.xy ? `X–Y${state}` : `trigger ${this.edge === 'rising' ? '↑' : '↓'} ${formatValue(this.level)}${state}`);
    // A changed level or edge shows at once, frozen or not.
    this.schedule();
  }

  colour(name, fallback) {
    return getComputedStyle(this).getPropertyValue(name).trim() || fallback;
  }

  draw() {
    const canvas = this.$('canvas');
    if (!canvas) return;
    const box = canvas.getBoundingClientRect();
    const ratio = globalThis.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(box.width * ratio));
    const height = Math.max(1, Math.round(box.height * ratio));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const g = canvas.getContext('2d');
    g.clearRect(0, 0, width, height);
    g.lineWidth = Math.max(1, ratio);
    g.strokeStyle = this.colour('--signal-track', '#2a2f38');
    g.beginPath();
    g.moveTo(0, height / 2);
    g.lineTo(width, height / 2);
    g.moveTo(width / 2, 0);
    g.lineTo(width / 2, height);
    g.stroke();
    const y = (v) => height / 2 - (clamp(v, -1.1, 1.1) * height) / 2.2;
    g.strokeStyle = this.colour('--signal-accent', '#4fc3f7');
    g.lineWidth = 1.5 * ratio;
    g.beginPath();
    if (this.xy) {
      const [px, py] = this.ports;
      const pair = alignXY(this.latest[px], this.latest[py]);
      if (!pair) return;
      const x = (v) => width / 2 + (clamp(v, -1.1, 1.1) * width) / 2.2;
      pair.xs.forEach((v, i) => (i === 0 ? g.moveTo(x(v), y(pair.ys[i])) : g.lineTo(x(v), y(pair.ys[i]))));
    } else {
      if (this.blocks.length === 0) return;
      const points = this.blocks.length === 1 ? this.blocks[0].values : Float32Array.from([...this.blocks[0].values, ...this.blocks[1].values]);
      const length = Math.min(SHOWN, Math.floor(points.length / 2));
      const start = triggerStart(points, length, { level: this.level, edge: this.edge });
      for (let i = 0; i < length; i++) {
        const px = (i / (length - 1)) * width;
        if (i === 0) g.moveTo(px, y(points[start + i]));
        else g.lineTo(px, y(points[start + i]));
      }
      g.stroke();
      g.strokeStyle = this.colour('--signal-effective', '#ffb74d');
      g.lineWidth = Math.max(1, ratio);
      g.setLineDash([4 * ratio, 4 * ratio]);
      g.beginPath();
      g.moveTo(0, y(this.level));
      g.lineTo(width, y(this.level));
      g.stroke();
      g.setLineDash([]);
      return;
    }
    g.stroke();
  }
}

customElements.get('signal-scope') ?? customElements.define('signal-scope', SignalScope);

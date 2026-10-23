// <signal-meter>: peak and RMS with peak hold and a clip latch, on a dB
// scale, with the M2.5-1 ballistics (lib/ballistics.js) (docs/ui-controls.md).

import { ChannelMeter, blocksFromFrame } from '../lib/ballistics.js';
import { formatValue } from '../lib/format.js';
import { clamp } from '../lib/taper.js';
import { ariaShortcuts, keyCommand } from '../lib/keys.js';
import { findMeter } from '../signal-ui.js';
import { SignalElement, bindAddresses, numberAttribute } from './base.js';

const HEIGHT = 200;
const TOP = 14; // the clip lamp above the bar
const BAR = HEIGHT - TOP;
const DEFAULT_FLOOR = -60;
const FLOORS = [-96, -24];

export class SignalMeter extends SignalElement {
  static role = 'meter';
  static observedAttributes = ['bind', 'skin', 'source', 'channel', 'floor', 'label'];
  static resubscribe = ['source', 'channel'];
  static css = `
.control { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 4px; }
.art { width: var(--signal-meter-width, 44px); height: var(--signal-meter-length, 200px); cursor: pointer; }
.bg { fill: var(--signal-track, #2a2f38); }
.peak { fill: var(--signal-meter-peak, #3ecf8e); opacity: 0.45; }
.rms { fill: var(--signal-meter-rms, #3ecf8e); }
.hot { fill: var(--signal-meter-hot, #e6c84f); }
.held { fill: var(--signal-thumb, #e8ecf2); }
.lamp { fill: var(--signal-track, #2a2f38); }
.clip .lamp { fill: var(--signal-clip, #ff5252); }
.tick { stroke: var(--signal-muted, #8a93a3); stroke-width: 1; }
.tick-label { fill: var(--signal-muted, #8a93a3); font-size: 8px; }
.readout { width: 9ch; }
`;

  constructor() {
    super();
    this.meter = new ChannelMeter(48000);
    this.view = null;
  }

  /** The bottom of the scale, in dB (−96 to −24; −60 by default). */
  get floor() {
    return clamp(this.view ?? numberAttribute(this, 'floor') ?? DEFAULT_FLOOR, FLOORS[0], FLOORS[1]);
  }

  template() {
    return `<div class="control">
  <div class="face"><svg class="art" viewBox="0 0 44 ${HEIGHT}" part="art">
    <rect class="lamp" x="4" y="0" width="16" height="8" rx="2"/>
    <rect class="bg" x="4" y="${TOP}" width="16" height="${BAR}"/>
    <rect class="peak"/><rect class="rms"/><rect class="hot"/><rect class="held" height="2"/>
    <g class="scale"></g>
  </svg></div>
  <div class="skin" hidden></div>
  <div class="label"></div>
  <div class="readout"></div>
</div>`;
  }

  setup() {
    this.addEventListener('click', () => this.clearClip());
  }

  clearClip() {
    this.meter.clearClip();
    this.update();
  }

  /** A dB value's height fraction on the scale (0 at the floor, 1 at 0 dBFS). */
  fraction(db) {
    return Number.isFinite(db) ? clamp((db - this.floor) / -this.floor, 0, 1) : 0;
  }

  /**
   * The meter shown, `{source, channel}`: the one `bind` names (a signal
   * input of the audio sink, `/out.l`), with the `source` and `channel`
   * attributes over it (the Setup's input has no port: `source="input"`).
   */
  target(description) {
    const bound = findMeter(description, bindAddresses(this)[0]);
    return {
      source: this.getAttribute('source') ?? bound?.source ?? 'output',
      channel: numberAttribute(this, 'channel') ?? bound?.channel ?? 0,
    };
  }

  described(description) {
    if (description.sampleRate && this.rate !== description.sampleRate) {
      this.rate = description.sampleRate;
      this.meter = new ChannelMeter(description.sampleRate);
    }
    // A plan swap can move the bound port to another source or channel.
    const target = this.target(description);
    if (this.subscribed && target.source !== this.subscribed.source) {
      this.attach(this.connection);
      return;
    }
    if (this.subscribed && target.channel !== this.subscribed.channel) {
      this.subscribed = target;
      this.meter = new ChannelMeter(this.rate ?? 48000);
    }
    this.update();
  }

  streams(connection) {
    const target = this.target(connection.description);
    this.subscribed = target;
    this.meter = new ChannelMeter(this.rate ?? 48000);
    return [connection.subscribe({ stream: 'meters', source: target.source, rate: 30 }, (frame) => {
      const block = blocksFromFrame(frame)[this.subscribed.channel];
      if (!block) return;
      this.meter.push(block);
      this.update();
    })];
  }

  detach() {
    super.detach();
    this.subscribed = null;
  }

  /** The `meter` scope's keys (ui/lib/keys.default.js): what it shows, nothing is sent. */
  onKey(event) {
    const view = this.floor;
    const command = keyCommand('meter', event)?.id;
    const steps = { 'meter.floor_up': 6, 'meter.floor_down': -6, 'meter.floor_page_up': 24, 'meter.floor_page_down': -24 };
    if (command in steps) this.view = clamp(view + steps[command], FLOORS[0], FLOORS[1]);
    else if (command === 'meter.floor_lowest') this.view = FLOORS[0];
    else if (command === 'meter.floor_default') this.view = null;
    else if (command === 'meter.clear_clip') this.clearClip();
    else return false;
    this.drawScale();
    this.update();
    return true;
  }

  drawScale() {
    const floor = this.floor;
    const ticks = [0, -6, -12, -18, -24, -36, -48, -60, -72, -84, -96].filter((db) => db >= floor);
    this.$('.scale').innerHTML = ticks
      .map((db) => {
        const y = TOP + (1 - this.fraction(db)) * BAR;
        return `<line class="tick" x1="21" x2="24" y1="${y}" y2="${y}"/><text class="tick-label" x="26" y="${y + 3}">${db}</text>`;
      })
      .join('');
    this.drawnFloor = floor;
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const r = this.meter.reading();
    const floor = this.floor;
    const label = this.getAttribute('label') ?? '';
    this.$('.label').textContent = label;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label || `meter ${bindAddresses(this)[0] ?? `channel ${numberAttribute(this, 'channel') ?? 0}`}`);
    this.setAttribute('aria-keyshortcuts', ariaShortcuts('meter'));
    this.setAttribute('aria-valuemin', String(floor));
    this.setAttribute('aria-valuemax', '0');
    // aria-valuenow must be a number: silence (−∞) reads as the floor.
    this.setAttribute('aria-valuenow', String(Number.isFinite(r.held) ? clamp(r.held, floor, 0) : floor));
    this.setAttribute('aria-valuetext', `peak ${formatValue(r.held, 'dB')}, RMS ${formatValue(r.rms, 'dB')}${r.clip ? ', clipped (Enter clears)' : ''}`);
    this.$('.readout').textContent = formatValue(r.held, 'dB');
    this.$('.control').classList.toggle('clip', r.clip);
    if (this.showSkin({ position: this.fraction(r.peak), text: formatValue(r.held, 'dB') })) return;
    if (this.drawnFloor !== floor) this.drawScale();
    const bar = (element, from, to) => {
      const y0 = TOP + (1 - to) * BAR;
      element.setAttribute('x', '4');
      element.setAttribute('width', '16');
      element.setAttribute('y', String(y0));
      element.setAttribute('height', String(Math.max(0, (to - from) * BAR)));
    };
    const hot = this.fraction(-6);
    const rms = this.fraction(r.rms);
    bar(this.$('.peak'), 0, this.fraction(r.peak));
    bar(this.$('.rms'), 0, Math.min(rms, hot));
    bar(this.$('.hot'), hot, Math.max(rms, hot));
    const held = this.$('.held');
    held.setAttribute('x', '4');
    held.setAttribute('width', '16');
    held.setAttribute('y', String(TOP + (1 - this.fraction(r.held)) * BAR - 1));
    held.style.display = Number.isFinite(r.held) && r.held > floor ? '' : 'none';
  }
}

customElements.get('signal-meter') ?? customElements.define('signal-meter', SignalMeter);

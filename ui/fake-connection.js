// A connection without an engine, for ui/gallery.html: the same interface
// as `Connection` in signal-ui.js, a fixed demo project, and deterministic
// test signals for its meters and scopes. Nothing runs until `start()`.

import { KIND } from './lib/protocol.js';

const RATE = 48000;
/** Display updates per second while started. */
const TICKS = 30;
const POINTS = 256;

function control(address, unit, min, max, initial, extra = {}) {
  const [instance, parameter] = address.slice(1).split('.');
  return { address, instance, parameter, module: 'demo', ports: [address], unit, min, max, default: initial, base: initial, inFile: false, modulated: false, combine: 'replace', ...extra };
}

/** The demo project's `describe` reply. */
export function fakeDescription() {
  return {
    type: 'describe',
    planVersion: 0,
    sampleRate: RATE,
    project: 'gallery (fake connection)',
    playing: true,
    instances: [
      { address: '/osc', module: 'demo.osc', sink: null, parameters: [], ports: [{ name: 'out', address: '/osc.out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null }] },
      { address: '/lfo', module: 'demo.osc', sink: null, parameters: [], ports: [{ name: 'out', address: '/lfo.out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null }] },
      { address: '/synth', module: 'demo.voice', sink: null, parameters: [], ports: [{ name: 'trigger', address: '/synth.trigger', direction: 'in', shape: 'event', rate: 1, kind: 'trigger', unit: null, range: null }] },
      {
        address: '/out',
        module: 'std.audio_out',
        sink: 'audio',
        parameters: [],
        ports: ['l', 'r'].map((name) => ({ name, address: `/out.${name}`, direction: 'in', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null })),
      },
    ],
    controls: [
      control('/synth.cutoff', 'Hz', 20, 20000, 1000),
      control('/synth.level', null, 0, 4, 1, { combine: 'multiply' }),
      control('/synth.drive', null, 0, 1, 0.5, { modulated: true }),
      control('/synth.pan', null, -1, 1, 0),
      control('/synth.decay', 'ms', 1, 2000, 300),
      control('/synth.on', null, 0, 1, 0),
      control('/pad.x', null, 0, 1, 0.5),
      control('/pad.y', null, 0, 1, 0.5),
    ],
    sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 2 }],
    meters: { output: [{ channel: 0, device: 0 }, { channel: 1, device: 1 }], input: [{ channel: 0 }] },
    scopes: { slots: 8, points: POINTS, maxRate: RATE },
  };
}

/** The demo signals, at absolute frame `n`. */
const SIGNALS = {
  '/osc.out': (n) => 0.8 * Math.sin((2 * Math.PI * 220 * n) / RATE) + 0.1 * Math.sin((2 * Math.PI * 660 * n) / RATE),
  '/lfo.out': (n) => 0.8 * Math.sin((2 * Math.PI * 330 * n) / RATE + Math.PI / 4),
};

function failure(code, message) {
  const error = new Error(message);
  error.code = code;
  return Promise.reject(error);
}

export class FakeConnection {
  constructor({ timers = globalThis } = {}) {
    this.timers = timers;
    this.status = 'connecting';
    this.description = fakeDescription();
    this.values = new Map(this.description.controls.map((c) => [c.address, c.default]));
    this.frameNumber = 0;
    this.interval = null;
    this.subscriptions = new Set();
    this.nextSub = 1;
    this.listeners = new Map();
    this.triggers = [];
    this.unsaved = null;
  }

  on(event, fn) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event).add(fn);
    return () => this.listeners.get(event)?.delete(fn);
  }

  emit(event, value) {
    for (const fn of [...(this.listeners.get(event) ?? [])]) fn(value);
  }

  /** Starts the signals and says the project is described. */
  start() {
    if (this.interval !== null) return;
    this.status = 'open';
    this.emit('status', 'open');
    this.emit('describe', this.description);
    this.interval = this.timers.setInterval(() => this.tick(), 1000 / TICKS);
  }

  stop() {
    if (this.interval !== null) this.timers.clearInterval(this.interval);
    this.interval = null;
  }

  close() {
    this.stop();
    this.status = 'closed';
    this.emit('status', 'closed');
  }

  control(address) {
    return this.description.controls.find((c) => c.address === address);
  }

  /** A control's effective value: `/synth.drive` is modulated by a slow LFO. */
  effective(address) {
    const base = this.values.get(address);
    if (address !== '/synth.drive') return base;
    return base * (0.6 + 0.4 * Math.sin((2 * Math.PI * 0.25 * this.frameNumber) / RATE));
  }

  describe() {
    for (const c of this.description.controls) c.base = this.values.get(c.address);
    this.emit('describe', this.description);
    return Promise.resolve(this.description);
  }

  edit(address, value) {
    const c = this.control(address);
    if (!c) return failure('unknown-address', `no control ${address}`);
    if (!Number.isFinite(value)) return failure('bad-value', `not a number: ${value}`);
    this.values.set(address, Math.min(c.max, Math.max(c.min, value)));
    return Promise.resolve({ type: 'ack', frame: this.frameNumber, requested: this.frameNumber, late: false, outcome: 'scheduled' });
  }

  set(address, value) {
    return this.edit(address, value);
  }

  ramp(address, to) {
    return this.edit(address, to);
  }

  commit(address, value) {
    const c = this.control(address);
    if (!c) return failure('not-editable', `no control ${address}`);
    return this.edit(address, value ?? c.default).then(() => ({ type: 'committed', address, value, file: 'project.toml', writes: 1, sent: true, note: null }));
  }

  /** Every commit is written here: nothing is ever unsaved. */
  retry(address) {
    return failure('not-unsaved', `${address} is not unsaved`);
  }

  revert(address) {
    return failure('not-unsaved', `${address} is not unsaved`);
  }

  trigger(address) {
    if (address !== '/synth.trigger') return failure('unknown-address', `no event input ${address}`);
    this.triggers.push(this.frameNumber);
    this.emit('trigger', { address, frame: this.frameNumber });
    return Promise.resolve({ type: 'ack', frame: this.frameNumber, requested: this.frameNumber, late: false, outcome: 'scheduled' });
  }

  subscribe(stream, onFrame, { onSubscribed = () => {}, onError = () => {} } = {}) {
    if (stream.stream === 'scope' && !SIGNALS[stream.port]) {
      const error = new Error(`${stream.port} is not a signal output`);
      error.code = 'not-a-signal-output';
      onError(error);
      return () => {};
    }
    const subscription = { stream, onFrame, sub: this.nextSub++ };
    this.subscriptions.add(subscription);
    const reply = { type: 'subscribed', sub: subscription.sub, stream: stream.stream };
    if (stream.stream === 'scope') {
      reply.decimation = Math.max(1, Math.round(RATE / (stream.rate ?? RATE)));
      // Blocks start on a grid of whole blocks, so two scopes of the same
      // rate share their frames (X–Y), and each block follows the last.
      const span = POINTS * reply.decimation;
      subscription.next = Math.floor(this.frameNumber / span) * span;
    }
    onSubscribed(reply);
    return () => this.subscriptions.delete(subscription);
  }

  /** One display update: advances the clock and sends every stream a frame. */
  tick() {
    const frames = RATE / TICKS;
    const first = this.frameNumber;
    this.frameNumber += frames;
    for (const subscription of [...this.subscriptions]) {
      const { stream, onFrame, sub } = subscription;
      if (stream.stream === 'parameters') {
        const values = (stream.addresses ?? []).map((a) => (this.values.has(a) ? { base: this.values.get(a), effective: this.effective(a) } : { base: NaN, effective: NaN }));
        onFrame({ kind: KIND.PARAMETERS, version: 1, sub, firstFrame: this.frameNumber, count: values.length, extra: 0, values });
      } else if (stream.stream === 'meters') {
        let values;
        if (stream.source === 'input') {
          // A quiet input, breathing between −40 and −12 dBFS.
          const level = 10 ** ((-26 + 14 * Math.sin((2 * Math.PI * 0.3 * first) / RATE)) / 20);
          values = [{ peak: level, rms: level / 2 }];
        } else {
          // A level that swells past full scale every 4 s, so the clip latches.
          const level = 0.05 + 1.15 * Math.abs(Math.sin((2 * Math.PI * 0.125 * first) / RATE)) ** 3;
          values = [level, level * 0.5].map((peak) => ({ peak, rms: peak / Math.SQRT2 }));
        }
        onFrame({ kind: KIND.METERS, version: 1, sub, firstFrame: first, count: values.length, extra: frames, values });
      } else if (stream.stream === 'scope') {
        const decimation = Math.max(1, Math.round(RATE / (stream.rate ?? RATE)));
        // Every whole block of points the clock has passed, each following the last.
        while (subscription.next + POINTS * decimation <= this.frameNumber) {
          const start = subscription.next;
          const values = new Float32Array(POINTS);
          for (let i = 0; i < POINTS; i++) values[i] = SIGNALS[stream.port](start + i * decimation);
          subscription.next += POINTS * decimation;
          onFrame({ kind: KIND.SCOPE, version: 1, sub, firstFrame: start, count: POINTS, extra: decimation, values });
        }
      }
    }
  }
}

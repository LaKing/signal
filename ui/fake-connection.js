// A connection without an engine, for ui/gallery.html: the same interface
// as `Connection` in signal-ui.js, a fixed demo project, and deterministic
// test signals for its meters, scopes and canvases. Nothing runs until `start()`.

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
      { address: '/synth', module: 'demo.voice', sink: null, parameters: [{ name: 'level', address: '/synth.level', kind: 'gain', rate: 1, unit: null, range: { min: 0, max: 4 }, default: 1, combine: 'multiply', base: 1 }], ports: [{ name: 'trigger', address: '/synth.trigger', direction: 'in', shape: 'event', rate: 1, kind: 'trigger', unit: null, range: null }] },
      ...Object.entries(VECTORS).map(([address, { rows }]) => {
        const [instance, name] = address.slice(1).split('.');
        return { address: `/${instance}`, module: 'demo.visual', sink: null, parameters: [], ports: [{ name, address, direction: 'out', shape: { vector: rows }, rate: VISUAL, kind: 'control', unit: null, range: null }] };
      }),
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
    vectors: { slots: 8, maxRows: 512 },
  };
}

/** The demo signals, at absolute frame `n`. */
const SIGNALS = {
  '/osc.out': (n) => 0.8 * Math.sin((2 * Math.PI * 220 * n) / RATE) + 0.1 * Math.sin((2 * Math.PI * 660 * n) / RATE),
  '/lfo.out': (n) => 0.8 * Math.sin((2 * Math.PI * 330 * n) / RATE + Math.PI / 4),
};

/** The demo vector ports' rate divisor: 60 snapshots per second. */
const VISUAL = RATE / 60;

/**
 * The demo vector outputs at snapshot `t` (seconds): fixed patterns for the
 * gallery's canvases, not models (the engine's are in signal-std).
 */
const VECTORS = {
  '/membrane.z': {
    rows: 256,
    at: (t) => Float32Array.from({ length: 256 }, (_, k) => {
      const i = k % 16;
      const j = Math.floor(k / 16);
      return 0.4 * Math.sin((Math.PI * (i + 1)) / 17) * Math.sin((Math.PI * (j + 1)) / 17) * Math.cos(2 * Math.PI * 0.7 * t);
    }),
  },
  '/sparks.pos': {
    rows: 128,
    at: (t) => Float32Array.from({ length: 128 }, (_, e) => {
      const k = e >> 1;
      const a = k * 2.399963 + 0.3 * t;
      const r = 0.15 + 0.75 * (((k * 0.618034 + 0.2 * t) % 1 + 1) % 1);
      return e % 2 === 0 ? r * Math.cos(a) : r * Math.sin(a) - 0.1;
    }),
  },
  '/sparks.life': {
    rows: 64,
    at: (t) => Float32Array.from({ length: 64 }, (_, k) => 1 - ((((k * 0.618034 + 0.2 * t) % 1) + 1) % 1)),
  },
  '/bodies.bodies': {
    rows: 24,
    at: (t) => Float32Array.from({ length: 24 }, (_, e) => {
      const b = e >> 2;
      const box = b >= 4;
      const field = e % 4;
      if (field === 0) return -0.6 + 0.24 * b;
      if (field === 1) return -0.9 + (box ? 0.1 : 0.08) + Math.abs(Math.sin(2 * Math.PI * 0.4 * t + b)) * 0.8;
      if (field === 2) return box ? 0.1 : 0.08;
      return box ? 1 : 0;
    }),
  },
};

function failure(code, message) {
  const error = new Error(message);
  error.code = code;
  return Promise.reject(error);
}

/** The demo project's pattern files (`patterns/<name>.toml`, project-format.md §12). */
export const FAKE_PATTERNS = Object.freeze({
  melody: `schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 60, start = 0, length = 0.5, velocity = 0.8 },
  { key = 64, start = 1, length = 0.5, velocity = 0.8 },
  { key = 67, start = 2, length = 1, velocity = 1 },
]
`,
  drums: `schema_version = 1
kind = "steps"
steps = 16
step = 0.25

[lanes]
kick  = "X...x...x...x..x"
snare = "....X.......X..."

[levels]
kick = 0.9
`,
});

/**
 * The fake's stand-in for a file's SHA-256: a deterministic 64-bit FNV-1a
 * hash of the text, as 16 hex digits. Clients only compare it.
 */
export function fakeSha(text) {
  let h = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) {
    h ^= BigInt(byte);
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, '0');
}

const PATTERN_NAME = /^[a-z][a-z0-9_]*$/;

/** The kind a pattern text declares (`kind = "notes"` or `"steps"`), or null. */
function patternKind(text) {
  return /^kind\s*=\s*"(notes|steps)"\s*(#.*)?$/m.exec(text)?.[1] ?? null;
}

export class FakeConnection {
  constructor({ timers = globalThis, patterns = FAKE_PATTERNS } = {}) {
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
    /** The pattern files, in memory: name → text. */
    this.patterns = new Map(Object.entries(patterns));
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

  /** `Connection.readPattern`: `{ text, sha, base, exists, kind }` of an in-memory pattern. */
  readPattern(name) {
    if (typeof name !== 'string' || !PATTERN_NAME.test(name)) return failure('bad-value', `\`${name}\` is not a pattern name`);
    const text = this.patterns.get(name);
    if (text === undefined) return Promise.resolve({ text: null, sha: null, base: null, exists: false, kind: null });
    const sha = fakeSha(text);
    return Promise.resolve({ text, sha, base: sha, exists: true, kind: patternKind(text) });
  }

  /**
   * `Connection.writePattern`: replaces the pattern when `base` is its
   * current sha (null: it does not exist yet), else rejects `conflict`
   * (its `reply` carries the current `text` and `base`); a text without a
   * pattern `kind` is refused (`does-not-load`; the server validates the
   * whole format). Every listener gets `pattern.written`, as the server
   * pushes it.
   */
  writePattern(name, text, base) {
    if (typeof name !== 'string' || !PATTERN_NAME.test(name)) return failure('bad-value', `\`${name}\` is not a pattern name`);
    if (typeof text !== 'string') return failure('bad-message', 'pattern.write needs `text`');
    const before = this.patterns.get(name);
    const current = before === undefined ? null : fakeSha(before);
    if ((base ?? null) !== current) {
      const error = new Error(`patterns/${name}.toml changed since the version the editor started from`);
      error.code = 'conflict';
      error.reply = { type: 'error', code: 'conflict', message: error.message, name, base: current, text: before ?? null };
      return Promise.reject(error);
    }
    const kind = patternKind(text);
    if (!kind) return failure('does-not-load', `patterns/${name}.toml: a pattern file needs \`kind\`: "notes" or "steps"`);
    this.patterns.set(name, text);
    const sha = fakeSha(text);
    this.emit('pattern.written', { type: 'pattern.written', name, base: sha, text });
    return Promise.resolve({ sha, base: sha, created: before === undefined });
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
    if (stream.stream === 'vector' && !VECTORS[stream.port]) {
      const error = new Error(`${stream.port} is not a vector output`);
      error.code = 'not-a-vector-output';
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
    if (stream.stream === 'vector') {
      const ticks = RATE / VISUAL;
      reply.rows = VECTORS[stream.port].rows;
      reply.decimation = VISUAL * Math.max(1, Math.round(ticks / Math.min(ticks, stream.rate ?? 60)));
      subscription.next = Math.ceil(this.frameNumber / reply.decimation) * reply.decimation;
      subscription.decimation = reply.decimation;
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
      } else if (stream.stream === 'vector') {
        // Every snapshot frame the clock passed, the latest last.
        while (subscription.next < this.frameNumber) {
          const at = subscription.next;
          const values = VECTORS[stream.port].at(at / RATE);
          subscription.next += subscription.decimation;
          onFrame({ kind: KIND.VECTOR, version: 1, sub, firstFrame: at, count: values.length, extra: subscription.decimation, values });
        }
      }
    }
  }
}

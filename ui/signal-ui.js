// The UI protocol client (docs/ui-protocol.md) for pages and controls:
// connection, describe, subscribe, set, ramp, trigger, commit, reconnect.
// Plain ES module, no dependencies; built on lib/protocol.js. Nothing here
// touches the DOM when imported, so `node --test ui/` can load it.

import { decodeFrame, helloMessage, parseFragment, socketUrl } from './lib/protocol.js';

/** Close codes after which reconnecting cannot help: unauthorized, protocol. */
export const FINAL_CLOSE_CODES = Object.freeze([1008, 1002]);

/** Waits before each reconnect attempt, in ms; the last repeats. */
export const BACKOFF_MS = Object.freeze([250, 500, 1000, 2000, 5000]);

/**
 * Signal.app's power as a `welcome` or `app` message gives it: `'on'`,
 * `'off'`, or null without one (`signal ui`, which always streams).
 */
export function powerOf(message) {
  const power = message?.power;
  return power === 'on' || power === 'off' ? power : null;
}

/** The error a request gets when its connection is or goes down. */
function closedError() {
  const error = new Error('connection closed');
  error.code = 'closed';
  return error;
}

/** The plan parameter port at `address` in a `describe` reply, or null. */
function planParameter(description, address) {
  for (const instance of description.instances ?? []) {
    const parameter = (instance.parameters ?? []).find((p) => p.address === address);
    if (parameter) return parameter;
  }
  return null;
}

/**
 * The parameter or control at `address` in a `describe` reply, as a range,
 * with its plan `kind` (a control's: its first plan port's; a `gain` takes
 * dB typed, docs/ui-controls.md "Value entry"), null when not described.
 */
export function findParameter(description, address) {
  if (!description || !address) return null;
  const control = (description.controls ?? []).find((c) => c.address === address);
  if (control) {
    const kind = planParameter(description, control.ports?.[0])?.kind ?? null;
    return { min: control.min, max: control.max, default: control.default, unit: control.unit ?? null, base: control.base, control: true, kind };
  }
  const parameter = planParameter(description, address);
  if (!parameter) return null;
  return {
    min: parameter.range?.min ?? 0,
    max: parameter.range?.max ?? 1,
    default: parameter.default,
    unit: parameter.unit ?? null,
    base: parameter.base,
    control: false,
    kind: parameter.kind ?? null,
  };
}

/**
 * The meter a port address names, as `{source, channel}`: a signal input of
 * the audio sink (`/out.l`) is the output meter's channel of the same index,
 * since `meters.output` lists the sink's signal inputs in order. Null when
 * the address is not one.
 */
export function findMeter(description, address) {
  if (!description || !address) return null;
  for (const sink of description.sinks ?? []) {
    if (sink.kind !== 'audio') continue;
    const instance = (description.instances ?? []).find((i) => i.address === sink.address);
    const inputs = (instance?.ports ?? []).filter((p) => p.direction === 'in' && p.shape === 'signal');
    const channel = inputs.findIndex((p) => p.address === address);
    if (channel >= 0) return { source: 'output', channel };
  }
  return null;
}

/**
 * A connection to a running project. It says `hello`, asks `describe` once
 * welcomed (and after every reconnect), and keeps its subscriptions across
 * reconnects (with the server's new `sub` ids). Requests return promises
 * that settle with the reply carrying their `id`, reject with the server's
 * `error`, and reject with `code: 'closed'` when the connection drops
 * before the reply (they are never sent again: a repeated `commit` or
 * `trigger` would act twice) or is down when they are made.
 *
 * Events (`on(name, fn)`): `status` (`connecting`, `open`, `reconnecting`,
 * `failed`, `closed`), `welcome`, `describe` (replies and pushes),
 * `reload`, `reload.error`, `unsaved` (also kept as `connection.unsaved`,
 * with `describe`'s), `scope.bound`, `scope.unbound`, `error`
 * (errors that answer no request) and `message` (every message).
 */
export class Connection {
  constructor({ url, token, WebSocketImpl = globalThis.WebSocket, timers = globalThis, backoff = BACKOFF_MS } = {}) {
    this.url = url;
    this.token = token;
    this.WebSocketImpl = WebSocketImpl;
    this.timers = timers;
    this.backoff = backoff;
    this.status = 'connecting';
    this.welcome = null;
    this.description = null;
    /** The host's unsaved parameters, as last described or pushed. */
    this.unsaved = null;
    this.socket = null;
    this.nextId = 1;
    this.attempt = 0;
    this.reconnectTimer = null;
    this.pending = new Map();
    this.subscriptions = new Set();
    this.bySub = new Map();
    this.listeners = new Map();
    /** False while Signal.app's power is off: no subscription is sent. */
    this.streaming = true;
    this.open();
  }

  /** Calls `fn(value)` on each `event`; returns a function that stops it. */
  on(event, fn) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event).add(fn);
    return () => this.listeners.get(event)?.delete(fn);
  }

  emit(event, value) {
    // One listener's failure is reported and never keeps the others from
    // their event (a throwing `describe` listener once left Signal.app's
    // parameter list empty).
    for (const fn of [...(this.listeners.get(event) ?? [])]) {
      try {
        fn(value);
      } catch (error) {
        (globalThis.reportError ?? console.error)(error);
      }
    }
  }

  /**
   * Whether streams flow (Signal.app's power switch, `docs/ui-protocol.md`
   * "Signal.app"): off, every subscription is let go on the server and kept
   * here; on, each is sent again.
   */
  setStreaming(on) {
    if (this.streaming === on) return;
    this.streaming = on;
    for (const subscription of this.subscriptions) {
      if (on) {
        if (this.status === 'open') this.sendSubscribe(subscription);
      } else if (subscription.sub !== null) {
        this.bySub.delete(subscription.sub);
        // The server let it go itself when its power went off.
        this.request({ type: 'unsubscribe', sub: subscription.sub }).catch(() => {});
        subscription.sub = null;
      }
    }
  }

  setStatus(status) {
    if (this.status === status) return;
    this.status = status;
    this.emit('status', status);
  }

  open() {
    this.reconnectTimer = null;
    const socket = new this.WebSocketImpl(this.url);
    this.socket = socket;
    socket.binaryType = 'arraybuffer';
    socket.onopen = () => socket.send(JSON.stringify(helloMessage(this.token)));
    socket.onmessage = (event) => {
      if (socket !== this.socket) return;
      if (typeof event.data === 'string') this.received(JSON.parse(event.data));
      else this.frame(decodeFrame(event.data));
    };
    socket.onclose = (event) => {
      if (socket === this.socket) this.closed(event?.code);
    };
  }

  received(message) {
    this.emit('message', message);
    if (message.type === 'welcome') {
      this.welcome = message;
      this.attempt = 0;
      // Before any subscription is sent: a client that connects while
      // Signal.app is off learns it here.
      this.streaming = powerOf(message) !== 'off';
      this.setStatus('open');
      this.emit('welcome', message);
      this.describe().catch(() => {});
      if (this.streaming) for (const subscription of this.subscriptions) this.sendSubscribe(subscription);
      return;
    }
    if (message.type === 'app' && powerOf(message)) this.setStreaming(powerOf(message) !== 'off');
    const waiting = message.id !== undefined ? this.pending.get(message.id) : undefined;
    if (waiting) {
      this.pending.delete(message.id);
      if (message.type === 'error') {
        const error = new Error(message.message ?? message.code);
        error.code = message.code;
        error.reply = message;
        waiting.reject(error);
      } else {
        waiting.resolve(message);
      }
    } else if (message.type === 'error') {
      this.emit('error', message);
    }
    if (message.type === 'describe') {
      this.description = message;
      if (message.unsaved) this.unsaved = message.unsaved;
      this.emit('describe', message);
    } else if (message.type !== 'error' && message.id === undefined) {
      if (message.type === 'unsaved') this.unsaved = message;
      this.emit(message.type, message);
    }
  }

  frame(frame) {
    this.bySub.get(frame.sub)?.onFrame(frame);
  }

  closed(code) {
    this.socket = null;
    for (const { reject } of this.pending.values()) reject(closedError());
    this.pending.clear();
    this.bySub.clear();
    for (const subscription of this.subscriptions) subscription.sub = null;
    if (this.status === 'closed') return;
    if (FINAL_CLOSE_CODES.includes(code)) {
      this.setStatus('failed');
      return;
    }
    this.setStatus('reconnecting');
    const wait = this.backoff[Math.min(this.attempt, this.backoff.length - 1)];
    this.attempt += 1;
    this.reconnectTimer = this.timers.setTimeout(() => this.open(), wait);
  }

  /** Sends `message` with a new id; the promise settles with its reply. */
  request(message) {
    return new Promise((resolve, reject) => this.send(message, resolve, reject));
  }

  /** `request` with callbacks, called as the reply arrives (not a task later). */
  send(message, resolve, reject) {
    if (this.status !== 'open' || !this.socket) {
      reject(closedError());
      return;
    }
    const id = this.nextId++;
    this.pending.set(id, { resolve, reject });
    this.socket.send(JSON.stringify({ ...message, id }));
  }

  /** The running project (also emitted as `describe`). */
  describe() {
    return this.request({ type: 'describe' });
  }

  /** A live edit: `value` at `address` (a control or plan parameter port). */
  set(address, value, { at } = {}) {
    return this.request({ type: 'set', address, value, ...(at === undefined ? {} : { at }) });
  }

  /** A live ramp to `to` over `ms` (or `frames`); the server's 5 ms without either. */
  ramp(address, to, { ms, frames, at } = {}) {
    return this.request({ type: 'ramp', address, to, ...(ms === undefined ? {} : { ms }), ...(frames === undefined ? {} : { frames }), ...(at === undefined ? {} : { at }) });
  }

  /** An event at an event input of the plan. */
  trigger(address, { at } = {}) {
    return this.request({ type: 'trigger', address, ...(at === undefined ? {} : { at }) });
  }

  /** Writes a control's value into project.toml (null removes the key). */
  commit(address, value) {
    return this.request({ type: 'commit', address, value });
  }

  /** Writes an unsaved control's live value into project.toml again. */
  retry(address) {
    return this.request({ type: 'retry', address });
  }

  /** Sets an unsaved parameter's live value back to the file's. */
  revert(address) {
    return this.request({ type: 'revert', address });
  }

  /**
   * Subscribes to a stream (`{stream: 'parameters' | 'meters' | 'scope', …}`
   * as in the protocol); `onFrame` gets its decoded frames, `onSubscribed`
   * each `subscribed` reply (again after a reconnect), `onError` a refusal.
   * Returns a function that unsubscribes.
   */
  subscribe(stream, onFrame, { onSubscribed = () => {}, onError = () => {} } = {}) {
    const subscription = { stream, onFrame, onSubscribed, onError, sub: null, active: true };
    this.subscriptions.add(subscription);
    if (this.status === 'open' && this.streaming) this.sendSubscribe(subscription);
    return () => {
      subscription.active = false;
      this.subscriptions.delete(subscription);
      if (subscription.sub !== null) {
        this.bySub.delete(subscription.sub);
        this.request({ type: 'unsubscribe', sub: subscription.sub }).catch(() => {});
        subscription.sub = null;
      }
    };
  }

  sendSubscribe(subscription) {
    // Callbacks rather than a promise: the subscription must be known before
    // the next message, which may be its first frame.
    this.send(
      { type: 'subscribe', ...subscription.stream },
      (reply) => {
        if (!subscription.active) {
          this.request({ type: 'unsubscribe', sub: reply.sub }).catch(() => {});
          return;
        }
        subscription.sub = reply.sub;
        this.bySub.set(reply.sub, subscription);
        subscription.onSubscribed(reply);
      },
      (error) => {
        // `powered-off`: kept, and sent again when the power is back on.
        if (error.code !== 'closed' && error.code !== 'powered-off') subscription.onError(error);
      },
    );
  }

  /** Closes the connection for good. */
  close() {
    this.setStatus('closed');
    if (this.reconnectTimer !== null) this.timers.clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const socket = this.socket;
    if (socket) {
      this.closed(1000);
      socket.close();
    }
  }
}

/** A new connection to `url` with `token`. */
export function connect(options) {
  return new Connection(options);
}

/**
 * The connection the page's URL names (`#ws=<port>&token=<token>`, as
 * `signal ui` prints it), or null when it names none.
 */
export function connectFromPage(place = globalThis.location, options = {}) {
  const { ws, token } = parseFragment(place?.hash);
  if (!ws || !token) return null;
  return connect({ url: socketUrl(place.hostname || '127.0.0.1', ws), token, ...options });
}

let defaultConnection = null;
const defaultListeners = new Set();

/** Sets the connection controls use when none is given to them. */
export function setDefaultConnection(connection) {
  defaultConnection = connection;
  for (const fn of [...defaultListeners]) fn(connection);
}

export function getDefaultConnection() {
  return defaultConnection;
}

/** Calls `fn` with the default connection now (if set) and whenever it changes. */
export function onDefaultConnection(fn) {
  defaultListeners.add(fn);
  if (defaultConnection) fn(defaultConnection);
  return () => defaultListeners.delete(fn);
}

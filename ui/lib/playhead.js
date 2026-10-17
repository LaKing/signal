// A pattern control's playhead from a signal output of the plan (no DOM):
// `<signal-piano-roll position-bind="/melody.position">` and
// `<signal-step-grid step-bind="/drums.step">` (docs/ui-controls.md).
//
// The output is read through a `scope` stream (docs/ui-protocol.md "Scope
// streams"), as a `<signal-scope>` reads one: blocks of 256 consecutive
// points, here at PLAYHEAD_RATE points per second, so a block every 1/30 s.
// The playhead shows each block's last point, the newest value, once per
// display frame however many blocks arrive (`schedule`, requestAnimationFrame
// in a page). It writes nothing. A refused slot (`no-scope-slot`, `busy`) is
// asked for again after SLOT_RETRY_MS, as a scope does; any other refusal is
// reported through `onProblem` and not asked again.

import { SLOT_RETRY_MS } from './trigger.js';

/** Points per second: one 256-point block every 1/30 s. */
export const PLAYHEAD_RATE = 7680;

/** The address of a playhead attribute, or null when there is none (empty, blank). */
export function playheadAddress(value) {
  const address = (value ?? '').trim();
  return address === '' ? null : address;
}

/**
 * Subscribes `connection` (one offering `subscribe`) to the scope stream of
 * `port`; calls `onValue(v)` with the newest point at most once per
 * `schedule` callback. `onProblem(text|null)` says why there is no playhead
 * (null once subscribed). Returns the stop.
 */
export function playheadStream(connection, port, onValue, {
  rate = PLAYHEAD_RATE,
  onProblem = () => {},
  schedule = (fn) => globalThis.requestAnimationFrame(fn),
  timers = globalThis,
  retryMs = SLOT_RETRY_MS,
} = {}) {
  if (!port || typeof connection?.subscribe !== 'function') return () => {};
  let latest = null;
  let scheduled = false;
  let stopped = false;
  let unsubscribe = () => {};
  let timer = null;
  const show = () => {
    scheduled = false;
    if (stopped || latest === null) return;
    const value = latest;
    latest = null;
    onValue(value);
  };
  const subscribe = () => {
    timer = null;
    if (stopped) return;
    unsubscribe = connection.subscribe(
      { stream: 'scope', port, rate },
      (frame) => {
        const values = frame?.values;
        if (stopped || !values || values.length === 0) return;
        latest = values[values.length - 1];
        if (!scheduled) {
          scheduled = true;
          schedule(show);
        }
      },
      {
        onSubscribed: () => onProblem(null),
        onError: (error) => {
          const code = error?.code ?? error?.message ?? String(error);
          onProblem(`${port}: ${code}`);
          if (code === 'no-scope-slot' || code === 'busy') {
            unsubscribe();
            unsubscribe = () => {};
            timers.clearTimeout(timer);
            timer = timers.setTimeout(subscribe, retryMs);
          }
        },
      },
    );
  };
  subscribe();
  return () => {
    stopped = true;
    timers.clearTimeout(timer);
    timer = null;
    unsubscribe();
  };
}

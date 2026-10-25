// The schematic's live levels (M2.9-1, AC14 as decided in operator-request:008;
// since 2026-10-25 the owner's "csak a kijelölt elem kapjon mérőket, de
// minden bemeneten és kimeneten - a tartományának megfelelően": only the
// selected instance gets meters, on every input and output, each scaled to
// its range). A signal input or output of the selected instance on screen
// has a bar from a `scope` stream (an input's: the sum it receives); a
// parameter's shows its effective value from one `parameters` stream. There
// are 8 scope slots per run, shared by every client (`docs/ui-protocol.md`
// "Schematic level bars"), so the selected instance's signal ports are
// taken in order, inputs then outputs, top to bottom; those past the free
// slots show the same bar marked "no slot". Panel scopes come first
// (M2.9-1b): the schematic asks only for slots the host says are free
// (`scopes`), and only CLAIM_MS after the free count last rose, longer than
// a panel scope's 2 s retry. Event and vector ports have no meter. Nothing
// is subscribed while the tab or the page is hidden, the power is off or
// nothing plays. No DOM.

import { meterFraction } from './protocol.js';
import { planPort } from './schematic-scene.js';
import { toScreen } from './schematic-view.js';

/** Points per second of a level's scope: a 256-point block about every 33 ms. */
export const LEVEL_RATE = 7680;
/** A refused port is asked again at most this often (ms) while shown. */
export const RETRY_MS = 2000;
/** A slot freed (anyone's) is taken only this long (ms) after it was: a waiting panel scope retries first. */
export const CLAIM_MS = RETRY_MS + 500;
/** The scope slots of a run (`describe.scopes.slots`, else this). */
export const SLOTS = 8;

/**
 * The metered ports of the selected instance (`selected`, an instance
 * address) as drawn in `drawn` (a scene), as `[{address, port, instance,
 * direction, source, range, x, y}]`: `address` the drawn port's, `port` the
 * plan's primitive port behind it (a composite's input or parameter: its
 * export's first target; an output: the primitive it comes from),
 * `source` `scope` for a signal input or output and `parameter` for a
 * parameter, `range` the port's declared `{min, max}` or null. Inputs
 * first, then outputs, each top to bottom. Only a port whose own point is
 * inside `viewport` (`{w, h}`) counts. Nothing without a selection, for an
 * open composite (its inside is drawn instead) or a frame's export pins;
 * event and vector ports have no meter.
 */
export function candidates(drawn, graph, selected = null, viewport = null) {
  const found = [];
  if (!selected) return found;
  for (const level of drawn.levels) {
    for (const node of level.nodes) {
      if (node.kind !== 'instance') continue;
      const instance = `${level.path}/${node.name}`;
      if (instance !== selected || drawn.open?.has(instance)) continue;
      for (const pin of [...node.ins, ...node.outs]) {
        if (pin.shape !== 'signal') continue;
        const address = `${instance}.${pin.name}`;
        const parameter = Boolean(pin.parameter);
        const input = pin.direction === 'in' && !parameter;
        const port = node.composite ? planPort(graph, address, { parameter, input }) : address;
        if (!port) continue;
        const at = toScreen(level.transform, pin);
        if (viewport && !(at.x >= 0 && at.x <= viewport.w && at.y >= 0 && at.y <= viewport.h)) continue;
        found.push({
          address,
          port,
          instance,
          direction: pin.direction,
          source: parameter ? 'parameter' : 'scope',
          range: usable(pin.range),
          x: at.x,
          y: at.y,
        });
      }
    }
  }
  found.sort((a, b) => (a.direction === 'out') - (b.direction === 'out') || a.y - b.y || a.x - b.x);
  return found;
}

/** A range a meter can scale to: finite, `min < max`; else null. */
function usable(range) {
  if (!range || !Number.isFinite(range.min) || !Number.isFinite(range.max) || !(range.min < range.max)) return null;
  return { min: range.min, max: range.max };
}

/** `value`'s place in `range` (0…1, clamped). */
function within(value, range) {
  return Math.min(1, Math.max(0, (value - range.min) / (range.max - range.min)));
}

/**
 * A scope meter's span (`{lo, hi}`, 0…1 of its bar) from a block's points:
 * with a range, the block's lowest to highest value in it; without one,
 * from 0 to its peak on the −60…0 dBFS scale (`levelFraction`).
 */
export function levelSpan(points, range = null) {
  if (!range) return { lo: 0, hi: levelFraction(points) };
  let lo = Infinity;
  let hi = -Infinity;
  for (const v of points ?? []) {
    if (!Number.isFinite(v)) continue;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  if (lo > hi) return { lo: 0, hi: 0 };
  return { lo: within(lo, range), hi: within(hi, range) };
}

/** A parameter meter's span: from its range's minimum to its effective value. */
export function parameterSpan(value, range = null) {
  if (!Number.isFinite(value) || !range) return { lo: 0, hi: 0 };
  return { lo: 0, hi: within(value, range) };
}

/**
 * What to subscribe and unsubscribe, each candidate's state, and when to
 * plan again. `wanted` is `candidates()`; `granted` the plan ports
 * subscribed now (asked or answered); `refused` plan port → when the
 * server last refused it (`no-scope-slot` or `busy`); `active` false while
 * hidden, off or stopped. `scopes` is the host's last word on its slots
 * (`{free, sinceFreedMs, at, held}`: the `scopes` push or `describe`, `at`
 * when it arrived and `held` how many of `granted` there were then);
 * without it, as from an older host, `slots` are taken as free.
 *
 * The ports held, plus the free ones not yet taken by this schematic since
 * `scopes`, are live (or `pending` while asked), the first in order; free
 * slots count only CLAIM_MS after the free count last rose, so a panel
 * scope waiting for one gets it first, and own releases are not credited
 * (a port leaving and one entering in the same plan swap at once). A port
 * further down, or one refused less than RETRY_MS ago, is `no-slot`;
 * inactive, every one is `idle`. Returns `{subscribe, unsubscribe, states,
 * retry}`: `retry` is how long (ms) until a request could succeed, or null
 * when none can (no free slot, or nothing waiting).
 */
export function levelPlan({ wanted, granted = new Set(), refused = new Map(), active, now = 0, slots = SLOTS, scopes = null }) {
  const states = new Map();
  if (!active) {
    for (const c of wanted) states.set(c.address, 'idle');
    return { subscribe: [], unsubscribe: [...granted], states, retry: null };
  }
  const ports = [];
  for (const c of wanted) if (!ports.includes(c.port)) ports.push(c.port);
  let free = Math.max(0, slots - granted.size);
  let claimIn = 0;
  if (scopes) {
    free = Math.max(0, scopes.free - Math.max(0, granted.size - scopes.held));
    if (scopes.sinceFreedMs !== null && scopes.sinceFreedMs !== undefined) {
      claimIn = Math.max(0, CLAIM_MS - scopes.sinceFreedMs - (now - scopes.at));
    }
  }
  const target = new Set(ports.slice(0, granted.size + (claimIn > 0 ? 0 : free)));
  const cooling = (port) => refused.has(port) && now - refused.get(port) < RETRY_MS;
  const subscribe = [...target].filter((port) => !granted.has(port) && !cooling(port));
  const unsubscribe = [...granted].filter((port) => !target.has(port));
  for (const c of wanted) {
    let state = 'no-slot';
    if (target.has(c.port) && granted.has(c.port)) state = 'live';
    else if (target.has(c.port) && !cooling(c.port)) state = 'pending';
    states.set(c.address, state);
  }
  // A request could succeed once a free slot may be claimed or a refused
  // port's cooldown ends; with no free slot, the next `scopes` says when.
  let retry = null;
  const waiting = ports.filter((port) => !granted.has(port) && !subscribe.includes(port));
  if (waiting.length && free > 0) {
    const cooled = waiting.filter((port) => target.has(port)).map((port) => RETRY_MS - (now - refused.get(port)));
    const due = [...(claimIn > 0 ? [claimIn] : []), ...cooled];
    retry = due.length ? Math.min(...due) : null;
  }
  return { subscribe, unsubscribe, states, retry };
}

/**
 * Whether the instance at `address` is muted (`'self'`), holds a muted one
 * (`'inside'`, a composite: one of `muted`'s addresses is below it), or
 * neither (null). `muted` is a set of instance addresses (`app.mutedInstances`).
 */
export function mutedOf(muted, address) {
  if (!muted?.size) return null;
  if (muted.has(address)) return 'self';
  const prefix = `${address}/`;
  for (const m of muted) if (m.startsWith(prefix)) return 'inside';
  return null;
}

/** A level bar's length (0…1) from a scope block's points: their peak, on a −60…0 dBFS scale. */
export function levelFraction(points) {
  let peak = 0;
  for (const v of points ?? []) peak = Math.max(peak, Math.abs(v));
  return meterFraction(peak);
}

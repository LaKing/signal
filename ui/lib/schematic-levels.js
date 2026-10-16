// The schematic's live levels (M2.9-1, AC14 as decided in operator-request:008):
// a small bar on each signal output of an instance on screen, from the
// existing `scope` streams. There are 8 scope slots per run, shared by every
// client (`docs/ui-protocol.md` "Schematic level bars"), so the visible
// outputs are taken in order, the selected instance's first, then by their
// place on screen; those past the free slots show the same bar marked "no
// slot". Panel scopes come first (M2.9-1b): the schematic asks only for
// slots the host says are free (`scopes`), and only CLAIM_MS after the free
// count last rose, longer than a panel scope's 2 s retry. Nothing is
// subscribed while the tab or the page is hidden, the power is off or
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
 * The signal outputs of the instances drawn in `drawn` (a scene), as
 * `[{address, port, x, y}]` (`address` the drawn port's, `port` the plan's
 * primitive port behind it): the selected instance's first (`selected`, an
 * instance address), then top to bottom, left to right on screen. Only a
 * port whose own point is inside `viewport` (`{w, h}`) counts, whatever
 * part of its box is on screen. Vector and event outputs have no level; the
 * export pins of a composite's frame are not instances.
 */
export function candidates(drawn, graph, selected = null, viewport = null) {
  const found = [];
  for (const level of drawn.levels) {
    for (const node of level.nodes) {
      if (node.kind !== 'instance') continue;
      // An open composite's outputs are drawn by what is inside it.
      const instance = `${level.path}/${node.name}`;
      if (drawn.open?.has(instance)) continue;
      for (const out of node.outs) {
        if (out.shape !== 'signal') continue;
        const address = `${instance}.${out.name}`;
        const port = node.composite ? planPort(graph, address) : address;
        if (!port) continue;
        const at = toScreen(level.transform, out);
        if (viewport && !(at.x >= 0 && at.x <= viewport.w && at.y >= 0 && at.y <= viewport.h)) continue;
        found.push({ address, port, instance, x: at.x, y: at.y });
      }
    }
  }
  found.sort((a, b) => (b.instance === selected) - (a.instance === selected) || a.y - b.y || a.x - b.x);
  return found;
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

/** A level bar's length (0…1) from a scope block's points: their peak, on a −60…0 dBFS scale. */
export function levelFraction(points) {
  let peak = 0;
  for (const v of points ?? []) peak = Math.max(peak, Math.abs(v));
  return meterFraction(peak);
}

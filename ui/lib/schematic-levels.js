// The schematic's live levels (M2.9-1, AC14 as decided in operator-request:008):
// a small bar on each signal output of an instance on screen, from the
// existing `scope` streams. There are 8 scope slots per run, shared by every
// client (`docs/ui-protocol.md` "Schematic level bars"), so the visible
// outputs are taken in order, the selected instance's first, then by their
// place on screen; those past the free slots show the same bar marked "no
// slot". Nothing is subscribed while the tab or the page is hidden, the
// power is off or nothing plays. No DOM.

import { meterFraction } from './protocol.js';
import { planPort } from './schematic-scene.js';
import { toScreen } from './schematic-view.js';

/** Points per second of a level's scope: a 256-point block about every 33 ms. */
export const LEVEL_RATE = 7680;
/** A refused port is asked again at most this often (ms) while shown. */
export const RETRY_MS = 2000;
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
 * What to subscribe and unsubscribe, and each candidate's state. `wanted`
 * is `candidates()`; `granted` the plan ports subscribed now; `refused`
 * plan port → when the server last refused it (`no-scope-slot`); `active`
 * false while hidden, off or stopped. The first `slots` wanted ports are
 * live (or `pending` while their subscription is asked); a port further
 * down, or one refused less than RETRY_MS ago, is `no-slot`; inactive,
 * every one is `idle`. Returns `{subscribe, unsubscribe, states}` (states:
 * drawn address → state).
 */
export function levelPlan({ wanted, granted = new Set(), refused = new Map(), active, now = 0, slots = SLOTS }) {
  const states = new Map();
  if (!active) {
    for (const c of wanted) states.set(c.address, 'idle');
    return { subscribe: [], unsubscribe: [...granted], states };
  }
  const ports = [];
  for (const c of wanted) if (!ports.includes(c.port)) ports.push(c.port);
  const target = new Set(ports.slice(0, slots));
  const subscribe = [];
  for (const port of target) {
    const at = refused.get(port);
    if (!granted.has(port) && !(at !== undefined && now - at < RETRY_MS)) subscribe.push(port);
  }
  const unsubscribe = [...granted].filter((port) => !target.has(port));
  for (const c of wanted) {
    const recent = refused.has(c.port) && now - refused.get(c.port) < RETRY_MS;
    let state = 'no-slot';
    if (target.has(c.port) && granted.has(c.port)) state = 'live';
    else if (target.has(c.port) && !recent) state = 'pending';
    states.set(c.address, state);
  }
  return { subscribe, unsubscribe, states };
}

/** Whether a retry is due: some shown port is waiting for a slot. */
export function retryDue(states) {
  return [...states.values()].includes('no-slot');
}

/** A level bar's length (0…1) from a scope block's points: their peak, on a −60…0 dBFS scale. */
export function levelFraction(points) {
  let peak = 0;
  for (const v of points ?? []) peak = Math.max(peak, Math.abs(v));
  return meterFraction(peak);
}

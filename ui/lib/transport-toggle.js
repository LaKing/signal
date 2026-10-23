// Space plays and stops the project's transport (ADR 0039, the keymap's
// `transport.toggle`): which controls that is, and the value each gets.
// Pure (no DOM, no connection), so `node --test ui/` checks it.
//
// The transport is `std.playhead`'s `run` parameter (ADR 0033), reached by
// a top-level control (`describe.controls`, a composite's export such as
// studio-transport's `/transport.run`, or an include's). Every such control
// whose run is not wired (`modulated`: a connection drives it, and a value
// set by hand would not be what plays) is a target. Several (two transports,
// two included projects): toggled together, so they stay in step: if any
// runs, all stop; else all play.

/** The `run` parameter ports of the `std.playhead` instances of a `describe`. */
export function playheadRuns(description) {
  const runs = new Set();
  for (const instance of description?.instances ?? []) {
    if (instance.module !== 'std.playhead') continue;
    const run = (instance.parameters ?? []).find((p) => p.name === 'run');
    runs.add(run?.address ?? `${instance.address}.run`);
  }
  return runs;
}

/**
 * The controls Space toggles in a `describe` message: `[{address, base,
 * min, max}]` in the order `describe` lists them (top-level controls whose
 * plan ports reach a `std.playhead`'s `run`, not modulated). Empty for no
 * description, or none.
 */
export function transportTargets(description) {
  const runs = playheadRuns(description);
  if (runs.size === 0) return [];
  const targets = [];
  for (const control of description?.controls ?? []) {
    if (control.modulated) continue;
    if (!(control.ports ?? []).some((port) => runs.has(port))) continue;
    const min = Number.isFinite(control.min) ? control.min : 0;
    const max = Number.isFinite(control.max) ? control.max : 1;
    const base = Number.isFinite(control.base) ? control.base : Number.isFinite(control.default) ? control.default : max;
    targets.push({ address: control.address, base, min, max });
  }
  return targets;
}

/** Whether a target runs: its value at or above 0.5 (`run` is a gate, ADR 0033). */
export const running = (target) => target.base >= 0.5;

/**
 * What Space does to `targets` (`transportTargets`, with any bases a page
 * sent since that description in `sent`, address → value): `{value,
 * sets: [{address, value}]}`, 0 for every one when any runs, else 1 (each
 * within its control's range); null when there is nothing to toggle.
 */
export function toggleTransport(targets, sent = new Map()) {
  if (!targets?.length) return null;
  const now = targets.map((t) => (sent.has(t.address) ? { ...t, base: sent.get(t.address) } : t));
  const value = now.some(running) ? 0 : 1;
  return {
    value,
    sets: now.map((t) => ({ address: t.address, value: Math.min(t.max, Math.max(t.min, value)) })),
  };
}

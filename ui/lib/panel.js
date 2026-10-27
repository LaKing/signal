// Panels (docs/ui-controls.md "Panels"): which panel file a page is, how a
// binding diagnostic reads, when the binding check is asked, and the
// generated default panel of a project without ui/main.html. For
// <signal-panel>; no DOM.

import { topSlots } from './slot.js';

/** Units whose values a log taper suits (when the range starts above 0). */
const LOG_UNITS = new Set(['Hz', 'ms', 's']);

/**
 * The panel file a page is, as the protocol's `panel.check` names it: the
 * file in the project's ui/ folder for a path `/project/<file>.html`, else
 * null (the page is no project panel).
 */
export function panelFile(pathname) {
  const match = /^\/project\/([^/]+\.html)$/.exec(pathname ?? '');
  return match ? match[1] : null;
}

/** A diagnostic as `signal ui check` prints it: `file:line:column: severity: message`. */
export function formatDiagnostic({ file, line, column, severity, message }) {
  return `${file}:${line}:${column}: ${severity}: ${message}`;
}

/** The plan parameter at `address` in a `describe` reply. */
function planParameter(description, address) {
  for (const instance of description.instances ?? []) {
    const parameter = (instance.parameters ?? []).find((p) => p.address === address);
    if (parameter) return parameter;
  }
  return null;
}

/**
 * How the default panel shows a control: a fader for levels (unit `dB`, or
 * a gain without unit), a knob otherwise; a log taper for Hz and times
 * whose range starts above 0, a dB taper for a gain fader.
 */
export function controlSpec(control, description = {}) {
  const kind = planParameter(description, control.ports?.[0])?.kind ?? null;
  const gain = control.unit === null && kind === 'gain';
  const level = control.unit === 'dB' || gain;
  let taper = 'linear';
  if (LOG_UNITS.has(control.unit) && control.min > 0 && control.min < control.max) taper = 'log';
  else if (gain && control.min >= 0) taper = 'db';
  return {
    element: level ? 'signal-fader' : 'signal-knob',
    bind: control.address,
    label: control.parameter ?? control.address,
    taper,
  };
}

/**
 * The generated panel of a `describe` reply: the top-level parameters
 * (`controls`) grouped by instance, in order, but a slot's (ADR 0037 §7:
 * `slots`, the top-level slots' names, each a card of its own that shows
 * its interface parameters); a meter per signal input of the audio sink;
 * and a scope on the sink's first input (the sum of its sources, each
 * scaled), or null.
 */
export function defaultPanel(description) {
  const d = description ?? {};
  const groups = [];
  // ADR 0037 §7: every top-level slot, loaded or not, has its card (<signal-slot>), which shows its interface parameters.
  const slots = Object.keys(topSlots(d));
  for (const control of d.controls ?? []) {
    if (slots.includes(control.instance)) continue;
    let group = groups.find((g) => g.instance === control.instance);
    if (!group) {
      group = { instance: control.instance, module: control.module, controls: [] };
      groups.push(group);
    }
    group.controls.push(controlSpec(control, d));
  }
  const sink = (d.sinks ?? []).find((s) => s.kind === 'audio');
  const inputs = (d.instances ?? [])
    .find((i) => i.address === sink?.address)
    ?.ports?.filter((p) => p.direction === 'in' && p.shape === 'signal') ?? [];
  const meters = inputs.map((p) => ({ bind: p.address, label: p.name ?? p.address }));
  const first = inputs[0];
  const scope = first ? { bind: first.address, label: first.address } : null;
  return { project: d.project ?? null, groups, slots, meters, scope };
}

/**
 * When a panel asks the server's binding check (`panel.check`): once per
 * plan version, from the `describe` that names it. A check the connection
 * dropped (`closed`) did not happen, so the next `describe` (the client
 * asks one on every reconnect) asks again. `request(message)` returns the
 * reply's promise; `show(reply, error)` gets the reply, or a refusal.
 */
export class BindingCheck {
  constructor({ request, show }) {
    this.request = request;
    this.show = show;
    this.checked = undefined;
    this.asking = undefined;
  }

  /** A `describe` for panel `file`: the check's promise, or null when not due. */
  described(description, file) {
    const version = description?.planVersion;
    if (version === this.checked || version === this.asking) return null;
    this.asking = version;
    const settle = () => {
      if (this.asking === version) this.asking = undefined;
    };
    return this.request({ type: 'panel.check', file }).then(
      (reply) => {
        settle();
        this.checked = version;
        this.show(reply, null);
      },
      (error) => {
        settle();
        if (error?.code === 'closed') return;
        this.checked = version;
        this.show(null, error);
      },
    );
  }
}

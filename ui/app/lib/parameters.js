// The parameter list (the M2.5-2 panel, now web): the project's top-level
// instances with their parameters from `describe`, editable; a composite's
// own parameters inside it, read-only (they are set in the composite's
// file). A drag is a 5 ms ramp, the release one commit, Reset a commit of
// null (the default: the key removed from project.toml); a typed value is
// sent as a release is, the ramp then the commit. No DOM.

import { controlSpec } from '../../lib/panel.js';
import { findParameter } from '../../signal-ui.js';

/** The ramp a drag moves along, against zipper noise. */
export const RAMP_MS = 5;

/**
 * The list: per top-level instance (in the panel's order), its editable
 * parameters (`describe.controls`) and the read-only ones inside it. A
 * row's `taper` is the default panel's (`dB` for a gain) and its `kind` the
 * plan's, for typed values.
 */
export function rows(description) {
  const instances = new Map();
  for (const c of description?.controls ?? []) {
    if (!instances.has(c.instance)) {
      instances.set(c.instance, { name: c.instance, module: c.module, parameters: [], inner: [] });
    }
    instances.get(c.instance).parameters.push({
      address: c.address,
      name: c.parameter,
      unit: c.unit ?? null,
      min: c.min,
      max: c.max,
      default: c.default,
      base: c.base,
      inFile: c.inFile,
      modulated: c.modulated,
      // ADR 0041 §8: whether it goes into snapshots (an older server says nothing: it does).
      inSnapshots: c.inSnapshots !== false,
      taper: controlSpec(c, description).taper,
      kind: findParameter(description, c.address)?.kind ?? null,
      editable: true,
    });
  }
  for (const row of instances.values()) {
    const prefix = `/${row.name}/`;
    for (const instance of description?.instances ?? []) {
      if (!instance.address.startsWith(prefix)) continue;
      const inside = instance.address.slice(prefix.length);
      for (const p of instance.parameters ?? []) {
        row.inner.push({
          address: p.address,
          name: `${inside}.${p.name}`,
          unit: p.unit ?? null,
          min: p.range?.min ?? 0,
          max: p.range?.max ?? 1,
          default: p.default,
          base: p.base,
          editable: false,
        });
      }
    }
  }
  return [...instances.values()];
}

/**
 * What the list's DOM is built from (M2.8-11): the instances and their
 * parameters, without the values. A `describe` with the same structure
 * updates the values in place; only a new structure rebuilds the list.
 */
export function structureKey(list) {
  return JSON.stringify(
    list.map((i) => [
      i.name,
      i.module,
      i.parameters.map((p) => [p.address, p.name, p.unit, p.min, p.max, p.editable]),
      i.inner.map((p) => [p.address, p.name, p.unit, p.min, p.max]),
    ]),
  );
}

/**
 * What a `describe` does to the list shown with `shownKey`: `update` the
 * values in place, `rebuild` it, or `defer` the rebuild until the slider
 * being dragged is released (its row would be replaced under the pointer).
 */
export function planUpdate(shownKey, nextKey, dragging) {
  if (shownKey === nextKey) return 'update';
  return dragging ? 'defer' : 'rebuild';
}

/** What follows a parameter's base: `(not in file)` when the file has no value. */
export function fileNote(parameter) {
  return parameter.inFile === false ? '(not in file)' : '';
}

/** A slider moved: the value, live, as a 5 ms ramp. */
export function dragMessage(address, value) {
  return { type: 'ramp', address, to: value, ms: RAMP_MS };
}

/** The slider was released: the value, written once into project.toml. */
export function releaseMessage(address, value) {
  return { type: 'commit', address, value };
}

/** Reset to default: each port back to its own default, the key removed. */
export function resetMessage(address) {
  return { type: 'commit', address, value: null };
}

/** A parameters frame's values by address (`{base, effective}`; NaN unknown). */
export function valuesOf(addresses, frame) {
  const values = new Map();
  addresses.forEach((address, i) => {
    const v = frame?.values?.[i];
    if (v) values.set(address, v);
  });
  return values;
}

/** A value as the list shows it (4 significant digits, then its unit). */
export function shown(value, unit) {
  if (!Number.isFinite(value)) return '—';
  const text = Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(4)));
  return unit ? `${text} ${unit}` : text;
}

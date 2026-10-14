// The parameter list (the M2.5-2 panel, now web): the project's top-level
// instances with their parameters from `describe`, editable; a composite's
// own parameters inside it, read-only (they are set in the composite's
// file). A drag is a 5 ms ramp, the release one commit, Reset a commit of
// null (the default: the key removed from project.toml). No DOM.

/** The ramp a drag moves along, against zipper noise. */
export const RAMP_MS = 5;

/**
 * The list: per top-level instance (in the panel's order), its editable
 * parameters (`describe.controls`) and the read-only ones inside it.
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

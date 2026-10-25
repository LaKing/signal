// A discrete parameter's value set (ADR 0043): `[{value, label}]`, as
// `describe.controls[].values` and a descriptor's `values` give it. A
// parameter with one is shown as a selector instead of a slider; the
// server takes the nearest value of any number sent. No DOM.

/** The value of `values` nearest to `value` (the lower of two as near); `value` without a set. */
export function nearestValue(values, value) {
  let best = null;
  for (const entry of values ?? []) {
    if (best === null || Math.abs(entry.value - value) < Math.abs(best - value)) best = entry.value;
  }
  return best ?? value;
}

/** The options of a selector: each value as its text (the option's value) and its name. */
export function selectOptions(values) {
  return (values ?? []).map((entry) => ({ value: String(entry.value), label: entry.label, title: `${entry.label} (${entry.value})` }));
}

/** What a selector shows for `value`: its nearest value's text (an option's value), or '' without a set or a finite value. */
export function selectValue(values, value) {
  if (!values?.length || !Number.isFinite(value)) return '';
  return String(nearestValue(values, value));
}

/**
 * A value as a value-set parameter reads: the name of the entry it is
 * (`saw`), else the nearest entry's name with the number (`saw (1.4)`),
 * or '' for a value that is not a number.
 */
export function valueLabel(values, value) {
  if (!Number.isFinite(value)) return '';
  const exact = (values ?? []).find((entry) => entry.value === value);
  if (exact) return exact.label;
  const near = (values ?? []).find((entry) => entry.value === nearestValue(values, value));
  return near ? `${near.label} (${Number(value.toPrecision(4))})` : String(value);
}

/** Whether a parameter (a row, a control) is discrete: it has a value set. */
export function isDiscrete(parameter) {
  return Array.isArray(parameter?.values) && parameter.values.length > 0;
}

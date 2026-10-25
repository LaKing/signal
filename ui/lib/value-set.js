// A discrete parameter's value set (ADR 0043): `[{value, label}]`, as
// `describe.controls[].values` and a descriptor's `values` give it, with
// its `snap` (`'positive'`, else left out: the nearest). A parameter with
// one is shown as a selector instead of a slider; the server snaps any
// number sent as the modules read it. No DOM.

/**
 * The value of `values` that `value` snaps to, as the modules read it
 * (`signal_core::snap_value`): the nearest, of two as near the greater;
 * with `snap` `'positive'`, the nearest above 0 for a value above 0, else
 * the nearest at or below 0. `value` itself without a set.
 */
export function nearestValue(values, value, snap) {
  const positive = snap === 'positive';
  // Consecutive integers: exactly ⌊x + ½⌋ within the ends, as the modules read them.
  const ends = consecutiveIntegers(values);
  if (!positive && ends) {
    const r = Math.floor(value + 0.5);
    return r >= ends[1] ? ends[1] : r >= ends[0] ? r : ends[0];
  }
  let best = null;
  for (const pass of [true, false]) {
    for (const entry of values ?? []) {
      if (pass && positive && entry.value > 0 !== value > 0) continue;
      const d = Math.abs(entry.value - value);
      const e = best === null ? Infinity : Math.abs(best - value);
      if (best === null || d < e || (d === e && entry.value > best)) best = entry.value;
    }
    if (best !== null || !positive) break;
  }
  return best ?? value;
}

/** The ends of `values` when they are distinct consecutive integers, else null. */
function consecutiveIntegers(values) {
  if (!values?.length) return null;
  const seen = new Set();
  for (const entry of values) {
    if (!Number.isInteger(entry.value) || seen.has(entry.value)) return null;
    seen.add(entry.value);
  }
  const lo = Math.min(...seen);
  const hi = Math.max(...seen);
  return hi - lo === values.length - 1 ? [lo, hi] : null;
}

/** The options of a selector: each value as its text (the option's value) and its name. */
export function selectOptions(values) {
  return (values ?? []).map((entry) => ({ value: String(entry.value), label: entry.label, title: `${entry.label} (${entry.value})` }));
}

/** What a selector shows for `value`: the text of the value it snaps to (an option's value), or '' without a set or a finite value. */
export function selectValue(values, value, snap) {
  if (!values?.length || !Number.isFinite(value)) return '';
  return String(nearestValue(values, value, snap));
}

/**
 * A value as a value-set parameter reads: the name of the entry it is
 * (`saw`), else the name of the entry it snaps to with the number
 * (`saw (1.4)`), or '' for a value that is not a number.
 */
export function valueLabel(values, value, snap) {
  if (!Number.isFinite(value)) return '';
  const exact = (values ?? []).find((entry) => entry.value === value);
  if (exact) return exact.label;
  const near = (values ?? []).find((entry) => entry.value === nearestValue(values, value, snap));
  return near ? `${near.label} (${Number(value.toPrecision(4))})` : String(value);
}

/** Whether a parameter (a row, a control) is discrete: it has a value set. */
export function isDiscrete(parameter) {
  return Array.isArray(parameter?.values) && parameter.values.length > 0;
}

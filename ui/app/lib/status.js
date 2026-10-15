// Signal.app's status, at fixed widths (M2.8-11, docs/ui-controls.md
// "Stable layout"): the toolbar's compact line and the Parameters tab's
// status card. Plain ES module, no DOM.

/** The toolbar status's characters, always. */
export const COMPACT_WIDTH = 25;

/** `text` in exactly `width` characters: padded at `align`, cut with … when longer. */
export function fit(text, width, align = 'end') {
  const s = String(text);
  if (s.length > width) return `${s.slice(0, Math.max(0, width - 1))}…`;
  return align === 'start' ? s.padStart(width) : s.padEnd(width);
}

/** A whole number at most `max`, shown as `max+` above it. */
function capped(n, max) {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  return v > max ? `${max}+` : String(v);
}

/** Seconds as `m:ss` (at most 999:59). */
function clock(seconds) {
  const s = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const minutes = Math.min(999, Math.floor(s / 60));
  const rest = minutes === 999 ? 59 : s % 60;
  return `${minutes}:${String(rest).padStart(2, '0')}`;
}

/**
 * The toolbar's status: off, stopped, or playing with the time, the
 * callback load and the xruns; `COMPACT_WIDTH` characters whatever it shows.
 */
export function compactStatus({ power = 'on', playing = false, status = null } = {}) {
  if (power === 'off') return fit('⏻ off', COMPACT_WIDTH);
  if (!playing || !status) return fit('■ stopped', COMPACT_WIDTH);
  const load = `${capped((status.loadAverage ?? 0) * 100, 999)}%`;
  const text = `▶ ${fit(clock(status.elapsed), 6, 'start')}  ${fit(load, 5, 'start')}  xr ${fit(capped(status.xruns, 9999), 5)}`;
  return fit(text, COMPACT_WIDTH);
}

/** The status card's fields: label and text, each text of its own fixed width. */
export function statusFields(status) {
  const s = status ?? {};
  const percent = (v) => (Number.isFinite(v) ? `${(Math.min(v, 9.999) * 100).toFixed(1)} %` : '—');
  return [
    ['frame', fit(capped(s.frame, 99_999_999_999), 12, 'start')],
    ['elapsed', fit(Number.isFinite(s.elapsed) ? `${Math.min(s.elapsed, 9_999_999).toFixed(3)} s` : '—', 13, 'start')],
    ['xruns', fit(capped(s.xruns, 9999), 5, 'start')],
    ['load avg', fit(percent(s.loadAverage), 7, 'start')],
    ['load worst', fit(percent(s.loadWorst), 7, 'start')],
    ['clipped', fit(capped(s.clipped, 9_999_999), 8, 'start')],
    ['buffer', fit(capped(s.buffer, 99_999), 5, 'start')],
    ['threads', fit(s.threads === undefined ? '—' : `${capped(s.threads, 999)} (${capped(s.workers, 999)} w)`, 11, 'start')],
    ['scheduling', fit(s.scheduling ?? '—', 15)],
  ];
}

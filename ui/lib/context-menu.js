// A context menu's keys and place (the owner's "every node and every cable
// should have its own context menu", 2026-10-17): one model for Signal.app's
// menu and <signal-schematic>'s own (ui/controls/context-menu.js). Items are
// `{id, label, disabled?, title?}`; `{id: 'separator'}` divides groups. A
// disabled item can take the focus (its title says why it is off) but is
// never activated. No DOM.

/** Whether item `item` can take the focus: anything but a separator. */
function focusable(item) {
  return Boolean(item) && item.id !== 'separator';
}

/** The first focusable index from `from` in direction `step` (wrapping), or -1. */
function next(items, from, step) {
  const n = items.length;
  for (let i = 1; i <= n; i++) {
    const at = (((from + step * i) % n) + n) % n;
    if (focusable(items[at])) return at;
  }
  return -1;
}

/** Where the focus goes when the menu opens: the first enabled item, else the first item, else -1. */
export function firstIndex(items) {
  const enabled = items.findIndex((item) => focusable(item) && !item.disabled);
  return enabled >= 0 ? enabled : items.findIndex(focusable);
}

/**
 * What key `key` does in a menu of `items` with the focus at `at`:
 * `{focus: index}` (ArrowDown, ArrowUp, wrapping past separators; Home,
 * End), `{activate: id}` (Enter, Space, on an enabled item), `{close: true}`
 * (Escape, Tab), `{none: true}` (Enter on a disabled item: kept open), or
 * null for a key the menu does not take.
 */
export function menuKey(key, at, items) {
  if (key === 'ArrowDown') return { focus: next(items, at < 0 ? -1 : at, 1) };
  if (key === 'ArrowUp') return { focus: next(items, at < 0 ? 0 : at, -1) };
  if (key === 'Home') return { focus: next(items, -1, 1) };
  if (key === 'End') return { focus: next(items, 0, -1) };
  if (key === 'Escape' || key === 'Tab') return { close: true };
  if (key === 'Enter' || key === ' ') {
    const item = items[at];
    return focusable(item) && !item.disabled ? { activate: item.id } : { none: true };
  }
  return null;
}

/** Where the menu goes: at the click, moved inside the window. */
export function placeMenu(x, y, width, height, windowWidth, windowHeight) {
  const left = Math.max(0, Math.min(x, windowWidth - width));
  const top = Math.max(0, Math.min(y, windowHeight - height));
  return { left, top };
}

// Signal.app's context menu (owner's request, 2026-10-16): instead of the
// WebView's own (Reload, Open Frame in New Window, …), which do nothing
// useful here. A text field keeps the system's menu (Cut, Copy, Paste).
// Plain ES module, no DOM.

import { TABS } from './tabs.js';

/**
 * Whether a right click on `target` keeps the system's menu: in a text
 * field or an editable element, so Cut, Copy and Paste stay.
 */
export function keepsSystemMenu(target) {
  const tag = String(target?.tagName ?? '').toLowerCase();
  if (tag === 'textarea' || target?.isContentEditable) return true;
  if (tag !== 'input') return false;
  const type = String(target.type ?? 'text').toLowerCase();
  return !['range', 'button', 'checkbox', 'radio', 'submit', 'reset', 'color', 'file'].includes(type);
}

/**
 * The menu's items, in order: for a control bound to a parameter
 * (`control: { address, label, editable }`), Reset to default and Copy
 * address; for a part of the schematic (`control: { address, part: true }`:
 * an instance, a port or a wire, M2.9-1), Copy address; then Play or Stop;
 * then the tabs other than the shown one.
 * Each item is `{ id, label, disabled? }`; `separator` items divide groups.
 */
export function contextItems({ control = null, playing = false, power = true, tab = null } = {}) {
  const items = [];
  if (control?.address && control.part) {
    items.push({ id: 'copy-address', label: 'Copy address' });
    items.push({ id: 'separator' });
  } else if (control?.address) {
    const name = control.label || control.address;
    items.push({ id: 'reset', label: `Reset ${name} to default`, disabled: control.editable === false });
    items.push({ id: 'copy-address', label: 'Copy address' });
    items.push({ id: 'separator' });
  }
  items.push(playing ? { id: 'stop', label: 'Stop' } : { id: 'play', label: 'Play', disabled: !power });
  items.push({ id: 'separator' });
  for (const t of TABS) if (t.id !== tab) items.push({ id: `tab:${t.id}`, label: `Show ${t.label}` });
  return items;
}

/** Where the menu goes: at the click, moved inside the window. */
export function placeMenu(x, y, width, height, windowWidth, windowHeight) {
  const left = Math.max(0, Math.min(x, windowWidth - width));
  const top = Math.max(0, Math.min(y, windowHeight - height));
  return { left, top };
}

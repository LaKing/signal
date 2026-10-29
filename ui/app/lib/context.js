// Signal.app's context menu (owner's request, 2026-10-16): instead of the
// WebView's own (Reload, Open Frame in New Window, …), which do nothing
// useful here. A text field keeps the system's menu (Cut, Copy, Paste).
// A right click on the schematic shows the schematic's own items first
// (ui/lib/schematic-menu.js, 2026-10-17): one menu, not two. Its keys and
// its place are ui/lib/context-menu.js's. Plain ES module, no DOM.

import { tabItems } from './show-in.js';

export { placeMenu } from '../../lib/context-menu.js';

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
 * The menu's items, in order: the schematic's items for what was clicked
 * (`part`: `schematicMenu`'s, an instance, a pin, a wire or the background),
 * their ids prefixed `part:`; else for a control bound to a parameter
 * (`control: { address, label, editable }`), Reset to default and Copy
 * address; for a part of the schematic without its items (`control: {
 * address, part: true }`), Copy address; then Restart from Files (the
 * project again from frame 0, ADR 0033: no Play or Stop, the project runs
 * while the power is on); then the tabs other than the shown one: Show
 * in Panel and so on when what was clicked has an address (`places`, its
 * places in the views: ./show-in.js), else Show Panel and so on.
 * Each item is `{ id, label, disabled?, title? }`; `separator` items divide groups.
 */
export function contextItems({ part = null, control = null, power = true, tab = null, places = null } = {}) {
  const items = [];
  if (part?.length) {
    for (const item of part) items.push(item.id === 'separator' ? item : { ...item, id: `part:${item.id}` });
    items.push({ id: 'separator' });
  } else if (control?.address && control.part) {
    items.push({ id: 'copy-address', label: 'Copy address' });
    items.push({ id: 'separator' });
  } else if (control?.address) {
    const name = control.label || control.address;
    items.push({ id: 'reset', label: `Reset ${name} to default`, disabled: control.editable === false });
    items.push({ id: 'copy-address', label: 'Copy address' });
    items.push({ id: 'separator' });
  }
  items.push({ id: 'restart', label: 'Restart from Files', disabled: !power });
  items.push({ id: 'separator' });
  items.push(...tabItems(tab, places));
  return items;
}

// Show in Panel, Show in Parameters, Show in Schematic (the owner's,
// 2026-10-29: "instead of Show Panel … it should be Show in Panel …, and
// on click it should somehow indicate which item it is about … so that in
// the given view you can identify the given connection"). The context
// menu's tab items, for what was right-clicked: its subject (an instance,
// a pin or parameter, or a wire's two ends), where each view has it, and
// the items. Plain ES module, no DOM: app.js finds the elements and
// flashes them.

import { TABS } from './tabs.js';

/** The tabs that can show a subject; the others (Setup) are switched to plainly. */
export const SHOWN_IN = Object.freeze(['panel', 'parameters', 'schematic']);

/** How long a shown element is marked (ms): the flash's length, with a margin. */
export const FLASH_MS = 1300;

/** The class of a shown element (app.css has none: `FLASH_CSS` is put into each document that shows one). */
export const FLASH_CLASS = 'signal-shown-flash';

/**
 * The mark: an outline and a tint pulsing twice; with reduced motion, the
 * same outline, still, fading once.
 */
export const FLASH_CSS = `
.${FLASH_CLASS} { outline: 2px solid var(--signal-focus, #5aa9ff); outline-offset: 2px; animation: signal-shown-pulse 0.3s ease-in-out 4 alternate; }
@keyframes signal-shown-pulse {
  from { outline-color: var(--signal-focus, #5aa9ff); background-color: color-mix(in srgb, var(--signal-focus, #5aa9ff) 30%, transparent); }
  to { outline-color: transparent; background-color: transparent; }
}
@media (prefers-reduced-motion: reduce) {
  .${FLASH_CLASS} { animation: signal-shown-fade ${FLASH_MS}ms ease-in 1 forwards; }
}
@keyframes signal-shown-fade {
  from { outline-color: var(--signal-focus, #5aa9ff); }
  to { outline-color: transparent; }
}`;

const PLACES = { panel: 'the panel', parameters: 'the Parameters tab', schematic: 'the schematic' };

/** Whether `address` is an instance's (`/drums/kick`), not a port's or a parameter's (`/drums/kick.out`). */
export function isInstance(address) {
  const text = String(address ?? '');
  return !text.slice(text.lastIndexOf('/') + 1).includes('.');
}

/** Whether `address` is inside instance `instance`: one of its ports or parameters, or of an instance in it. */
export function within(address, instance) {
  return address.startsWith(`${instance}.`) || address.startsWith(`${instance}/`);
}

/**
 * What a right click is about: of the schematic, its `hit` (an instance,
 * a pin or a collapsed group's box: `{kind: 'instance' | 'pin', address}`;
 * a wire: `{kind: 'wire', from, to}`); else a `control` (a panel control,
 * a parameter row, a schematic part): `{kind, address}`. Null for the
 * background or a bundle: nothing in particular.
 */
export function subjectOf({ hit = null, control = null } = {}) {
  if (hit) {
    if (hit.kind === 'wire') return hit.from && hit.to ? { kind: 'wire', from: hit.from, to: hit.to } : null;
    if (['instance', 'port', 'group'].includes(hit.kind) && hit.address) return { kind: hit.kind === 'port' ? 'pin' : 'instance', address: hit.address };
    return null;
  }
  if (control?.address) return { kind: isInstance(control.address) ? 'instance' : 'pin', address: control.address };
  return null;
}

/** The addresses a subject is: its own, or a wire's two ends. */
export function subjectAddresses(subject) {
  if (!subject) return [];
  return subject.kind === 'wire' ? [subject.from, subject.to] : [subject.address];
}

/**
 * Where a view (`shown`: the addresses it has, a control's, a row's, a
 * card's) has `subject`: `{addresses, whole}`. The subject's own addresses
 * first (a wire: each end it has); else, for an instance, what is inside it
 * (`whole`: shown by the card or group holding them).
 */
export function matchIn(subject, shown) {
  const have = new Set(shown ?? []);
  const own = subjectAddresses(subject).filter((a) => have.has(a));
  if (own.length || subject?.kind !== 'instance') return { addresses: own, whole: false };
  const inner = [...have].filter((a) => within(a, subject.address));
  return { addresses: inner, whole: inner.length > 0 };
}

/** Why `subject` is not in `tab`: the item's title. */
export function missingReason(subject, tab) {
  const place = PLACES[tab] ?? tab;
  if (subject.kind === 'wire') return `Neither end of ${subject.from} → ${subject.to} is in ${place}`;
  return `${subject.address} is not in ${place}`;
}

/**
 * Where each view has `subject`: `{panel, parameters, schematic}`, each
 * `{addresses, whole, reason}` (`reason` when it has nothing). `views` are
 * the addresses each has.
 */
export function placesOf(subject, views = {}) {
  const places = {};
  for (const tab of SHOWN_IN) {
    const found = matchIn(subject, views[tab]);
    places[tab] = found.addresses.length ? found : { ...found, reason: missingReason(subject, tab) };
  }
  return places;
}

/**
 * The tab items: for a subject (`places` given), Show in Panel, Show in
 * Parameters, Show in Schematic (`show:<tab>`; disabled with its reason
 * where the view has nothing of it) and Show Setup; without one, Show and
 * each tab's name (`tab:<tab>`). Never the shown tab.
 */
export function tabItems(tab, places = null) {
  const items = [];
  for (const t of TABS) {
    if (t.id === tab) continue;
    const place = places && SHOWN_IN.includes(t.id) ? places[t.id] : null;
    if (!place) items.push({ id: `tab:${t.id}`, label: `Show ${t.label}` });
    else if (place.reason) items.push({ id: `show:${t.id}`, label: `Show in ${t.label}`, disabled: true, title: place.reason });
    else items.push({ id: `show:${t.id}`, label: `Show in ${t.label}` });
  }
  return items;
}

// The schematic's context menu (the owner's "every node and every cable
// should have its own context menu", 2026-10-17; docs/ui-controls.md
// "Schematic"): the items for what a right click is over (an instance, a
// wire, a pin, the background) in the schematic's state, and the ops of
// "Disconnect all". The same items show in Signal.app's menu, before its
// own, and in <signal-schematic>'s menu in a browser. An item that does not
// apply is disabled, its title saying why. No DOM.

import { removeConnection, written } from './schematic-patch.js';

const SEPARATOR = { id: 'separator' };
/** Trace signal (ADR 0030 §1): the part's path upstream and downstream, across composites; T on the selection. */
const TRACE = { id: 'trace', label: 'Trace signal' };
/** Why an edit is off outside Edit patch. */
export const EDIT_OFF = 'Turn on Edit patch to change the patch';

/** `item`, disabled with `reason` when `reason` is not null. */
function unless(item, reason) {
  return reason ? { ...item, disabled: true, title: reason } : { ...item, disabled: false };
}

/**
 * The menu of `hit` (`hitTest`'s: `{kind, node?, port?, wire?, …}`) in
 * `state`: `editing` (Edit patch on), and for an instance whether its
 * composite is `open` in place and stays open by the zoom alone (`wide`)
 * and whether something can be added inside it (`addable`); for a pin how
 * many connections reach it at its level (`connected`) and how many of the
 * level's exports (`exported`, which are not edited here), and for a
 * composite's pin whether its inside is not in `graph` (`opaque`, a library
 * composite: Go inside off; a box pin goes inside, a frame pin outside); for the background
 * the level shown (`level`, `""` the top level) and `addable`. A part that
 * is one of several selected (`group`: how many) ends with Delete selected,
 * which removes them all (`deletePlan`, ui/lib/schematic-select.js),
 * counting what it would delete (`deleted`), off with `tooLarge`'s reason.
 * `around` (M2.13-3): why the part's level is not edited now, it being
 * around the innermost open composite; its edits are off with it (an open
 * composite's own Add module here… adds inside it and stays). A library
 * composite (`opaque`) is not opened in place. Items are
 * `{id, label, disabled, title?}` and separators, `{id: 'separator'}`.
 */
export function schematicMenu(hit, state = {}) {
  const items = partMenu(hit, state);
  if (hit?.kind !== 'background' && hit && state.group > 1) {
    // Counted as Delete would delete (`deleted`, `deletePlan`'s): pins and exports stay.
    const n = state.deleted ?? state.group;
    const why = state.editing ? (state.around ?? (n > 0 ? (state.tooLarge ?? null) : 'Nothing selected is deleted: pins and exports stay')) : EDIT_OFF;
    items.push(SEPARATOR, unless({ id: 'delete-selected', label: `Delete selected (${n})` }, why));
  }
  return items;
}

function partMenu(hit, state) {
  const editing = Boolean(state.editing);
  const inside = editing ? null : EDIT_OFF;
  const editOff = inside ?? state.around ?? null;
  // A collapsed port group (M2.13-2): its members inspected or traced together; zoom in to work on one.
  if (hit?.kind === 'group') return [unless({ id: 'inspect', label: 'Inspect' }, null), unless(TRACE, null)];
  if (hit?.kind === 'instance') {
    const composite = Boolean(hit.node?.composite);
    const notComposite = composite ? null : 'A primitive has no inside';
    const items = [unless({ id: 'inspect', label: 'Inspect' }, null), unless(TRACE, null)];
    if (composite && state.open) items.push(unless({ id: 'close', label: 'Close' }, state.wide ? 'Open by the zoom: zoom out to close it' : null));
    else items.push(unless({ id: 'open', label: 'Open in place' }, notComposite ?? (state.opaque ? 'A library composite: its inside is not in the graph' : null)));
    items.push(unless({ id: 'enter', label: 'Enter' }, notComposite), SEPARATOR, unless({ id: 'copy-address', label: 'Copy address' }, null), SEPARATOR);
    if (composite && state.open) items.push(unless({ id: 'add', label: 'Add module here…' }, inside ?? (state.addable ? null : 'Nothing can be added here')));
    items.push(unless({ id: 'delete', label: 'Delete instance' }, editOff));
    return items;
  }
  if (hit?.kind === 'wire') {
    const exported = hit.wire?.kind === 'connection' ? null : 'An export is not edited here';
    return [
      unless({ id: 'inspect', label: 'Inspect' }, null),
      unless(TRACE, null),
      unless({ id: 'scale', label: 'Edit scale…' }, editOff ?? exported),
      SEPARATOR,
      unless({ id: 'source', label: 'Go to source' }, null),
      unless({ id: 'target', label: 'Go to target' }, null),
      unless({ id: 'copy-address', label: 'Copy addresses' }, null),
      SEPARATOR,
      unless({ id: 'delete', label: 'Delete connection' }, editOff ?? exported),
    ];
  }
  if (hit?.kind === 'port') {
    // A boundary pin (2026-10-20): to its other side, the composite's inside or around it.
    const across = [];
    if (hit.node?.kind === 'instance' && hit.node.composite) across.push(unless({ id: 'go-inside', label: 'Go inside' }, state.opaque ? 'A library composite: its inside is not in the graph' : null));
    if (hit.node && hit.node.kind !== 'instance') across.push(unless({ id: 'go-outside', label: 'Go outside' }, null));
    return [
      unless({ id: 'inspect', label: 'Inspect' }, null),
      unless(TRACE, null),
      ...across,
      unless({ id: 'copy-address', label: 'Copy address' }, null),
      SEPARATOR,
      unless({ id: 'disconnect', label: 'Disconnect all' }, editOff ?? (state.connected > 0 ? null : state.exported > 0 ? 'Only an export reaches it: exports are not edited here' : 'Nothing is connected here')),
    ];
  }
  const level = state.level ? state.level : 'top level';
  return [
    unless({ id: 'add', label: 'Add module here…' }, editOff ?? (state.addable ? null : 'Nothing can be added here')),
    SEPARATOR,
    unless({ id: 'fit', label: 'Fit' }, null),
    unless({ id: 'reset-layout', label: `Reset layout (${level})`, title: `Forget the moved positions of ${state.level ? state.level : 'the top level'}` }, null),
    SEPARATOR,
    unless({ id: 'edit', label: editing ? 'Leave Edit patch' : 'Edit patch' }, null),
  ];
}

/**
 * "Disconnect all" on pin `port` (`{name, direction}`) of instance
 * `instance` in `levelGraph` (`graph`'s level): one `connection.remove` op
 * per connection from an output or to an input or parameter, in the
 * level's order, written as one patch. Exports are not edited.
 */
export function disconnectOps(levelGraph, instance, port) {
  const end = `${instance}.${port.name}`;
  const side = port.direction === 'out' ? 'from' : 'to';
  return (levelGraph?.connections ?? []).filter((c) => written(c[side]) === end).map((c) => removeConnection(c.from, c.to));
}

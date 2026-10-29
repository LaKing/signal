// The schematic's context menu (the owner's "every node and every cable
// should have its own context menu", 2026-10-17; docs/ui-controls.md
// "Schematic"): the items for what a right click is over (an instance, a
// wire, a pin, the background) in the schematic's state, and the ops of
// "Disconnect all". The same items show in Signal.app's menu, before its
// own, and in <signal-schematic>'s menu in a browser. An item that does not
// apply is disabled, its title saying why. No DOM.

import { removeConnection, written } from './schematic-patch.js';
import { insideReason } from './slot.js';

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
 * composite is `open` in place (Close then, as its × does; 2026-10-22)
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
 * composite's own Add module here… adds inside it and stays; `shown`: the
 * part is the open frame whose inside is edited, or a pin of its box, so
 * a slot's own items, its pins' and Delete stay on too, 2026-10-29). A
 * library composite (`opaque`) is not opened in place. Items are `{id,
 * label, disabled, title?}` and separators, `{id: 'separator'}`.
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
    const slot = hit.node?.slot ?? null;
    const notComposite = composite ? null : slot ? (insideReason(slot) ?? (slot.load ? 'Its content is not a project composite' : 'An empty slot has no inside')) : 'A primitive has no inside';
    const items = [unless({ id: 'inspect', label: 'Inspect' }, null), unless(TRACE, null)];
    if (composite && state.open) items.push(unless({ id: 'close', label: 'Close' }, null));
    else items.push(unless({ id: 'open', label: 'Open in place' }, notComposite ?? (state.opaque ? 'A library composite: its inside is not in the graph' : null)));
    items.push(unless({ id: 'enter', label: 'Enter' }, notComposite), SEPARATOR, unless({ id: 'copy-address', label: 'Copy address' }, null), SEPARATOR);
    // The open frame whose inside is edited (`shown`, 2026-10-29): its own items and Delete act on it from there (Delete closes it).
    const own = state.shown ? inside : editOff;
    if (slot) items.push(...slotItems(slot, own, notComposite), SEPARATOR);
    if (composite && state.open) items.push(unless({ id: 'add', label: 'Add module here…' }, inside ?? (state.addable ? null : 'Nothing can be added here')));
    if (composite && state.open) items.push(unless({ id: 'add-slot', label: 'Add slot here…' }, inside ?? (state.addable ? null : 'Nothing can be added here')));
    items.push(unless({ id: 'make-interface', label: 'Make interface from this instance…', title: 'Write its ports and parameters as interfaces/<name>.toml, a template slots may start from' }, own));
    items.push(unless({ id: 'delete', label: slot ? 'Delete slot' : 'Delete instance' }, own));
    return items;
  }
  if (hit?.kind === 'wire') {
    const exported = hit.wire?.kind === 'connection' ? null : 'An export is not edited here';
    // ADR 0037, editable insides: a slot inside's frame wire (an export op) is deleted, not scaled.
    const frame = hit.wire?.kind === 'export' && hit.wire.written?.role ? null : exported;
    return [
      unless({ id: 'inspect', label: 'Inspect' }, null),
      unless(TRACE, null),
      unless({ id: 'scale', label: 'Edit scale…' }, editOff ?? exported),
      SEPARATOR,
      unless({ id: 'source', label: 'Go to source' }, null),
      unless({ id: 'target', label: 'Go to target' }, null),
      unless({ id: 'copy-address', label: 'Copy addresses' }, null),
      SEPARATOR,
      unless({ id: 'delete', label: 'Delete connection' }, editOff ?? frame),
    ];
  }
  if (hit?.kind === 'port') {
    // A boundary pin (2026-10-20): to its other side, the composite's inside or around it.
    const across = [];
    if (hit.node?.kind === 'instance' && hit.node.composite) across.push(unless({ id: 'go-inside', label: 'Go inside' }, state.opaque ? 'A library composite: its inside is not in the graph' : null));
    if (hit.node && hit.node.kind !== 'instance') across.push(unless({ id: 'go-outside', label: 'Go outside' }, null));
    const items = [
      unless({ id: 'inspect', label: 'Inspect' }, null),
      unless(TRACE, null),
      ...across,
      unless({ id: 'copy-address', label: 'Copy address' }, null),
      SEPARATOR,
      unless({ id: 'disconnect', label: 'Disconnect all' }, editOff ?? (state.connected > 0 ? null : state.exported > 0 ? 'Only an export reaches it: exports are not edited here' : 'Nothing is connected here')),
    ];
    // A slot's pin (ADR 0037 §7): its port renamed, retyped or removed.
    if (hit.node?.kind === 'instance' && hit.node.slot) {
      const wired = state.connected > 0;
      // Its interface: edited from the slot's inside too (`shown`).
      const off = state.shown ? inside : editOff;
      items.push(
        SEPARATOR,
        unless({ id: 'port-edit', label: 'Rename or retype port…' }, off),
        unless({ id: 'port-remove', label: 'Remove port' }, off ?? (wired ? 'Wires name it: remove it with its wires' : null)),
        unless({ id: 'port-remove-wires', label: 'Remove port and its wires' }, off ?? (wired ? null : 'No wire names it')),
      );
    }
    return items;
  }
  const level = state.level ? state.level : 'top level';
  // ADR 0037, editable insides: inside a slot, its interface (the frame's pins) is edited from here too.
  const insideItems = state.slotInside ? [unless({ id: 'slot-interface-inside', label: 'Edit the slot’s interface…', title: 'Add, remove, rename or retype the frame’s pins: the slot’s ports' }, editOff)] : [];
  return [
    unless({ id: 'add', label: 'Add module here…' }, editOff ?? (state.addable ? null : 'Nothing can be added here')),
    unless({ id: 'add-slot', label: 'Add slot here…', title: 'A slot: an instance whose ports are its own interface, and whose content is loaded into it (ADR 0037)' }, editOff ?? (state.addable ? null : 'Nothing can be added here')),
    ...insideItems,
    SEPARATOR,
    unless({ id: 'fit', label: 'Fit' }, null),
    unless({ id: 'reset-layout', label: `Reset layout (${level})`, title: `Forget the moved positions of ${state.level ? state.level : 'the top level'}` }, null),
    SEPARATOR,
    unless({ id: 'edit', label: editing ? 'Leave Edit patch' : 'Edit patch' }, null),
  ];
}

/**
 * A slot box's items (ADR 0037 §7): Load… when it is empty, Replace… and
 * Unload when loaded, Edit map…, Edit interface…, Save interface as
 * template… and Detach from template (off without a template); each off
 * with `editOff`'s reason outside Edit patch.
 */
function slotItems(slot, editOff, closed = null) {
  const loaded = typeof slot.load === 'string' && slot.load !== '';
  const template = slot.from ?? slot.interface?.from ?? null;
  return [
    // ADR 0037, editable insides: open it to edit what is inside (read-only until the first edit makes its own copy).
    unless({ id: 'slot-inside', label: 'Edit inside', title: 'Open the slot in place: what you change there becomes its own inside' }, closed),
    loaded ? unless({ id: 'slot-replace', label: 'Replace…', title: `Load other content in place of ${slot.load}` }, editOff) : unless({ id: 'slot-load', label: 'Load…' }, editOff),
    unless({ id: 'slot-unload', label: 'Unload' }, editOff ?? (loaded ? null : 'The slot is empty')),
    unless({ id: 'slot-map', label: 'Edit map…' }, editOff),
    unless({ id: 'slot-interface', label: 'Edit interface…' }, editOff),
    unless({ id: 'slot-save-template', label: 'Save interface as template…' }, editOff),
    unless({ id: 'slot-detach', label: 'Detach from template' }, editOff ?? (template ? null : 'Its interface has no template')),
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

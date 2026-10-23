// The Settings page's Keyboard Shortcuts (ADR 0034): what it lists (the
// commands by scope, searched), what differs from the built-in keymap, what
// a chord about to be added collides with, and the messages that write the
// person's keys.toml (`keys.set`, `keys.reset`, docs/ui-protocol.md). The
// keymap is ui/lib/keys.js's. Plain ES module, no DOM.

import { ancestors, describeConflict, eventChord, formatChord, isWheel, parseChord, sameKeys, wheelChord } from '../../lib/keys.js';

/** Lower-case words of `text`. */
function words(text) {
  return String(text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * The groups the page lists: each scope (in the keymap's order) with its
 * commands, `{scope, label, about, fixed, commands: [{id, label, keys,
 * defaults, overridden, fixed, wheel, conflicts}]}`, only those matching
 * `search` (every word in the command's name, id, chords or scope name),
 * empty groups left out. `conflicts` are the keymap's that name the command,
 * as sentences.
 */
export function groups(map, { search = '', mac = true } = {}) {
  const wanted = words(search);
  const out = [];
  for (const scope of map.scopes.values()) {
    const commands = [];
    for (const c of map.commands.values()) {
      if (c.scope !== scope.id) continue;
      const shown = c.keys.map((k) => formatChord(k, { mac }));
      const haystack = `${c.label} ${c.id} ${scope.label} ${shown.join(' ')} ${c.keys.join(' ')}`.toLowerCase();
      if (!wanted.every((w) => haystack.includes(w))) continue;
      const conflicts = map.conflicts.filter((x) => x.command === c.id || x.with === c.id).map((x) => describeConflict(map, x, { mac }));
      commands.push({ id: c.id, label: c.label, keys: c.keys, defaults: c.defaults, overridden: c.overridden, fixed: c.fixed, wheel: c.wheel, fine: c.fine, conflicts });
    }
    if (commands.length) out.push({ scope: scope.id, label: scope.label, about: scope.about, fixed: scope.fixed, commands });
  }
  return out;
}

/** How many commands differ from the built-in keymap. */
export function changedCount(map) {
  let n = 0;
  for (const c of map.commands.values()) if (c.overridden) n += 1;
  return n;
}

/**
 * What `chord` would collide with if command `id` took it: the commands
 * that hold it in `id`'s scope (`same`: one of the two would not get it),
 * in a parent scope (`hides`: the new one hides it there) or in a scope
 * under it (`hidden`: there it does the other command). `[{command, label,
 * scope, scopeLabel, kind}]`.
 */
export function collisions(map, id, chord) {
  const command = map.commands.get(id);
  if (!command) return [];
  const found = [];
  const add = (scope, kind) => {
    const holder = map.bindings.get(scope)?.get(chord);
    if (holder && holder !== id) found.push({ command: holder, label: map.commands.get(holder)?.label ?? holder, scope, scopeLabel: map.scopes.get(scope)?.label ?? scope, kind });
  };
  add(command.scope, 'same');
  for (const parent of ancestors(map.scopes, command.scope)) add(parent, 'hides');
  for (const scope of map.scopes.keys()) {
    if (scope !== command.scope && ancestors(map.scopes, scope).includes(command.scope)) add(scope, 'hidden');
  }
  return found;
}

/** A collision as a sentence for the page. */
export function describeCollision(c, chord, { mac = true } = {}) {
  const shown = formatChord(chord, { mac });
  if (c.kind === 'same') return `${shown} is ${c.scopeLabel} ▸ ${c.label}`;
  if (c.kind === 'hides') return `${shown} is also ${c.scopeLabel} ▸ ${c.label}: this one wins where it applies`;
  return `${shown} is ${c.scopeLabel} ▸ ${c.label} there`;
}

/** The chord a key press records for command `command` (`{wheel}`: a wheel event's), or null (a modifier alone). */
export function recorded(event, command, { mac = true } = {}) {
  if (command?.wheel) return event?.type === 'wheel' ? wheelChord(event, { mac }) : null;
  return event?.type === 'wheel' ? null : eventChord(event, { mac });
}

/** A chord typed as text for command `command`: `{chord}` or `{error}`. */
export function typed(text, command) {
  const parsed = parseChord(text);
  if (parsed.error) return parsed;
  if (isWheel(parsed.chord) !== Boolean(command?.wheel)) return { error: command?.wheel ? 'this takes a wheel chord (Wheel, Shift+Wheel, …)' : 'this takes a key, not the wheel' };
  return parsed;
}

/** `keys.set` giving command `id` the chords `keys`; null: back to the built-in ones. */
export function setMessage(id, keys) {
  return { type: 'keys.set', command: id, keys: keys === null ? null : [...keys] };
}

/** `keys.reset` of command `id`, or (without one) of every command. */
export function resetMessage(id = null) {
  return id ? { type: 'keys.reset', command: id } : { type: 'keys.reset' };
}

/**
 * The writes that give command `id` `chord`: its keys with the chord added,
 * and with `replace`, the same-scope commands that held it without it.
 * A chord it already has writes nothing.
 */
export function addWrites(map, id, chord, { replace = false } = {}) {
  const command = map.commands.get(id);
  if (!command || command.keys.includes(chord)) return [];
  const writes = [];
  if (replace) {
    for (const c of collisions(map, id, chord)) {
      if (c.kind !== 'same') continue;
      const other = map.commands.get(c.command);
      if (other && !other.fixed) writes.push(keysWrite(other, other.keys.filter((k) => k !== chord)));
    }
  }
  writes.push(keysWrite(command, [...command.keys, chord]));
  return writes;
}

/** The write that removes `chord` from command `id`. */
export function removeWrite(map, id, chord) {
  const command = map.commands.get(id);
  if (!command || !command.keys.includes(chord)) return null;
  return keysWrite(command, command.keys.filter((k) => k !== chord));
}

/** `keys` for `command`: back to the built-in ones (null) when they are those. */
function keysWrite(command, keys) {
  return setMessage(command.id, sameKeys(keys, command.defaults) ? null : keys);
}

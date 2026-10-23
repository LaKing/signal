// Keyboard shortcuts and wheel gestures (ADR 0034): the built-in keymap
// (./keys.default.js, a JSON object: the single source), chords as text, the keymap with
// the person's overrides applied (the `keys` message, docs/ui-protocol.md),
// its conflicts, and the lookup a control makes for a key or a wheel event.
// The server checks the same file with the same rules
// (crates/signal-ui/src/keys.rs; both read ./fixtures/keys-cases.json in
// their tests). Plain ES module, no DOM.

import DEFAULTS from './keys.default.js';

export { DEFAULTS };

/** The modifiers, in the order a chord is written (as macOS shows them: ⌃⌥⇧⌘). */
export const MODIFIERS = Object.freeze(['Ctrl', 'Alt', 'Shift', 'Cmd']);

const MODIFIER_NAMES = new Map([
  ['ctrl', 'Ctrl'], ['control', 'Ctrl'], ['⌃', 'Ctrl'],
  ['alt', 'Alt'], ['option', 'Alt'], ['opt', 'Alt'], ['⌥', 'Alt'],
  ['shift', 'Shift'], ['⇧', 'Shift'],
  ['cmd', 'Cmd'], ['command', 'Cmd'], ['meta', 'Cmd'], ['super', 'Cmd'], ['⌘', 'Cmd'],
]);

/** Named keys by their lower-case spelling (and aliases). */
const NAMED = new Map([
  ['space', 'Space'], ['enter', 'Enter'], ['return', 'Enter'], ['tab', 'Tab'],
  ['escape', 'Escape'], ['esc', 'Escape'], ['backspace', 'Backspace'],
  ['delete', 'Delete'], ['del', 'Delete'],
  ['arrowup', 'ArrowUp'], ['up', 'ArrowUp'], ['arrowdown', 'ArrowDown'], ['down', 'ArrowDown'],
  ['arrowleft', 'ArrowLeft'], ['left', 'ArrowLeft'], ['arrowright', 'ArrowRight'], ['right', 'ArrowRight'],
  ['pageup', 'PageUp'], ['pgup', 'PageUp'], ['pagedown', 'PageDown'], ['pgdn', 'PageDown'],
  ['home', 'Home'], ['end', 'End'], ['contextmenu', 'ContextMenu'], ['menu', 'ContextMenu'],
  ['wheel', 'Wheel'], ['plus', '+'], ['minus', '-'], ['comma', ','], ['period', '.'],
]);
for (let n = 1; n <= 20; n += 1) NAMED.set(`f${n}`, `F${n}`);

/** Whether `key` (a chord's key) is a letter: Shift is part of its chord. */
function isLetter(key) {
  return key.length === 1 && /\p{L}/u.test(key) && key.toUpperCase() !== key.toLowerCase();
}

/**
 * The canonical text of chord `text` (`"cmd+shift+n"` → `"Shift+Cmd+N"`),
 * or `{error}` saying why it is not one. Shift goes with a letter or a named
 * key, never with another character: write the character it types (`+`,
 * not `Shift+=`), which is what a keyboard layout gives.
 */
export function parseChord(text) {
  const raw = String(text ?? '').trim();
  if (!raw) return { error: 'an empty chord' };
  let tokens;
  if (raw === '+') tokens = ['+'];
  else if (raw.endsWith('++')) tokens = [...raw.slice(0, -2).split('+'), '+'];
  else tokens = raw.split('+');
  tokens = tokens.map((t) => t.trim());
  const keyToken = tokens.pop();
  const present = new Set();
  for (const token of tokens) {
    const name = MODIFIER_NAMES.get(token.toLowerCase());
    if (!name) return { error: token ? `'${token}' is not a modifier (Ctrl, Alt, Shift, Cmd)` : `'${raw}' has an empty part` };
    if (present.has(name)) return { error: `${name} twice` };
    present.add(name);
  }
  if (!keyToken) return { error: `'${raw}' has no key` };
  let key = NAMED.get(keyToken.toLowerCase());
  if (!key) {
    if ([...keyToken].length !== 1) return { error: `'${keyToken}' is not a key (a letter, a digit, a character, or a name such as PageDown)` };
    if (MODIFIER_NAMES.has(keyToken.toLowerCase())) return { error: `'${raw}' has no key` };
    key = isLetter(keyToken) ? keyToken.toUpperCase() : keyToken;
  }
  if (/\s/u.test(key)) return { error: 'write Space for the space bar' };
  if (present.has('Shift') && key.length === 1 && !isLetter(key)) {
    return { error: `Shift with '${key}': write the character Shift types, without Shift` };
  }
  return { chord: [...MODIFIERS.filter((m) => present.has(m)), key].join('+') };
}

/** The canonical chord of `text`, or null when it is not one. */
export function chordOf(text) {
  return parseChord(text).chord ?? null;
}

/** A chord's modifiers and key: `{mods: Set, key}`. */
export function splitChord(chord) {
  const parts = chord === '+' ? ['+'] : chord.endsWith('++') ? [...chord.slice(0, -2).split('+'), '+'] : chord.split('+');
  const key = parts.pop();
  return { mods: new Set(parts), key };
}

/** Why a menu item's chord is refused. */
export const MENU_CHORD = 'a menu command needs Cmd, Ctrl or Alt (or a function key)';

/**
 * Whether `chord` may be a menu item's: with Cmd, Ctrl or Alt, or a
 * function key (a bare key would be taken from every text field by the
 * native menu).
 */
export function menuChord(chord) {
  const { mods, key } = splitChord(chord);
  return mods.has('Cmd') || mods.has('Ctrl') || mods.has('Alt') || /^F\d+$/.test(key);
}

/** Whether `chord` is a wheel chord (its key is Wheel). */
export function isWheel(chord) {
  return splitChord(chord).key === 'Wheel';
}

const GLYPHS = { Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Cmd: '⌘' };
const WORDS = { Ctrl: 'Ctrl+', Alt: 'Alt+', Shift: 'Shift+', Cmd: 'Ctrl+' };
const KEY_GLYPHS = {
  ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→', Enter: '↩', Backspace: '⌫',
  Delete: '⌦', Tab: '⇥', Escape: 'Esc', PageUp: 'Page Up', PageDown: 'Page Down', ContextMenu: 'Menu',
};

/** How a chord is shown: macOS glyphs (`⇧⌘Z`, `⌥ Wheel`), or words elsewhere (`Ctrl+Shift+Z`). */
export function formatChord(chord, { mac = true } = {}) {
  const { mods, key } = splitChord(chord);
  const shown = KEY_GLYPHS[key] ?? key;
  if (!mac) return `${MODIFIERS.filter((m) => mods.has(m) && !(m === 'Ctrl' && mods.has('Cmd'))).map((m) => WORDS[m]).join('')}${shown}`;
  const glyphs = MODIFIERS.filter((m) => mods.has(m)).map((m) => GLYPHS[m]).join('');
  return glyphs && shown.length > 1 ? `${glyphs} ${shown}` : `${glyphs}${shown}`;
}

/** Whether the page runs on a Mac (⌘ is Cmd; elsewhere Ctrl acts as Cmd). */
export function isMac(nav = globalThis.navigator) {
  return /Mac|iPhone|iPad/.test(nav?.platform ?? nav?.userAgent ?? 'Mac');
}

const EVENT_NAMES = { ' ': 'Space', Spacebar: 'Space', Esc: 'Escape', Up: 'ArrowUp', Down: 'ArrowDown', Left: 'ArrowLeft', Right: 'ArrowRight', Del: 'Delete', Apps: 'ContextMenu' };
const IGNORED = new Set(['Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'Fn', 'FnLock', 'OS', 'Hyper', 'Super', 'Process', 'Unidentified', 'AltGraph']);
/** What a key's code types without a modifier on a US layout (⌥ on a Mac types other characters). */
const CODE_CHARACTERS = { Comma: ',', Period: '.', Slash: '/', Backslash: '\\', Semicolon: ';', Quote: "'", BracketLeft: '[', BracketRight: ']', Minus: '-', Equal: '=', Backquote: '`', Space: 'Space' };

function codeKey(code) {
  const m = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(code ?? '');
  if (m) return m[1] ?? m[2];
  return CODE_CHARACTERS[code] ?? null;
}

/** The modifiers of `event`; elsewhere than a Mac, Ctrl (or the Windows key) acts as Cmd. */
function eventMods(event, mac) {
  const mods = new Set();
  if (mac) {
    if (event.ctrlKey) mods.add('Ctrl');
    if (event.metaKey) mods.add('Cmd');
  } else if (event.ctrlKey || event.metaKey) mods.add('Cmd');
  if (event.altKey) mods.add('Alt');
  if (event.shiftKey) mods.add('Shift');
  return mods;
}

function joined(mods, key) {
  return [...MODIFIERS.filter((m) => mods.has(m)), key].join('+');
}

/**
 * The chord a `keydown` makes, or null (a modifier alone). The character
 * typed decides the key (a layout's own), except with ⌥, whose characters
 * are the key's code's; Shift counts only with a letter or a named key.
 */
export function eventChord(event, { mac = isMac() } = {}) {
  let key = event?.key;
  if (!key || IGNORED.has(key)) return null;
  key = EVENT_NAMES[key] ?? key;
  const mods = eventMods(event, mac);
  if ((mods.has('Alt') && [...key].length === 1 && !/^[\x21-\x7e]$/.test(key)) || key === 'Dead') {
    key = codeKey(event.code);
    if (!key) return null;
  }
  if ([...key].length === 1) {
    if (isLetter(key)) key = key.toUpperCase();
    else mods.delete('Shift');
  }
  // Another named key (media keys, …) is as the browser names it.
  return joined(mods, key);
}

/** The chord a `wheel` event makes: its modifiers and Wheel. */
export function wheelChord(event, { mac = isMac() } = {}) {
  return joined(eventMods(event, mac), 'Wheel');
}

/**
 * The keymap: the built-in one (`defaults`, ./keys.default.js's object)
 * with `overrides` (`{commandId: [chord, …]}`, the person's keys.toml as
 * the `keys` message gives it). Returns `{scopes, commands, bindings,
 * conflicts, problems}`:
 * - `scopes`: id → `{id, label, parent, fixed, about}`, in file order;
 * - `commands`: id → `{id, scope, label, defaults, keys, fixed, wheel,
 *   fine, menu, overridden}` (`keys` canonical, after the overrides);
 * - `bindings`: scope → chord → the command that chord does there;
 * - `conflicts`: `{kind, chord, scope, command, with, withScope}`:
 *   `same` (two commands of one scope: the first, fixed ones first, keeps
 *   it; `command` is the one that does not get it), `shadow` (a scope's
 *   chord hides its parent's: the focused control's wins; only reported
 *   when the built-in keymap does not do the same) and `wheel` (a wheel
 *   chord holding all of a wheel modifier's chord, which only takes the wheel);
 * - `problems`: overrides that are not used (unknown or fixed command, a
 *   chord that does not parse or is of the wrong kind), each a sentence.
 */
export function buildKeymap(defaults = DEFAULTS, overrides = {}) {
  const scopes = new Map();
  for (const s of defaults.scopes) scopes.set(s.id, { id: s.id, label: s.label, parent: s.parent ?? null, fixed: Boolean(s.fixed), about: s.about ?? '' });
  const commands = new Map();
  const problems = [];
  for (const c of defaults.commands) {
    const keys = c.keys.map((k) => chordOf(k)).filter(Boolean);
    commands.set(c.id, {
      id: c.id, scope: c.scope, label: c.label, defaults: keys, keys,
      fixed: Boolean(c.fixed || scopes.get(c.scope)?.fixed), wheel: Boolean(c.wheel), fine: Boolean(c.fine), menu: Boolean(c.menu), overridden: false,
    });
  }
  for (const [id, chords] of Object.entries(overrides ?? {})) {
    const command = commands.get(id);
    if (!command) {
      problems.push(`unknown command '${id}'; ignored`);
      continue;
    }
    if (command.fixed) {
      problems.push(`${id} is fixed (${scopes.get(command.scope)?.fixed ? 'a convention' : "the system's"}); ignored`);
      continue;
    }
    const list = Array.isArray(chords) ? chords : [];
    const keys = [];
    for (const text of list) {
      const parsed = parseChord(text);
      if (parsed.error) problems.push(`${id}: '${text}': ${parsed.error}; ignored`);
      else if (isWheel(parsed.chord) !== command.wheel) problems.push(`${id}: '${text}': ${command.wheel ? 'takes a wheel chord (Wheel, Shift+Wheel, …)' : 'takes a key, not the wheel'}; ignored`);
      else if (id === 'view.wheel_inside' && splitChord(parsed.chord).mods.size === 0) problems.push(`${id}: '${text}': needs a modifier (Alt+Wheel); ignored`);
      else if (command.menu && !menuChord(parsed.chord)) problems.push(`${id}: '${text}': ${MENU_CHORD}; ignored`);
      else if (!keys.includes(parsed.chord)) keys.push(parsed.chord);
    }
    command.keys = keys;
    command.overridden = !sameKeys(keys, command.defaults);
  }
  const { bindings, conflicts } = bind(scopes, commands);
  if (overrides && Object.keys(overrides).length) {
    const base = bind(scopes, new Map([...commands].map(([id, c]) => [id, { ...c, keys: c.defaults }])));
    const known = new Set(base.conflicts.map(conflictKey));
    return { scopes, commands, bindings, conflicts: conflicts.filter((c) => !known.has(conflictKey(c))), problems };
  }
  return { scopes, commands, bindings, conflicts: [], problems };
}

/** A conflict's identity: a shadow is the same whichever parent's chord it hides (ADR 0039; the server's `Conflict::same_as`). */
function conflictKey(c) {
  return c.kind === 'shadow' ? `shadow|${c.chord}|${c.scope}|${c.command}` : `${c.kind}|${c.chord}|${c.command}|${c.with}`;
}

/** Whether two chord lists hold the same chords (any order). */
export function sameKeys(a, b) {
  return a.length === b.length && a.every((k) => b.includes(k));
}

/** The scope's ancestors, nearest first. */
export function ancestors(scopes, id) {
  const found = [];
  for (let at = scopes.get(id)?.parent; at && !found.includes(at); at = scopes.get(at)?.parent) found.push(at);
  return found;
}

function bind(scopes, commands) {
  const bindings = new Map([...scopes.keys()].map((id) => [id, new Map()]));
  const conflicts = [];
  const ordered = [...commands.values()].sort((a, b) => Number(b.fixed) - Number(a.fixed));
  for (const command of ordered) {
    const table = bindings.get(command.scope);
    if (!table) continue;
    for (const chord of command.keys) {
      const holder = table.get(chord);
      if (holder && holder !== command.id) conflicts.push({ kind: 'same', chord, scope: command.scope, command: command.id, with: holder, withScope: command.scope });
      else table.set(chord, command.id);
    }
  }
  for (const [scope, table] of bindings) {
    for (const [chord, id] of table) {
      for (const parent of ancestors(scopes, scope)) {
        const hidden = bindings.get(parent)?.get(chord);
        // The wheel modifier's chord is the `wheel` kind's (below).
        if (hidden === 'view.wheel_inside') break;
        if (hidden) {
          conflicts.push({ kind: 'shadow', chord, scope, command: id, with: hidden, withScope: parent });
          break;
        }
      }
    }
  }
  const owners = (commands.get('view.wheel_inside')?.keys ?? []).map((k) => splitChord(k).mods);
  for (const command of commands.values()) {
    if (!command.wheel || command.id === 'view.wheel_inside') continue;
    for (const chord of command.keys) {
      const { mods } = splitChord(chord);
      if (owners.some((owner) => owner.size && [...owner].every((m) => mods.has(m)))) conflicts.push({ kind: 'wheel', chord, scope: command.scope, command: command.id, with: 'view.wheel_inside', withScope: 'view' });
    }
  }
  return { bindings, conflicts };
}

/** A conflict as a sentence (the Settings page's, and the server's problems). */
export function describeConflict(keymap, c, { mac = true } = {}) {
  const label = (id) => keymap.commands.get(id)?.label ?? id;
  const scope = (id) => keymap.scopes.get(id)?.label ?? id;
  const chord = formatChord(c.chord, { mac });
  if (c.kind === 'same') return `${chord} is both ${scope(c.scope)} ▸ ${label(c.with)} and ${label(c.command)}: ${label(c.with)} keeps it`;
  if (c.kind === 'shadow') return `${chord} in ${scope(c.scope)} ▸ ${label(c.command)} hides ${scope(c.withScope)} ▸ ${label(c.with)} there`;
  return `${chord} (${scope(c.scope)} ▸ ${label(c.command)}) holds the wheel modifier, which only takes the wheel`;
}

/**
 * The command `chord` does in `scope` of `keymap`: `{id, fine}`, or null.
 * A command marked fine also takes Shift added to its chord, as a finer
 * step (`fine: true`).
 */
export function lookup(keymap, scope, chord) {
  if (!chord) return null;
  const table = keymap.bindings.get(scope);
  if (!table) return null;
  const id = table.get(chord);
  if (id) return { id, fine: false };
  const { mods, key } = splitChord(chord);
  if (!mods.has('Shift')) return null;
  mods.delete('Shift');
  const plain = table.get(joined(mods, key));
  return plain && keymap.commands.get(plain)?.fine ? { id: plain, fine: true } : null;
}

/** The modifier sets that take the wheel for a control that does not own it (each of `view.wheel_inside`'s chords). */
export function wheelModifiers(keymap) {
  return (keymap.commands.get('view.wheel_inside')?.keys ?? []).map((k) => splitChord(k).mods).filter((m) => m.size);
}

/** `chord` without a wheel modifier set it holds whole (it only takes the wheel). */
export function withoutWheelModifiers(keymap, chord) {
  const { mods, key } = splitChord(chord);
  const held = wheelModifiers(keymap).find((owner) => [...owner].every((m) => mods.has(m)));
  for (const m of held ?? []) mods.delete(m);
  return joined(mods, key);
}

/** The first key chord of command `id` as shown (a menu's, a hint's), or ''. */
export function shownKeys(keymap, id, { mac = true, all = false } = {}) {
  const keys = keymap.commands.get(id)?.keys ?? [];
  const shown = keys.map((k) => formatChord(k, { mac }));
  return all ? shown.join(', ') : (shown[0] ?? '');
}

// ------------------------------------------------ the page's keymap

let current = buildKeymap(DEFAULTS, {});
let info = { path: null, problems: [], fallback: false };
const listeners = new Set();

/** The keymap in force on this page: the built-in one until the server's `keys` arrives. */
export function keymap() {
  return current;
}

/** What the server said of the file: `{path, problems, fallback}`. */
export function keysInfo() {
  return info;
}

/** Takes the server's `keys` message (`{overrides, path, problems, fallback}`); listeners are told. */
export function applyKeys(message) {
  current = buildKeymap(DEFAULTS, message?.overrides ?? {});
  info = { path: message?.path ?? null, problems: [...(message?.problems ?? [])], fallback: Boolean(message?.fallback) };
  for (const fn of [...listeners]) fn(current, info);
}

/** Calls `fn(keymap, info)` whenever the keymap changes; returns the stop. */
export function onKeys(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** The command a `keydown` does in `scope` on this page: `{id, fine}`, or null. */
export function keyCommand(scope, event, { mac = isMac() } = {}) {
  return lookup(current, scope, eventChord(event, { mac }));
}

/**
 * The command a `wheel` event does in `scope` on this page (the wheel
 * modifiers left out: they only take the wheel): `{id, fine}`, or null.
 */
export function wheelCommand(scope, event, { mac = isMac() } = {}) {
  return lookup(current, scope, withoutWheelModifiers(current, wheelChord(event, { mac })));
}

/** Whether the wheel modifier is held in `event` (`view.wheel_inside`, default ⌥). */
export function wheelModifierHeld(event, { mac = isMac() } = {}) {
  const mods = eventMods(event, mac);
  return wheelModifiers(current).some((owner) => [...owner].every((m) => mods.has(m)));
}

const ARIA_MODIFIERS = { Ctrl: 'Control', Alt: 'Alt', Shift: 'Shift', Cmd: 'Meta' };

/** The `aria-keyshortcuts` of `scope` in the page's keymap (its key chords, as ARIA writes them). */
export function ariaShortcuts(scope, map = current) {
  const shown = [];
  for (const [chord] of map.bindings.get(scope) ?? []) {
    const { mods, key } = splitChord(chord);
    if (key === 'Wheel') continue;
    shown.push([...MODIFIERS.filter((m) => mods.has(m)).map((m) => ARIA_MODIFIERS[m]), key === '+' ? 'Plus' : key].join('+'));
  }
  return shown.join(' ');
}

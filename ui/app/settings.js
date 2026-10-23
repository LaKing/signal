// The Settings window's Keyboard Shortcuts page (ADR 0034): every command
// by scope, searchable, with its chords; record a chord by pressing it (or
// type one), remove one, reset a command or all of them; a chord another
// command holds is said inline, with Replace. A mark shows what differs
// from the built-in keymap. Each change is written to the person's
// keys.toml at once (`keys.set`, `keys.reset`); the page shows the keymap
// the server pushes (`keys`), so an edit of the file elsewhere shows here
// too. `?view=reference` (Help ▸ Keyboard Shortcuts) shows it read-only
// first. The pure parts are ./lib/keys-editor.js. Plain ES module; every
// text goes in with textContent.

import { getDefaultConnection } from '../signal-ui.js';
import '../controls/keyboard.js';
import { MENU_CHORD, formatChord, isMac, keymap, keysInfo, applyKeys, menuChord, onKeys } from '../lib/keys.js';
import { addWrites, changedCount, collisions, describeCollision, groups, recorded, removeWrite, resetMessage, typed } from './lib/keys-editor.js';
import { applyProps } from './lib/dom.js';

function el(tag, props = {}, ...children) {
  const element = applyProps(document.createElement(tag), props);
  for (const child of children) if (child !== null && child !== undefined) element.append(child);
  return element;
}

export class SignalSettingsKeys extends HTMLElement {
  connectedCallback() {
    if (this.firstChild) return;
    this.mac = isMac();
    this.view = new URLSearchParams(location.search).get('view') === 'reference' ? 'reference' : 'edit';
    this.recording = null;
    this.pending = null;
    this.search = el('input', { type: 'search', className: 'search', placeholder: 'Search commands or keys', spellcheck: false, autocomplete: 'off' });
    this.search.setAttribute('aria-label', 'Search commands or keys');
    this.toggle = el('button', { type: 'button' });
    this.resetAll = el('button', { type: 'button', textContent: 'Reset All…' });
    this.heading = el('h1', { textContent: 'Keyboard Shortcuts' });
    this.file = el('p', { className: 'keys-file muted' });
    this.problems = el('ul', { className: 'keys-problems error' });
    this.note = el('p', { className: 'keys-note muted' });
    this.note.setAttribute('role', 'status');
    this.list = el('div', { className: 'keys-list' });
    this.append(el('div', { className: 'keys-head' }, this.heading, this.search, this.toggle, this.resetAll), this.file, this.problems, this.note, this.list);
    this.search.addEventListener('input', () => this.render());
    this.toggle.addEventListener('click', () => this.setView(this.view === 'reference' ? 'edit' : 'reference'));
    this.resetAll.addEventListener('click', () => this.resetEverything());
    this.stops = [onKeys(() => this.render())];
    this.onSettings = (event) => this.setView(event.detail?.view === 'reference' ? 'reference' : 'edit');
    window.addEventListener('signal-settings', this.onSettings);
    // While recording, the next chord is the recording's, not the page's or the menu's.
    this.onKey = (event) => this.key(event);
    this.onWheel = (event) => this.wheel(event);
    window.addEventListener('keydown', this.onKey, true);
    window.addEventListener('wheel', this.onWheel, { capture: true, passive: false });
    this.render();
  }

  disconnectedCallback() {
    for (const stop of this.stops ?? []) stop();
    window.removeEventListener('signal-settings', this.onSettings);
    window.removeEventListener('keydown', this.onKey, true);
    window.removeEventListener('wheel', this.onWheel, { capture: true });
  }

  setView(view) {
    this.view = view;
    this.recording = null;
    this.pending = null;
    this.render();
  }

  say(text, error = false) {
    this.note.textContent = text ?? '';
    this.note.classList.toggle('error', Boolean(error));
  }

  /** Sends the writes in order; each reply is the keymap as written. */
  async write(messages) {
    const connection = getDefaultConnection();
    if (!connection) {
      this.say('Not connected: nothing is written.', true);
      return false;
    }
    try {
      for (const message of messages) applyKeys(await connection.request(message));
      return true;
    } catch (error) {
      this.say(`Not written: ${error.message ?? error}`, true);
      return false;
    }
  }

  async resetEverything() {
    const changed = changedCount(keymap());
    if (!changed) {
      this.say('Every command already has its built-in keys.');
      return;
    }
    if ((await this.ask(`Put all ${changed} changed commands back to their built-in keys?`, 'Reset All')) !== 'yes') return;
    if (await this.write([resetMessage()])) this.say('Every command has its built-in keys again.');
  }

  /** A modal question in the page (WebKit in Signal.app shows no `confirm`): 'yes' or 'cancel'. */
  ask(text, yes) {
    return new Promise((resolve) => {
      const dialog = el('dialog', { className: 'prompt' });
      const cancel = el('button', { type: 'button', textContent: 'Cancel' });
      const confirm = el('button', { type: 'button', textContent: yes });
      cancel.addEventListener('click', () => dialog.close('cancel'));
      confirm.addEventListener('click', () => dialog.close('yes'));
      dialog.append(el('p', { textContent: text }), el('div', { className: 'buttons' }, cancel, confirm));
      dialog.addEventListener('close', () => {
        resolve(dialog.returnValue || 'cancel');
        dialog.remove();
      }, { once: true });
      this.append(dialog);
      dialog.showModal();
      cancel.focus();
    });
  }

  startRecording(id) {
    this.recording = { id };
    this.pending = null;
    this.say('');
    this.render();
  }

  stopRecording() {
    this.recording = null;
    this.pending = null;
    this.render();
  }

  /** A chord recorded or typed for the command being recorded: added, or its collision asked about. */
  async take(chord) {
    const id = this.recording?.id;
    if (!id) return;
    const map = keymap();
    // Only what the file can hold (a key the browser names otherwise, a bare key for a menu item: refused, still recording).
    const command = map.commands.get(id);
    const checked = typed(chord, command);
    if (checked.error || (command?.menu && !menuChord(checked.chord))) {
      this.say(`${formatChord(chord, { mac: this.mac })}: ${checked.error ?? MENU_CHORD}`, true);
      return;
    }
    chord = checked.chord;
    const same = collisions(map, id, chord).filter((c) => c.kind === 'same');
    if (same.length) {
      this.pending = { id, chord, same };
      this.render();
      return;
    }
    await this.add(id, chord, false);
  }

  async add(id, chord, replace) {
    const map = keymap();
    const other = collisions(map, id, chord).filter((c) => c.kind !== 'same');
    this.recording = null;
    this.pending = null;
    if (await this.write(addWrites(map, id, chord, { replace }))) {
      this.say(other.length ? other.map((c) => describeCollision(c, chord, { mac: this.mac })).join('; ') : `${formatChord(chord, { mac: this.mac })} added.`);
    }
    this.render();
  }

  key(event) {
    if (!this.recording || this.pending) return;
    const target = event.composedPath()[0];
    // The typed-chord field takes its own keys.
    if (target?.classList?.contains('typed')) return;
    const command = keymap().commands.get(this.recording.id);
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape' && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
      this.stopRecording();
      return;
    }
    const chord = recorded(event, command, { mac: this.mac });
    if (chord) this.take(chord);
  }

  wheel(event) {
    if (!this.recording || this.pending) return;
    const command = keymap().commands.get(this.recording.id);
    if (!command?.wheel) return;
    event.preventDefault();
    event.stopPropagation();
    const chord = recorded(event, command, { mac: this.mac });
    if (chord) this.take(chord);
  }

  render() {
    const map = keymap();
    const info = keysInfo();
    const reference = this.view === 'reference';
    this.classList.toggle('reference', reference);
    this.toggle.textContent = reference ? 'Customize…' : 'Show as Reference';
    this.resetAll.hidden = reference;
    const changed = changedCount(map);
    this.file.textContent = `${changed ? `${changed} changed from the built-in keys` : 'The built-in keys'} · ${info.path ?? 'keys.toml beside the Setup'}`;
    this.file.title = info.path ?? '';
    this.problems.replaceChildren(...info.problems.map((p) => el('li', { textContent: p })));
    this.problems.hidden = !info.problems.length;
    const shown = groups(map, { search: this.search.value, mac: this.mac });
    this.list.replaceChildren(...shown.map((group) => this.renderGroup(group, map)));
    if (!shown.length) this.list.append(el('p', { className: 'muted', textContent: 'No command matches.' }));
  }

  renderGroup(group, map) {
    const body = el('tbody');
    for (const command of group.commands) body.append(this.renderRow(command, map));
    const table = el('table', { className: 'commands' }, el('colgroup', {}, el('col', { className: 'col-changed' }), el('col', { className: 'col-label' }), el('col'), el('col', { className: 'col-actions' })), body);
    return el(
      'section',
      { className: 'scope' },
      el('h2', { textContent: group.label }),
      el('p', { className: 'about muted', textContent: group.fixed ? `${group.about} Fixed: shown for reference.` : group.about }),
      table,
    );
  }

  renderRow(command, map) {
    const mac = this.mac;
    const changed = el('td', { className: 'changed', textContent: command.overridden ? '●' : '' });
    if (command.overridden) changed.title = `Changed; built in: ${command.defaults.map((k) => formatChord(k, { mac })).join(', ') || 'none'}`;
    const label = el('td', {}, el('span', { textContent: command.label }), el('span', { className: 'id muted', textContent: command.id }));
    const chords = el('div', { className: 'chords' });
    for (const chord of command.keys) {
      const chip = el('span', { className: 'chord', textContent: formatChord(chord, { mac }) });
      chip.title = chord;
      if (!command.fixed) {
        const remove = el('button', { type: 'button', className: 'remove', textContent: '×' });
        remove.setAttribute('aria-label', `Remove ${formatChord(chord, { mac })} from ${command.label}`);
        remove.addEventListener('click', async () => {
          const write = removeWrite(map, command.id, chord);
          if (write && (await this.write([write]))) this.say(`${formatChord(chord, { mac })} removed from ${command.label}.`);
        });
        chip.append(remove);
      }
      chords.append(chip);
    }
    if (!command.keys.length) chords.append(el('span', { className: 'none muted', textContent: 'none' }));
    if (command.fine) chords.append(el('span', { className: 'muted fixed-mark', textContent: '(⇧ finer)' }));
    if (command.fixed) chords.append(el('span', { className: 'muted fixed-mark', textContent: 'fixed' }));
    const cell = el('td', {}, chords);
    for (const conflict of command.conflicts) cell.append(el('div', { className: 'conflict', textContent: conflict }));
    if (this.recording?.id === command.id) cell.append(this.renderRecording(command));
    const actions = el('td', { className: 'actions' });
    if (!command.fixed) {
      const add = el('button', { type: 'button', textContent: this.recording?.id === command.id ? 'Cancel' : 'Add…' });
      add.setAttribute('aria-label', `${this.recording?.id === command.id ? 'Cancel adding to' : 'Add a chord to'} ${command.label}`);
      add.addEventListener('click', () => (this.recording?.id === command.id ? this.stopRecording() : this.startRecording(command.id)));
      actions.append(add);
      if (command.overridden) {
        const reset = el('button', { type: 'button', textContent: 'Reset' });
        reset.setAttribute('aria-label', `Reset ${command.label} to its built-in keys`);
        reset.addEventListener('click', async () => {
          if (await this.write([resetMessage(command.id)])) this.say(`${command.label}: the built-in keys again.`);
        });
        actions.append(' ', reset);
      }
    }
    return el('tr', {}, changed, label, cell, actions);
  }

  renderRecording(command) {
    const mac = this.mac;
    const box = el('div', { className: 'record' });
    if (this.pending) {
      const { id, chord, same } = this.pending;
      box.append(el('span', { className: 'warn', textContent: `${same.map((c) => describeCollision(c, chord, { mac })).join('; ')}.` }));
      const replace = el('button', { type: 'button', textContent: 'Replace' });
      replace.title = `Take ${formatChord(chord, { mac })} from ${same.map((c) => c.label).join(', ')}`;
      replace.addEventListener('click', () => this.add(id, chord, true));
      const both = el('button', { type: 'button', textContent: 'Add Anyway' });
      both.title = 'Both keep it; the one listed first does it (said as a conflict)';
      both.addEventListener('click', () => this.add(id, chord, false));
      const cancel = el('button', { type: 'button', textContent: 'Cancel' });
      cancel.addEventListener('click', () => this.stopRecording());
      box.append(replace, both, cancel);
      queueMicrotask(() => replace.focus());
      return box;
    }
    box.append(el('span', { className: 'listening', textContent: command.wheel ? 'Hold the modifiers and turn the wheel… (Esc cancels)' : 'Press the chord… (Esc cancels)' }));
    const field = el('input', { type: 'text', className: 'typed', placeholder: command.wheel ? 'or type: Alt+Wheel' : 'or type: Cmd+K', spellcheck: false, autocomplete: 'off' });
    field.setAttribute('aria-label', `Type a chord for ${command.label}`);
    field.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        this.stopRecording();
      } else if (event.key === 'Enter') {
        event.preventDefault();
        const result = typed(field.value, command);
        if (result.error) this.say(result.error, true);
        else this.take(result.chord);
      }
    });
    box.append(field);
    return box;
  }
}

customElements.get('signal-settings-keys') ?? customElements.define('signal-settings-keys', SignalSettingsKeys);

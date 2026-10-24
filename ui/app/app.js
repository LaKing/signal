// Signal.app's window (ADR 0023 §2, M2.8-4, M2.8-11, M2.8-12): one page, web
// components over the page's connection to the app's server. The toolbar
// (power switch, tabs, the project's name with "Edited", the status) is
// always there; the project plays while the power is on, and starts and stops
// from its own panel (ADR 0033), so there is no Play or Stop: Project ▸
// Restart from Files plays it again from frame 0 (`transport.play`). The tabs
// are the project's panel, the parameters (status, meters, messages and the
// parameter list) and the setup (the project folder and the interface
// setup), switched without a reload or a new connection. The app's own
// messages (`app`, `status`, `app.power`, `transport.*`, `project.open`,
// `project.save`) are in docs/ui-protocol.md "Signal.app". Plain ES modules, no build step.
// Every text from the server goes in with textContent, never as HTML.
// Nothing here changes size with a value (docs/ui-controls.md "Stable layout").

import { getDefaultConnection, onDefaultConnection } from '../signal-ui.js';
import '../controls/meter.js';
import '../controls/schematic.js';
import { ValueEntry } from '../controls/entry-field.js';
import { ContextMenu, claimMenus, focusable } from '../controls/context-menu.js';
import { formatFixed } from '../lib/format.js';
import { applyProps } from './lib/dom.js';
import { connectCards } from './lib/cards.js';
import { menuAction, pageHandlesShortcuts, panelReloads, panelSource } from './lib/menu.js';
import { setViewScroller } from '../controls/keyboard.js';
import { onKeys } from '../lib/keys.js';
import { dragMessage, fileNote, planUpdate, releaseMessage, resetMessage, rows, shown, structureKey, valuesOf } from './lib/parameters.js';
import { INITIAL, controls, powerRequest, reduce, restartRequest } from './lib/power.js';
import { contextItems, keepsSystemMenu } from './lib/context.js';
import { PROMPT, fieldValue, openRetry, projectLabel, windowTitle } from './lib/document.js';
import { archiveSave, saveReport, saveRequests } from './lib/save.js';
import {
  MAX_NAME,
  countText,
  deleteMessage,
  deletePrompt,
  deletedText,
  entries,
  exists,
  listMessage,
  listProblem,
  overwritePrompt,
  refusalText,
  restoreMessage,
  restoredText,
  excludeMessage,
  hasUndo,
  undoMessage,
  undoneText,
  saveMessage,
  savedText,
  snapshotName,
} from './lib/snapshots.js';
import { compactStatus, statusFields } from './lib/status.js';
import { INITIAL_TAB, TABS, select, tabForAction } from './lib/tabs.js';
import { retryMessage, revertMessage, unsavedLine, unsavedOf } from '../lib/unsaved.js';

/** The messages card's lines: the newest, at a fixed height. */
const MESSAGE_LINES = 6;

/** A new element `tag` with `props` (className, textContent, …) and children. */
export function el(tag, props = {}, ...children) {
  const element = applyProps(document.createElement(tag), props);
  for (const child of children) if (child !== null && child !== undefined) element.append(child);
  return element;
}

/** A request's error, shown where the app shows its messages. */
function failed(error) {
  window.dispatchEvent(new CustomEvent('signal-app-error', { detail: error?.message ?? String(error) }));
}

/** A custom element that renders from the page's connection. */
class AppElement extends HTMLElement {
  connectedCallback() {
    // Before the call: it attaches at once when the page is connected.
    this.stops = [];
    this.stops.push(onDefaultConnection((connection) => this.attach(connection)));
  }

  disconnectedCallback() {
    for (const stop of this.stops ?? []) stop();
    this.stops = [];
  }

  attach(connection) {
    this.connection = connection;
    // `status` is both the connection's state (a string) and the server's
    // pushed status line (an object).
    this.stops.push(connection.on('status', (s) => s === 'open' && this.opened?.()));
    if (connection.status === 'open') this.opened?.();
  }

  request(message) {
    const connection = this.connection ?? getDefaultConnection();
    return connection ? connection.request(message) : Promise.reject(new Error('not connected'));
  }
}

/**
 * The toolbar: the power switch, the tabs (Panel | Parameters | Setup), the
 * project's name (with "Edited" while a .signal project's working folder
 * differs from its file), and the compact status with the last notice. No
 * Play or Stop (ADR 0033): the project runs while the power is on, and its
 * transport is its own. It switches the tabs, and runs the menu's actions:
 * the native menu's (`signal-menu`) and, without one (a browser), the app
 * commands' chords (`signal-command`, ui/controls/keyboard.js). A problem
 * with the person's keyboard shortcuts file shows as its notice.
 */
export class SignalAppToolbar extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.power = INITIAL;
      this.playing = false;
      this.status = null;
      this.switch = el('button', { type: 'button', className: 'power', role: 'switch', title: 'Power: off stops everything, on restores it' });
      this.switch.addEventListener('click', () => this.toggle());
      this.tabs = new Map(
        TABS.map((t) => {
          const button = el('button', { type: 'button', role: 'tab', textContent: t.label });
          button.addEventListener('click', () => this.select(t.id));
          return [t.id, button];
        }),
      );
      this.line = el('span', { className: 'line' });
      this.note = el('span', { className: 'note' });
      this.project = null;
      this.name = el('span', { className: 'name' });
      // Its space is kept, edited or not ("Stable layout").
      this.edited = el('span', { className: 'edited', textContent: 'Edited' });
      this.append(
        this.switch,
        el('div', { className: 'tabs', role: 'tablist' }, ...this.tabs.values()),
        el('span', { className: 'project-name' }, this.name, this.edited),
        el('output', { className: 'compact-status' }, this.line, this.note),
      );
      this.menu = (event) => this.run(event.detail);
      this.noticed = (event) => this.notice(event.detail, false);
      this.errored = (event) => this.notice(event.detail, true);
      window.addEventListener('signal-menu', this.menu);
      window.addEventListener('signal-app-notice', this.noticed);
      window.addEventListener('signal-app-error', this.errored);
      // An app command's chord in a browser (ui/controls/keyboard.js): run here, if the page can.
      this.command = (event) => {
        const action = menuAction(event.detail);
        if (!action || !pageHandlesShortcuts(globalThis)) return;
        event.preventDefault();
        if (!event.repeat) this.run(action);
      };
      window.addEventListener('signal-command', this.command);
      this.unkeys = onKeys((map, info) => {
        if (info.problems.length) this.notice(`Keyboard shortcuts: ${info.problems.length === 1 ? 'a problem' : `${info.problems.length} problems`} in ${info.path ?? 'keys.toml'}${info.fallback ? ' (the built-in keys are used)' : ''}; see Settings`, true);
      });
      // The view the page's keys scroll (Page Down, Space, …): the shown tab, or the panel frame's page.
      setViewScroller(() => {
        const tab = document.querySelector('[role="tabpanel"][data-tab]:not([hidden])');
        const frame = tab?.querySelector('iframe');
        if (frame) return frame.contentDocument?.scrollingElement ?? null;
        return tab?.classList.contains('tab-scroll') ? tab : null;
      });
      this.select(INITIAL_TAB);
      this.render();
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    window.removeEventListener('signal-menu', this.menu);
    window.removeEventListener('signal-app-notice', this.noticed);
    window.removeEventListener('signal-app-error', this.errored);
    window.removeEventListener('signal-command', this.command);
    this.unkeys?.();
    super.disconnectedCallback();
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('app', (app) => this.app(app)));
    this.stops.push(connection.on('status', (status) => typeof status === 'object' && this.showStatus(status)));
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  /** Shows tab `id`: the others are hidden, never unloaded. */
  select(id) {
    this.tab = select(this.tab, id);
    for (const [tab, button] of this.tabs) button.setAttribute('aria-selected', String(tab === this.tab));
    for (const panel of document.querySelectorAll('[role="tabpanel"][data-tab]')) panel.hidden = panel.dataset.tab !== this.tab;
  }

  /** A menu action: Open…, Save, Interface Setup…, Restart from Files; in a browser also Settings… and Keyboard Shortcuts. */
  run(action) {
    if (action === 'settings' || action === 'keys') {
      // The Settings page (ui/app/settings.html), in its own window.
      const view = action === 'keys' ? '?view=reference' : '';
      window.open(`settings.html${view}${location.hash}`, 'signal-settings');
      return;
    }
    if (action === 'save') {
      this.save();
      return;
    }
    if (action === 'restart') {
      // The project again from frame 0, from its files (ADR 0033).
      this.request(restartRequest()).catch(failed);
      return;
    }
    const target = tabForAction(action);
    if (!target) return;
    this.select(target.tab);
    if (target.focus === 'project') {
      // Without the app's folder chooser (in a browser, ⌘O): the Setup
      // tab's folder field (ADR 0023).
      const field = document.querySelector('signal-app-project input[name="project"]');
      field?.focus();
      field?.select();
    }
  }

  /** Writes every unsaved parameter again (the existing retry), then a .signal project to its file, and says how it went. */
  async save() {
    const unsaved = (this.connection ?? getDefaultConnection())?.unsaved ?? null;
    const results = await Promise.all(
      saveRequests(unsaved).map((message) =>
        this.request(message).then(
          () => ({ address: message.address }),
          (error) => ({ address: message.address, error: { code: error.code, message: error.message } }),
        ),
      ),
    );
    const report = saveReport(unsaved, results);
    const archive = report.saved ? archiveSave(this.project) : null;
    if (archive) {
      try {
        await this.request(archive);
      } catch (error) {
        failed(error);
        return;
      }
    }
    window.dispatchEvent(new CustomEvent(report.saved ? 'signal-app-notice' : 'signal-app-error', { detail: report.text }));
  }

  toggle() {
    if (this.power.pending) return;
    const message = powerRequest(this.power);
    this.power = reduce(this.power, { type: 'toggle' });
    this.render();
    this.request(message).then(
      (reply) => {
        this.power = reduce(this.power, { type: 'reply', reply });
        this.playing = reply.playing;
        this.render();
      },
      (error) => {
        this.power = reduce(this.power, { type: 'error' });
        this.render();
        failed(error);
      },
    );
  }

  app(app) {
    this.power = reduce(this.power, { type: 'app', app });
    this.playing = app.playing;
    this.project = app.project ?? null;
    this.render();
  }

  showStatus(status) {
    this.status = status.status;
    this.playing = status.playing;
    this.render();
  }

  notice(text, error) {
    this.note.textContent = text ?? '';
    this.note.title = text ?? '';
    this.note.classList.toggle('error', Boolean(error));
  }

  render() {
    const c = controls(this.power);
    this.switch.setAttribute('aria-checked', String(c.checked));
    this.switch.textContent = `⏻ ${c.label}`;
    this.switch.disabled = c.switchDisabled;
    this.line.textContent = compactStatus({ power: this.power.power, playing: this.playing, status: this.status });
    const label = projectLabel(this.project);
    this.name.textContent = label.name;
    this.name.title = this.project?.file ?? this.project?.dir ?? '';
    this.edited.style.visibility = label.edited ? 'visible' : 'hidden';
    document.title = windowTitle(this.project);
  }
}

/**
 * The project (Setup tab): a folder, its project.toml or a .signal file,
 * typed, and Open. Open… focuses its field without the app's chooser. While
 * a .signal project is edited, opening another asks "Save changes?" here
 * (the server refuses with `edited` until the person chooses).
 */
export class SignalAppProject extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.field = el('input', { name: 'project', type: 'text', placeholder: 'project folder or .signal file', spellcheck: false, size: 48 });
      const open = el('button', { type: 'button', textContent: 'Open' });
      const form = el('form', { className: 'project' }, el('label', { textContent: 'Project ' }, this.field), open);
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        this.open(this.field.value);
      });
      open.addEventListener('click', () => this.open(this.field.value));
      this.prompt = el('dialog', { className: 'prompt' });
      this.append(form, this.prompt);
    }
    super.connectedCallback();
  }

  open(path) {
    const text = String(path ?? '').trim();
    if (!text) return;
    this.request({ type: 'project.open', path: text }).catch(async (error) => {
      if (error.code !== 'edited') return failed(error);
      const retry = openRetry(text, error, await this.ask(error.message));
      if (retry) this.request(retry).catch(failed);
    });
  }

  /** "Save changes to <name>?" in the page: resolves to the button chosen (`save`, `cancel` or `discard`). */
  ask(text) {
    return new Promise((resolve) => {
      const buttons = PROMPT.map((b) => {
        const button = el('button', { type: 'button', textContent: b.label });
        button.addEventListener('click', () => this.prompt.close(b.id));
        return button;
      });
      this.prompt.replaceChildren(el('p', { textContent: text }), el('div', { className: 'buttons' }, ...buttons));
      this.prompt.returnValue = 'cancel';
      this.prompt.addEventListener('close', () => resolve(this.prompt.returnValue || 'cancel'), { once: true });
      this.prompt.showModal();
      buttons[0].focus();
    });
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('app', (app) => this.app(app)));
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  app(app) {
    if (document.activeElement !== this.field) this.field.value = fieldValue(app.project);
  }
}

/** The status card: the last status line's fields, each at a fixed width. */
export class SignalAppStatus extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.values = statusFields(null).map(([label, text]) => {
        const value = el('dd', { textContent: text });
        this.append(el('dt', { textContent: label }), value);
        return value;
      });
      this.className = 'status-fields';
    }
    super.connectedCallback();
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('status', (status) => typeof status === 'object' && this.show(status.status)));
  }

  show(status) {
    statusFields(status).forEach(([, text], i) => {
      if (this.values[i].textContent !== text) this.values[i].textContent = text;
    });
  }
}

/** The output meters (`out 1 → 1`) and the Setup's input meters, in a box of fixed height. */
export class SignalAppMeters extends AppElement {
  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('describe', (description) => this.described(description)));
    if (connection.description) this.described(connection.description);
  }

  described(description) {
    const meters = description.meters ?? { output: [], input: [] };
    const key = JSON.stringify(meters);
    if (key === this.shown) return;
    // Stopped, `describe` lists no meters: the last ones stay, at rest, so
    // a restart or the power switch does not rebuild the card ("Stable layout").
    if (this.shown && (meters.output ?? []).length + (meters.input ?? []).length === 0) return;
    const row = (source, list, label) =>
      list.map((m, i) => el('signal-meter', { source, channel: String(m.channel ?? i), label: label(m, i), floor: '-60' }));
    const output = row('output', meters.output ?? [], (m, i) => m.label ?? `out ${i + 1}`);
    const input = row('input', meters.input ?? [], (_, i) => `in ${i + 1}`);
    this.replaceChildren(
      el('div', { className: 'meters' }, ...output, ...input),
      output.length + input.length === 0 ? el('p', { className: 'muted', textContent: 'Meters appear while playing.' }) : null,
    );
    this.shown = key;
  }
}

/**
 * The live reload and diagnostics: the last refusal or notice, the
 * project's diagnostics, the run's notes, the Setup file's problems, and
 * the unsaved parameters' line, in a card of fixed height.
 */
export class SignalAppMessages extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.reload = el('div', { className: 'reload line' });
      // The project's unsaved parameters (docs/ui-controls.md "Unsaved"):
      // its row is always there, shown or not.
      this.unsavedText = el('span', { className: 'line' });
      const every = (message, which) => () => {
        for (const p of (this.unsavedState?.parameters ?? []).filter(which)) {
          this.request(message(p.address)).catch((error) => error.code !== 'not-unsaved' && failed(error));
        }
      };
      const retry = el('button', { type: 'button', textContent: 'Retry all' });
      retry.addEventListener('click', every(retryMessage, (p) => p.control));
      const revert = el('button', { type: 'button', textContent: 'Revert all' });
      revert.addEventListener('click', every(revertMessage, () => true));
      this.unsaved = el('div', { className: 'unsaved', role: 'status' }, this.unsavedText, retry, revert);
      this.unsaved.style.visibility = 'hidden';
      this.list = el('ul', { className: 'messages' });
      this.notes = [];
      this.events = [];
      this.append(this.reload, this.unsaved, this.list);
      this.errored = (event) => this.event(event.detail, 'warn');
      this.noticed = (event) => this.event(event.detail, '');
      window.addEventListener('signal-app-error', this.errored);
      window.addEventListener('signal-app-notice', this.noticed);
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    window.removeEventListener('signal-app-error', this.errored);
    window.removeEventListener('signal-app-notice', this.noticed);
    super.disconnectedCallback();
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('app', (app) => this.app(app)));
    this.stops.push(connection.on('reload', (r) => this.line(r.message, false)));
    this.stops.push(connection.on('reload.error', (r) => this.line(r.message, true)));
    this.stops.push(connection.on('unsaved', (u) => this.showUnsaved(u)));
    this.stops.push(connection.on('describe', (d) => d.unsaved && this.showUnsaved(d.unsaved)));
    if (connection.unsaved) this.showUnsaved(connection.unsaved);
  }

  /** "N unsaved parameters — last error", its space kept when nothing is unsaved. */
  showUnsaved(state) {
    this.unsavedState = state;
    const line = unsavedLine(state);
    this.unsavedText.textContent = line;
    this.unsavedText.title = line;
    this.unsaved.style.visibility = line ? 'visible' : 'hidden';
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  line(message, error) {
    this.reload.textContent = message ? `Live reload: ${message}` : '';
    this.reload.title = this.reload.textContent;
    this.reload.classList.toggle('error', Boolean(error));
  }

  event(text, className) {
    this.events = [{ text, className }, ...this.events].slice(0, MESSAGE_LINES);
    this.draw();
  }

  app(app) {
    this.line(app.reload?.message, app.reload?.error);
    const notes = [];
    if (app.message) notes.push({ text: app.message, className: 'warn' });
    for (const d of app.project?.diagnostics ?? []) notes.push({ text: d, className: 'error' });
    for (const n of app.notes ?? []) notes.push({ text: n, className: '' });
    for (const p of app.setupProblems ?? []) notes.push({ text: p, className: 'warn' });
    this.notes = notes;
    this.draw();
  }

  /** The newest lines first, at most `MESSAGE_LINES`, each cut to one line (all of it in its tooltip). */
  draw() {
    const items = [...this.events, ...this.notes].slice(0, MESSAGE_LINES);
    this.list.replaceChildren(...items.map(({ text, className }) => el('li', { className, textContent: text, title: text })));
  }
}

/**
 * The parameter list (the M2.5-2 panel), a card per top-level instance: a
 * slider per parameter (a drag ramps live, the release writes once into
 * project.toml), Reset to default, the base (a double-click on it, or Enter
 * on the slider, types a value: docs/ui-controls.md "Value entry"; a clamp's
 * reason shows where `not in file` does), `not in file`, the effective
 * value while playing, the unsaved mark; a composite's own parameters
 * read-only. Columns have fixed widths; a `describe` with the same
 * structure updates the values in place.
 */
export class SignalAppParameters extends AppElement {
  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('describe', (description) => this.described(description)));
    this.stops.push(connection.on('unsaved', (state) => this.showUnsaved(state)));
    if (connection.description) this.described(connection.description);
  }

  disconnectedCallback() {
    this.unsubscribe?.();
    this.unsubscribe = null;
    super.disconnectedCallback();
  }

  /** Each row's failed write: its mark (space always kept), with Retry and Revert. */
  showUnsaved(state) {
    for (const [address, cells] of this.cells ?? []) {
      if (!cells.mark) continue;
      const error = unsavedOf(state, address)?.error ?? null;
      cells.mark.style.visibility = error ? 'visible' : 'hidden';
      cells.mark.title = error ? `Unsaved: ${error}` : '';
    }
  }

  described(description) {
    const list = rows(description);
    const key = structureKey(list);
    const plan = planUpdate(this.shownKey, key, Boolean(this.dragging));
    if (plan === 'defer') {
      this.pending = description;
      return;
    }
    this.pending = null;
    if (plan === 'rebuild') this.build(list, key);
    else this.update(list);
    const addresses = list.flatMap((i) => i.parameters.map((p) => p.address));
    this.stream(addresses, Boolean(description.playing));
    this.showUnsaved(description.unsaved ?? this.connection?.unsaved);
  }

  /** The effective values while playing; subscribed again only when the addresses or playing change. */
  stream(addresses, playing) {
    const key = playing ? JSON.stringify(addresses) : null;
    if (key === this.streamKey) return;
    this.streamKey = key;
    this.unsubscribe?.();
    this.unsubscribe = null;
    for (const cells of this.cells.values()) if (cells.effective) cells.effective.textContent = '';
    if (!key || !addresses.length) return;
    this.unsubscribe = this.connection.subscribe({ stream: 'parameters', addresses, rate: 15 }, (frame) => {
      for (const [address, v] of valuesOf(addresses, frame)) {
        const cells = this.cells.get(address);
        if (cells?.effective) cells.effective.textContent = Number.isFinite(v.effective) ? formatFixed(v.effective, cells.unit) : '';
      }
    });
  }

  build(list, key) {
    this.shownKey = key;
    this.streamKey = undefined;
    this.cells = new Map();
    const columns = () =>
      el('colgroup', {}, ...['name', 'slider', 'base', 'note', 'effective', 'reset', 'snap', 'mark'].map((c) => el('col', { className: `col-${c}` })));
    const cards = list.map((instance) => {
      const table = el('table', { className: 'parameters' }, columns());
      for (const p of instance.parameters) table.append(this.row(p));
      const inner = instance.inner.length
        ? el('details', {}, el('summary', { textContent: 'Inside the composite' }), el('table', { className: 'parameters inner' }, columns(), ...instance.inner.map((p) => this.row(p))))
        : null;
      const title = `${instance.name} (${instance.module})`;
      return el('section', { className: 'signal-card wide' }, el('h2', { textContent: title, title }), table, inner);
    });
    this.replaceChildren(
      ...(cards.length ? cards : [el('section', { className: 'signal-card' }, el('p', { className: 'muted', textContent: 'No parameters: open a project that loads.' }))]),
    );
  }

  /** The values of a list with the shown structure, in place (not the slider being dragged). */
  update(list) {
    for (const p of list.flatMap((i) => [...i.parameters, ...i.inner])) {
      const cells = this.cells.get(p.address);
      if (!cells) continue;
      if (cells.slider && cells.slider !== this.dragging) {
        cells.value = p.base;
        cells.slider.value = String(p.base);
        if (!cells.entry.isOpen) cells.base.textContent = formatFixed(p.base, p.unit);
      } else if (!cells.slider) {
        cells.base.textContent = formatFixed(p.base, p.unit);
      }
      cells.file = fileNote(p);
      if (!cells.notice) cells.note.textContent = cells.file;
      if (cells.snap) cells.snap.checked = p.inSnapshots !== false;
    }
  }

  row(p) {
    const slider = el('input', { type: 'range', min: String(p.min), max: String(p.max), step: 'any', value: String(p.base), disabled: !p.editable });
    const base = el('td', { className: 'value', textContent: formatFixed(p.base, p.unit) });
    const note = el('td', { className: 'muted', textContent: fileNote(p) });
    const effective = el('td', { className: 'value effective' });
    const name = el('td', { className: 'name', textContent: p.name, title: p.name });
    if (!p.editable) {
      this.cells.set(p.address, { base, note, unit: p.unit });
      return el('tr', { className: 'read-only' }, name, el('td', {}, slider), base, note, effective, el('td', { className: 'muted', textContent: 'read-only' }), el('td'), el('td'));
    }
    const reset = el('button', { type: 'button', textContent: 'Reset', title: `Reset to default (${shown(p.default, p.unit)})` });
    reset.addEventListener('click', () => this.request(resetMessage(p.address)).catch(failed));
    const retry = el('button', { type: 'button', textContent: 'Retry' });
    retry.addEventListener('click', () => this.request(retryMessage(p.address)).catch(failed));
    const revert = el('button', { type: 'button', textContent: 'Revert' });
    revert.addEventListener('click', () => this.request(revertMessage(p.address)).catch(failed));
    const mark = el('td', { className: 'unsaved', role: 'alert' }, el('span', { textContent: '⚠ ' }), retry, revert);
    mark.style.visibility = 'hidden';
    // ADR 0041 §8: the control's "in snapshots" property, written to project.toml.
    const snap = el('input', { type: 'checkbox', checked: p.inSnapshots !== false, title: 'In snapshots: saved, restored and undone with them' });
    snap.setAttribute('aria-label', `${p.name} in snapshots`);
    snap.addEventListener('change', () => {
      this.request(excludeMessage(p.address, !snap.checked)).catch((error) => {
        snap.checked = !snap.checked;
        failed(error);
      });
    });
    const snapCell = el('td', { className: 'snap' }, el('label', { title: snap.title }, snap, el('span', { className: 'muted', textContent: ' snapshots' })));
    const cells = { slider, base, note, effective, mark, snap, unit: p.unit, value: p.base, file: fileNote(p), notice: null };
    slider.addEventListener('input', () => {
      this.dragging = slider;
      cells.value = Number(slider.value);
      if (!cells.entry.isOpen) base.textContent = formatFixed(cells.value, p.unit);
      this.request(dragMessage(p.address, Number(slider.value))).catch(() => {});
    });
    const released = () => {
      if (this.dragging !== slider) return;
      this.dragging = null;
      if (this.pending) this.described(this.pending);
    };
    slider.addEventListener('change', () => {
      released();
      this.request(releaseMessage(p.address, Number(slider.value))).catch(failed);
    });
    slider.addEventListener('pointerup', released);
    slider.addEventListener('pointercancel', released);
    cells.entry = new ValueEntry(base, {
      spec: () => ({ unit: p.unit, range: { min: p.min, max: p.max }, taper: p.taper, kind: p.kind }),
      value: () => cells.value,
      apply: (value) => {
        cells.value = value;
        slider.value = String(value);
        this.request(dragMessage(p.address, value)).catch(() => {});
        this.request(releaseMessage(p.address, value)).catch(failed);
      },
      note: (text) => {
        cells.notice = text;
        note.textContent = text ?? cells.file;
        note.title = text ?? '';
      },
      closed: () => {
        base.textContent = formatFixed(cells.value, p.unit);
      },
      focus: () => slider.focus({ preventScroll: true }),
    });
    slider.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && cells.entry.open()) event.preventDefault();
    });
    this.cells.set(p.address, cells);
    const tr = el('tr', {}, name, el('td', {}, slider), base, note, effective, el('td', {}, reset), snapCell, mark);
    // The context menu's Reset and Copy address.
    tr.dataset.address = p.address;
    tr.dataset.label = p.name;
    return tr;
  }
}

/**
 * The project's snapshots (ADR 0041), a card of the Parameters tab: a name
 * field with Save (Enter in the field saves), then a row per snapshot with
 * Restore and Delete. Saving under a name in use asks "Replace?" and
 * Delete asks first (Cancel focused, Escape cancels). Every request carries
 * the list's `base`, so another writer's change meanwhile is a conflict
 * and the list is read again. The list follows the server's pushes, a
 * reload of the files and another project; the last outcome shows in a
 * line of fixed height.
 */
export class SignalAppSnapshots extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.list = null;
      this.field = el('input', { name: 'snapshot', type: 'text', placeholder: 'snapshot name', spellcheck: false, maxLength: MAX_NAME * 2, size: 32 });
      this.field.setAttribute('aria-label', 'Snapshot name');
      const save = el('button', { type: 'submit', textContent: 'Save', title: 'Save every parameter’s value under this name' });
      const form = el('form', { className: 'snapshot-save' }, this.field, save);
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        this.save(this.field.value);
      });
      this.undo = el('button', { type: 'button', className: 'snapshot-undo', textContent: 'Undo restore', title: 'Put back the values the last restore replaced (again: redo it)' });
      this.undo.addEventListener('click', () => this.undoRestore());
      form.append(this.undo);
      this.rows = el('ul', { className: 'snapshots' });
      this.line = el('div', { className: 'snapshot-line line', role: 'status' });
      this.prompt = el('dialog', { className: 'prompt' });
      this.append(form, this.rows, this.line, this.prompt);
      this.draw();
    }
    super.connectedCallback();
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('snapshots', (list) => this.show(list)));
    // The files changed (an editor, another writer) or another project opened.
    this.stops.push(connection.on('reload', () => this.refresh()));
    this.stops.push(connection.on('app', () => this.refresh()));
    // Stopped, the folder's watch pushes `graph` (not `reload`) after an edit.
    this.stops.push(connection.on('graph', () => this.refresh()));
  }

  opened() {
    this.refresh();
  }

  refresh() {
    this.request(listMessage()).then(
      (list) => this.show(list),
      (error) => this.show(null, error),
    );
  }

  show(list, error = null) {
    this.list = list;
    this.problem = list ? null : error;
    this.draw();
  }

  say(text, warn = false) {
    this.line.textContent = text ?? '';
    this.line.title = text ?? '';
    this.line.classList.toggle('warn', Boolean(warn));
  }

  draw() {
    // Its place is kept whether or not there is something to undo ("Stable layout").
    this.undo.style.visibility = hasUndo(this.list) ? 'visible' : 'hidden';
    const rows = entries(this.list);
    this.rows.replaceChildren(
      ...rows.map(({ name, count }) => {
        const restore = el('button', { type: 'button', textContent: 'Restore', title: `Write “${name}”’s values into project.toml and play them` });
        restore.addEventListener('click', () => this.restore(name));
        const remove = el('button', { type: 'button', textContent: 'Delete', title: `Delete “${name}”` });
        remove.addEventListener('click', () => this.remove(name));
        return el('li', {}, el('span', { className: 'snapshot-name', textContent: name, title: name }), el('span', { className: 'muted snapshot-count', textContent: countText(count) }), restore, remove);
      }),
    );
    if (!rows.length) {
      const text = this.list ? 'No snapshots yet: type a name and Save.' : listProblem(this.problem);
      this.rows.append(el('li', { className: this.problem && this.problem.code !== 'no-project' ? 'warn' : 'muted', textContent: text, title: text }));
    }
  }

  /** The refusal said, and the list read again after a conflict. */
  refused(error) {
    this.say(refusalText(error), true);
    if (error?.code === 'conflict') this.refresh();
  }

  async save(raw) {
    const { name, error } = snapshotName(raw);
    if (error) {
      this.say(error, true);
      this.field.focus();
      return;
    }
    const base = this.list ? (this.list.base ?? null) : undefined;
    const overwrite = exists(this.list, name);
    if (overwrite && (await this.ask(overwritePrompt(name))) !== 'confirm') return;
    try {
      const reply = await this.request(saveMessage(name, { overwrite, base }));
      this.say(savedText(reply));
      this.field.value = '';
    } catch (refusal) {
      this.refused(refusal);
    }
  }

  async restore(name) {
    try {
      const reply = await this.request(restoreMessage(name, this.list?.base ?? undefined));
      const { text, warn } = restoredText(reply);
      this.say(text, warn);
    } catch (refusal) {
      this.refused(refusal);
    }
  }

  async undoRestore() {
    try {
      const reply = await this.request(undoMessage(this.list?.base ?? undefined));
      const { text, warn } = undoneText(reply);
      this.say(text, warn);
    } catch (refusal) {
      this.refused(refusal);
    }
  }

  async remove(name) {
    if ((await this.ask(deletePrompt(name))) !== 'confirm') return;
    try {
      const reply = await this.request(deleteMessage(name, this.list?.base ?? undefined));
      this.say(deletedText(reply));
    } catch (refusal) {
      this.refused(refusal);
    }
  }

  /** A confirmation in the page: resolves to the id of the button chosen (`cancel` on Escape). */
  ask({ text, buttons }) {
    return new Promise((resolve) => {
      const shown = buttons.map((b) => {
        const button = el('button', { type: 'button', textContent: b.label });
        button.addEventListener('click', () => this.prompt.close(b.id));
        return button;
      });
      this.prompt.replaceChildren(el('p', { textContent: text }), el('div', { className: 'buttons' }, ...shown));
      this.prompt.returnValue = 'cancel';
      this.prompt.addEventListener('close', () => resolve(this.prompt.returnValue || 'cancel'), { once: true });
      this.prompt.showModal();
      shown[0].focus();
    });
  }
}

/**
 * The project's panel (M2.8-3, `/project/main.html`) in the Panel tab: a
 * same-origin frame filling the tab, kept while other tabs show, whose
 * keys are its own page's (ui/controls/keyboard.js: the app commands'
 * chords reach this page as `signal-command`). Opening another project shows its panel. In Signal.app it shows
 * every card, one another window holds as a placeholder, and a right click
 * on a card puts its Move items first in this page's menu (ADR 0032,
 * ./lib/cards.js).
 */
export class SignalAppPanel extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.frame = el('iframe', { className: 'project-panel', title: 'Project panel', src: panelSource(location.hash) });
      this.frame.addEventListener('load', () => {
        this.unlisten?.();
        const doc = this.frame.contentDocument;
        const menu = doc ? SignalAppContextMenu.listen(doc, this.frame) : null;
        this.unlisten = () => menu?.();
      });
      this.append(this.frame);
      connectCards(this.frame);
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    this.unlisten?.();
    super.disconnectedCallback();
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('app', (app) => this.app(app)));
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  app(app) {
    const dir = app.project?.dir ?? null;
    // Not `this.dir`: that is HTMLElement's text direction, which reads
    // back "" for a folder, so every `app` message (a restart, the power, a written
    // parameter) reloaded the panel in sight.
    if (panelReloads(this.shownDir, dir)) this.frame.contentWindow?.location.reload();
    // A moment without a project keeps the folder shown.
    if (dir || this.shownDir === undefined) this.shownDir = dir;
  }
}

/**
 * The context menu (owner's request, 2026-10-16), in place of the WebView's
 * own: on the schematic, the schematic's items for what was clicked (its
 * `contextMenu`, 2026-10-17); on a control bound to a parameter, Reset to
 * default and Copy address; then Restart from Files and the other tabs. A text
 * field keeps the system's menu (Cut, Copy, Paste). One per page; the
 * panel frame's right clicks come here too. The element is
 * ui/controls/context-menu.js's, as the schematic's own in a browser.
 */
export class SignalAppContextMenu extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.menu = new ContextMenu({ label: 'Signal' });
      this.list = this.menu.list;
      this.append(this.list);
      this.unlisten = SignalAppContextMenu.listen(document, null);
      this.closer = (event) => {
        if (this.menu.isOpen && !event.composedPath().includes(this.list)) this.close();
      };
      this.escape = (event) => event.key === 'Escape' && this.close();
      window.addEventListener('pointerdown', this.closer, true);
      window.addEventListener('keydown', this.escape);
      window.addEventListener('blur', () => this.close());
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    this.unlisten?.();
    window.removeEventListener('pointerdown', this.closer, true);
    window.removeEventListener('keydown', this.escape);
    super.disconnectedCallback();
  }

  /**
   * Right clicks in `doc` (this page's, or the panel frame's: `frame`);
   * returns the remover. Taken as they bubble, so what swallows them first
   * keeps them (the panel editor's edit mode). The document is claimed
   * (`claimMenus`): the schematic opens no menu of its own there and gives
   * its items to this one.
   */
  static listen(doc, frame) {
    const open = (event) => {
      const target = event.composedPath?.()[0] ?? event.target;
      if (keepsSystemMenu(target)) return;
      // A right click on the open menu itself rebuilds nothing.
      const list = document.querySelector('signal-app-context-menu')?.list;
      if (list && event.composedPath?.().includes(list)) {
        event.preventDefault();
        return;
      }
      event.preventDefault();
      const offset = frame?.getBoundingClientRect() ?? { left: 0, top: 0 };
      document.querySelector('signal-app-context-menu')?.show(event, offset);
    };
    const close = () => document.querySelector('signal-app-context-menu')?.close();
    doc.addEventListener('contextmenu', open);
    const unclaim = claimMenus(doc);
    if (frame) doc.addEventListener('pointerdown', close, true);
    return () => {
      unclaim();
      doc.removeEventListener('contextmenu', open);
      if (frame) doc.removeEventListener('pointerdown', close, true);
    };
  }

  /** The parameter control under the click, if any: `{ address, label, editable }`. */
  static control(event) {
    for (const node of event.composedPath?.() ?? []) {
      if (node?.binding?.address) {
        return { address: node.binding.address, label: node.getAttribute?.('label') ?? '', editable: true };
      }
      // A schematic part (M2.9-1) has an address to copy, nothing to reset.
      if (node?.dataset?.address) return { address: node.dataset.address, label: node.dataset.label ?? '', editable: true, part: node.dataset.part === 'schematic' };
    }
    return null;
  }

  /**
   * The menu of an element under the click that has one (`contextMenu(event,
   * close)`: <signal-schematic>), `{items, act(id), closed()}`, or null.
   */
  static part(event, close) {
    for (const node of event.composedPath?.() ?? []) {
      if (typeof node?.contextMenu !== 'function') continue;
      const menu = node.contextMenu(event, close);
      if (menu) return menu;
    }
    return null;
  }

  show(event, offset) {
    const toolbar = document.querySelector('signal-app-toolbar');
    this.close();
    const part = SignalAppContextMenu.part(event, () => this.close());
    const control = part ? null : SignalAppContextMenu.control(event);
    const items = contextItems({
      part: part?.items,
      control,
      power: (toolbar?.power?.power ?? 'on') === 'on',
      tab: toolbar?.tab ?? null,
    });
    // The focus goes back where the click was (duck-typed: the panel frame's nodes are of another realm).
    const back = event.composedPath?.().find(focusable) ?? null;
    this.menu.show(items, {
      x: event.clientX + offset.left,
      y: event.clientY + offset.top,
      pick: (id) => this.run(id, control, toolbar, part),
      closed: () => part?.closed?.(),
      back,
    });
  }

  close() {
    this.menu?.close();
  }

  run(id, control, toolbar, part) {
    if (id.startsWith('part:')) part?.act(id.slice(5));
    else if (id === 'reset' && control) this.request(resetMessage(control.address)).catch(failed);
    else if (id === 'copy-address' && control) navigator.clipboard?.writeText(control.address).catch(failed);
    else if (id === 'restart') this.request(restartRequest()).catch(failed);
    else if (id.startsWith('tab:')) toolbar?.select(id.slice(4));
  }
}

for (const [name, element] of [
  ['signal-app-toolbar', SignalAppToolbar],
  ['signal-app-project', SignalAppProject],
  ['signal-app-status', SignalAppStatus],
  ['signal-app-panel', SignalAppPanel],
  ['signal-app-context-menu', SignalAppContextMenu],
  ['signal-app-meters', SignalAppMeters],
  ['signal-app-messages', SignalAppMessages],
  ['signal-app-parameters', SignalAppParameters],
  ['signal-app-snapshots', SignalAppSnapshots],
]) {
  customElements.get(name) ?? customElements.define(name, element);
}

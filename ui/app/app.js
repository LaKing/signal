// Signal.app's pages (ADR 0023 §2, M2.8-4): web components over the page's
// connection to the app's server. The window shows ui/app/index.html; the
// app's own messages (`app`, `status`, `transport.*`, `project.open`) are
// in docs/ui-protocol.md "Signal.app". Plain ES modules, no build step.
// Every text from the server goes in with textContent, never as HTML.

import { getDefaultConnection, onDefaultConnection } from '../signal-ui.js';
import '../controls/meter.js';
import { MENU, PANEL_PAGE, panelSource, quit, shortcut, withFragment } from './lib/menu.js';
import { dragMessage, fileNote, releaseMessage, resetMessage, rows, shown, valuesOf } from './lib/parameters.js';
import { retryMessage, revertMessage, unsavedLine, unsavedOf } from '../lib/unsaved.js';

/** A new element `tag` with `props` (className, textContent, …) and children. */
export function el(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key in element) element[key] = value;
    else element.setAttribute(key, value);
  }
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

/** The menu: Open project…, Interface setup, Quit, with their shortcuts. */
export class SignalAppMenu extends HTMLElement {
  connectedCallback() {
    if (this.firstChild) return;
    const nav = el('nav', { className: 'menu' });
    for (const item of MENU) {
      const button = el('button', { type: 'button', textContent: item.label, title: item.shortcut });
      button.addEventListener('click', () => SignalAppMenu.run(item.action));
      nav.append(button);
    }
    // The project's panel opens inside the app's shell (panel.html), which
    // keeps this menu and its shortcuts.
    nav.append(el('a', { className: 'panel-link', href: withFragment(PANEL_PAGE, location.hash), textContent: 'Panel' }));
    nav.append(el('a', { href: withFragment('index.html', location.hash), textContent: 'Transport' }));
    this.append(nav);
    this.unlisten = SignalAppMenu.listen(document);
  }

  disconnectedCallback() {
    this.unlisten?.();
  }

  /** The menu's shortcuts in `doc` (this page's, or its panel frame's); returns the remover. */
  static listen(doc) {
    const keys = (event) => {
      const action = shortcut(event);
      if (!action) return;
      event.preventDefault();
      SignalAppMenu.run(action);
    };
    doc.addEventListener('keydown', keys);
    return () => doc.removeEventListener('keydown', keys);
  }

  static run(action) {
    if (action === 'quit') {
      if (!quit()) failed(new Error('Quit closes Signal.app; in a browser, close the tab'));
    } else if (action === 'setup') {
      location.assign(withFragment('setup.html', location.hash));
    } else if (action === 'open') {
      const field = document.querySelector('signal-app-transport input[name="project"]');
      if (field) {
        field.focus();
        field.select();
      } else {
        location.assign(withFragment('index.html', location.hash));
      }
    }
  }
}

/**
 * The project bar and the transport: the folder (typed, or dropped on the
 * window), Open, Play, Stop, and the status line (frame, elapsed, xruns,
 * load, clipped, buffer, threads).
 */
export class SignalAppTransport extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.field = el('input', { name: 'project', type: 'text', placeholder: 'project folder', spellcheck: false, size: 60 });
      const open = el('button', { type: 'button', textContent: 'Open' });
      this.play = el('button', { type: 'button', textContent: '▶ Play' });
      this.stop = el('button', { type: 'button', textContent: '■ Stop' });
      this.line = el('div', { className: 'status-line', textContent: 'stopped' });
      const form = el('form', { className: 'project' }, el('label', { textContent: 'Project folder ' }, this.field), open);
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        this.open(this.field.value);
      });
      open.addEventListener('click', () => this.open(this.field.value));
      this.play.addEventListener('click', () => this.request({ type: 'transport.play' }).catch(failed));
      this.stop.addEventListener('click', () => this.request({ type: 'transport.stop' }).catch(failed));
      this.append(form, el('div', { className: 'transport' }, this.play, this.stop), this.line);
      this.dropped = (event) => this.open(event.detail);
      window.addEventListener('signal-drop', this.dropped);
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    window.removeEventListener('signal-drop', this.dropped);
    super.disconnectedCallback();
  }

  open(path) {
    const text = String(path ?? '').trim();
    if (text) this.request({ type: 'project.open', path: text }).catch(failed);
  }

  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('app', (app) => this.app(app)));
    this.stops.push(connection.on('status', (status) => typeof status === 'object' && this.status(status)));
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  app(app) {
    if (document.activeElement !== this.field) this.field.value = app.project?.dir ?? '';
    this.play.disabled = app.playing;
    this.stop.disabled = !app.playing;
    if (!app.playing && !this.line.textContent) this.line.textContent = 'stopped';
  }

  status(status) {
    this.line.textContent = status.line;
  }
}

/** The output meters (`out 1 → 1`) and the Setup's input meters. */
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
    this.shown = key;
    const row = (source, list, label) =>
      list.map((m, i) => el('signal-meter', { source, channel: String(m.channel ?? i), label: label(m, i), floor: '-60' }));
    const output = row('output', meters.output ?? [], (m, i) => m.label ?? `out ${i + 1}`);
    const input = row('input', meters.input ?? [], (_, i) => `in ${i + 1}`);
    this.replaceChildren(
      el('div', { className: 'meters' }, ...output, ...input),
      output.length + input.length === 0 ? el('p', { className: 'muted', textContent: 'Meters appear while playing.' }) : null,
    );
  }
}

/**
 * The live reload and diagnostics: the last refusal or notice, the
 * project's diagnostics, the run's notes, the Setup file's problems.
 */
export class SignalAppMessages extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.reload = el('div', { className: 'reload' });
      // The project's unsaved parameters (docs/ui-controls.md "Unsaved").
      this.unsavedText = el('span');
      const every = (message, which) => () => {
        for (const p of (this.unsavedState?.parameters ?? []).filter(which)) {
          this.request(message(p.address)).catch((error) => error.code !== 'not-unsaved' && failed(error));
        }
      };
      const retry = el('button', { type: 'button', textContent: 'Retry all' });
      retry.addEventListener('click', every(retryMessage, (p) => p.control));
      const revert = el('button', { type: 'button', textContent: 'Revert all' });
      revert.addEventListener('click', every(revertMessage, () => true));
      this.unsaved = el('div', { className: 'unsaved', role: 'status', hidden: true }, this.unsavedText, retry, revert);
      this.list = el('ul', { className: 'messages' });
      this.append(this.reload, this.unsaved, this.list);
      this.errored = (event) => this.error(event.detail);
      window.addEventListener('signal-app-error', this.errored);
    }
    super.connectedCallback();
  }

  disconnectedCallback() {
    window.removeEventListener('signal-app-error', this.errored);
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

  /** "N unsaved parameters — last error", hidden when nothing is unsaved. */
  showUnsaved(state) {
    this.unsavedState = state;
    const line = unsavedLine(state);
    this.unsavedText.textContent = line;
    this.unsaved.hidden = !line;
  }

  opened() {
    this.request({ type: 'app.get' }).then((app) => this.app(app), failed);
  }

  line(message, error) {
    this.reload.textContent = message ? `Live reload: ${message}` : '';
    this.reload.classList.toggle('error', Boolean(error));
  }

  error(text) {
    this.list.prepend(el('li', { className: 'warn', textContent: text }));
  }

  app(app) {
    this.line(app.reload?.message, app.reload?.error);
    const items = [];
    if (app.message) items.push(el('li', { className: 'warn', textContent: app.message }));
    for (const d of app.project?.diagnostics ?? []) items.push(el('li', { className: 'error', textContent: d }));
    for (const n of app.notes ?? []) items.push(el('li', { textContent: n }));
    for (const p of app.setupProblems ?? []) items.push(el('li', { className: 'warn', textContent: p }));
    this.list.replaceChildren(...items);
  }
}

/**
 * The parameter list (the M2.5-2 panel): a slider per top-level parameter
 * (a drag ramps live, the release writes once into project.toml), Reset to
 * default, the base, the effective value while playing, `(not in file)`,
 * and a composite's own parameters read-only.
 */
export class SignalAppParameters extends AppElement {
  attach(connection) {
    super.attach(connection);
    this.stops.push(connection.on('describe', (description) => this.described(description)));
    this.stops.push(connection.on('unsaved', (state) => this.showUnsaved(state)));
    if (connection.description) this.described(connection.description);
  }

  /** Each row's failed write: its error, with Retry and Revert. */
  showUnsaved(state) {
    for (const [address, cell] of this.marks ?? []) {
      const error = unsavedOf(state, address)?.error ?? null;
      cell.hidden = !error;
      cell.firstChild.textContent = error ? `Unsaved: ${error}` : '';
    }
  }

  described(description) {
    const list = rows(description);
    if (this.dragging) return; // redrawn after the release
    if (this.unsubscribe) this.unsubscribe();
    this.cells = new Map();
    this.marks = new Map();
    const blocks = list.map((instance) => {
      const table = el('table', { className: 'parameters' });
      for (const p of instance.parameters) table.append(this.row(p));
      const inner = instance.inner.length
        ? el('details', {}, el('summary', { textContent: 'Inside the composite' }), el('table', { className: 'parameters inner' }, ...instance.inner.map((p) => this.row(p))))
        : null;
      return el('section', {}, el('h3', { textContent: `${instance.name} (${instance.module})` }), table, inner);
    });
    this.replaceChildren(...(blocks.length ? blocks : [el('p', { className: 'muted', textContent: 'No parameters: open a project that loads.' })]));
    const addresses = list.flatMap((i) => i.parameters.map((p) => p.address));
    if (addresses.length && description.playing) {
      this.unsubscribe = this.connection.subscribe({ stream: 'parameters', addresses, rate: 15 }, (frame) => {
        for (const [address, v] of valuesOf(addresses, frame)) {
          const cell = this.cells.get(address);
          if (cell) cell.textContent = Number.isFinite(v.effective) ? `effective ${shown(v.effective, cell.dataset.unit || null)}` : '';
        }
      });
    } else {
      this.unsubscribe = null;
    }
    this.showUnsaved(description.unsaved ?? this.connection?.unsaved);
  }

  row(p) {
    const slider = el('input', { type: 'range', min: String(p.min), max: String(p.max), step: 'any', value: String(p.base), disabled: !p.editable });
    const base = el('span', { className: 'base', textContent: `${shown(p.base, p.unit)} ${fileNote(p)}`.trim() });
    const effective = el('span', { className: 'effective' });
    effective.dataset.unit = p.unit ?? '';
    const cells = [el('td', { textContent: p.name }), el('td', {}, slider), el('td', {}, base), el('td', {}, effective)];
    if (p.editable) {
      this.cells.set(p.address, effective);
      slider.addEventListener('input', () => {
        this.dragging = true;
        base.textContent = shown(Number(slider.value), p.unit);
        this.request(dragMessage(p.address, Number(slider.value))).catch(() => {});
      });
      slider.addEventListener('change', () => {
        this.dragging = false;
        this.request(releaseMessage(p.address, Number(slider.value))).catch(failed);
      });
      const reset = el('button', { type: 'button', textContent: 'Reset', title: `Reset to default (${shown(p.default, p.unit)})` });
      reset.addEventListener('click', () => this.request(resetMessage(p.address)).catch(failed));
      const retry = el('button', { type: 'button', textContent: 'Retry' });
      retry.addEventListener('click', () => this.request(retryMessage(p.address)).catch(failed));
      const revert = el('button', { type: 'button', textContent: 'Revert' });
      revert.addEventListener('click', () => this.request(revertMessage(p.address)).catch(failed));
      const mark = el('td', { className: 'unsaved', role: 'alert', hidden: true }, el('span'), retry, revert);
      this.marks.set(p.address, mark);
      cells.push(el('td', {}, reset), mark);
    } else {
      cells.push(el('td', { className: 'muted', textContent: 'read-only' }));
    }
    return el('tr', { className: p.editable ? '' : 'read-only' }, ...cells);
  }
}

/**
 * The project's panel (M2.8-3, `/project/main.html`) inside the app's
 * shell: a same-origin frame below the menu, whose shortcuts also work while
 * the panel has the focus. Opening another project shows its panel.
 */
export class SignalAppPanel extends AppElement {
  connectedCallback() {
    if (!this.firstChild) {
      this.frame = el('iframe', { className: 'project-panel', title: 'Project panel', src: panelSource(location.hash) });
      this.frame.addEventListener('load', () => {
        this.unlisten?.();
        const doc = this.frame.contentDocument;
        this.unlisten = doc ? SignalAppMenu.listen(doc) : null;
      });
      this.append(this.frame);
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
    if (this.dir !== undefined && dir !== this.dir) this.frame.contentWindow?.location.reload();
    this.dir = dir;
  }
}

for (const [name, element] of [
  ['signal-app-menu', SignalAppMenu],
  ['signal-app-panel', SignalAppPanel],
  ['signal-app-transport', SignalAppTransport],
  ['signal-app-meters', SignalAppMeters],
  ['signal-app-messages', SignalAppMessages],
  ['signal-app-parameters', SignalAppParameters],
]) {
  customElements.get(name) ?? customElements.define(name, element);
}

// <signal-panel>: the loader of a project panel (docs/ui-controls.md
// "Panels"). It connects with the page's URL (`#ws=…&token=…`), makes that
// the controls' connection, and once described asks the server's binding
// check (`panel.check`, the check `signal ui check` runs) for its file,
// showing every diagnostic in the page and marking the controls they are
// about. With `generated` it builds the default panel from `describe`
// instead. Attributes: `file` (the panel file; by default the page's
// `/project/<file>.html`), `generated`.

import { connectFromPage, getDefaultConnection, setDefaultConnection } from '../signal-ui.js';
import { BindingCheck, defaultPanel, formatDiagnostic, panelFile } from '../lib/panel.js';
import { unsavedLine } from '../lib/unsaved.js';

const CSS = `
:host { display: block; color: var(--signal-fg, #d6dbe4); font: 13px/1.45 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
.bar { display: flex; gap: .75rem; align-items: baseline; margin: 0 0 .75rem; color: var(--signal-muted, #8a93a3); font-size: 11px; }
.status { color: var(--signal-accent, #4fc3f7); }
.status.failed, .status.closed { color: var(--signal-clip, #ff5252); }
.problems { margin: 0 0 1rem; padding: .5rem .75rem .5rem 2rem; border: 1px solid var(--signal-clip, #ff5252); border-radius: var(--signal-radius, 6px); background: color-mix(in srgb, var(--signal-clip, #ff5252) 10%, transparent); font: 11px/1.5 ui-monospace, Menlo, monospace; }
.problems .warning { color: var(--signal-effective, #ffb74d); }
.problems .error { color: var(--signal-clip, #ff5252); }
.generated section { margin: 0 0 1.25rem; }
.generated h2 { margin: 0 0 .5rem; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); }
.generated h2 small { font-weight: 400; letter-spacing: 0; text-transform: none; margin-left: .5em; }
.generated .row { display: flex; flex-wrap: wrap; gap: .75rem 1.25rem; align-items: flex-end; }
.generated signal-scope { flex: 1 1 20rem; min-width: 16rem; }
.unsaved { color: var(--signal-unsaved, #ffb74d); }
.unsaved button { margin-left: .4em; font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-unsaved, #ffb74d); border-radius: 3px; cursor: pointer; }
[hidden] { display: none !important; }
`;

/** The controls of `root` in document order (the check's indexes). */
function controlsOf(root) {
  return [...root.querySelectorAll('*')].filter((e) => e.localName.startsWith('signal-') && e.localName !== 'signal-panel');
}

export class SignalPanel extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this.shadowRoot.innerHTML = `<style>${CSS}</style>
<div class="bar" part="bar"><span class="status">starting…</span><span class="title"></span><span class="summary"></span><span class="unsaved" part="unsaved" role="status" hidden><span class="unsaved-text"></span><button type="button" class="retry-all">Retry all</button><button type="button" class="revert-all">Revert all</button></span></div>
<ol class="problems" part="problems" role="alert" hidden></ol>
<div class="generated" part="generated"></div>
<slot></slot>`;
    this.connection = null;
    this.stops = [];
    this.planVersion = undefined;
    this.marked = [];
  }

  $(selector) {
    return this.shadowRoot.querySelector(selector);
  }

  /** The panel file the check is asked for. */
  get file() {
    return this.getAttribute('file') ?? panelFile(globalThis.location?.pathname);
  }

  connectedCallback() {
    if (this.connection) return;
    const connection = getDefaultConnection() ?? connectFromPage();
    if (!connection) {
      this.showStatus('not connected');
      this.showProblems([{ severity: 'error', text: 'no #ws=…&token=… in the URL: open the panel URL that `signal ui <project>` prints' }]);
      return;
    }
    this.connection = connection;
    this.binding = new BindingCheck({
      request: (message) => connection.request(message),
      show: (reply, error) => this.checked(reply, error),
    });
    if (getDefaultConnection() !== connection) setDefaultConnection(connection);
    this.stops.push(connection.on('status', (s) => this.showStatus(s)));
    this.stops.push(connection.on('describe', (d) => this.described(d)));
    this.stops.push(connection.on('unsaved', (u) => this.showUnsaved(u)));
    this.$('.retry-all').onclick = () => this.everyUnsaved((address) => connection.retry(address), (p) => p.control);
    this.$('.revert-all').onclick = () => this.everyUnsaved((address) => connection.revert(address), () => true);
    this.showStatus(connection.status);
    if (connection.description) this.described(connection.description);
  }

  /**
   * The project's unsaved parameters (docs/ui-controls.md "Unsaved"):
   * "N unsaved parameters — last error", with Retry all and Revert all.
   */
  showUnsaved(state, note = null) {
    this.unsaved = state;
    const line = unsavedLine(state);
    this.$('.unsaved-text').textContent = note ? `${line} (${note})` : line;
    this.$('.unsaved').hidden = !line;
  }

  /** `request(address)` for every listed parameter `which` takes; a refusal is shown. */
  everyUnsaved(request, which) {
    for (const p of (this.unsaved?.parameters ?? []).filter(which)) {
      request(p.address).catch((error) => {
        if (error.code !== 'not-unsaved') this.showUnsaved(this.unsaved, `${p.address}: ${error.message}`);
      });
    }
  }

  disconnectedCallback() {
    for (const stop of this.stops.splice(0)) stop();
  }

  showStatus(status) {
    const element = this.$('.status');
    element.textContent = status;
    element.className = `status ${status}`;
  }

  /** A pushed or requested describe: check again (or rebuild) when the plan changed. */
  described(description) {
    this.$('.title').textContent = description.project ?? '';
    if (description.unsaved) this.showUnsaved(description.unsaved);
    if (this.hasAttribute('generated')) {
      if (description.planVersion === this.planVersion) return;
      this.planVersion = description.planVersion;
      this.generate(description);
      return;
    }
    const file = this.file;
    if (!file) {
      this.showProblems([{ severity: 'warning', text: 'this page is not a project panel (/project/<file>.html): its bindings are not checked' }]);
      return;
    }
    this.binding.described(description, file);
  }

  /** The binding check's reply (or its refusal): shown in the page. */
  checked(reply, error) {
    if (error) {
      this.showProblems([{ severity: 'error', text: `the binding check failed: ${error.message}` }]);
      return;
    }
    const controls = controlsOf(this.ownerDocument);
    for (const element of this.marked.splice(0)) {
      element.style.removeProperty('outline');
      element.removeAttribute('title');
      element.removeAttribute('aria-invalid');
    }
    const problems = [];
    for (const d of reply.diagnostics ?? []) {
      const text = formatDiagnostic(d);
      problems.push({ severity: d.severity, text });
      const element = d.control === null || d.control === undefined ? null : controls[d.control];
      if (element) {
        element.style.outline = `2px solid var(${d.severity === 'error' ? '--signal-clip, #ff5252' : '--signal-effective, #ffb74d'})`;
        element.title = element.title ? `${element.title}\n${text}` : text;
        if (d.severity === 'error') element.setAttribute('aria-invalid', 'true');
        this.marked.push(element);
      }
    }
    this.$('.summary').textContent = `${reply.file}: ${reply.controls} controls, ${reply.bindings} bindings${reply.ok ? '' : ', with errors'}`;
    this.showProblems(problems);
  }

  showProblems(problems) {
    const list = this.$('.problems');
    list.replaceChildren(
      ...problems.map(({ severity, text }) => {
        const item = document.createElement('li');
        item.className = severity;
        item.textContent = text;
        return item;
      }),
    );
    list.hidden = problems.length === 0;
  }

  /** Builds the default panel of `description`. */
  generate(description) {
    const spec = defaultPanel(description);
    const element = (name, attributes) => {
      const e = document.createElement(name);
      for (const [key, value] of Object.entries(attributes)) e.setAttribute(key, value);
      return e;
    };
    const section = (title, note, children) => {
      const s = document.createElement('section');
      const h = document.createElement('h2');
      h.textContent = title;
      if (note) {
        const small = document.createElement('small');
        small.textContent = note;
        h.append(small);
      }
      const row = document.createElement('div');
      row.className = 'row';
      row.append(...children);
      s.append(h, row);
      return s;
    };
    const sections = spec.groups.map((g) =>
      section(g.instance, g.module, g.controls.map((c) => element(c.element, { bind: c.bind, label: c.label, taper: c.taper }))),
    );
    const output = [
      ...spec.meters.map((m) => element('signal-meter', { bind: m.bind, label: m.label })),
      ...(spec.scope ? [element('signal-scope', { bind: spec.scope.bind, label: spec.scope.label })] : []),
    ];
    if (output.length) sections.push(section('output', null, output));
    this.$('.generated').replaceChildren(...sections);
    this.$('.summary').textContent = 'generated panel (the project has no ui/main.html)';
  }
}

customElements.get('signal-panel') ?? customElements.define('signal-panel', SignalPanel);

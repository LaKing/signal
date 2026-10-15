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
/* Cards (docs/ui-controls.md "Panels"): the bar, the problems and the
   generated panel's groups are cards too, so no text is outside one. The
   bar is one line of fixed height whatever it says ("Stable layout"). */
.bar, .problems, .generated section { box-sizing: border-box; border-radius: 10px; background: var(--signal-panel, #1f232b); }
.bar { display: flex; gap: .75rem; align-items: center; height: 2rem; margin: 0 0 .75rem; padding: 0 1rem; color: var(--signal-muted, #8a93a3); font-size: 11px; white-space: nowrap; overflow: hidden; }
.bar > span { overflow: hidden; text-overflow: ellipsis; }
.status { flex: none; width: 11ch; color: var(--signal-accent, #4fc3f7); }
.status.failed, .status.closed { color: var(--signal-clip, #ff5252); }
.title { flex: none; width: 16ch; min-width: 0; }
.summary { min-width: 0; }
.problems { margin: 0 0 .75rem; padding: .5rem .75rem .5rem 2rem; border: 1px solid var(--signal-clip, #ff5252); font: 11px/1.5 ui-monospace, Menlo, monospace; }
.problems .warning { color: var(--signal-effective, #ffb74d); }
.problems .error { color: var(--signal-clip, #ff5252); }
.generated { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(var(--signal-card-min, 16rem), 100%), 1fr)); gap: .75rem; }
.generated:empty { display: none; }
.generated section { min-width: 0; padding: .75rem 1rem; display: flex; flex-direction: column; gap: .6rem; }
.generated section.wide { grid-column: 1 / -1; }
.generated h2 { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.generated h2 small { font-weight: 400; letter-spacing: 0; text-transform: none; margin-left: .5em; }
.generated .row { display: flex; flex-wrap: wrap; gap: .75rem 1.25rem; align-items: flex-end; }
.generated signal-scope { flex: 1 1 20rem; min-width: 16rem; }
/* The unsaved indicator is a box of fixed width at the bar's end, its text cut
   to its own fixed width: the count or the error changing moves nothing. */
.unsaved { flex: none; width: 46ch; margin-left: auto; display: flex; align-items: center; color: var(--signal-unsaved, #ffb74d); }
.unsaved-text { flex: none; width: 28ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
<div class="bar" part="bar"><span class="status">starting…</span><span class="title"></span><span class="summary"></span><span class="unsaved" part="unsaved" role="status" style="visibility: hidden"><span class="unsaved-text"></span><button type="button" class="retry-all">Retry all</button><button type="button" class="revert-all">Revert all</button></span></div>
<ol class="problems" part="problems" role="alert" hidden></ol>
<div class="generated" part="generated"></div>
<slot></slot>`;
    this.connection = null;
    this.stops = [];
    /** The structure of the generated panel shown (`defaultPanel`), with `generated`. */
    this.generated = undefined;
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
    this.$('.unsaved-text').title = this.$('.unsaved-text').textContent;
    // Its space is kept: the indicator appearing moves nothing.
    this.$('.unsaved').style.visibility = line ? 'visible' : 'hidden';
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
    // The name comes only while playing; the title's fixed width keeps the
    // bar still when Play and Stop set and clear it ("Stable layout").
    this.$('.title').textContent = description.project ?? '';
    if (description.unsaved) this.showUnsaved(description.unsaved);
    if (this.hasAttribute('generated')) {
      // Rebuilt only when what it shows changes (docs/ui-controls.md
      // "Stable layout"): a new plan version with the same controls, meters
      // and scope keeps its elements, which take the new values themselves.
      // Only what generate() builds from: not the project's name, which a
      // describe gives only while playing (Play and Stop change nothing here).
      const { groups, meters, scope } = defaultPanel(description);
      const structure = JSON.stringify({ groups, meters, scope });
      if (structure === this.generated) return;
      this.generated = structure;
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
    // The output card (meters and the scope) spans the grid's width.
    if (output.length) sections.push(Object.assign(section('output', null, output), { className: 'wide' }));
    this.$('.generated').replaceChildren(...sections);
    this.$('.summary').textContent = 'generated panel (the project has no ui/main.html)';
  }
}

customElements.get('signal-panel') ?? customElements.define('signal-panel', SignalPanel);

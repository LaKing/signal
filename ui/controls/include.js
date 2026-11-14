// <signal-include bind="/drums"> (ADR 0035 §10): the panel of the project
// the top-level instance `drums` includes (or its slot loads), shown in the
// host's panel, rooted: every bind prefixed with the include's address
// (`/transport.bpm` → `/drums/transport.bpm`), its relative skins and its
// style sheets read from the include's own `ui/` (served under
// `/include/<key>/`). Its controls write the host, as any control does: an
// exported parameter the include's entry, any other top-level parameter an
// override in its `set`. Its layout is the linked project's, read-only
// here ("open <name> to edit it"). Drawn in its own shadow root, so the
// include's sheets style only its cards.

import { SignalElement } from './base.js';
import { showWritten } from './panel-editor.js';
import { faceAttribute, stylesheets } from '../lib/schematic-face.js';
import { includePanelFile, includeTitle, includePattern } from '../lib/include.js';

const CSS = `
:host { display: block; grid-column: 1 / -1; }
.head { display: flex; gap: 0.5rem; align-items: baseline; padding: 0.4rem 0.6rem; margin: 0.5rem 0 0.25rem; border-left: 3px solid var(--signal-accent, #4fc3f7); background: var(--signal-panel, #1f232b); }
.head h2 { margin: 0; font-size: 13px; letter-spacing: 0.08em; }
.head .note { color: var(--signal-muted, #8a93a3); font-size: 11px; }
.problem { color: var(--signal-clip, #ff5252); font-size: 12px; padding: 0 0.6rem; }
`;

/** An attribute of the included panel's control: its addresses rooted, its skins from the include's folder, its pattern the include's (read-only). */
function includeAttribute(name, value, key, base) {
  if (name === 'pattern') return includePattern(key, value);
  return faceAttribute(name, value, `/${key}`, base);
}

export class SignalInclude extends SignalElement {
  static role = 'group';
  static observedAttributes = ['bind'];
  static css = CSS;

  /** The include's key, from `bind` (`/drums`). */
  get key() {
    return /^\/([a-z][a-z0-9_]*)$/.exec(this.getAttribute('bind')?.trim() ?? '')?.[1] ?? null;
  }

  template() {
    return `<link rel="stylesheet" href="/ui/controls/theme.css"><div class="sheets"></div><div class="head"><h2></h2><span class="note"></span></div><div class="problem" hidden></div><div class="body"></div>`;
  }

  described(description) {
    this.description = description;
    const include = (description?.content?.includes ?? []).find((i) => i.key === this.key);
    this.$('h2').textContent = includeTitle(this.key, include);
    this.$('.note').textContent = include ? 'linked: open it to edit its panel' : '';
  }

  streams(connection) {
    this.read(connection);
    return [];
  }

  /** Reads the include's panel and draws it, rooted. */
  async read(connection) {
    const key = this.key;
    if (!key || !connection?.request) return;
    let read;
    try {
      read = await connection.request({ type: 'panel.read', file: includePanelFile(key) });
    } catch (error) {
      if (error?.code === 'closed') return;
      this.showProblem(error?.code === 'unknown-panel' ? null : (error?.message ?? String(error)));
      this.$('.body').replaceChildren();
      return;
    }
    if (key !== this.key) return;
    const base = new URL(`/include/${key}/main.html`, location.href).href;
    this.$('.sheets').replaceChildren(
      ...stylesheets(read?.text ?? '', base).map((href) => Object.assign(document.createElement('link'), { rel: 'stylesheet', href })),
    );
    if (typeof read?.html !== 'string') {
      this.showProblem(read?.reason ?? null);
      return;
    }
    const error = await showWritten(this.$('.body'), read.html, (name, value) => includeAttribute(name, value, key, base));
    this.showProblem(error);
  }

  showProblem(text) {
    const box = this.$('.problem');
    box.textContent = text ?? '';
    box.hidden = !text;
  }

  update() {}
}

customElements.get('signal-include') ?? customElements.define('signal-include', SignalInclude);

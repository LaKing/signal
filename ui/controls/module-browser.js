// The Add module browser (Edit patch ▸ Add module here…, docs/ui-controls.md
// "Editing the patch"; the owner's request of 2026-10-27): a modal dialog
// over the whole window. At the top a search field that filters the tree;
// at the left the module types as a tree, by source and category; at the
// right what the type under the cursor is: its full description, its
// inputs, outputs and parameters (kind, range or value set, default, unit),
// and previews of its panel card and of its schematic box (a composite's
// inside too), built from its `modules` entry with the real controls and
// the schematic's layout, inert: no instance exists and nothing is sent
// until Add. At the bottom the new instance's name, Cancel and Add. The
// logic is ui/lib/module-browser.js and ui/lib/module-preview.js; adding is
// the editor's (./schematic-editor.js).

import { browserKey, browserTree, detail, moveCursor, passesToPage, placeCursor, revealed, summary, toggled, treeRows } from '../lib/module-browser.js';
import { PreviewConnection, insideLevel, nodeLevel, previewDescribe, previewShapes, storedPositions } from '../lib/module-preview.js';
import { defaultPanel } from '../lib/panel.js';
import { exportFacts } from '../lib/schematic-groups.js';
import { GENERATED_CSS, generatedSections } from './panel.js';
import './knob.js';
import './fader.js';

const SVG = 'http://www.w3.org/2000/svg';

/** The browser's look, in <signal-schematic>'s shadow root (whose `.box`, `.pin`, `.wire`… draw the schematic preview). */
export const BROWSER_CSS = `
dialog.browser { box-sizing: border-box; width: calc(100vw - 24px); height: calc(100vh - 24px); max-width: none; max-height: none; margin: 12px; padding: 0;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-bg, #16181d); border: 1px solid var(--signal-focus, #5aa9ff); border-radius: var(--signal-radius, 6px);
  font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); overflow: hidden; }
dialog.browser[open] { display: grid; grid-template-rows: auto minmax(0, 1fr) auto; }
dialog.browser::backdrop { background: var(--signal-backdrop, rgba(0, 0, 0, 0.5)); }
.browser input { font: inherit; color: inherit; background: var(--signal-bg, #16181d); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 3px; padding: 3px 6px; box-sizing: border-box; min-width: 0; }
.browser .head, .browser .foot { display: flex; gap: 8px; align-items: center; padding: 8px 12px; background: var(--signal-panel, #1f232b); min-width: 0; }
.browser .head { border-bottom: 1px solid var(--signal-knob-edge, #3a414d); }
.browser .foot { border-top: 1px solid var(--signal-knob-edge, #3a414d); flex-wrap: wrap; }
.browser .head h2 { flex: none; margin: 0; font-size: 13px; font-weight: 600; white-space: nowrap; }
.browser .head .where { flex: none; max-width: 16rem; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.browser .head .search { flex: 1; }
.browser .head .count { flex: none; width: 14ch; text-align: right; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.browser .panes { display: grid; grid-template-columns: minmax(14rem, 24rem) minmax(0, 1fr); min-height: 0; }
.browser .tree { list-style: none; margin: 0; padding: 4px; overflow: auto; min-height: 0; border-right: 1px solid var(--signal-knob-edge, #3a414d); background: var(--signal-panel, #1f232b); }
.browser .tree li { display: flex; gap: 0.5ch; align-items: baseline; height: 1.7em; line-height: 1.7em; padding-right: 6px; border-radius: 3px; cursor: pointer; white-space: nowrap; overflow: hidden; }
.browser .tree li .twisty { flex: none; width: 1.2em; text-align: center; color: var(--signal-muted, #8a93a3); }
.browser .tree li .label { flex: none; max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
.browser .tree li .what { flex: 1; min-width: 0; color: var(--signal-muted, #8a93a3); overflow: hidden; text-overflow: ellipsis; }
.browser .tree li .n { flex: none; margin-left: auto; color: var(--signal-muted, #8a93a3); font-variant-numeric: tabular-nums; }
.browser .tree li.source { font-weight: 600; }
.browser .tree li.category .label { color: var(--signal-muted, #8a93a3); text-transform: uppercase; letter-spacing: 0.04em; font-size: 0.92em; font-weight: 600; }
.browser .tree li[aria-selected="true"] { background: var(--signal-knob, #262b34); color: var(--signal-accent, #4fc3f7); outline: 1px solid var(--signal-focus, #5aa9ff); outline-offset: -1px; }
.browser .tree li[aria-selected="true"] .label { color: var(--signal-accent, #4fc3f7); }
.browser .tree .empty { color: var(--signal-muted, #8a93a3); padding: 8px; cursor: default; }
.browser .failed { color: var(--signal-effective, #ffb74d); white-space: pre-wrap; overflow-wrap: anywhere; }
.browser .detail { overflow: auto; min-height: 0; min-width: 0; padding: 12px 16px 16px; }
.browser .detail h3 { margin: 0; font-size: 16px; overflow-wrap: anywhere; }
.browser .detail h4 { margin: 16px 0 6px; font-size: 11px; color: var(--signal-muted, #8a93a3); font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; }
.browser .detail .sub { color: var(--signal-muted, #8a93a3); margin: 2px 0 8px; }
.browser .detail .about { margin: 0; max-width: 72ch; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12px; }
.browser .detail .about.none { color: var(--signal-muted, #8a93a3); font-style: italic; }
.browser .detail .reference { color: var(--signal-effective, #ffb74d); margin-top: 6px; }
.browser .table { max-width: 100%; }
.browser table { border-collapse: collapse; font-variant-numeric: tabular-nums; width: 100%; }
.browser th, .browser td { text-align: left; vertical-align: top; padding: 3px 12px 3px 0; border-bottom: 1px solid var(--signal-track, #2a2f38); overflow-wrap: anywhere; }
.browser th { color: var(--signal-muted, #8a93a3); font-weight: 600; }
.browser td.name { font-weight: 600; }
.browser td ul { list-style: none; margin: 0; padding: 0; }
.browser td.muted, .browser .detail .none-row { color: var(--signal-muted, #8a93a3); }
.browser .previews { display: flex; flex-wrap: wrap; gap: 16px; align-items: flex-start; }
.browser .preview { flex: 1 1 22rem; min-width: 0; max-width: 100%; }
.browser .preview .frame-box { border: 1px dashed var(--signal-knob-edge, #3a414d); border-radius: var(--signal-radius, 6px); padding: 8px; overflow: hidden; background: var(--signal-bg, #16181d); }
.browser .preview .note { color: var(--signal-muted, #8a93a3); margin-top: 4px; }
.browser .preview .generated section { border-radius: 10px; background: var(--signal-panel, #1f232b); }
.browser .preview .inert { pointer-events: none; user-select: none; -webkit-user-select: none; }
.browser svg.preview-svg { position: static; inset: auto; display: block; width: 100%; height: auto; touch-action: auto; }
.browser .foot label { display: flex; gap: 6px; align-items: center; }
.browser .foot label[hidden] { display: none; }
.browser .foot .instance-name { width: 16rem; max-width: 50vw; }
.browser .foot .why { flex: 1; min-width: 8rem; color: var(--signal-effective, #ffb74d); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.browser .foot .buttons { display: flex; gap: 6px; margin-left: auto; }
.browser .foot button.add { border-color: var(--signal-focus, #5aa9ff); }
@media (max-width: 720px) {
  .browser .panes { grid-template-columns: minmax(0, 1fr); grid-template-rows: minmax(7rem, 38%) minmax(0, 1fr); }
  .browser .tree { border-right: none; border-bottom: 1px solid var(--signal-knob-edge, #3a414d); }
  .browser .head .where, .browser .head .count { display: none; }
}
${GENERATED_CSS}
`;

function html(name, properties = {}, children = []) {
  const node = Object.assign(document.createElement(name), properties);
  node.append(...children);
  return node;
}

/** A preview's shapes (`previewShapes`) as SVG elements. */
function svgShape(shape) {
  const node = document.createElementNS(SVG, shape.tag);
  for (const [key, value] of Object.entries(shape.attrs ?? {})) node.setAttribute(key, String(value));
  if (shape.text !== undefined) node.textContent = shape.text;
  for (const child of shape.children ?? []) node.append(svgShape(child));
  return node;
}

function previewSvg({ viewBox, shapes }, label) {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('class', 'preview-svg');
  svg.setAttribute('viewBox', `${viewBox.x} ${viewBox.y} ${viewBox.w} ${viewBox.h}`);
  svg.setAttribute('role', 'img');
  svg.setAttribute('aria-label', label);
  // Never larger than the schematic draws it at a scale of 1.
  svg.style.maxWidth = `${Math.ceil(viewBox.w)}px`;
  svg.append(...shapes.map(svgShape));
  return svg;
}

/** A table of `rows` with `columns` (`[key, heading]`); a cell may be a list of lines. */
function table(rows, columns) {
  const head = html('tr', {}, columns.map(([, heading]) => html('th', { scope: 'col', textContent: heading })));
  const body = rows.map((row) =>
    html(
      'tr',
      {},
      columns.map(([key]) => {
        const value = row[key];
        const cell = html('td', { className: key });
        if (Array.isArray(value)) cell.append(html('ul', {}, value.map((line) => html('li', { textContent: line }))));
        else cell.textContent = value;
        if (value === '—') cell.classList.add('muted');
        return cell;
      }),
    ),
  );
  return html('div', { className: 'table' }, [html('table', {}, [html('thead', {}, [head]), html('tbody', {}, body)])]);
}

export class ModuleBrowser {
  /**
   * `add(module, name)`: adds the type (returns an error text, or null once
   * it is added: the browser closes); `closed()`: the browser closed
   * without adding; `graph()`: the schematic's graph (stored positions and
   * composites for the previews); `defaultName(module)`: the name proposed.
   */
  constructor({ add, closed, graph, defaultName, mode = 'add', matchOf = null }) {
    // ADR 0037 §7: `load` is the Load dialog (Load…, Replace… of a slot):
    // no name, its entries sorted by how many of the slot's ports they
    // match (`matchOf(entry)`: `{matched, total, unmatched}`), and the
    // offer to add a content's unmatched ports to the slot.
    this.mode = mode;
    this.matchOf = matchOf;
    this.addType = add;
    this.defaultName = defaultName;
    this.closed = closed;
    this.graph = graph;
    this.entries = null;
    this.cursor = null;
    this.chosen = null;
    this.view = null;
    this.tree = null;
    this.previewKey = null;
    // The open branches, kept for the page's session (memory only).
    this.expanded = null;
  }

  /** The dialog, built once. */
  build(expanded) {
    this.expanded = new Set(expanded);
    const loading = this.mode === 'load';
    const dialog = html('dialog', { className: 'browser' });
    dialog.setAttribute('aria-label', loading ? 'Load into slot' : 'Add module');
    this.dialog = dialog;
    this.where = html('span', { className: 'where' });
    this.search = html('input', { type: 'search', className: 'search', placeholder: 'Search modules: name or description', spellcheck: false, autocomplete: 'off' });
    this.search.setAttribute('aria-label', 'Search modules');
    this.count = html('span', { className: 'count' });
    this.heading = html('h2', { textContent: loading ? 'Load into slot' : 'Add module' });
    const head = html('div', { className: 'head' }, [this.heading, this.where, this.search, this.count]);
    this.list = html('ul', { className: 'tree', id: 'module-tree' });
    this.list.setAttribute('role', 'tree');
    this.list.setAttribute('aria-label', 'Module types by source and category');
    this.search.setAttribute('aria-controls', 'module-tree');
    this.detailPane = html('div', { className: 'detail' });
    const panes = html('div', { className: 'panes' }, [this.list, this.detailPane]);
    this.name = html('input', { type: 'text', className: 'instance-name', placeholder: 'name', spellcheck: false, autocomplete: 'off' });
    this.name.setAttribute('aria-label', 'Name of the new instance');
    this.why = html('span', { className: 'why' });
    const cancel = html('button', { type: 'button', textContent: 'Cancel' });
    this.addButton = html('button', { type: 'button', className: 'add', textContent: loading ? 'Load' : 'Add' });
    this.unmatchedBox = html('input', { type: 'checkbox', className: 'add-unmatched' });
    this.unmatchedLabel = html('label', { className: 'unmatched' }, [this.unmatchedBox, document.createTextNode('Add its unmatched ports to the slot')]);
    const foot = html('div', { className: 'foot' }, [
      loading ? this.unmatchedLabel : html('label', {}, [document.createTextNode('Name'), this.name]),
      this.why,
      html('span', { className: 'buttons' }, [cancel, this.addButton]),
    ]);
    dialog.append(head, panes, foot);
    this.list.addEventListener('click', (event) => this.clicked(event));
    this.list.addEventListener('dblclick', (event) => this.doubleClicked(event));
    this.search.addEventListener('input', () => this.show());
    this.search.addEventListener('keydown', (event) => this.searchKey(event));
    this.name.addEventListener('keydown', (event) => {
      // An IME's Enter ends its composition: it adds nothing.
      if (event.key === 'Enter' && !event.isComposing) {
        event.preventDefault();
        this.add();
      }
    });
    // The previews show the instance by the name typed (its card's heading, its box).
    this.name.addEventListener('input', () => {
      this.why.textContent = '';
      this.showDetail();
    });
    this.addButton.addEventListener('click', () => this.add());
    cancel.addEventListener('click', () => this.close());
    // Escape (the dialog's cancel): closes it, the schematic keeps its Edit patch.
    dialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      this.close();
    });
    // Its keys are its own, never the schematic's (a modal: ⌘Z here is the
    // field's, Delete removes nothing), but for ⌘ and Ctrl chords, which go
    // on to the app's commands (`passesToPage`).
    dialog.addEventListener('keydown', (event) => {
      if (!passesToPage(event)) event.stopPropagation();
      if (event.key === 'Escape' && !event.isComposing) {
        event.preventDefault();
        event.stopPropagation();
        this.close();
      }
    });
    for (const type of ['pointerdown', 'pointerup', 'pointermove', 'dblclick']) dialog.addEventListener(type, (event) => event.stopPropagation());
    return dialog;
  }

  get isOpen() {
    return Boolean(this.dialog?.open);
  }

  /** Whether `event` happened in the browser (its wheel scrolls its panes, not the schematic). */
  holds(event) {
    return this.isOpen && (event.composedPath?.() ?? []).includes(this.dialog);
  }

  /**
   * Opens it, saying where the type will be added (`at the top level`,
   * `inside drums`), with the types known so far (`entries`, null until
   * `modules` answers: `setEntries`).
   */
  open({ where, entries = null, title = null }) {
    if (title) this.heading.textContent = title;
    if (this.unmatchedBox) this.unmatchedBox.checked = false;
    this.entries = this.ranked(entries);
    this.failure = null;
    this.detailModule = null;
    this.cursor = null;
    this.tree = null;
    this.where.textContent = where ?? '';
    this.search.value = '';
    this.name.value = '';
    this.why.textContent = '';
    this.chosen = null;
    this.previewKey = null;
    if (!this.dialog.open) {
      if (typeof this.dialog.showModal === 'function') this.dialog.showModal();
      else this.dialog.setAttribute('open', '');
    }
    this.show();
    this.search.focus({ preventScroll: true });
  }

  /** The types to browse (`browserEntries`), once `modules` answered. */
  setEntries(entries) {
    this.entries = this.ranked(entries);
    this.failure = null;
    if (this.isOpen) this.show();
  }

  /** The Load dialog's entries by how many of the slot's ports each matches (most first; a stable sort keeps the reply's order otherwise). */
  ranked(entries) {
    if (this.mode !== 'load' || !this.matchOf || !entries) return entries;
    return entries
      .map((entry, i) => ({ entry, i, match: this.matchOf(entry) }))
      .sort((a, b) => b.match.matched - a.match.matched || a.i - b.i)
      .map(({ entry }) => entry);
  }

  /** `modules` was refused: says why, in the panes, until the types come (no more "Loading…"). */
  failed(message) {
    this.failure = message || 'the module types could not be read';
    if (this.isOpen) this.show();
  }

  close() {
    if (!this.dialog?.open) return;
    if (typeof this.dialog.close === 'function') this.dialog.close();
    else this.dialog.removeAttribute('open');
    this.clearPreviews();
    this.closed?.();
  }

  searchKey(event) {
    if (!this.view) return;
    const action = browserKey(event, this.view, this.cursor, { fieldEmpty: this.search.value === '' });
    if (!action) return;
    event.preventDefault();
    if (action === 'add') this.add();
    else if (action.toggle) this.toggle(action.toggle);
    else if (action.cursor) {
      this.cursor = action.cursor;
      this.show();
    } else if (action.move) {
      this.cursor = moveCursor(this.view, this.cursor, action.move);
      this.show();
    }
  }

  toggle(key) {
    this.expanded = toggled(this.expanded, key);
    this.cursor = key;
    this.show();
  }

  /**
   * The tree built again for the search, the cursor kept on its row (a type
   * chosen during a search stays in sight once it is cleared), its row
   * marked and scrolled into view, and the detail of the type chosen.
   */
  show() {
    if (!this.dialog) return;
    const tree = browserTree(this.entries ?? [], this.search.value);
    // A search cleared: the branches above the type chosen open, so it stays the cursor's.
    if (this.tree?.searching && !tree.searching && this.cursor) this.expanded = revealed(this.expanded, tree, this.cursor);
    this.tree = tree;
    const view = treeRows(tree, this.expanded);
    this.view = view;
    this.cursor = placeCursor(view, this.cursor);
    const row = view.rows.find((r) => r.key === this.cursor);
    if (row?.kind === 'entry' && row.entry.module !== this.chosen?.module) this.choose(row.entry);
    else if (!row && this.chosen) this.choose(null);
    let selected = null;
    const items = view.rows.map((r, i) => {
      const item = this.rowItem(r, i);
      const on = r.key === this.cursor;
      item.setAttribute('aria-selected', String(on));
      if (on) selected = item;
      return item;
    });
    if (this.entries === null && this.failure) items.push(html('li', { className: 'empty failed', textContent: `No module types: ${this.failure}` }));
    else if (this.entries !== null && !items.length) items.push(html('li', { className: 'empty', textContent: view.searching ? 'No module matches' : 'No module types' }));
    this.list.replaceChildren(...items);
    if (selected) {
      this.search.setAttribute('aria-activedescendant', selected.id);
      selected.scrollIntoView?.({ block: 'nearest' });
    } else this.search.removeAttribute('aria-activedescendant');
    this.count.textContent = this.entries === null ? (this.failure ? 'Not loaded' : 'Loading…') : view.searching ? `${view.matched} match${view.matched === 1 ? '' : 'es'}` : `${view.matched} types`;
    this.showDetail();
  }

  rowItem(r, index) {
    const item = html('li', { className: r.kind, id: `module-row-${index}` });
    item.setAttribute('role', 'treeitem');
    item.setAttribute('aria-level', String(r.depth + 1));
    item.style.paddingLeft = `${4 + r.depth * 16}px`;
    item.dataset.key = r.key;
    if (r.kind === 'entry') {
      item.title = r.entry.description ?? r.entry.module;
      const match = this.mode === 'load' && this.matchOf ? this.matchOf(r.entry) : null;
      const what = match ? `${match.matched}/${match.total} match · ${summary(r.entry.description)}` : summary(r.entry.description);
      item.append(html('span', { className: 'twisty' }), html('span', { className: 'label', textContent: r.entry.module }), html('span', { className: 'what', textContent: what }));
    } else {
      item.setAttribute('aria-expanded', String(r.expanded));
      item.append(html('span', { className: 'twisty', textContent: r.expanded ? '▾' : '▸' }), html('span', { className: 'label', textContent: r.label }), html('span', { className: 'n', textContent: String(r.count) }));
    }
    return item;
  }

  /** The row a pointer event is on (the list's own rows, by key), else null. */
  rowAt(event) {
    const key = event.target?.closest?.('li[data-key]')?.dataset.key;
    return key ? (this.view?.rows.find((r) => r.key === key) ?? null) : null;
  }

  /** A click: a type becomes the cursor's; a branch opens or closes. The focus stays in the search field. */
  clicked(event) {
    const row = this.rowAt(event);
    if (!row) return;
    if (row.kind === 'entry') {
      if (this.cursor !== row.key) {
        this.cursor = row.key;
        this.show();
      }
    } else if (!this.view.searching && !(event.detail > 1)) {
      // A branch opens or closes on its first click: a double-click toggles it once. With a search every branch is open, and the cursor stays on its type.
      this.toggle(row.key);
    }
    this.search.focus({ preventScroll: true });
  }

  /** A double-click on a type adds it, as Enter does (its first click made it the cursor's). */
  doubleClicked(event) {
    const row = this.rowAt(event);
    if (row?.kind !== 'entry') return;
    if (this.cursor !== row.key) {
      this.cursor = row.key;
      this.show();
    }
    this.add();
  }

  /** `entry` chosen: its default name, editable before adding. */
  choose(entry) {
    this.chosen = entry;
    this.name.value = entry ? (this.defaultName?.(entry.module) ?? '') : '';
    this.why.textContent = '';
  }

  add() {
    if (!this.chosen) return;
    const options = this.mode === 'load' ? { addUnmatched: Boolean(this.unmatchedBox?.checked), entry: this.chosen } : undefined;
    const error = this.addType?.(this.chosen.module, this.name.value.trim(), options);
    if (error) this.why.textContent = error;
  }

  // The detail pane.

  showDetail() {
    const entry = this.chosen;
    const key = entry ? `${entry.module}\n${this.instanceName(entry)}` : `\n${this.entries === null ? (this.failure ?? 'loading') : ''}`;
    if (key === this.previewKey) return;
    this.previewKey = key;
    this.clearPreviews();
    if (!entry) {
      const text = this.entries !== null ? 'Choose a module type to see what it is.' : this.failure ? `The module types could not be read: ${this.failure}` : 'Loading the module types…';
      this.detailPane.replaceChildren(html('p', { className: this.failure && this.entries === null ? 'failed' : 'none-row', textContent: text }));
      return;
    }
    const d = detail(entry);
    const parts = [html('h3', { textContent: d.module }), html('div', { className: 'sub', textContent: `${d.where} · ${d.counts}` })];
    parts.push(html('p', { className: d.description ? 'about' : 'about none', textContent: d.description ?? (entry.composite ? 'This composite has no description (its file may give one: [composite] description).' : 'No description.') }));
    if (d.referenceFor) parts.push(html('div', { className: 'reference', textContent: `A validation reference: rebuilds ${d.referenceFor} from core primitives, to be compared with it.` }));
    // The Load dialog: how it matches the slot, by name (ADR 0037 §4).
    const match = this.mode === 'load' && this.matchOf ? this.matchOf(entry) : null;
    if (match) {
      const unmatched = match.unmatched.map((c) => `${c.name} (${c.role})`).join(', ');
      parts.push(html('h4', { textContent: 'Match' }), html('div', { className: 'sub', textContent: `${match.matched} of the slot’s ${match.total} ports and parameters match by name.${unmatched ? ` Not in the slot: ${unmatched} (Add its unmatched ports adds them).` : ''}` }));
      this.unmatchedLabel.hidden = match.unmatched.length === 0;
    }
    const portColumns = [['name', 'Name'], ['kind', 'Kind'], ['shape', 'Shape'], ['rate', 'Rate'], ['range', 'Range'], ['unit', 'Unit']];
    const section = (title, rows, columns) => [html('h4', { textContent: `${title} (${rows.length})` }), rows.length ? table(rows, columns) : html('div', { className: 'none-row', textContent: 'none' })];
    parts.push(...section('Inputs', d.inputs, portColumns));
    parts.push(...section('Outputs', d.outputs, portColumns));
    parts.push(...section('Parameters', d.parameters.map((p) => ({ ...p, domain: p.values ?? p.range })), [['name', 'Name'], ['kind', 'Kind'], ['domain', 'Range / values'], ['default', 'Default'], ['unit', 'Unit'], ['combine', 'Combine']]));
    // The Load dialog shows how a content matches instead: it adds no instance to preview.
    if (!entry.project && this.mode !== 'load') parts.push(html('h4', { textContent: 'Previews' }), html('div', { className: 'previews' }, [this.panelPreview(entry), this.schematicPreview(entry)]));
    const scroll = this.detailPane.scrollTop;
    const same = this.detailModule === entry.module;
    this.detailModule = entry.module;
    this.detailPane.replaceChildren(...parts);
    // A new type starts at its top; a rename keeps the place.
    this.detailPane.scrollTop = same ? scroll : 0;
  }

  /** The name the previews show the instance by: the one typed when it is a name, else the type's last segment. */
  instanceName(entry) {
    const typed = this.name.value.trim();
    return /^[a-z][a-z0-9_]*$/.test(typed) ? typed : entry.module.split('.').pop();
  }

  /** The instance's card as the generated panel would show it: the real controls, inert, over a connection that sends nothing. */
  panelPreview(entry) {
    const name = this.instanceName(entry);
    const description = previewDescribe(entry, name);
    const spec = defaultPanel(description);
    const box = html('div', { className: 'frame-box' });
    const note = html('div', { className: 'note' });
    if (spec.groups.length) {
      const connection = new PreviewConnection(description);
      this.previewConnection = connection;
      const grid = html('div', { className: 'generated inert' });
      grid.inert = true;
      grid.setAttribute('aria-hidden', 'true');
      const sections = generatedSections(spec);
      for (const control of sections.flatMap((s) => [...s.querySelectorAll('*')]).filter((e) => e.localName.startsWith('signal-'))) {
        control.useConnection?.(connection);
        control.tabIndex = -1;
      }
      grid.append(...sections);
      box.append(grid);
      note.textContent = 'The card the generated panel gives a top-level instance, at the defaults (a preview: nothing moves or is sent).';
    } else {
      box.append(html('div', { className: 'none-row', textContent: 'No parameters: the generated panel gives it no card.' }));
      note.textContent = 'A panel shows a type’s parameters; this one has none.';
    }
    return html('div', { className: 'preview' }, [html('h4', { textContent: 'Panel' }), box, note]);
  }

  /** The instance's box as the schematic draws it, and a composite's inside. */
  schematicPreview(entry) {
    const name = this.instanceName(entry);
    const known = new Map((this.entries ?? []).filter((e) => e.inside).map((e) => [e.module, e.inside]));
    const factsOf = (module) => exportFacts(known.get(module) ?? this.graph?.()?.composites?.[module] ?? null);
    const box = html('div', { className: 'frame-box' }, [previewSvg(previewShapes(nodeLevel(entry, name), {}, factsOf), `${entry.module} as a box in the schematic`)]);
    const parts = [html('h4', { textContent: 'Schematic' }), box, html('div', { className: 'note', textContent: 'Its box, as the schematic lays it out.' })];
    const inside = insideLevel(entry);
    if (inside) {
      const positions = storedPositions(this.graph?.(), entry);
      const shapes = previewShapes(inside, positions, factsOf);
      parts.push(
        html('div', { className: 'frame-box' }, [previewSvg(shapes, `the inside of ${entry.module}`)]),
        html('div', { className: 'note', textContent: `Its inside: ${inside.instances.length} instance${inside.instances.length === 1 ? '' : 's'}, ${(inside.connections ?? []).length} connection${(inside.connections ?? []).length === 1 ? '' : 's'}, its exports at the frame’s inputs and outputs.` }),
      );
    }
    return html('div', { className: 'preview' }, parts);
  }

  clearPreviews() {
    this.previewConnection = null;
  }
}

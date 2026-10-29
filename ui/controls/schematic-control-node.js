// The schematic sidebar's properties of a selected panel control (the
// owner's, 2026-10-29: "a small square element, with a property table";
// ui/lib/schematic-controls.js). Its label, range, default and unit are
// fields, written into the panel file the panel editor writes (`panel.read`,
// then `panel.write` of the panel's content with that one attribute
// changed, `docs/ui-protocol.md`), so the file stays what the panel means:
// nothing is kept here. A panel the editor cannot write (outside its
// whitelist, ui/lib/panel-whitelist.json) shows its fields read-only, with
// why.

import { parseContent, serialize } from '../lib/panel-tree.js';
import { setAttributes } from '../lib/panel-edit.js';
import { attributeValue, editableAttributes, findInTree } from '../lib/schematic-controls.js';
import { loadWhitelist } from './panel-editor.js';

const PANEL_FILE = 'main.html';

function html(name, properties = {}) {
  return Object.assign(document.createElement(name), properties);
}

export class ControlProperties {
  /** For `<signal-schematic>` `schematic` (its connection, `say`, `panelSeen`). */
  constructor(schematic) {
    this.s = schematic;
    this.rules = null;
    loadWhitelist().then(
      (rules) => {
        this.rules = rules;
      },
      () => {},
    );
  }

  /** The sidebar's Properties section of control `node`: a field per attribute it may edit. */
  parts(node) {
    const names = editableAttributes(node.tag, this.rules);
    if (!names.length) return [];
    const attributes = node.control?.attributes ?? {};
    const parts = [html('h4', { textContent: 'Properties (ui/main.html)' })];
    for (const name of names) {
      const row = html('label', { className: 'row prop' });
      const input = html('input', { type: 'text', value: attributes[name] ?? '', placeholder: '—', spellcheck: false });
      input.setAttribute('aria-label', `${node.name}: ${name}`);
      const commit = () => {
        if (input.value === (attributes[name] ?? '')) return;
        this.write(node, name, input.value);
      };
      input.addEventListener('change', commit);
      input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          input.blur();
        } else if (event.key === 'Escape') {
          event.stopPropagation();
          input.value = attributes[name] ?? '';
          input.blur();
        }
      });
      row.append(html('span', { textContent: name }), input);
      parts.push(row);
    }
    parts.push(html('div', { className: 'subtitle', textContent: 'Written as Edit layout writes the panel (its canonical form). Empty removes the attribute.' }));
    return parts;
  }

  /** Attribute `name` of control `node` set to `text` in the panel file: read, changed in the editor's tree, written against the base read. */
  async write(node, name, text) {
    const checked = attributeValue(name, text);
    if (checked.error) {
      this.s.say(`Not written: ${checked.error}`);
      return;
    }
    const connection = this.s.connection;
    if (!connection?.request) return;
    try {
      const rules = this.rules ?? (await loadWhitelist());
      const read = await connection.request({ type: 'panel.read', file: PANEL_FILE });
      if (!read.exists) return this.s.say('Not written: the project has no panel file');
      if (!read.editable) return this.s.say(`Not written: ${read.reason ?? 'the panel is not editable'}`);
      if (!read.planned) return this.s.say('Not written: the project has no plan, and a panel is written only when its bindings check');
      const parsed = parseContent(read.html, rules, { mode: 'read' });
      if (parsed.error) return this.s.say(`Not written: the panel is not one the editor writes (${parsed.error}); edit ui/main.html by hand`);
      const found = findInTree(parsed.tree, node.control);
      if (!found) return this.s.say('Not written: the control is no longer in the panel');
      const changed = setAttributes(parsed.tree, found.id, { [name]: checked.value });
      if (changed.error) return this.s.say(`Not written: ${changed.error}`);
      const content = serialize(changed.tree, rules);
      await connection.request({ type: 'panel.write', file: PANEL_FILE, html: content, base: read.base });
      this.s.say(`${node.name}: ${name} ${checked.value === null ? 'removed' : `= ${checked.value}`} (ui/main.html)`);
      this.s.panelSeen(content);
    } catch (error) {
      this.s.say(`Not written: ${error.message}`);
    }
  }
}

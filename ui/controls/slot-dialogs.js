// The slot dialogs of the schematic's Edit patch (ADR 0037 §7, phase 3;
// docs/ui-controls.md "Slots"): Add slot (a name and the interface to
// start from), the interface editor (add, remove, rename and retype ports
// and parameters), the map editor, and a template's name (Save interface as
// template…, Make interface from this instance…). Each is a modal dialog
// over the schematic; what it writes is the editor's (./schematic-editor.js),
// a `patch` (or `file.write` for a template) the server judges. The rows and
// ops are ui/lib/slot.js's. Its keys are its own, as the Add module
// browser's are (⌘ and Ctrl chords but undo and redo go on to the page).

import { passesToPage } from '../lib/module-browser.js';
import { KINDS, ROLES, freeName, interfaceOp, interfaceRows, mapOp, newRow, portNameError, templateNameError } from '../lib/slot.js';

/** The dialogs' look, in <signal-schematic>'s shadow root: theme tokens only, fitting a small window. */
export const SLOT_DIALOG_CSS = `
dialog.slot-dialog { box-sizing: border-box; width: min(56rem, calc(100vw - 24px)); max-height: calc(100vh - 24px); margin: auto; padding: 0;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-bg, #16181d); border: 1px solid var(--signal-focus, #5aa9ff); border-radius: var(--signal-radius, 6px);
  font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); overflow: hidden; }
dialog.slot-dialog[open] { display: grid; grid-template-rows: auto minmax(0, 1fr) auto; }
dialog.slot-dialog::backdrop { background: var(--signal-backdrop, rgba(0, 0, 0, 0.5)); }
.slot-dialog h2 { margin: 0; font-size: 13px; font-weight: 600; overflow-wrap: anywhere; }
.slot-dialog .dialog-head, .slot-dialog .dialog-foot { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; padding: 8px 12px; background: var(--signal-panel, #1f232b); }
.slot-dialog .dialog-head { border-bottom: 1px solid var(--signal-knob-edge, #3a414d); }
.slot-dialog .dialog-foot { border-top: 1px solid var(--signal-knob-edge, #3a414d); }
.slot-dialog .dialog-head .dialog-note { flex: 1 1 12rem; color: var(--signal-muted, #8a93a3); }
.slot-dialog .body { overflow: auto; min-height: 0; padding: 8px 12px; }
.slot-dialog input, .slot-dialog select { font: inherit; color: inherit; background: var(--signal-bg, #16181d); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 3px; padding: 2px 4px; box-sizing: border-box; min-width: 0; }
.slot-dialog input.bad { border-color: var(--signal-effective, #ffb74d); }
.slot-dialog button { font: inherit; }
.slot-dialog table { border-collapse: collapse; width: 100%; }
.slot-dialog th, .slot-dialog td { text-align: left; padding: 2px 6px 2px 0; border-bottom: 1px solid var(--signal-track, #2a2f38); vertical-align: middle; }
.slot-dialog th { color: var(--signal-muted, #8a93a3); font-weight: 600; white-space: nowrap; }
.slot-dialog td input.port-name { width: 10rem; }
.slot-dialog td input.small { width: 4rem; }
.slot-dialog td input.unit { width: 5rem; }
.slot-dialog td.reason { color: var(--signal-muted, #8a93a3); }
.slot-dialog tr.focus td { background: var(--signal-knob, #262b34); }
.slot-dialog .field { display: flex; flex-direction: column; gap: 4px; margin: 6px 0 10px; }
.slot-dialog .field input, .slot-dialog .field select { width: min(24rem, 100%); }
.slot-dialog .why { flex: 1 1 10rem; color: var(--signal-effective, #ffb74d); white-space: pre-wrap; overflow-wrap: anywhere; }
.slot-dialog .dialog-buttons { display: flex; gap: 6px; margin-left: auto; }
.slot-dialog button.primary { border-color: var(--signal-focus, #5aa9ff); }
.slot-dialog .adds { display: flex; flex-wrap: wrap; gap: 6px; margin: 8px 0; }
.slot-dialog label.check { display: flex; gap: 6px; align-items: center; }
@media (max-width: 720px) {
  .slot-dialog td input.port-name { width: 7rem; }
  .slot-dialog th.optional, .slot-dialog td.optional { display: none; }
}`;

function html(name, properties = {}, children = []) {
  const node = Object.assign(document.createElement(name), properties);
  node.append(...children);
  return node;
}

function select(options, value, label) {
  const s = html('select');
  s.setAttribute('aria-label', label);
  for (const option of options) {
    const o = html('option', { value: option.value, textContent: option.label });
    if (option.title) o.title = option.title;
    s.append(o);
  }
  s.value = value;
  return s;
}

export class SlotDialogs {
  /** `closed()`: a dialog closed (the schematic takes the focus back). */
  constructor({ closed } = {}) {
    this.closed = closed;
    this.dialog = null;
  }

  /** The dialog element, built once (its content per use). */
  build() {
    const dialog = html('dialog', { className: 'slot-dialog' });
    this.dialog = dialog;
    dialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      this.close();
    });
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

  /** Whether `event` happened in a dialog (its wheel scrolls it, not the schematic). */
  holds(event) {
    return this.isOpen && (event.composedPath?.() ?? []).includes(this.dialog);
  }

  close() {
    if (!this.dialog?.open) return;
    if (typeof this.dialog.close === 'function') this.dialog.close();
    else this.dialog.removeAttribute('open');
    this.closed?.();
  }

  /**
   * Shows `title`, `note`, `body` (nodes) and the buttons; `primary` is the
   * button Enter presses. Returns the place for what a write says (`why`).
   */
  show({ title, note = '', body, primary, extra = [] }) {
    const why = html('span', { className: 'why' });
    const cancel = html('button', { type: 'button', textContent: 'Cancel' });
    cancel.addEventListener('click', () => this.close());
    primary.classList.add('primary');
    this.dialog.replaceChildren(
      html('div', { className: 'dialog-head' }, [html('h2', { textContent: title }), html('span', { className: 'dialog-note', textContent: note })]),
      html('div', { className: 'body' }, body),
      html('div', { className: 'dialog-foot' }, [...extra, why, html('span', { className: 'dialog-buttons' }, [cancel, primary])]),
    );
    this.dialog.setAttribute('aria-label', title);
    if (!this.dialog.open) {
      if (typeof this.dialog.showModal === 'function') this.dialog.showModal();
      else this.dialog.setAttribute('open', '');
    }
    return why;
  }

  /** Runs `write()` (a promise of `{ok, error}`): closes on success, else says why and stays. */
  async run(write, why, button) {
    button.disabled = true;
    why.textContent = '';
    try {
      const result = await write();
      if (result?.ok) this.close();
      else why.textContent = result?.error ?? 'Not written';
    } finally {
      button.disabled = false;
    }
  }

  /**
   * Add slot (Edit patch ▸ Add slot here…): its name (`defaultName`, unique
   * among `names`) and the interface it starts from (`choices`:
   * `templateChoices`'). `add(name, template)` writes it.
   */
  openAddSlot({ where, choices, names, defaultName, add }) {
    const name = html('input', { type: 'text', className: 'slot-name', value: defaultName, spellcheck: false, autocomplete: 'off' });
    name.setAttribute('aria-label', 'Name of the new slot');
    const template = select(choices, '', 'The interface it starts from');
    const button = html('button', { type: 'button', textContent: 'Add slot' });
    const why = this.show({
      title: 'Add slot',
      note: `${where}: an instance whose inputs, outputs and parameters are its own, and whose content you load into it`,
      body: [
        html('label', { className: 'field' }, [document.createTextNode('Name'), name]),
        html('label', { className: 'field' }, [document.createTextNode('Interface (a template it starts from; editable afterwards)'), template]),
      ],
      primary: button,
    });
    const submit = () => {
      const typed = name.value.trim();
      const error = portNameError(typed) ? 'A name is a lower-case letter, then lower-case letters, digits and _' : names.has(typed) ? `The level already has an instance ${typed}` : null;
      if (error) {
        why.textContent = error;
        return;
      }
      this.run(() => add(typed, template.value), why, button);
    };
    button.addEventListener('click', submit);
    name.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && !event.isComposing) {
        event.preventDefault();
        submit();
      }
    });
    name.focus?.({ preventScroll: true });
    name.select?.();
  }

  /**
   * The interface editor (Edit interface…, a "+" pin, a pin's Rename or
   * retype port…): one row per port and parameter of `slot`, each edited in
   * place; "+ input", "+ output", "+ parameter" add rows; × removes one.
   * `focus`: the row of that port is marked (and its name field focused), or
   * `{add: 'in' | 'out'}` adds a row first. `apply(op)` writes the
   * `slot.interface` op; `saveTemplate()` and `detach()` are the template's.
   */
  openInterface({ slotName, slot, focus = null, apply, saveTemplate, detach }) {
    const before = interfaceRows(slot);
    const rows = before.map((r) => ({ ...r }));
    const template = slot?.from ?? slot?.interface?.from ?? null;
    if (focus?.add) rows.push(newRow(focus.add === 'out' ? 'output' : 'input', freeName(rows, focus.add === 'out' ? 'out' : 'in')));
    const focusName = focus?.port ?? (focus?.add ? rows.at(-1).name : null);
    const table = html('table');
    const removeWires = html('input', { type: 'checkbox' });
    let firstFocus = null;
    const render = () => {
      const head = html('tr', {}, ['Name', 'Role', 'Kind', 'Shape', 'Rate', 'Unit', 'Group', ''].map((t, i) => html('th', { textContent: t, className: i >= 4 && i <= 6 ? 'optional' : '' })));
      const body = rows.map((row, index) => {
        const name = html('input', { type: 'text', className: 'port-name', value: row.name, spellcheck: false, autocomplete: 'off' });
        name.setAttribute('aria-label', `Name of ${row.original ?? 'the new port'}`);
        name.addEventListener('input', () => {
          row.name = name.value.trim();
          name.classList.toggle('bad', Boolean(portNameError(row.name)));
        });
        const role = select(ROLES.map((r) => ({ value: r, label: r })), row.role, `Role of ${row.name}`);
        role.addEventListener('change', () => {
          row.role = role.value;
          render();
        });
        const parameter = row.role === 'parameter';
        const kind = select(KINDS.map((k) => ({ value: k, label: k })), row.kind, `Kind of ${row.name}`);
        kind.disabled = parameter;
        kind.addEventListener('change', () => (row.kind = kind.value));
        const shape = select(['signal', 'events', 'vector'].map((v) => ({ value: v, label: v })), row.shape, `Shape of ${row.name}`);
        shape.disabled = parameter;
        const width = html('input', { type: 'number', className: 'small', min: 1, value: row.width ?? '', hidden: row.shape !== 'vector' });
        width.setAttribute('aria-label', `Vector width of ${row.name}`);
        width.addEventListener('input', () => (row.width = Number(width.value)));
        shape.addEventListener('change', () => {
          row.shape = shape.value;
          width.hidden = row.shape !== 'vector';
          if (row.shape === 'vector' && !row.width) row.width = 2;
          width.value = row.width ?? '';
        });
        const rate = html('input', { type: 'number', className: 'small', min: 1, value: row.rate ?? 1, disabled: parameter });
        rate.setAttribute('aria-label', `Rate divisor of ${row.name}`);
        rate.addEventListener('input', () => (row.rate = Number(rate.value)));
        const unit = html('input', { type: 'text', className: 'unit', value: row.unit ?? '', spellcheck: false });
        unit.setAttribute('aria-label', `Unit of ${row.name}`);
        unit.addEventListener('input', () => (row.unit = unit.value.trim()));
        const group = html('input', { type: 'text', className: 'unit', value: row.group ?? '', spellcheck: false });
        group.setAttribute('aria-label', `Group of ${row.name}`);
        group.addEventListener('input', () => (row.group = group.value.trim()));
        const remove = html('button', { type: 'button', textContent: '×', title: `Remove ${row.name}` });
        remove.addEventListener('click', () => {
          rows.splice(index, 1);
          render();
        });
        const tr = html('tr', { className: row.name === focusName ? 'focus' : '' }, [
          html('td', {}, [name]),
          html('td', {}, [role]),
          html('td', {}, [kind]),
          html('td', {}, [shape, width]),
          html('td', { className: 'optional' }, [rate]),
          html('td', { className: 'optional' }, [unit]),
          html('td', { className: 'optional' }, [group]),
          html('td', {}, [remove]),
        ]);
        if (row.name === focusName && !firstFocus) firstFocus = name;
        return tr;
      });
      table.replaceChildren(html('thead', {}, [head]), html('tbody', {}, body.length ? body : [html('tr', {}, [html('td', { colSpan: 8, textContent: 'No ports yet: add an input, an output or a parameter.' })])]));
    };
    render();
    const adds = html('div', { className: 'adds' }, ['input', 'output', 'parameter'].map((role) => {
      const b = html('button', { type: 'button', textContent: `+ ${role}` });
      b.addEventListener('click', () => {
        rows.push(newRow(role, freeName(rows, role === 'input' ? 'in' : role === 'output' ? 'out' : 'param')));
        render();
      });
      return b;
    }));
    const button = html('button', { type: 'button', textContent: 'Apply' });
    const extra = [html('label', { className: 'check' }, [removeWires, document.createTextNode('Remove the wires of removed ports too')])];
    const save = html('button', { type: 'button', textContent: 'Save as template…' });
    save.addEventListener('click', () => saveTemplate?.());
    extra.push(save);
    if (template) {
      const off = html('button', { type: 'button', textContent: 'Detach from template' });
      off.addEventListener('click', () => detach?.());
      extra.push(off);
    }
    const why = this.show({
      title: `Edit interface: ${slotName}`,
      note: template
        ? `From ${template}: removing one of its ports names it in remove; a rename rewrites the wires, map keys and panel bindings on it.`
        : 'Its own interface: a rename rewrites the wires, map keys and panel bindings on the port.',
      body: [table, adds],
      primary: button,
      extra,
    });
    button.addEventListener('click', () => {
      const { op, problems } = interfaceOp(slotName, before, rows, { removeWires: removeWires.checked });
      if (problems.length) {
        why.textContent = problems.join('\n');
        return;
      }
      if (!op) {
        this.close();
        return;
      }
      this.run(() => apply(op), why, button);
    });
    (firstFocus ?? table.querySelector?.('input'))?.focus?.({ preventScroll: true });
  }

  /**
   * The map editor (Edit map…): per interface port and parameter, the
   * content's name it takes (by name, a content name of its role, or none:
   * `false`). `rows`: `mapRows`'; `apply(op)` writes `slot.map`.
   */
  openMap({ slotName, load, rows, apply }) {
    const table = html('table');
    const head = html('tr', {}, ['Slot port', 'Role', 'Content’s'].map((t) => html('th', { textContent: t })));
    const body = rows.map((row) => {
      const options = [{ value: '', label: `by name (${row.port})` }, { value: 'false', label: 'none (false: unmatched)' }, ...row.options.filter((o) => o !== row.port).map((o) => ({ value: o, label: o }))];
      if (typeof row.value === 'string' && row.value && !row.options.includes(row.value)) options.push({ value: row.value, label: `${row.value} (not in the content)` });
      const value = row.value === false ? 'false' : (row.value ?? '');
      const choice = select(options, value, `What ${row.port} takes`);
      choice.addEventListener('change', () => {
        row.value = choice.value === 'false' ? false : choice.value;
      });
      return html('tr', {}, [html('td', { textContent: row.port }), html('td', { textContent: row.role }), html('td', {}, [choice])]);
    });
    table.append(html('thead', {}, [head]), html('tbody', {}, body.length ? body : [html('tr', {}, [html('td', { colSpan: 3, textContent: 'The slot has no ports to map.' })])]));
    const button = html('button', { type: 'button', textContent: 'Apply' });
    const why = this.show({
      title: `Edit map: ${slotName}`,
      note: load ? `What each port of the slot takes of ${load}; by name unless the map says otherwise (ADR 0037 §4).` : 'The slot is empty: the map applies to what is loaded next.',
      body: [table],
      primary: button,
    });
    button.addEventListener('click', () => this.run(() => apply(mapOp(slotName, rows)), why, button));
  }

  /**
   * A template's name (Save interface as template…, Make interface from
   * this instance…): `save(name, description)` writes
   * `interfaces/<name>.toml`. `taken`: the templates the project has.
   */
  openTemplateName({ title, note, defaultName, taken, save }) {
    const name = html('input', { type: 'text', value: defaultName, spellcheck: false, autocomplete: 'off' });
    name.setAttribute('aria-label', 'Template name');
    const description = html('input', { type: 'text', value: '', spellcheck: true });
    description.setAttribute('aria-label', 'What the interface is for');
    const button = html('button', { type: 'button', textContent: 'Save' });
    const why = this.show({
      title,
      note,
      body: [
        html('label', { className: 'field' }, [document.createTextNode('Name (interfaces/<name>.toml)'), name]),
        html('label', { className: 'field' }, [document.createTextNode('Description (optional)'), description]),
      ],
      primary: button,
    });
    const submit = () => {
      const typed = name.value.trim();
      const error = templateNameError(typed) ?? (taken.has(typed) ? `interfaces/${typed}.toml exists already` : null);
      if (error) {
        why.textContent = error;
        return;
      }
      this.run(() => save(typed, description.value.trim() || null), why, button);
    };
    button.addEventListener('click', submit);
    name.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && !event.isComposing) {
        event.preventDefault();
        submit();
      }
    });
    name.focus?.({ preventScroll: true });
  }
}

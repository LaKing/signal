// Slots in the UI (ADR 0037 §2, §7, phase 3; docs/ui-controls.md "Slots"):
// what a slot box and the slot card say of a slot (`graph`'s `slot` field,
// `describe`'s `content.slots`), the rows of the interface editor and the
// map editor and the `slot.*` patch ops they write, how well a content
// matches an interface (the Load dialog's counts), the projects the Load
// dialog lists, and an interface template's text. The server writes the
// text and is the judge (docs/ui-protocol.md "patch"). No DOM.

/** The port kinds an interface port may declare (`PortKind`). */
export const KINDS = Object.freeze(['audio', 'control', 'gate', 'trigger', 'frequency', 'gain', 'phase', 'generic']);

/** The roles of an interface entry, in the order an interface lists them. */
export const ROLES = Object.freeze(['input', 'output', 'parameter']);

/** The library templates, when `graph` names none (an older server). */
export const LIBRARY_TEMPLATES = Object.freeze(['lib.voice', 'lib.instrument', 'lib.deck']);

/** A port or slot name: `[a-z][a-z0-9_]*`, never `from` or `remove` (reserved in a slot's table). */
const NAME = /^[a-z][a-z0-9_]*$/;

/** Why `name` cannot name a port of an interface, or null. */
export function portNameError(name) {
  if (!NAME.test(name ?? '')) return 'A port name is a lower-case letter, then lower-case letters, digits and _';
  if (name === 'from' || name === 'remove') return `${name} is reserved in a slot’s interface`;
  return null;
}

/** The `slot` field of a `graph` instance (or of a schematic node built from one), or null. */
export function slotOf(instance) {
  const slot = instance?.slot;
  return slot && typeof slot === 'object' ? slot : null;
}

/** Whether a project path (`../studio`, `./bass`, `x.signal`) rather than a module reference. */
export function isProjectPath(load) {
  return typeof load === 'string' && (load.includes('/') || load.endsWith('.signal'));
}

/**
 * What a slot box says under its name (ADR 0037 §7): `{empty, subtitle,
 * template, project}`. An empty slot reads "empty", with its template's
 * name when it has one ("empty: lib.voice"); a loaded one the content's
 * name, a project's with a link marker (↗).
 */
export function slotLabel(slot) {
  const template = slot?.from ?? slot?.interface?.from ?? null;
  const load = typeof slot?.load === 'string' ? slot.load : null;
  if (!load) return { empty: true, subtitle: template ? `empty: ${template}` : 'empty', template, project: false };
  const project = Boolean(slot.project ?? isProjectPath(load));
  return { empty: false, subtitle: project ? `${load} ↗` : load, template, project };
}

/**
 * What a slot holds, as the sidebar's module field and the card's subtitle
 * say it (2026-10-29, the operator's review: an empty slot's was blank and
 * its category "project"): "empty" (with its template, as `slotLabel`), its
 * own inside ("fx_inside, its own inside"), a project ("../studio ↗, a
 * project"), else the content it loads.
 */
export function slotContent(slot) {
  const label = slotLabel(slot);
  if (label.empty) return label.subtitle;
  if (insideOf(slot)?.own) return `${slot.load}, its own inside`;
  return label.project ? `${label.subtitle}, a project` : slot.load;
}

/** What a slot's inside is, as the sidebar says it: its own or read-only (`count` instances in it), or why it does not open. */
export function slotInsideText(slot, count) {
  const inside = insideOf(slot);
  const instances = `${count} instance${count === 1 ? '' : 's'}`;
  if (inside?.level && inside.own) return `its own, ${instances}`;
  if (inside?.level) return `read-only, ${instances} (the first edit makes its own copy)`;
  return insideReason(slot) ?? 'none';
}

/** Why slot port `port` is unmatched (the reason a hollow pin's tooltip gives), or null when it is matched or the slot is empty. */
export function unmatchedReason(slot, port) {
  if (!slot || !slot.load) return null;
  const row = (slot.unmatched ?? []).find((u) => u.port === port);
  return row ? (row.reason ?? 'unmatched') : null;
}

/** The interface templates a slot may start from: `graph.interfaces` (the library's, then the project's), else the library's names. */
export function templates(graph) {
  const listed = Array.isArray(graph?.interfaces) ? graph.interfaces : null;
  if (!listed) return LIBRARY_TEMPLATES.map((name) => ({ name, library: true, description: null, ports: [] }));
  return listed.map((t) => ({ name: t.name, library: Boolean(t.library), description: t.description ?? null, ports: t.ports ?? [], base: t.base ?? null }));
}

/** The Add slot dialog's template choices: its own ports (none), then each template. `[{value, label, title}]`. */
export function templateChoices(graph) {
  return [
    { value: '', label: 'none: its own ports', title: 'An interface of its own, to which ports are added (slot = {})' },
    ...templates(graph).map((t) => ({ value: t.name, label: t.library ? t.name : `${t.name} (interfaces/${t.name}.toml)`, title: t.description ?? t.name })),
  ];
}

/** `slot.add`: slot `name` with `template` (a name, or '' for its own empty interface) and `load`, at `x`, `y`. */
export function addSlotOp(name, template = '', { x, y } = {}, load = null) {
  const op = { op: 'slot.add', name };
  if (template) op.slot = template;
  if (load) op.load = load;
  if (Number.isFinite(x) && Number.isFinite(y)) Object.assign(op, { x: Math.round(x), y: Math.round(y) });
  return op;
}

/** `shape` as the editor shows it: `signal`, `events` or `vector`, and a vector's width. */
function shapeOf(shape) {
  if (shape && typeof shape === 'object' && Number.isFinite(shape.vector)) return { shape: 'vector', width: shape.vector };
  return { shape: shape === 'events' ? 'events' : 'signal', width: null };
}

/**
 * The interface editor's rows of a slot: one per port and parameter, in the
 * interface's order, `{original, name, role, shape, width, rate, kind,
 * unit, range, group}` (`original`: the name it has in the file; null for
 * a row added in the editor).
 */
export function interfaceRows(slot) {
  return (slot?.interface?.ports ?? []).map((p) => {
    const { shape, width } = shapeOf(p.shape);
    return {
      original: p.name,
      name: p.name,
      role: p.role,
      shape,
      width,
      rate: p.rate ?? 1,
      kind: p.kind ?? 'generic',
      unit: p.unit ?? '',
      range: Array.isArray(p.range) ? [p.range[0], p.range[1]] : p.range && typeof p.range === 'object' ? [p.range.min, p.range.max] : null,
      group: p.group ?? '',
    };
  });
}

/** A new row of the interface editor for `role`, named `name`. */
export function newRow(role, name) {
  return { original: null, name, role, shape: 'signal', width: null, rate: 1, kind: role === 'parameter' ? 'generic' : 'audio', unit: '', range: null, group: '' };
}

/** A free name for a new port, `prefix` then the lowest free number (`in_1`, `in_2`). */
export function freeName(rows, prefix) {
  const taken = new Set(rows.map((r) => r.name));
  if (!taken.has(prefix)) return prefix;
  for (let n = 2; ; n++) if (!taken.has(`${prefix}_${n}`)) return `${prefix}_${n}`;
}

/** A row as `slot.interface` names a port (`PortJson`): its role and only the fields that differ from the defaults. */
export function portJson(row) {
  const json = { name: row.name, role: row.role };
  if (row.role !== 'parameter') {
    if (row.shape === 'events') json.shape = 'events';
    else if (row.shape === 'vector') json.shape = { vector: Math.max(1, Math.round(Number(row.width) || 1)) };
    const rate = Math.round(Number(row.rate) || 1);
    if (rate !== 1) json.rate = rate;
    if (row.kind && row.kind !== 'generic') json.kind = row.kind;
    if (Array.isArray(row.range) && row.range.every(Number.isFinite)) json.range = [row.range[0], row.range[1]];
  }
  if (row.unit) json.unit = row.unit;
  if (row.group) json.group = row.group;
  return json;
}

/** Whether two rows declare the same port (role and every field). */
function sameDeclaration(a, b) {
  return JSON.stringify({ ...portJson(a), name: '' }) === JSON.stringify({ ...portJson(b), name: '' });
}

/**
 * What the editor's rows mean against the slot's (`before`, from
 * `interfaceRows`): `{op, problems}`, the one `slot.interface` op (null
 * when nothing changed or a row is wrong) and what is wrong (a bad or
 * repeated name, a vector without a width). Rows whose `original` is gone
 * are removed; a row whose name changed is renamed (its wires, `map` keys
 * and panel bindings follow, server side); a new row is added; a row whose
 * fields changed is retyped. `removeWires`: the wires on removed ports go too.
 */
export function interfaceOp(slotName, before, after, { removeWires = false } = {}) {
  const problems = [];
  const seen = new Set();
  for (const row of after) {
    const error = portNameError(row.name);
    if (error) problems.push(`${row.name || '(no name)'}: ${error}`);
    else if (seen.has(row.name)) problems.push(`${row.name} is named twice: inputs, outputs and parameters share one namespace`);
    seen.add(row.name);
    if (!ROLES.includes(row.role)) problems.push(`${row.name}: no role`);
    if (row.role !== 'parameter' && row.shape === 'vector' && !(Number(row.width) >= 1)) problems.push(`${row.name}: a vector needs a width of at least 1`);
    if (row.role !== 'parameter' && !(Number(row.rate) >= 1)) problems.push(`${row.name}: a rate divisor is at least 1`);
  }
  if (problems.length) return { op: null, problems };
  const kept = new Set(after.filter((r) => r.original).map((r) => r.original));
  const op = { op: 'slot.interface', slot: slotName };
  const remove = before.filter((r) => !kept.has(r.original)).map((r) => r.original);
  const rename = {};
  const retype = [];
  const add = [];
  for (const row of after) {
    if (!row.original) {
      add.push(portJson(row));
      continue;
    }
    const was = before.find((r) => r.original === row.original);
    if (row.name !== row.original) rename[row.original] = row.name;
    if (was && !sameDeclaration(was, row)) retype.push({ ...portJson(row), name: row.name });
  }
  if (remove.length) op.remove = remove;
  if (Object.keys(rename).length) op.rename = rename;
  if (add.length) op.add = add;
  if (retype.length) op.retype = retype;
  if (remove.length && removeWires) op.removeWires = true;
  const changes = remove.length + Object.keys(rename).length + add.length + retype.length;
  return { op: changes ? op : null, problems };
}

/**
 * The ports and parameters of a content (a `modules` entry, a
 * `slot.projects` project): `[{name, role, shape, kind, rate}]`.
 */
export function contentPorts(entry) {
  if (!entry) return [];
  if (Array.isArray(entry.contentPorts)) return entry.contentPorts;
  const ports = (entry.ports ?? []).map((p) => ({
    name: p.name,
    role: p.role ?? (p.direction === 'out' ? 'output' : 'input'),
    shape: shapeOf(p.shape).shape,
    kind: p.kind ?? 'generic',
    rate: p.rate ?? 1,
  }));
  const parameters = (entry.parameters ?? []).map((p) => ({ name: p.name, role: 'parameter', shape: 'signal', kind: p.kind ?? 'generic', rate: p.rate ?? 1 }));
  return [...ports, ...parameters];
}

/**
 * How a content matches an interface by name (ADR 0037 §4, the Load
 * dialog): `{matched, total, unmatched}`: how many of the interface's
 * entries find a content entry of their name and role (`map` aside), of how
 * many, and the content's ports and parameters the interface lacks (which
 * the dialog offers to add to the slot).
 */
export function contentMatch(entry, slot) {
  const content = contentPorts(entry);
  const ports = slot?.interface?.ports ?? [];
  const mapped = slot?.map ?? {};
  let matched = 0;
  for (const p of ports) {
    const target = Object.hasOwn(mapped, p.name) ? mapped[p.name] : p.name;
    if (target !== false && content.some((c) => c.name === target && c.role === p.role)) matched++;
  }
  const declared = new Set(ports.map((p) => p.name));
  const targets = new Set(Object.values(mapped).filter((v) => typeof v === 'string'));
  const unmatched = content.filter((c) => !declared.has(c.name) && !targets.has(c.name));
  return { matched, total: ports.length, unmatched };
}

/** The content's unmatched entries as `slot.interface` adds them (`PortJson`). */
export function addedPorts(unmatched) {
  return unmatched.map((c) => portJson({ name: c.name, role: c.role, shape: c.shape, width: null, rate: c.rate ?? 1, kind: c.kind ?? 'generic', unit: '', range: null, group: '' }));
}

/**
 * The ops of a Load (or Replace) into slot `slotName` of `load`: the
 * content's unmatched ports added to the interface first when `add` names
 * them, then `slot.load` (a Replace keeps the `map` and `set` entries the
 * new content accepts, server side).
 */
export function loadOps(slotName, load, { add = [] } = {}) {
  const ops = [];
  if (add.length) ops.push({ op: 'slot.interface', slot: slotName, add: addedPorts(add) });
  ops.push({ op: 'slot.load', slot: slotName, load });
  return ops;
}

/** `slot.unload`. */
export function unloadOp(slotName) {
  return { op: 'slot.unload', slot: slotName };
}

/**
 * The map editor's rows (ADR 0037 §4): one per interface port and
 * parameter, `{port, role, value, options}`: `value` the `map` entry
 * (`''`: matched by name, `false`: deliberately unmatched, else a content
 * name), `options` the content's names of that role.
 */
export function mapRows(slot, content) {
  const map = slot?.map ?? {};
  const entries = contentPorts(content);
  return (slot?.interface?.ports ?? []).map((p) => ({
    port: p.name,
    role: p.role,
    value: Object.hasOwn(map, p.name) ? map[p.name] : '',
    options: entries.filter((c) => c.role === p.role).map((c) => c.name),
  }));
}

/** `slot.map` of the map editor's rows: by-name rows are left out; `{}` removes the slot's `map`. */
export function mapOp(slotName, rows) {
  const map = {};
  for (const row of rows) if (row.value !== '' && row.value !== null && row.value !== undefined) map[row.port] = row.value;
  return { op: 'slot.map', slot: slotName, map };
}

/** `slot.interface` "Detach from template": the template's ports written into the slot. */
export function detachOp(slotName) {
  return { op: 'slot.interface', slot: slotName, detach: true };
}

/** `slot.interface` that sets the slot's interface to template `name` alone (after "Save interface as template…"). */
export function templateOp(slotName, name) {
  return { op: 'slot.interface', slot: slotName, template: name };
}

/** A TOML basic string of `text` (JSON's escapes are TOML's). */
function quoted(text) {
  return JSON.stringify(String(text));
}

/** A port's declaration as an interface file writes it (`project-format.md` §14). */
function declaration(p) {
  const fields = [];
  const { shape, width } = shapeOf(p.shape);
  if (p.role === 'parameter') {
    if (p.unit) fields.push(`unit = ${quoted(p.unit)}`);
    if (p.group) fields.push(`group = ${quoted(p.group)}`);
    return fields.length ? `{ ${fields.join(', ')} }` : '{}';
  }
  if (shape === 'events') fields.push('shape = "events"');
  else if (shape === 'vector') fields.push(`shape = { vector = ${width} }`);
  if ((p.rate ?? 1) !== 1) fields.push(`rate = ${p.rate}`);
  if (p.kind && p.kind !== 'generic') fields.push(`kind = ${quoted(p.kind)}`);
  if (p.unit) fields.push(`unit = ${quoted(p.unit)}`);
  const range = Array.isArray(p.range) ? p.range : p.range && typeof p.range === 'object' ? [p.range.min, p.range.max] : null;
  if (range && range.every(Number.isFinite)) fields.push(`range = [${range[0]}, ${range[1]}]`);
  if (p.group) fields.push(`group = ${quoted(p.group)}`);
  return fields.length ? `{ ${fields.join(', ')} }` : '{}';
}

/** Why `name` cannot name an interface file, or null. */
export function templateNameError(name) {
  return NAME.test(name ?? '') ? null : 'A template’s name is a lower-case letter, then lower-case letters, digits and _';
}

/**
 * `interfaces/<name>.toml`'s text (ADR 0037 §3) for `ports` (`[{name,
 * role, shape, rate, kind, unit, range, group}]`, an interface's or an
 * instance's): `[interface]`, then `[inputs]`, `[outputs]`, `[parameters]`,
 * each in the order given.
 */
export function templateText(name, ports, description = null) {
  const lines = ['schema_version = 1', '', '[interface]', `name = ${quoted(name)}`];
  if (description) lines.push(`description = ${quoted(description)}`);
  for (const [role, table] of [['input', 'inputs'], ['output', 'outputs'], ['parameter', 'parameters']]) {
    const entries = ports.filter((p) => p.role === role);
    if (!entries.length) continue;
    lines.push('', `[${table}]`);
    for (const p of entries) lines.push(`${p.name} = ${declaration(p)}`);
  }
  return `${lines.join('\n')}\n`;
}

/**
 * "Make interface from this instance" (ADR 0037 §3): an instance's
 * resolved ports and parameters as `graph` gives them (a slot's are its
 * interface's), as interface entries.
 */
export function instancePorts(instance) {
  const ports = (instance?.ports ?? []).map((p) => ({
    name: p.name,
    role: p.direction === 'out' ? 'output' : 'input',
    shape: p.shape ?? 'signal',
    rate: p.rate ?? 1,
    kind: p.kind ?? 'generic',
    unit: p.unit ?? null,
    range: null,
    group: p.group ?? null,
  }));
  const parameters = (instance?.parameters ?? []).map((p) => ({ name: p.name, role: 'parameter', shape: 'signal', rate: 1, kind: 'generic', unit: p.unit ?? null, range: null, group: p.group ?? null }));
  return [...ports, ...parameters].filter((p) => !portNameError(p.name));
}

/**
 * The Load dialog's entries of a `slot.projects` reply: one per project
 * that loads, as the module browser lists a type (`source: 'folders'`,
 * `module`: the `load` it writes), its offered ports as `contentPorts`.
 */
export function projectEntries(reply) {
  return (reply?.projects ?? [])
    .filter((p) => !p.error && typeof p.load === 'string')
    .map((p) => ({
      module: p.load,
      composite: true,
      library: false,
      project: true,
      source: 'folders',
      category: null,
      group: null,
      description: p.name ? `The project ${p.name}, loaded as a slot's content (ADR 0035): its exports, or its sinks' channels l and r.` : null,
      referenceFor: null,
      // As a `modules` entry gives them, for the detail pane.
      ports: (p.ports ?? []).filter((c) => c.role !== 'parameter').map((c) => ({ name: c.name, direction: c.role === 'output' ? 'out' : 'in', shape: c.shape, kind: c.kind ?? null, rate: c.rate ?? 1, unit: null, range: null })),
      parameters: (p.ports ?? []).filter((c) => c.role === 'parameter').map((c) => ({ name: c.name, kind: c.kind ?? null, rate: 1 })),
      contentPorts: (p.ports ?? []).map((c) => ({ name: c.name, role: c.role, shape: shapeOf(c.shape).shape, kind: c.kind ?? 'generic', rate: c.rate ?? 1 })),
      inside: null,
    }));
}

/** The slots of a `describe` (`content.slots`) at the top level, by name: `{name: slot}`. */
export function topSlots(description) {
  const found = {};
  for (const slot of description?.content?.slots ?? []) {
    const name = /^\/([a-z][a-z0-9_]*)$/.exec(slot.slot ?? '')?.[1];
    if (name) found[name] = slot;
  }
  return found;
}

/**
 * The slot card's model (ADR 0037 §7) of top-level slot `name` in a
 * `describe`: `{name, address, interface, content, empty, project, ports,
 * parameters}`, `ports` each `{name, role, kind, shape, matched, reason,
 * address}` (`address` the slot port a meter binds, the server translating
 * it through the match), `parameters` the interface parameters each with
 * its control (`describe.controls`, null when unmatched or empty).
 */
export function slotCard(description, name) {
  const slot = topSlots(description)[name];
  if (!slot) return null;
  const template = slot.interface?.from ?? null;
  const load = slot.load ?? null;
  const rows = slot.matches ?? [];
  const reasonOf = (port) => {
    if (!load) return 'the slot is empty';
    const row = rows.find((m) => m.port === port);
    return row && !row.content ? (row.reason ?? 'unmatched') : null;
  };
  const controls = description?.controls ?? [];
  const ports = (slot.interface?.ports ?? []).map((p) => ({
    name: p.name,
    role: p.role,
    kind: p.kind ?? 'generic',
    shape: shapeOf(p.shape).shape,
    matched: reasonOf(p.name) === null,
    reason: reasonOf(p.name),
    address: `/${name}.${p.name}`,
  }));
  return {
    name,
    address: `/${name}`,
    interface: template ?? 'slot',
    content: load,
    empty: !load,
    project: isProjectPath(load),
    ports: ports.filter((p) => p.role !== 'parameter'),
    parameters: ports
      .filter((p) => p.role === 'parameter')
      .map((p) => ({ ...p, control: controls.find((c) => c.address === p.address) ?? null })),
    unheard: slot.unheard ?? [],
  };
}

/** A card's header text: "DECK_A: lib.deck ◂ ../studio-transport ↗", or "… ◂ empty". */
export function cardTitle(card) {
  if (!card) return '';
  const content = card.empty ? 'empty' : `${card.content}${card.project ? ' ↗' : ''}`;
  return `${card.name.toUpperCase()}: ${card.interface} ◂ ${content}`;
}

/**
 * What a drop onto a slot card loads (ADR 0037 §7): a module reference or a
 * project path given as text (a Load list's row, a folder's path), trimmed,
 * or null when it is neither.
 */
export function dropLoad(text) {
  const value = String(text ?? '').trim().split(/\r?\n/)[0]?.trim() ?? '';
  if (!value || value.length > 1024) return null;
  if (isProjectPath(value)) return value;
  return /^[a-z][a-z0-9_]*(\.[a-z0-9_]+)*$/.test(value) ? value : null;
}

// Editable insides (ADR 0037, 2026-10-27; the owner: "step into it with a
// double-click and edit its inside", every slot's inside its own). A slot
// opens like a composite into its inside: its own composite (`inside_of`
// names the slot), or, until it has one, a read-only inside (`graph.insides`,
// keyed `/slot` or `<composite>/slot`: what the first edit makes). The
// first edit there writes `slot.inside` at the slot's level, then the edit
// at the new composite's level. Its frame is the slot's interface, wired
// by `export.connect` and `export.disconnect`.

/** A slot's inside as `graph` gives it (`slot.inside`): `{level, own, make?, name?, reason?}`, or null (an older server). */
export function insideOf(slot) {
  const inside = slot?.inside;
  return inside && typeof inside === 'object' ? inside : null;
}

/** Whether `level` is a slot's read-only inside (`/fx`, `rack/fx`): a composite's name never holds a `/`. */
export function isPreviewLevel(level) {
  return typeof level === 'string' && level.includes('/');
}

/** The slot whose inside key `key` is (`/fx`, `rack/fx`): `{parent, slot}`, its level (`""`: the top) and name. */
export function previewSlot(key) {
  const at = String(key).lastIndexOf('/');
  return { parent: String(key).slice(0, Math.max(0, at)), slot: String(key).slice(at + 1) };
}

/** A level of `graph` by its key: `""` the top level, a composite's name, or a slot's read-only inside. */
export function levelData(graph, level) {
  if (level === '') return graph?.top ?? null;
  return graph?.composites?.[level] ?? graph?.insides?.[level] ?? null;
}

/** Whether `levelGraph` is a slot's inside, own or read-only: its frame is the slot's interface (its `slot`, the slot's key). */
export function isSlotInside(levelGraph) {
  return typeof levelGraph?.slot === 'string';
}

/** The slot instance whose inside `levelGraph` is, in `graph`: `{parent, name, instance, inside}`, or null. */
export function insideSlot(graph, levelGraph) {
  if (!isSlotInside(levelGraph)) return null;
  const { parent, slot } = previewSlot(levelGraph.slot);
  const instance = (levelData(graph, parent)?.instances ?? []).find((i) => i.name === slot) ?? null;
  return instance ? { parent, name: slot, instance, inside: insideOf(slotOf(instance)) } : null;
}

/**
 * The Add module browser's or the Load dialog's entries without the slots'
 * own insides that are not `owner`'s (`inside_of`: only its slot loads one;
 * `owner` null: none, as Add module adds no slot).
 */
export function loadable(entries, owner = null) {
  return (entries ?? []).filter((e) => !e?.insideOf || e.insideOf === owner);
}

/** The `inside_of` of slot `name` at `level` (`""`: the top level): `fx`, `rack/fx`. */
export function insideOwner(level, name) {
  return level ? `${level}/${name}` : name;
}

/** `slot.inside`: slot `slotName`'s own inside made (`name`: the composite's, else the server's default). */
export function insideOp(slotName, name = null) {
  const op = { op: 'slot.inside', slot: slotName };
  if (name) op.name = name;
  return op;
}

/** `export.connect` (or, `connect` false, `export.disconnect`) of export `name` of `role` and inner port `port` (`inst.port`). */
export function exportOp(connect, role, name, port) {
  return { op: connect ? 'export.connect' : 'export.disconnect', role, name, port };
}

/**
 * `ops` at a slot's inside `levelGraph` as written there: each export that
 * reaches an instance the ops remove is disconnected first (an export
 * naming a removed instance is refused, `in-use`). Other levels' ops as
 * they are.
 */
export function insideOps(levelGraph, ops) {
  if (!isSlotInside(levelGraph)) return ops;
  const removed = new Set(ops.filter((o) => o.op === 'instance.remove').map((o) => o.name));
  if (!removed.size) return ops;
  const reaches = (port) => removed.has(String(port ?? '').split('.')[0]);
  const before = [];
  const seen = new Set();
  const disconnect = (role, name, port) => {
    const key = `${role} ${name} ${port}`;
    if (seen.has(key)) return;
    seen.add(key);
    before.push(exportOp(false, role, name, port));
  };
  for (const [list, role] of [[levelGraph.inputs, 'input'], [levelGraph.parameters, 'parameter']]) {
    for (const e of list ?? []) for (const target of e.to ?? []) if (reaches(target)) disconnect(role, e.export ?? e.name, target);
  }
  for (const e of levelGraph.outputs ?? []) if (e.from && reaches(e.from)) disconnect('output', e.export ?? e.name, e.from);
  return [...before, ...ops];
}

/**
 * Where a frame pin of a slot's inside (`pin`: a layout's frame pin, with
 * its `role`) may be wired in `levelGraph`: an input's to the inner inputs
 * of its shape, a parameter's to the inner parameters, neither one it
 * reaches already. `[{instance, port, parameter}]`.
 */
export function frameTargets(levelGraph, pin) {
  const entry = [...(levelGraph?.inputs ?? []), ...(levelGraph?.parameters ?? [])].find((e) => e.name === pin.name);
  const reached = new Set(entry?.to ?? []);
  const targets = [];
  for (const instance of levelGraph?.instances ?? []) {
    if (pin.role === 'parameter') {
      for (const p of instance.parameters ?? []) if (!reached.has(`${instance.name}.${p.name}`)) targets.push({ instance: instance.name, port: p.name, parameter: true });
      continue;
    }
    for (const p of instance.ports ?? []) {
      if (p.direction !== 'in' || shapeOf(p.shape).shape !== (pin.shape ?? 'signal') || reached.has(`${instance.name}.${p.name}`)) continue;
      targets.push({ instance: instance.name, port: p.name, parameter: false });
    }
  }
  return targets;
}

/** The output pins of a slot's inside's frame an inner output of `shape` may be wired to: those of its shape not wired yet. */
export function frameOutputs(levelGraph, shape) {
  return (levelGraph?.outputs ?? []).filter((e) => !e.from && shapeOf(e.shape).shape === (shape ?? 'signal')).map((e) => e.name);
}

/**
 * What the schematic says over a slot's inside shown as the innermost
 * level (`level`), or null: `{own, text}`. Its own: what file it is, and
 * that an edit changes this slot only. Not its own yet: that it is shown
 * read-only and the first edit makes the slot's own copy (an empty slot's:
 * its own inside), naming the file; the content it copies stays as it is.
 */
export function insideBanner(graph, level, path = null) {
  const found = insideSlot(graph, levelData(graph, level));
  if (!found) return null;
  const { name, instance, inside } = found;
  const file = inside?.name ? `composites/${inside.name}.toml` : 'a composite of its own';
  if (inside?.own) return { own: true, text: `${name}’s own inside (${file}): edits here change this slot only.` };
  const load = slotOf(instance)?.load ?? null;
  if (!load) return { own: false, text: `${name} is empty. Editing makes its own inside (${file}): add modules here and wire them to the frame’s pins.` };
  const what = inside?.make === 'wrap' ? `the module ${load}` : `${load}, shared`;
  // A wrapped module moves one level down (`/fx` → `/fx/gain`): live, it starts again once.
  const inner = levelData(graph, level)?.instances?.[0]?.name;
  // `path`: the slot instance's address shown (`/rack1/fx`), else the top level's `/fx`.
  const at = path ?? (found.parent ? null : `/${name}`);
  const restart = inside?.make === 'wrap' ? ` Editing restarts this module’s sound once (it moves into its own inside${inner && at ? `, as ${at}/${inner}` : ''}).` : '';
  return { own: false, text: `Read-only: ${name} holds ${what}. Editing makes this slot’s own copy (${file}); ${load} stays as it is.${restart}` };
}

/** Why a slot box does not open (a project as its content), or null when it does. */
export function insideReason(slot) {
  const inside = insideOf(slot);
  if (!inside || inside.level) return null;
  return inside.reason ?? 'Its inside is not shown';
}

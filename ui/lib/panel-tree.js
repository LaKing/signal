// The panel editor's text (docs/ui-controls.md "Editing a panel", ADR 0026
// §5): the content of a <signal-panel> element as a tree, read from text
// and written back canonically, within the whitelist of
// ui/lib/panel-whitelist.json (which the server checks again,
// crates/signal-ui/src/panel_write.rs). Pure, no DOM.
//
// The tree: `{kind: 'panel', next, children: [cards…]}`; containers
// `{id, kind: 'cards' | 'card' | 'row' | 'group', classes, children}`
// (`classes`: the tokens after the required one, a card's `wide` and
// `scope` among them); headings and descriptions `{id, kind: 'h1' | 'h2' |
// 'p', classes, text}`; controls `{id, kind: 'control', tag, attributes}`.
// Ids are given when a tree is read or a node added; they are never written.

/** A container's element and required class, by kind. */
export const CONTAINERS = Object.freeze({
  cards: { tag: 'div', name: 'signal-cards', key: 'div.signal-cards' },
  card: { tag: 'section', name: 'signal-card', key: 'section.signal-card' },
  row: { tag: 'div', name: 'row', key: 'div.row' },
  group: { tag: 'div', name: 'signal-group', key: 'div.signal-group' },
});

const TEXTS = new Set(['h1', 'h2', 'p']);
const KIND_OF_KEY = Object.fromEntries(Object.entries(CONTAINERS).map(([kind, c]) => [c.key, kind]));
const REFERENCES = { amp: '&', lt: '<', gt: '>', quot: '"' };

/** `{line, column}` (1-based) of `offset` in `text`. */
function position(text, offset) {
  const before = text.slice(0, offset);
  const line = before.split('\n').length;
  return { line, column: offset - before.lastIndexOf('\n') };
}

class Refused extends Error {}

/** `text` with its character references decoded; only `&amp; &lt; &gt; &quot; &#N; &#xN;`. */
function decode(text, fail) {
  return text.replace(/&([^;\s&<]*);?/g, (whole, body) => {
    if (whole.endsWith(';')) {
      if (body in REFERENCES) return REFERENCES[body];
      const numeric = /^#(?:x([0-9a-fA-F]{1,6})|([0-9]{1,7}))$/.exec(body);
      if (numeric) {
        const code = numeric[1] !== undefined ? parseInt(numeric[1], 16) : Number(numeric[2]);
        if (code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff)) return String.fromCodePoint(code);
      }
    }
    return fail(`the character reference ${whole} is not one of &amp; &lt; &gt; &quot; &#…;`);
  });
}

/** The whitelist's key of an element: `div.row`, `h2`, `control`, or null. */
function keyOf(tag, classes, whitelist) {
  if (whitelist.controls[tag]) return 'control';
  if (TEXTS.has(tag)) return tag;
  const keys = Object.values(CONTAINERS).filter((c) => c.tag === tag && classes.includes(c.name));
  return keys.length === 1 ? keys[0].key : null;
}

/**
 * The content of a <signal-panel> as a tree, or `{error}` naming what is
 * refused and where (`line L, column C: …`). `mode: 'submit'` (what the
 * page sends, as the server checks it) refuses comments; `mode: 'read'` (a
 * panel file's existing content) skips them, so the first write drops them.
 * Anything else outside `whitelist` is refused in both.
 */
export function parseContent(html, whitelist, { mode = 'submit' } = {}) {
  const text = String(html ?? '');
  let at = 0;
  let next = 1;
  const root = { kind: 'panel', children: [] };
  const stack = [{ node: root, key: 'panel', tag: null }];
  const fail = (message, offset = at) => {
    const { line, column } = position(text, offset);
    throw new Refused(`line ${line}, column ${column}: ${message}`);
  };
  const allowed = (parent, key) =>
    parent === 'panel' ? key === 'div.signal-cards' : (whitelist.containers[parent]?.children ?? []).includes(key);
  try {
    while (at < text.length) {
      const lt = text.indexOf('<', at);
      const chunk = text.slice(at, lt === -1 ? text.length : lt);
      if (chunk) {
        const top = stack[stack.length - 1];
        const decoded = decode(chunk, (m) => fail(m));
        if (TEXTS.has(top.key)) top.node.text += decoded;
        else if (decoded.trim()) fail(`text "${decoded.trim().slice(0, 20)}" is outside a heading or description`);
      }
      if (lt === -1) break;
      at = lt;
      if (text.startsWith('<!--', at)) {
        const end = text.indexOf('-->', at + 4);
        if (mode !== 'read') fail('a comment is not written by the editor');
        if (end === -1) fail('an unterminated comment');
        at = end + 3;
        continue;
      }
      const end = /^<\/([a-zA-Z][a-zA-Z0-9-]*)\s*>/.exec(text.slice(at));
      if (end) {
        const tag = end[1].toLowerCase();
        const top = stack[stack.length - 1];
        if (stack.length === 1 || top.tag !== tag) fail(`</${tag}> does not close the open element`);
        stack.pop();
        at += end[0].length;
        continue;
      }
      const start = /^<([a-zA-Z][a-zA-Z0-9-]*)/.exec(text.slice(at));
      if (!start) fail(`"${text.slice(at, at + 2)}" is not a tag the editor writes`);
      const begin = at;
      const tag = start[1].toLowerCase();
      at += start[0].length;
      const attributes = [];
      for (;;) {
        const space = /^\s*/.exec(text.slice(at))[0];
        at += space.length;
        if (text[at] === '>') {
          at += 1;
          break;
        }
        if (at >= text.length) fail(`<${tag}> is not closed`, begin);
        if (!space) fail(`<${tag}>: attributes are separated by spaces`);
        const named = /^[^\s"'>/=<]+/.exec(text.slice(at));
        if (!named) fail(`<${tag}>: "${text[at]}" where an attribute belongs`);
        const name = named[0].toLowerCase();
        const offset = at;
        at += named[0].length;
        let value = null;
        const eq = /^\s*=\s*/.exec(text.slice(at));
        if (eq) {
          at += eq[0].length;
          if (text[at] !== '"') fail(`<${tag} ${name}>: the value is not in double quotes`);
          const close = text.indexOf('"', at + 1);
          if (close === -1) fail(`<${tag} ${name}>: the value is not closed`);
          value = decode(text.slice(at + 1, close), (m) => fail(m));
          at = close + 1;
        }
        attributes.push({ name, value, offset });
      }
      const classAttribute = attributes.find((a) => a.name === 'class');
      const classes = (classAttribute?.value ?? '').split(/\s+/).filter(Boolean);
      const key = keyOf(tag, classes, whitelist);
      if (!key) fail(`<${tag}${classes.length ? ` class="${classes.join(' ')}"` : ''}> is not an element the editor writes`, begin);
      const parent = stack[stack.length - 1];
      if (!allowed(parent.key, key)) fail(`<${tag}> is not allowed in ${parent.key === 'panel' ? '<signal-panel>' : parent.key}`, begin);
      const permitted = key === 'control' ? whitelist.controls[tag] : whitelist.containers[key].attributes;
      const seen = new Set();
      for (const a of attributes) {
        if (seen.has(a.name)) fail(`<${tag}>: ${a.name} is given twice`, a.offset);
        seen.add(a.name);
        if (!permitted.includes(a.name)) {
          const why = a.name === 'style' ? 'style is not allowed' : a.name.startsWith('on') ? 'event handlers are not allowed' : 'not an attribute it takes';
          fail(`<${tag} ${a.name}>: ${why}`, a.offset);
        }
        if (a.value === null && !(whitelist.boolean ?? []).includes(a.name)) fail(`<${tag} ${a.name}>: needs a value`, a.offset);
      }
      const token = new RegExp(whitelist.classToken);
      const tokens = new Set();
      for (const c of classes) {
        if (!token.test(c)) fail(`<${tag}>: the class "${c}" is not a lowercase name`, classAttribute.offset);
        if (tokens.has(c)) fail(`<${tag}>: the class "${c}" is given twice`, classAttribute.offset);
        tokens.add(c);
      }
      const id = `n${next++}`;
      let node;
      if (key === 'control') {
        const values = {};
        for (const a of attributes) values[a.name] = a.value ?? '';
        if (values.class !== undefined) values.class = classes.join(' ');
        if (values.class === '') delete values.class;
        node = { id, kind: 'control', tag, attributes: values };
      } else if (TEXTS.has(key)) {
        node = { id, kind: key, classes, text: '' };
      } else {
        const kind = KIND_OF_KEY[key];
        node = { id, kind, classes: classes.filter((c) => c !== CONTAINERS[kind].name), children: [] };
      }
      parent.node.children.push(node);
      stack.push({ node, key, tag });
    }
    if (stack.length > 1) fail(`<${stack[stack.length - 1].tag}> is not closed`);
  } catch (error) {
    if (error instanceof Refused) return { error: error.message };
    throw error;
  }
  // Text is kept as written, its surrounding space trimmed (a heading's
  // text is one line in the canonical form).
  const trim = (node) => {
    if (TEXTS.has(node.kind)) node.text = node.text.replace(/\s+/g, ' ').trim();
    for (const child of node.children ?? []) trim(child);
  };
  trim(root);
  root.next = next;
  return { tree: root };
}

const escapeText = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const escapeValue = (s) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;');

/** A container's or text's class attribute, canonical: the required name, then `wide`, `scope`, then the rest. */
function classText(required, classes) {
  const rest = classes.filter((c) => c !== 'wide' && c !== 'scope');
  const ordered = [required, ...['wide', 'scope'].filter((c) => classes.includes(c)), ...rest].filter(Boolean);
  return ordered.length ? ` class="${escapeValue(ordered.join(' '))}"` : '';
}

/**
 * The canonical text of a tree: the content of its <signal-panel>, a line
 * per element, two spaces of indentation per level (from one), attributes in
 * the whitelist's fixed `order`, boolean ones bare; the same tree always
 * gives the same text.
 */
export function serialize(tree, whitelist) {
  const order = whitelist.order;
  const boolean = new Set(whitelist.boolean ?? []);
  const rank = (name) => {
    const i = order.indexOf(name);
    return i === -1 ? order.length : i;
  };
  const lines = [];
  const write = (node, depth) => {
    const pad = '  '.repeat(depth);
    if (node.kind === 'control') {
      const names = Object.keys(node.attributes).sort((a, b) => rank(a) - rank(b) || (a < b ? -1 : 1));
      const attributes = names
        .map((n) => (boolean.has(n) ? ` ${n}` : ` ${n}="${escapeValue(String(node.attributes[n]))}"`))
        .join('');
      lines.push(`${pad}<${node.tag}${attributes}></${node.tag}>`);
    } else if (TEXTS.has(node.kind)) {
      lines.push(`${pad}<${node.kind}${classText(null, node.classes ?? [])}>${escapeText(node.text)}</${node.kind}>`);
    } else {
      const { tag, name } = CONTAINERS[node.kind];
      const open = `${pad}<${tag}${classText(name, node.classes ?? [])}>`;
      if (!node.children.length) {
        lines.push(`${open}</${tag}>`);
        return;
      }
      lines.push(open);
      for (const child of node.children) write(child, depth + 1);
      lines.push(`${pad}</${tag}>`);
    }
  };
  for (const child of tree.children) write(child, 1);
  return `\n${lines.map((l) => `${l}\n`).join('')}`;
}

/**
 * Whether a tree is within the whitelist, as the page checks it before
 * sending: its canonical text read back in submit mode. `{ok: true}` or
 * `{error}`.
 */
export function checkTree(tree, whitelist) {
  const parsed = parseContent(serialize(tree, whitelist), whitelist);
  return parsed.error ? { error: parsed.error } : { ok: true };
}

/** The tree without its ids (to compare trees). */
export function shape(tree) {
  return JSON.parse(JSON.stringify(tree, (key, value) => (key === 'id' || key === 'next' ? undefined : value)));
}

/**
 * The generated default panel (`defaultPanel` in ui/lib/panel.js) as a tree:
 * a card per instance (its name the heading, its module the description),
 * and a wide `output` card with the sink's meters in a group and the scope.
 */
export function generatedTree(spec) {
  let next = 1;
  const id = () => `n${next++}`;
  const control = (tag, attributes) => ({ id: id(), kind: 'control', tag, attributes });
  const card = (title, note, children, classes = []) => ({
    id: id(),
    kind: 'card',
    classes,
    children: [
      { id: id(), kind: 'h2', classes: [], text: title },
      ...(note ? [{ id: id(), kind: 'p', classes: [], text: note }] : []),
      { id: id(), kind: 'row', classes: [], children },
    ],
  });
  const cards = spec.groups.map((g) =>
    card(g.instance, g.module, g.controls.map((c) => control(c.element, { bind: c.bind, label: c.label, taper: c.taper }))),
  );
  const output = [
    ...(spec.meters.length
      ? [{ id: id(), kind: 'group', classes: [], children: spec.meters.map((m) => control('signal-meter', { bind: m.bind, label: m.label })) }]
      : []),
    ...(spec.scope ? [control('signal-scope', { bind: spec.scope.bind, label: spec.scope.label })] : []),
  ];
  if (output.length) cards.push(card('output', null, output, ['wide']));
  const grid = { id: id(), kind: 'cards', classes: [], children: cards };
  return { kind: 'panel', next, children: [grid] };
}

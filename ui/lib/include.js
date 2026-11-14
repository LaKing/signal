// ADR 0035 §10: an included project's panel in its host's (pure; no DOM).

/** The `panel.read` name of include `key`'s panel. */
export function includePanelFile(key) {
  return `include/${key}/main.html`;
}

/** The included project's title card: "DRUMS — studio-transport (linked)". */
export function includeTitle(key, include) {
  const name = include?.name ? ` — ${include.name}` : '';
  return `${String(key ?? '').toUpperCase()}${name} (linked)`;
}

/** The top-level includes of a `describe` (not a slot's load), whose panels the host's panel shows after its own. */
export function panelIncludes(description) {
  return (description?.content?.includes ?? []).filter((i) => !i.slot && /^[a-z][a-z0-9_]*$/.test(i.key ?? ''));
}

/** ADR 0035 §10: a pattern control's `pattern` in include `key`'s panel, naming the include's pattern (`include/<key>/<name>`, read-only in the host). */
export function includePattern(key, value) {
  if (value === undefined || value === null || value === '') return value;
  return `include/${key}/${value}`;
}


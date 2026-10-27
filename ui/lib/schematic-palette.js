// What a module type is, by its group (ADR 0027 §6: "the palette groups by
// `category` (and library, project)"; docs/ui-controls.md "Editing the
// patch"): the groups in their fixed order, the group of a `modules` entry,
// and what a type is (its description, a native primitive's formula, and
// its group) for the sidebar. The Add module browser builds its tree from
// these (ui/lib/module-browser.js). No DOM.

/**
 * The groups, in their order: the project's composites, the library's
 * (`lib.*`, ADR 0027 §4: `modules` lists them), then the std categories
 * (`signal_std::category`). A category not named here comes after them, in
 * name order, and an entry without one under `other`, last.
 */
export const GROUPS = Object.freeze(['project', 'library', 'core', 'generator', 'filter', 'envelope', 'delay', 'physical', 'event', 'visual', 'io']);

/** The group of entries that name none. */
export const OTHER = 'other';

/**
 * The group of the validation references (M2.10-4): library composites
 * that rebuild a native type from core primitives to be compared with it
 * (their `modules` entry names it as `reference_for`), not building blocks.
 * Listed last, after `other`, and collapsed until the viewer opens it.
 */
export const REFERENCE = 'reference';

/**
 * The group of a `modules` entry: a validation reference's `reference`
 * (it names a `reference_for`), a library composite's `library` (marked
 * `library`, or named `lib.*`), another composite's `project`, a
 * primitive's `category`, else `other`.
 */
export function groupOf(m) {
  if (typeof m?.reference_for === 'string' && m.reference_for) return REFERENCE;
  if (m?.library === true || /^lib\./.test(String(m?.module ?? ''))) return 'library';
  if (m?.composite) return 'project';
  return typeof m?.category === 'string' && m.category ? m.category : OTHER;
}

/** Sorts group names: `GROUPS` first in their order, then the others by name, `other`, then `reference` last. */
export function groupOrder(a, b) {
  const rank = (g) =>
    g === REFERENCE ? GROUPS.length + 2 : g === OTHER ? GROUPS.length + 1 : GROUPS.includes(g) ? GROUPS.indexOf(g) : GROUPS.length;
  return rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0);
}

/** The words of a search, lowercased. */
export function words(query) {
  return String(query ?? '').toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * What a type is, by module name, from a `modules` reply:
 * `Map(module → {module, description, group, composite, referenceFor})`,
 * the description a native primitive's rustdoc summary with its formula
 * (`"Adds its two signal inputs: out = a + b."`), a composite's its file's
 * `description` (null without one); `referenceFor` the native type a
 * validation reference rebuilds, else null.
 */
export function moduleInfo(modules) {
  const info = new Map();
  for (const m of [...(modules?.primitives ?? []), ...(modules?.composites ?? [])]) {
    info.set(m.module, {
      module: m.module,
      description: m.description ?? null,
      group: groupOf(m),
      composite: Boolean(m.composite),
      referenceFor: typeof m.reference_for === 'string' && m.reference_for ? m.reference_for : null,
    });
  }
  return info;
}

/** Which project a `graph` is (`graph.projectId`, else its folder and name), for what is known of its types. */
export function projectKey(graph) {
  return graph ? (graph.projectId ?? `${graph.folder ?? ''}\n${graph.project ?? ''}`) : null;
}

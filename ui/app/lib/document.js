// Project files (M2.8-12, ADR 0025): what the window shows of the open
// project (`app`'s `project`: `dir`, and for a .signal file `file`, `name`
// and `edited`), and what opening another one does while a .signal project
// is edited (`project.open` is refused with `edited` until the person
// chooses). Plain ES module, no DOM.

/** The last part of a path. */
function base(path) {
  const parts = String(path ?? '').split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : '';
}

/** The toolbar's project: its name (the file's, else the folder's) and whether it is edited. */
export function projectLabel(project) {
  if (!project) return { name: 'No project', edited: false };
  const name = project.file ? base(project.file) : project.name ?? base(project.dir);
  return { name, edited: Boolean(project.file && project.edited) };
}

/** The page's title, as the window's: `kick.signal — Edited`, or `Signal` without a project. */
export function windowTitle(project) {
  if (!project) return 'Signal';
  const { name, edited } = projectLabel(project);
  return edited ? `${name} — Edited` : name;
}

/** The Setup tab's field: the .signal file, else the folder. */
export function fieldValue(project) {
  return project?.file ?? project?.dir ?? '';
}

/** The in-page prompt's buttons, the default first, for an `edited` refusal. */
export const PROMPT = Object.freeze([
  Object.freeze({ id: 'save', label: 'Save' }),
  Object.freeze({ id: 'cancel', label: 'Cancel' }),
  Object.freeze({ id: 'discard', label: "Don't Save" }),
]);

/**
 * What `project.open` of `path` is sent again with after an `edited`
 * refusal (`error`) answered with `choice`: with `unsaved` "save" or
 * "discard"; null for Cancel (nothing is opened) or another refusal.
 */
export function openRetry(path, error, choice) {
  if (error?.code !== 'edited') return null;
  if (choice !== 'save' && choice !== 'discard') return null;
  return { type: 'project.open', path, unsaved: choice };
}

/** A refusal of `project.open` as the window shows it: its message, verbatim (`signal ui`'s `not-allowed` included). */
export function openRefusal(error) {
  return error?.message ?? String(error ?? '');
}

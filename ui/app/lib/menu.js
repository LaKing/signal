// Signal.app's menu actions (ADR 0023, M2.8-11, M2.8-12). The menu is
// native (muda): File (Open…, Save, Save As…, Interface Setup…, Quit), the
// app menu, Edit, Project (Restart from Files, ADR 0033) and Window. It
// hands Save, Interface Setup and Restart from Files (and Open… without its
// chooser) to the page as a `signal-menu` event; Save As… is the window's
// alone. In a browser (`signal ui`, or the app's URL opened
// elsewhere) there is no native menu, so the page takes Cmd-O, Cmd-S and
// Cmd-comma itself, and has no Save As and no chooser. Plain ES module, no
// DOM.

const KEYS = Object.freeze({ o: 'open', s: 'save', ',': 'setup' });

/** The action a keydown asks for (Cmd-O, Cmd-S, Cmd-comma), or null. */
export function shortcut(event) {
  if (!event?.metaKey || event.altKey || event.ctrlKey || event.shiftKey) return null;
  return KEYS[String(event.key ?? '').toLowerCase()] ?? null;
}

/**
 * Whether the page handles the shortcuts: not inside Signal.app, whose
 * native menu takes them (its WebView sets `window.signalApp.menu`).
 */
export function pageHandlesShortcuts(target = globalThis) {
  return !target?.signalApp?.menu;
}

/**
 * Whether the Panel tab's frame loads again for the project `next` (the
 * `app` message's `project.dir`) after `shown`, the folder whose panel it
 * shows (null: none; undefined: the first message, while the frame loads):
 * only for another project. A moment without a project (null, while
 * the app loads the plan again on a restart, the power or a written parameter) keeps
 * the frame, so the panel is never rebuilt in sight (docs/ui-controls.md
 * "Stable layout").
 */
export function panelReloads(shown, next) {
  return shown !== undefined && Boolean(next) && next !== shown;
}

/** The project's main panel (M2.8-3) in the Panel tab, with the page's fragment. */
export function panelSource(hash) {
  const fragment = String(hash ?? '');
  return fragment.startsWith('#') ? `/project/main.html${fragment}` : '/project/main.html';
}

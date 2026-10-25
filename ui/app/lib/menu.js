// Signal.app's menu actions (ADR 0023, M2.8-11, M2.8-12, ADR 0034). The
// menu is native (muda): the app menu (Settings…, Quit), File (Open…, Save,
// Save As…, Revert to Saved…, Interface Setup…, Quit), Edit, Project (Restart from Files, ADR
// 0033), Window and Help (Keyboard Shortcuts); its chords are the keymap's
// (ui/lib/keys.default.js and the person's keys.toml). It hands Save,
// Interface Setup and Restart from Files (and Open… without its chooser) to
// the page as a `signal-menu` event; Save As… and Revert to Saved… are the
// window's alone. In a
// browser (`signal ui`, or the app's URL opened elsewhere) there is no
// native menu, so the page takes the commands it can run itself
// (ui/controls/keyboard.js tells it with `signal-command`), and has no Save
// As and no chooser. Plain ES module, no DOM.

/** The page's action for app command `id` in a browser (`signal-command`), or null for one the page cannot run. */
const ACTIONS = Object.freeze({
  'file.open': 'open',
  'file.save': 'save',
  'file.interface_setup': 'setup',
  'transport.restart': 'restart',
  'app.settings': 'settings',
  'help.keyboard_shortcuts': 'keys',
});

/** The page action of app command `id` (ui/lib/keys.default.js), or null. */
export function menuAction(id) {
  return ACTIONS[id] ?? null;
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

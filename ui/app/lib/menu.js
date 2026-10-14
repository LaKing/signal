// Signal.app's menu (ADR 0023): in the page, since tao has no menu bar and a
// native one would need `unsafe`. Its items, their shortcuts, and Quit,
// which reaches the app through wry's IPC. Plain ES module, no DOM.

/** The menu's items, in order. */
export const MENU = Object.freeze([
  Object.freeze({ action: 'open', label: 'Open project…', shortcut: '⌘O' }),
  Object.freeze({ action: 'setup', label: 'Interface setup', shortcut: '⌘,' }),
  Object.freeze({ action: 'quit', label: 'Quit', shortcut: '⌘Q' }),
]);

const KEYS = Object.freeze({ o: 'open', ',': 'setup', q: 'quit' });

/** The action a keydown asks for (Cmd-O, Cmd-comma, Cmd-Q), or null. */
export function shortcut(event) {
  if (!event?.metaKey || event.altKey || event.ctrlKey || event.shiftKey) return null;
  return KEYS[String(event.key ?? '').toLowerCase()] ?? null;
}

/**
 * Asks the app to quit (its window closes as with its close button, and the
 * Setup records the window's size). False in a browser, which has no app.
 */
export function quit(target = globalThis) {
  const ipc = target?.ipc;
  if (typeof ipc?.postMessage !== 'function') return false;
  ipc.postMessage('quit');
  return true;
}

/** The app's page that shows the project's panel inside the app's shell. */
export const PANEL_PAGE = 'panel.html';

/** The project's main panel (M2.8-3), shown in the shell, with the fragment. */
export function panelSource(hash) {
  return withFragment('/project/main.html', hash);
}

/** `page` (e.g. `setup.html`) with this page's fragment (the ws port and token). */
export function withFragment(page, hash) {
  const fragment = String(hash ?? '');
  return fragment.startsWith('#') ? `${page}${fragment}` : page;
}

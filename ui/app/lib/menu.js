// Signal.app's menu actions (ADR 0023, M2.8-11). The menu is native (muda):
// File (Open Project…, Save Project, Interface Setup…, Quit), the app menu
// and Edit. It hands Open, Save and Interface Setup to the page as a
// `signal-menu` event. In a browser (`signal ui`, or the app's URL opened
// elsewhere) there is no native menu, so the page takes their shortcuts
// itself. Plain ES module, no DOM.

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

/** The project's main panel (M2.8-3) in the Panel tab, with the page's fragment. */
export function panelSource(hash) {
  const fragment = String(hash ?? '');
  return fragment.startsWith('#') ? `/project/main.html${fragment}` : '/project/main.html';
}

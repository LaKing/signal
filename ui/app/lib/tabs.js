// Signal.app's tabs (M2.8-11): one page, switched without a reload or a new
// connection. ADR 0051 §11 (phase 1): the Signal Project tab, one plane
// (<signal-plane>, ui/controls/plane.js), stands in place of the Panel and
// Schematic tabs; Parameters and Setup stay. Plain ES module, no DOM.

/** The tabs, in the toolbar's order. */
export const TABS = Object.freeze([
  Object.freeze({ id: 'plane', label: 'Deck' }),
  Object.freeze({ id: 'parameters', label: 'Parameters' }),
  Object.freeze({ id: 'setup', label: 'Config' }),
]);

/** The tab the window opens on. */
export const INITIAL_TAB = 'plane';

/** The tab shown after asking for `requested`: it, if there is one, else `current`. */
export function select(current, requested) {
  return TABS.some((t) => t.id === requested) ? requested : current;
}

/**
 * What a menu action shows: Interface Setup… the Setup tab; Open…
 * the Setup tab with its project folder field focused (in a browser;
 * Signal.app shows the system's folder chooser instead, ADR 0023). Null for
 * an action without a tab.
 */
export function tabForAction(action) {
  if (action === 'setup') return { tab: 'setup', focus: null };
  if (action === 'open') return { tab: 'setup', focus: 'project' };
  return null;
}

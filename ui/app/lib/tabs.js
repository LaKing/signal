// Signal.app's tabs (M2.8-11): one page, three tabs, switched without a
// reload or a new connection. Plain ES module, no DOM.

/** The tabs, in the toolbar's order. */
export const TABS = Object.freeze([
  Object.freeze({ id: 'panel', label: 'Panel' }),
  Object.freeze({ id: 'parameters', label: 'Parameters' }),
  Object.freeze({ id: 'setup', label: 'Setup' }),
]);

/** The tab the window opens on. */
export const INITIAL_TAB = 'panel';

/** The tab shown after asking for `requested`: it, if there is one, else `current`. */
export function select(current, requested) {
  return TABS.some((t) => t.id === requested) ? requested : current;
}

/**
 * What a menu action shows: Interface Setup… the Setup tab; Open Project…
 * the Setup tab with its project folder field focused (Signal.app has no
 * native folder chooser, ADR 0023). Null for an action without a tab.
 */
export function tabForAction(action) {
  if (action === 'setup') return { tab: 'setup', focus: null };
  if (action === 'open') return { tab: 'setup', focus: 'project' };
  return null;
}

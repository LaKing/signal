// A window's view state in Signal.app (ADR 0047 §3, §5): the schematic's
// device view (its layer, the board's arrangement, the hidden kinds) and
// the plate's scale (the panel's ⌘+, ⌘−, ⌘0), kept per window in the
// project's `ui/windows.toml` by the app, as the window's place is. The page
// says each change (`view.state`, its whole state); the app tells it back
// when the window opens (`signal-view`). Outside Signal.app (no
// `window.ipc`) nothing is kept: the state lasts as long as the page.

import { appChannel } from './cards.js';

/** A plate's zoom mode (`SignalPanel.zoom`) as the state says it, and back. */
export function zoomState(zoom) {
  if (zoom?.kind === 'manual' && Number.isFinite(zoom.scale)) return { scale: zoom.scale };
  if (zoom?.kind === 'whole') return { zoom: 'whole' };
  return {};
}

export function zoomOf(state) {
  if (Number.isFinite(state?.scale)) return { kind: 'manual', scale: state.scale };
  if (state?.zoom === 'whole') return { kind: 'whole' };
  return { kind: 'width' };
}

/** `state` with the plate's part replaced by `zoom`'s. */
export function withZoom(state, zoom) {
  const { scale: _scale, zoom: _zoom, ...rest } = state ?? {};
  return { ...rest, ...zoomState(zoom) };
}

/**
 * Connects this window's view state to the app: `schematic` (a
 * <signal-schematic>, or null) and `frame` (the panel's frame, or null).
 * `ready`: this page says `window.ready` itself (a schematic window; a
 * page with cards says it from ./cards.js). Returns the remover.
 */
export function connectViewState({ target = globalThis, schematic = null, frame = null, ready = false } = {}) {
  const post = appChannel(target);
  if (!post) return () => {};
  let state = {};
  const panel = () => frame?.contentDocument?.querySelector?.('signal-panel') ?? null;
  const told = (event) => {
    state = { ...(event.detail ?? {}) };
    schematic?.board?.restore?.(state);
    panel()?.setZoom?.(zoomOf(state));
  };
  const changed = (event) => {
    state = { ...state, ...(event.detail ?? {}) };
    post({ type: 'view.state', state });
  };
  const zoomed = (event) => {
    state = withZoom(state, event.detail);
    post({ type: 'view.state', state });
  };
  let frameWindow = null;
  const hookFrame = () => {
    frameWindow?.removeEventListener?.('signal-plate-zoom', zoomed);
    frameWindow = frame?.contentWindow ?? null;
    frameWindow?.addEventListener?.('signal-plate-zoom', zoomed);
    // A panel loaded after the state was told takes it now.
    if (state.scale !== undefined || state.zoom !== undefined) panel()?.setZoom?.(zoomOf(state));
  };
  target.addEventListener('signal-view', told);
  schematic?.addEventListener('signal-view-change', changed);
  frame?.addEventListener('load', hookFrame);
  if (frame?.contentDocument?.readyState === 'complete') hookFrame();
  if (ready) post({ type: 'window.ready' });
  return () => {
    target.removeEventListener('signal-view', told);
    schematic?.removeEventListener('signal-view-change', changed);
    frame?.removeEventListener('load', hookFrame);
    frameWindow?.removeEventListener?.('signal-plate-zoom', zoomed);
  };
}

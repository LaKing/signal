// Whether a control is shown, and when that may have changed (review
// 2026-10-31): a stream that claims one of the engine's 8 scope slots (a
// playhead) is held only while its control is shown. Shown is its page
// visible, the control not 0×0 (a tab not shown, display: none), in the
// viewport (an IntersectionObserver), not `visibility: hidden`
// (`checkVisibility`; not opacity, which a layer fades in after it is
// shown) and in no `inert` subtree (a schematic face hidden, the plate on
// Board). The faces dispatch `signal-visibility` on the document when a
// face's classes change, which no resize or intersection reports. Works
// without a DOM (every part it lacks counts as shown).

/** Whether `node` is in an `inert` subtree, through shadow roots (a schematic face's). */
export function inertAround(node) {
  for (let n = node; n; n = n.parentNode ?? n.host ?? null) if (n.inert) return true;
  return false;
}

/**
 * Watches `element`: `onChange()` when it may have been shown or hidden;
 * returns `{shown(), stop()}`.
 */
export function watchShown(element, onChange) {
  const doc = element.ownerDocument ?? globalThis.document ?? null;
  const View = doc?.defaultView ?? globalThis;
  let intersecting = true;
  const shown = () => {
    if (doc?.visibilityState === 'hidden' || !intersecting) return false;
    const box = element.getBoundingClientRect?.();
    if (box && !(box.width > 0 || box.height > 0)) return false;
    if (element.checkVisibility?.({ visibilityProperty: true }) === false) return false;
    return !inertAround(element);
  };
  doc?.addEventListener?.('visibilitychange', onChange);
  doc?.addEventListener?.('signal-visibility', onChange);
  const resize = typeof View.ResizeObserver === 'function' ? new View.ResizeObserver(() => onChange()) : null;
  resize?.observe(element);
  const sight =
    typeof View.IntersectionObserver === 'function'
      ? new View.IntersectionObserver((entries) => {
          intersecting = entries.at(-1)?.isIntersecting !== false;
          onChange();
        })
      : null;
  sight?.observe(element);
  return {
    shown,
    stop() {
      doc?.removeEventListener?.('visibilitychange', onChange);
      doc?.removeEventListener?.('signal-visibility', onChange);
      resize?.disconnect();
      sight?.disconnect();
    },
  };
}

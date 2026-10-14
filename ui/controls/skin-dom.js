// Skins in the DOM (docs/ui-controls.md "Skins"): loading an SVG part set
// (sanitized) or a filmstrip image, and showing a position with them. The
// string rules are lib/skin.js's.

import { SVG_SANITIZE, cleanCss, resolveSkinUrl, sanitizeAttribute, stripFrame } from '../lib/skin.js';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** Removes what an SVG skin may not carry: scripts, handlers, off-host references. */
export function sanitizeSvg(root) {
  for (const name of SVG_SANITIZE.elements) {
    for (const element of [...root.getElementsByTagName(name)]) element.remove();
  }
  for (const element of [root, ...root.querySelectorAll('*')]) {
    // Every attribute, so a presentation attribute (fill="url(…)") is checked too.
    for (const attribute of [...element.attributes]) {
      const value = sanitizeAttribute(attribute.name, attribute.value);
      if (value === null) element.removeAttributeNode(attribute);
      else if (value !== attribute.value) attribute.value = value;
    }
    if (element.localName === 'style') element.textContent = cleanCss(element.textContent);
  }
  return root;
}

async function loadSvg(spec, base) {
  const response = await fetch(resolveSkinUrl(spec.url, base));
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const parsed = new DOMParser().parseFromString(await response.text(), 'image/svg+xml');
  const svg = parsed.documentElement;
  if (svg.namespaceURI !== SVG_NS || svg.localName !== 'svg' || parsed.getElementsByTagName('parsererror').length > 0) {
    throw new Error('not an SVG document');
  }
  return { kind: 'svg', spec, svg: document.importNode(sanitizeSvg(svg), true) };
}

function loadImage(url) {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`${url} did not load`));
    image.src = url;
  });
}

async function loadStrip(spec, base) {
  const url = resolveSkinUrl(spec.url, base);
  const image = await loadImage(url);
  const hidpi = spec.hidpi ? resolveSkinUrl(spec.hidpi, base) : null;
  // The URLs go into CSS as they are (an @2x name stays @2x).
  const css = hidpi ? `image-set(url("${url}") 1x, url("${hidpi}") 2x)` : `url("${url}")`;
  return { kind: 'strip', spec, width: image.naturalWidth, height: image.naturalHeight, css, webkitCss: hidpi ? `-webkit-${css}` : css };
}

/** Loads a parsed skin (`parseSkin`) against `base` (the page's URL). */
export function loadSkin(spec, base) {
  return spec.kind === 'svg' ? loadSvg(spec, base) : loadStrip(spec, base);
}

/** Puts a loaded skin into `container`. */
export function mountSkin(container, skin) {
  container.replaceChildren();
  if (skin.kind === 'svg') {
    container.append(skin.svg);
  } else {
    const frame = document.createElement('div');
    frame.className = 'strip';
    frame.style.backgroundImage = skin.webkitCss;
    frame.style.backgroundImage = skin.css;
    frame.style.backgroundRepeat = 'no-repeat';
    container.append(frame);
  }
}

function part(svg, name) {
  return svg.querySelector(`[data-part="${name}"]`) ?? svg.getElementById?.(name) ?? svg.querySelector(`#${name}`);
}

function number(element, name, fallback) {
  const value = Number(element.getAttribute(name));
  return element.hasAttribute(name) && Number.isFinite(value) ? value : fallback;
}

/**
 * Shows a state with a mounted skin. `position` (0..1) picks a strip frame;
 * an SVG's named parts move: `pointer` rotates by `angle` (degrees) about
 * `data-cx`/`data-cy` (default: the view box's centre); `thumb` moves
 * `data-travel` units along `data-axis` (y, upwards, by default); `puck`
 * moves `data-travel-x` right by `x` and `data-travel-y` up by `y`; `on` and
 * `off` show by `position` ≥ 0.5; `value` shows `text`.
 */
export function showSkin(container, skin, { position = 0, angle = 0, x = 0, y = 0, text = '' } = {}) {
  if (skin.kind === 'strip') {
    const frame = stripFrame({ position, frames: skin.spec.frames, orientation: skin.spec.orientation, columns: skin.spec.columns, imageWidth: skin.width, imageHeight: skin.height, scale: skin.spec.scale });
    const element = container.firstElementChild;
    element.style.width = `${frame.width}px`;
    element.style.height = `${frame.height}px`;
    element.style.backgroundSize = frame.backgroundSize;
    element.style.backgroundPosition = frame.backgroundPosition;
    return;
  }
  const svg = skin.svg;
  const box = svg.viewBox?.baseVal;
  const pointer = part(svg, 'pointer');
  if (pointer) {
    const cx = number(pointer, 'data-cx', box && box.width ? box.x + box.width / 2 : 0);
    const cy = number(pointer, 'data-cy', box && box.height ? box.y + box.height / 2 : 0);
    pointer.setAttribute('transform', `rotate(${angle} ${cx} ${cy})`);
  }
  const thumb = part(svg, 'thumb');
  if (thumb) {
    const travel = number(thumb, 'data-travel', 0);
    thumb.setAttribute('transform', thumb.getAttribute('data-axis') === 'x' ? `translate(${position * travel} 0)` : `translate(0 ${(1 - position) * travel})`);
  }
  const puck = part(svg, 'puck');
  if (puck) {
    puck.setAttribute('transform', `translate(${x * number(puck, 'data-travel-x', 0)} ${(1 - y) * number(puck, 'data-travel-y', 0)})`);
  }
  const on = part(svg, 'on');
  if (on) on.style.display = position >= 0.5 ? '' : 'none';
  const off = part(svg, 'off');
  if (off) off.style.display = position >= 0.5 ? 'none' : '';
  const value = part(svg, 'value');
  if (value) value.textContent = text;
}

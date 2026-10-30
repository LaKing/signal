// The faces of slots in <signal-schematic> (2026-10-30; the owner: "could a
// slot have a panel header with the same layout as on the panel itself?";
// ui/lib/schematic-face.js has the geometry and the rules). A slot box with
// a part (`ui/parts/<key>.html`) shows the part in its box, rendered by the
// panel's own code (`showWritten`, ui/controls/panel-editor.js) with the
// panel's CSS (ui/controls/theme.css and the project panel's style sheets,
// the studio's studio.css) and its skins, its binds rooted at the slot
// drawn: the same cards and controls as on the panel, live (each control
// streams and writes as on the panel) and operable. Each face is an HTML
// element of its own (its own shadow root, so neither the schematic's CSS
// nor the project's reaches the other) in a layer over the drawing,
// placed over the face's area of its box, which the drawing marks
// (`.face-area`, its corners in the level's units): one CSS transform per
// face, written only when it moves. The face is built once per part text,
// never per frame. A press, a move, a double-click or the wheel on a face
// that is not on one of its controls is the drawing's (it selects, drags
// and pans the box as on its title); on a control it is the control's.

import { showWritten } from './panel-editor.js';
import { layOutCards } from './plate.js';
import { parsePlate } from '../lib/plate.js';
import { FACE_LAYOUT_PX, estimateFace, faceAttribute, faceFit, facePlacement, faceWidth, lit, measuredNatural, overlapping, rootRules, stylesheets } from '../lib/schematic-face.js';
import { screenRect } from '../lib/schematic-view.js';

/** The theme every panel links (ui/controls/theme.css). */
const THEME = new URL('./theme.css', import.meta.url).href;

/** The project's panel page, which a part's relative URLs are resolved from (ADR 0046 §2). */
function pageBase() {
  try {
    const page = globalThis.location?.href;
    return page ? new URL('/project/main.html', page).href : null;
  } catch {
    return null;
  }
}

const FACE_CSS = `
/* ADR 0047 §4: laid out at its plate's own width (a part without places at 1280 px), then scaled with its box: the part's plate, never a reflow. Only its cards take the pointer: between them it is the drawing's. */
:host { position: absolute; left: 0; top: 0; width: ${FACE_LAYOUT_PX}px; box-sizing: border-box; transform-origin: 0 0; pointer-events: none; visibility: hidden; contain: layout style; }
.face-panel .signal-cards > *, .face-note { pointer-events: auto; }
:host(.shown) { visibility: visible; }
/* Around an open frame its level is dimmed (as its boxes are). */
:host(.around) { opacity: 0.4; }
.face-panel { font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); color: var(--signal-fg, #d6dbe4); }
.face-note { margin: 0 0 6px; padding: 4px 6px; font: 11px/1.4 ui-monospace, Menlo, monospace; color: var(--signal-clip, #ff5252); background: var(--signal-panel, #1f232b); border-radius: 4px; }
.face-note:empty { display: none; }
/* A face control bound to the wire or pin under the pointer or selected. */
[data-face-hot] { outline: 2px solid var(--signal-focus, #5aa9ff); outline-offset: 3px; border-radius: 6px; }
`;

/** Whether `event` is on one of a face's controls (a signal-* element other than the face's own). */
function onControl(event) {
  for (const node of event.composedPath?.() ?? []) {
    if (node?.classList?.contains?.('face-panel')) return false;
    if (typeof node?.localName === 'string' && node.localName.startsWith('signal-')) return true;
  }
  return false;
}

export class SchematicFaces {
  /** `host`: the <signal-schematic>, whose drawing the faces lie over. */
  constructor(host) {
    this.host = host;
    /** The layer over the drawing, the faces in it. */
    this.layer = document.createElement('div');
    this.layer.className = 'faces';
    /** Each face shown, by its id (`<path>/<slot>`, or `<path>#frame`). */
    this.faces = new Map();
    /** Each part's text as read: file → `{html, base, error, reading}`. */
    this.parts = new Map();
    /** Each part's measured natural size, by file (`{w, h}`, CSS px at FACE_LAYOUT_PX), and the sizes the layout used (`w×h`). */
    this.sizes = new Map();
    this.used = new Map();
    /** The project panel's style sheets (`stylesheets`), and the addresses lit. */
    this.sheets = [THEME];
    /** The project sheets' page-wide rules made the faces' own (`rootRules`). */
    this.rootCss = '';
    this.hot = new Set();
    this.base = pageBase();
    this.resize = typeof ResizeObserver === 'function' ? new ResizeObserver((entries) => this.resized(entries)) : null;
  }

  /** Another project: every face and part let go. */
  clear() {
    for (const face of this.faces.values()) this.drop(face);
    this.faces.clear();
    this.parts.clear();
    this.sizes.clear();
    this.used.clear();
  }

  /** The project panel's whole text (`panel.read` of main.html): its style sheets style the faces too. */
  pageSeen(text) {
    const sheets = [THEME, ...stylesheets(text, this.base).filter((s) => s !== THEME)];
    if (sheets.join('\n') === this.sheets.join('\n')) return;
    this.sheets = sheets;
    for (const face of this.faces.values()) this.style(face);
    this.readRoots(sheets.filter((s) => s !== THEME));
  }

  /** The project sheets' `:root` and `body` rules (its card width, its font), fetched once per list, as every face's. */
  readRoots(sheets) {
    if (typeof fetch !== 'function' || !sheets.length) return;
    const asked = (this.rootsAsked = sheets.join('\n'));
    Promise.all(sheets.map((href) => fetch(href).then((r) => (r.ok ? r.text() : ''), () => '')))
      .then((texts) => {
        if (asked !== this.rootsAsked) return;
        const css = texts.map(rootRules).filter(Boolean).join('\n');
        if (css === this.rootCss) return;
        this.rootCss = css;
        for (const face of this.faces.values()) face.roots.textContent = css;
      })
      .catch(() => {});
  }

  /** The size the layout gives face `face` (`levelFaces`'): measured, else estimated. */
  sizeOf(face) {
    const natural = this.sizes.get(face.file) ?? estimateFace(face.count);
    this.used.set(face.file, `${natural.w}x${natural.h}`);
    return faceFit(natural);
  }

  /** A `panel.written` push: a part written (or removed: `html` null) shows at once in its faces. */
  written(message) {
    const file = message?.file;
    if (typeof file !== 'string' || !file.startsWith('parts/')) return;
    this.parts.set(file, { html: message.html ?? null, base: message.base ?? null, plate: message.plate ?? null, error: null, reading: false });
    for (const face of this.faces.values()) if (face.file === file) this.render(face);
  }

  /** Reads part `file` (`panel.read`) when it is not read yet or its text changed (`base`, the graph's). */
  read(file, base) {
    const known = this.parts.get(file);
    if (known?.reading || (known && (base === undefined || known.base === base || known.error))) return;
    const request = this.host.connection?.request?.({ type: 'panel.read', file });
    if (!request) {
      this.parts.set(file, { html: null, base: null, error: 'no connection to read the part', reading: false });
      return;
    }
    this.parts.set(file, { ...(known ?? { html: null, base: null, error: null }), reading: true });
    request.then(
      (read) => {
        this.parts.set(file, { html: read?.exists ? (read.html ?? '') : null, base: read?.base ?? null, plate: read?.plate ?? null, error: read?.exists ? null : 'no such part file', reading: false });
        for (const face of this.faces.values()) if (face.file === file) this.render(face);
      },
      (error) => {
        // Not open yet: read again at the next sync.
        if (error?.code === 'closed') this.parts.delete(file);
        else this.parts.set(file, { html: null, base: null, error: error?.message ?? String(error), reading: false });
        for (const face of this.faces.values()) if (face.file === file) this.render(face);
      },
    );
  }

  /** A new face element for the slot at `root`, part `file`. */
  create(id, file, root, error = null) {
    const element = document.createElement('div');
    element.className = 'face';
    element.dataset.face = id;
    const shadow = element.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent = FACE_CSS;
    const roots = document.createElement('style');
    roots.textContent = this.rootCss;
    const links = document.createElement('div');
    links.className = 'face-sheets';
    links.hidden = true;
    const note = document.createElement('p');
    note.className = 'face-note';
    const panel = document.createElement('div');
    panel.className = 'face-panel';
    shadow.append(links, style, roots, note, panel);
    const face = { id, file, root, error, element, shadow, links, roots, note, panel, shownHtml: undefined, placed: '', classes: '', layoutWidth: FACE_LAYOUT_PX };
    this.style(face);
    // On a control the control's; elsewhere the drawing's, as on the box under it.
    element.addEventListener('pointerdown', (event) => {
      if (onControl(event)) return;
      event.preventDefault();
      this.host.pointerDown(event);
      // A click on a face selects its box, never opens it (a double-click does).
      if (this.host.gesture) this.host.gesture.fromFace = true;
    });
    element.addEventListener('pointermove', (event) => {
      if (!this.host.gesture && !onControl(event)) this.host.pointerMove(event);
    });
    element.addEventListener('dblclick', (event) => {
      if (!onControl(event)) this.host.doubleClick(event);
    });
    // A control that took the wheel (it owns it: focused, clicked) prevented its default; else the drawing zooms or pans.
    element.addEventListener(
      'wheel',
      (event) => {
        if (!event.defaultPrevented) this.host.wheel(event);
      },
      { passive: false },
    );
    this.layer.append(element);
    this.resize?.observe?.(panel);
    this.faces.set(id, face);
    this.render(face);
    return face;
  }

  /** Its style sheets: the theme and the project panel's, linked in its own shadow root. */
  style(face) {
    const have = [...face.links.parentNode.querySelectorAll('link')].map((l) => l.href);
    if (have.join('\n') === this.sheets.join('\n')) return;
    for (const link of face.links.parentNode.querySelectorAll('link')) link.remove();
    face.links.after(
      ...this.sheets.map((href) => {
        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = href;
        return link;
      }),
    );
  }

  /** Face `face` drawn from its part as read (once per text: never per frame). */
  async render(face) {
    const part = this.parts.get(face.file);
    const html = part?.html ?? null;
    const note = part?.error ? `${face.file}: ${part.error}` : face.error ? `${face.file}: ${face.error}` : '';
    if (face.note.textContent !== note) face.note.textContent = note;
    const plate = part?.plate ?? null;
    const key = `${face.root}\n${plate}\n${html}`;
    if (face.shownHtml === key) return;
    face.shownHtml = key;
    if (html === null) {
      face.panel.replaceChildren();
      return;
    }
    const error = await showWritten(face.panel, html, (name, value) => faceAttribute(name, value, face.root, this.base));
    if (error && face.note.textContent === '') face.note.textContent = `${face.file}: ${error}`;
    // ADR 0047 §4: the part's plate, at its own size.
    face.layoutWidth = faceWidth(parsePlate(plate));
    face.element.style.width = `${face.layoutWidth}px`;
    layOutCards(face.panel.querySelector(':scope > .signal-cards'), plate);
    this.lightFace(face);
    // Its cards' width as drawn now (a card's width changes no height the observer sees).
    this.measure([face]);
  }

  drop(face) {
    this.resize?.unobserve?.(face.panel);
    face.element.remove();
  }

  /** A face's content changed size: its part measured again. */
  resized(entries) {
    this.measure([...this.faces.values()].filter((f) => entries.some((e) => e.target === f.panel)));
  }

  /**
   * The natural size of `faces`' parts (CSS px, at their plate's width): its
   * height, and its cards' extent across (screen rectangles over the
   * face's own scale, so any transform holds); the drawing laid out again
   * when one differs from what the layout used.
   */
  measure(faces) {
    let changed = false;
    for (const face of faces) {
      const box = face.panel.getBoundingClientRect?.();
      const scale = box?.width > 0 ? box.width / (face.layoutWidth || FACE_LAYOUT_PX) : 0;
      if (!scale) continue;
      let right = 0;
      for (const grid of face.panel.children ?? []) {
        for (const card of grid.children ?? []) {
          const r = card.getBoundingClientRect?.();
          if (r?.width > 0) right = Math.max(right, (r.right - box.left) / scale);
        }
      }
      const natural = measuredNatural(right, face.panel.offsetHeight + face.note.offsetHeight, face.layoutWidth || FACE_LAYOUT_PX);
      if (!natural) continue;
      const old = this.sizes.get(face.file);
      if (old?.w === natural.w && old?.h === natural.h) continue;
      this.sizes.set(face.file, natural);
      if (this.used.get(face.file) !== `${natural.w}x${natural.h}`) changed = true;
    }
    if (changed) this.host.facesResized();
  }

  /**
   * Every face placed over its area as drawn now (after a render, during a
   * drag): the drawing's `.face-area` marks give each face's corners on
   * screen; a face whose area is gone goes, one too small or off the stage
   * is hidden, one under an open frame of its level is hidden, one around
   * it dimmed.
   */
  sync(drawn) {
    const stage = this.layer.getBoundingClientRect();
    const view = { x: 0, y: 0, w: stage.width, h: stage.height };
    const frames = new Map();
    for (const level of drawn?.levels ?? []) {
      const name = level.layout?.opened;
      const frame = name ? level.layout.nodes.find((n) => n.kind === 'instance' && n.name === name) : null;
      if (frame) frames.set(level.path, { name, rect: screenRect(level.transform, frame) });
    }
    const seen = new Set();
    for (const area of this.host.svg.querySelectorAll('.face-area')) {
      const { face: id, file, root, path, slot, base, error } = area.dataset;
      if (!id || !file) continue;
      seen.add(id);
      let face = this.faces.get(id);
      if (face && (face.file !== file || face.root !== root)) {
        this.drop(face);
        this.faces.delete(id);
        face = null;
      }
      if (!face) face = this.create(id, file, root, error || null);
      this.read(file, base || undefined);
      const ctm = area.getScreenCTM?.();
      let rect = null;
      if (ctm) {
        const a = new DOMPoint(Number(area.dataset.x0), Number(area.dataset.y0)).matrixTransform(ctm);
        const b = new DOMPoint(Number(area.dataset.x1), Number(area.dataset.y1)).matrixTransform(ctm);
        rect = { x: a.x - stage.left, y: a.y - stage.top, w: b.x - a.x, h: b.y - a.y };
      }
      const place = facePlacement(rect, Number(area.dataset.nw) || FACE_LAYOUT_PX);
      const frame = frames.get(path);
      const covered = Boolean(frame && slot !== frame.name && overlapping(rect, frame.rect));
      const shown = Boolean(place) && !covered && overlapping(rect, view);
      const classes = `face${shown ? ' shown' : ''}${frame && !covered && slot !== frame.name ? ' around' : ''}`;
      if (face.classes !== classes) {
        face.element.className = classes;
        face.classes = classes;
      }
      const transform = shown ? place.transform : '';
      if (shown && face.placed !== transform) {
        face.element.style.transform = transform;
        face.placed = transform;
      }
    }
    for (const [id, face] of this.faces) {
      if (seen.has(id)) continue;
      this.drop(face);
      this.faces.delete(id);
    }
  }

  /** The addresses `hot` (`hotAddresses`') lit in every face: the controls binding one of them are outlined. */
  light(hot) {
    const key = [...hot].sort().join('\n');
    if (key === this.hotKey) return;
    this.hotKey = key;
    this.hot = hot;
    for (const face of this.faces.values()) this.lightFace(face);
  }

  lightFace(face) {
    for (const element of face.panel.querySelectorAll('*')) {
      if (!element.localName.startsWith('signal-')) continue;
      const on = lit(element.getAttribute('bind'), this.hot);
      if (on !== element.hasAttribute('data-face-hot')) element.toggleAttribute('data-face-hot', on);
    }
  }

  /** Whether `event` (a key) belongs to a face's control: the drawing's keys leave it alone. */
  owns(event) {
    return event.composedPath?.().includes(this.layer) && onControl(event);
  }
}

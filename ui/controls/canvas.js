// <signal-canvas>: subscribed vector streams (a visual-rate model's state,
// binary frames of kind 4) drawn by a renderer: points, mesh or bodies
// (docs/ui-controls.md "Canvas"). Canvas 2D, or WebGL when the browser
// gives one and the renderer is unskinned points or mesh; skinned with a
// sprite sheet (`strip:…;frames=N`) or an SVG (`svg:…`). It draws what
// arrives and simulates nothing: replaying a session draws the same.

import { chooseBackend, fitBox, rgba } from '../lib/canvas.js';
import { parseSkin, resolveSkinUrl } from '../lib/skin.js';
import { SignalElement, bindAddresses, numberAttribute } from './base.js';
import * as bodies from '../canvas/bodies.js';
import { webglDrawer } from '../canvas/gl.js';
import * as mesh from '../canvas/mesh.js';
import * as points from '../canvas/points.js';

const RENDERERS = { points, mesh, bodies };

export class SignalCanvas extends SignalElement {
  static role = 'img';
  static observedAttributes = ['bind', 'skin', 'renderer', 'rate', 'grid', 'tilt', 'height', 'backend', 'label'];
  static resubscribe = ['rate'];
  static css = `
:host { display: block; }
.control { display: flex; flex-direction: column; gap: 4px; padding: 4px; }
canvas { width: var(--signal-canvas-width, 100%); height: var(--signal-canvas-height, 240px); background: var(--signal-panel, #1f232b); border-radius: 4px; display: block; }
.readout { text-align: left; color: var(--signal-muted, #8a93a3); width: 100%; }
`;

  constructor() {
    super();
    this.latest = [];
    this.pending = false;
    this.sprite = null;
    /** The context drawn with: `webgl` or `2d` (null before the first draw). */
    this.backend = null;
    this.gl = null;
  }

  get rendererName() {
    const name = this.getAttribute('renderer') ?? 'points';
    return name in RENDERERS ? name : 'points';
  }

  template() {
    return `<div class="control"><canvas part="canvas"></canvas><div class="label"></div><div class="readout"></div></div>`;
  }

  streams(connection) {
    const ports = bindAddresses(this).slice(0, 3);
    this.latest = ports.map(() => null);
    const rate = numberAttribute(this, 'rate') ?? 60;
    return ports.map((port, i) =>
      connection.subscribe(
        { stream: 'vector', port, rate },
        (frame) => {
          this.latest[i] = frame.values;
          this.schedule();
        },
        {
          onSubscribed: () => this.problem(null),
          onError: (error) => this.problem(`${port}: ${error.code ?? error.message}`),
        },
      ),
    );
  }

  /** A sprite sheet or SVG, loaded as an image to draw (never inserted). */
  async loadSkin() {
    const attribute = this.getAttribute('skin');
    let spec = null;
    try {
      spec = parseSkin(attribute);
    } catch (error) {
      console.info(`<signal-canvas> skin="${attribute}": ${error.message}; drawing the default`);
    }
    if (!spec) {
      this.sprite = null;
    } else {
      const image = new Image();
      const url = spec.hidpi && (globalThis.devicePixelRatio || 1) > 1 ? spec.hidpi : spec.url;
      image.onload = () => this.schedule();
      image.onerror = () => console.info(`<signal-canvas> skin "${attribute}" did not load; drawing the default`);
      image.src = resolveSkinUrl(url, document.baseURI);
      this.sprite = { image, spec };
    }
    this.update();
  }

  problem(text) {
    this.problemText = text;
    this.update();
  }

  /** A canvas holds one kind of context: a change of backend takes a new one. */
  setupBackend() {
    const canvas = this.$('canvas');
    if (!canvas) return;
    let wanted = chooseBackend({ backend: this.getAttribute('backend') ?? 'auto', renderer: this.rendererName, skinned: Boolean(this.sprite), webgl: true });
    if (wanted === this.backend) return;
    let target = canvas;
    if (this.backend !== null) {
      target = document.createElement('canvas');
      target.setAttribute('part', 'canvas');
      canvas.replaceWith(target);
    }
    this.gl = null;
    if (wanted === 'webgl') {
      try {
        this.gl = webglDrawer(target);
      } catch (error) {
        console.info(`<signal-canvas> WebGL failed (${error.message}); drawing with Canvas 2D`);
      }
      if (!this.gl) {
        // This canvas may hold a failed WebGL attempt: start a fresh one.
        const fresh = document.createElement('canvas');
        fresh.setAttribute('part', 'canvas');
        target.replaceWith(fresh);
        wanted = '2d';
      }
    }
    this.backend = wanted;
    this.setAttribute('data-backend', wanted);
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    this.setupBackend();
    const label = this.getAttribute('label') ?? '';
    this.$('.label').textContent = label;
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', label ? `${label} canvas` : 'canvas');
    this.setAttribute('aria-description', `${this.rendererName} of ${bindAddresses(this).join(' and ') || 'no port'}`);
    this.$('.readout').textContent = this.problemText ?? `${this.rendererName} · ${this.backend ?? '…'}`;
    this.schedule();
  }

  /** Draws at the next animation frame, once however many frames arrive. */
  schedule() {
    if (this.pending) return;
    this.pending = true;
    requestAnimationFrame(() => {
      this.pending = false;
      this.draw();
    });
  }

  colour(name, fallback) {
    return getComputedStyle(this).getPropertyValue(name).trim() || fallback;
  }

  draw() {
    const canvas = this.$('canvas');
    if (!canvas) return;
    const box = canvas.getBoundingClientRect();
    const ratio = globalThis.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(box.width * ratio));
    const height = Math.max(1, Math.round(box.height * ratio));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const renderer = RENDERERS[this.rendererName];
    const colour = this.colour('--signal-accent', '#4fc3f7');
    const options = {
      fit: fitBox({ width: box.width, height: box.height, ratio, margin: 6 }),
      ratio,
      colour,
      sprite: this.sprite,
      grid: numberAttribute(this, 'grid') ?? undefined,
      tilt: numberAttribute(this, 'tilt') ?? 0.5,
      height: numberAttribute(this, 'height') ?? 0.5,
    };
    if (this.gl && renderer.vertices) {
      this.gl.draw(renderer.vertices(this.latest, options), options.fit, rgba(colour));
      return;
    }
    const g = canvas.getContext('2d');
    if (!g) return;
    g.clearRect(0, 0, width, height);
    renderer.draw2d(g, this.latest, options);
  }
}

customElements.get('signal-canvas') ?? customElements.define('signal-canvas', SignalCanvas);

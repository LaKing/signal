// ADR 0051 phase 2 (§6; ADR 0050 §11): an instrument's Control plane. The
// automation of a bound parameter is detected wherever it acts: a top-level
// connection into the bound address, from the engine or from outside (an
// external control cable), and a connection inside a composite into the
// export's target (the studio's filter contour into `ladder.cutoff_hz`,
// which MONO SYNTH's cutoff knob binds through `/synth.cutoff`); the inner
// value is read at the export's first target. The cube keeps clear of the
// controls, the legs never cross, and the layout is deterministic and
// within the budget.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent } from './panel-tree.js';
import { panelCards } from './plane-file.js';
import { assignInstruments } from './plane-instruments.js';
import { automations, bindTargets, controlLayout, engineOf, innerAddress, legLanes, overlaps, placeCube } from './plane-control.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

function studioCards() {
  const text = read('projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  return panelCards(parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' }).tree);
}

const SYNTH = 'card:mono-synth';

test('a bind’s targets: the parameter, then the export’s targets inside the composite; the inner value at the first', () => {
  assert.deepEqual(
    bindTargets('/synth.cutoff', STUDIO).map((t) => `${t.address} @${t.level || 'top'}`),
    ['/synth.cutoff @top', '/synth/ladder.cutoff_hz @moog_voice'],
  );
  assert.deepEqual(
    bindTargets('/synth.glide', STUDIO).map((t) => t.address),
    ['/synth.glide', '/synth/glide.rise_ms', '/synth/glide.fall_ms'],
  );
  assert.equal(innerAddress('/synth.cutoff', STUDIO), '/synth/ladder.cutoff_hz');
  assert.equal(innerAddress('/transport.bpm', STUDIO), null);
  assert.deepEqual(bindTargets('/nothing', STUDIO), []);
});

test('the studio’s cutoff: the filter contour and the key tracking drive ladder.cutoff_hz inside the engine, so they land on the cutoff knob', () => {
  const cards = studioCards();
  const owner = assignInstruments({ instances: STUDIO.top.instances, connections: STUDIO.top.connections, cards });
  const binds = cards.find((c) => c.id === 'mono-synth').binds;
  const autos = automations({ entry: SYNTH, binds, owner, graph: STUDIO });
  assert.deepEqual(
    autos.map((a) => `${a.bind} ← ${a.from} → ${a.to} (${a.source}${a.inner ? `, inside ${a.level}` : ''})`),
    [
      // The glide's own forwarding is no automation; the oscillators' pitch chain drives no bound parameter.
      '/synth.glide ← /synth/hz_none.out → /synth/glide.rise_ms (engine, inside moog_voice)',
      '/synth.glide ← /synth/hz_none.out → /synth/glide.fall_ms (engine, inside moog_voice)',
      '/synth.cutoff ← /synth/env_amt.out → /synth/ladder.cutoff_hz (engine, inside moog_voice)',
      '/synth.cutoff ← /synth/key_amt.out → /synth/ladder.cutoff_hz (engine, inside moog_voice)',
    ],
  );
  assert.deepEqual(autos[2].written, { from: 'env_amt.out', to: 'ladder.cutoff_hz' });
  assert.deepEqual(engineOf(SYNTH, owner), ['melody', 'synth']);
});

test('automation at the top level: from the engine, or an external control cable from another instrument', () => {
  const graph = structuredClone(STUDIO);
  graph.top.instances.push({ name: 'lfo', module: 'std.lfo', ports: [{ name: 'out', address: '/lfo.out', direction: 'out', shape: 'signal', kind: 'control', rate: 1 }], parameters: [] });
  graph.top.instances.push({ name: 'wobble', module: 'std.lfo', ports: [{ name: 'out', address: '/wobble.out', direction: 'out', shape: 'signal', kind: 'control', rate: 1 }], parameters: [] });
  graph.top.connections.push({ from: '/lfo.out', to: '/synth.cutoff', scale: 400, converters: [] });
  graph.top.connections.push({ from: '/wobble.out', to: '/synth.emphasis', converters: [] });
  const cards = studioCards();
  const owner = assignInstruments({ instances: graph.top.instances, connections: graph.top.connections, cards });
  // The LFO is the echo's (by flow it would join the synth: put it on another instrument by hand for the test).
  owner.set('lfo', 'card:tape-echo');
  owner.set('wobble', SYNTH);
  const binds = cards.find((c) => c.id === 'mono-synth').binds;
  const autos = automations({ entry: SYNTH, binds, owner, graph }).filter((a) => !a.inner);
  assert.deepEqual(
    autos.map((a) => `${a.bind} ← ${a.from} ${a.source} ${a.kind}`),
    ['/synth.cutoff ← /lfo.out external control', '/synth.emphasis ← /wobble.out engine control'],
  );
  assert.deepEqual(autos[0].written, { from: '/lfo.out', to: '/synth.cutoff' });
  assert.equal(autos[0].level, '');
});

/** MONO SYNTH's knobs in a row near the top of its rectangle, its piano roll below (no obstacle). */
function synthControls(rect, binds) {
  const knobs = binds.filter((b) => b !== '/melody.position');
  return knobs.map((address, i) => ({ id: `c${i}`, tag: 'signal-knob', binds: [address], writes: true, label: address, rect: { x: rect.x + 24 + i * 52, y: rect.y + 48, w: 48, h: 96 } }));
}

test('the Control plane: the cube clear of every control, legs that never cross, automation landing on its control, deterministic, in budget', () => {
  const cards = studioCards();
  const owner = assignInstruments({ instances: STUDIO.top.instances, connections: STUDIO.top.connections, cards });
  const binds = cards.find((c) => c.id === 'mono-synth').binds;
  const rect = { x: 0, y: 100, w: 1280, h: 420 };
  const controls = synthControls(rect, binds);
  const autos = automations({ entry: SYNTH, binds, owner, graph: STUDIO });
  const inner = new Map(binds.map((b) => [b, innerAddress(b, STUDIO)]).filter(([, a]) => a));
  const t0 = performance.now();
  const c = controlLayout({ rect, controls, engine: engineOf(SYNTH, owner), autos, inner, title: 'MONO SYNTH' });
  const ms = performance.now() - t0;
  assert.ok(ms < 50, `laid out in ${ms.toFixed(1)} ms (budget 50)`);
  for (const control of controls) assert.ok(!overlaps(c.cube, control.rect), `the cube clears ${control.id}`);
  assert.ok(c.cube.x >= rect.x && c.cube.x + c.cube.w <= rect.x + rect.w && c.cube.y >= rect.y && c.cube.y + c.cube.h <= rect.y + rect.h, 'the cube is in the instrument');
  assert.equal(c.pins.length, controls.length, 'one connector pin per bound parameter');
  // No two legs cross: their horizontal runs never meet another's vertical.
  for (const a of c.legs) {
    for (const b of c.legs) {
      if (a === b) continue;
      const ha = a.points.slice(1, -1);
      const [p, q] = [ha[0], ha[1]];
      const xs = [b.points[0].x, b.points.at(-1).x];
      for (const x of xs) {
        const yb = [b.points[0].y, b.points.at(-1).y];
        const inside = x > Math.min(p.x, q.x) + 1e-6 && x < Math.max(p.x, q.x) - 1e-6;
        const spans = p.y > Math.min(...yb) && p.y < Math.max(...yb);
        assert.ok(!(inside && spans && (x === b.points[0].x ? p.y < b.points[1].y : p.y > b.points.at(-2).y)), `${a.id} crosses ${b.id}`);
      }
    }
  }
  // The cutoff's automation lands on the cutoff knob's top, from the cube's automation pins.
  const cutoff = controls.find((k) => k.binds[0] === '/synth.cutoff');
  const landing = c.wires.filter((w) => w.control === cutoff.id);
  assert.equal(landing.length, 2);
  for (const w of landing) {
    assert.equal(w.points.at(-1).y, cutoff.rect.y);
    assert.ok(c.autoPins.some((p) => p.auto === w.auto.id));
  }
  assert.ok(c.readouts.some((r) => r.bind === '/synth.cutoff' && r.address === '/synth/ladder.cutoff_hz'));
  assert.deepEqual(controlLayout({ rect, controls, engine: engineOf(SYNTH, owner), autos, inner, title: 'MONO SYNTH' }), c);
});

test('an external cable enters at the instrument’s edge, where the Wiring level’s port is, and lands on its control', () => {
  const rect = { x: 0, y: 0, w: 1280, h: 300 };
  const controls = [{ id: 'k', tag: 'signal-knob', binds: ['/synth.cutoff'], writes: true, rect: { x: 600, y: 40, w: 48, h: 96 } }];
  const autos = [{ id: 'auto 0', bind: '/synth.cutoff', from: '/lfo.out', to: '/synth.cutoff', level: '', inner: false, source: 'external', kind: 'control' }];
  const c = controlLayout({ rect, controls, engine: ['synth'], autos, edgeYs: new Map([['/synth.cutoff', 72]]) });
  assert.deepEqual(c.externalPins.map((p) => [p.x, p.y, p.from]), [[0, 72, '/lfo.out']]);
  assert.deepEqual(c.wires[0].points.at(-1), { x: 624, y: 40 });
});

test('leg lanes nest: rightward legs leftmost deepest, leftward rightmost deepest', () => {
  assert.deepEqual(legLanes([{ c: 0, p: 100 }, { c: 10, p: 110 }, { c: 20, p: 120 }]), [2, 1, 0]);
  assert.deepEqual(legLanes([{ c: 100, p: 0 }, { c: 110, p: 10 }]), [0, 1]);
  assert.deepEqual(legLanes([{ c: 0, p: 50 }, { c: 100, p: 60 }]), [0, 0]);
});

test('the cube’s place: the free spot nearest its controls, ties by y then x; never over an obstacle', () => {
  const area = { x: 0, y: 0, w: 400, h: 200 };
  const r = placeCube({ w: 64, h: 48 }, area, [{ x: 0, y: 0, w: 400, h: 80 }], { x: 200, y: 0 });
  assert.ok(r.y >= 80 + 8);
  assert.equal(r.x + 32 - 200 <= 16, true);
  const full = placeCube({ w: 64, h: 48 }, area, [{ x: -10, y: -10, w: 500, h: 500 }], { x: 0, y: 0 });
  assert.deepEqual(full, { x: 336, y: 152, w: 64, h: 48 });
});

test('automation and external cables never run over a control: corridors between the columns, the left edge, lanes in the row gaps', () => {
  const cards = studioCards();
  const graph = structuredClone(STUDIO);
  graph.top.instances.push({ name: 'lfo', module: 'std.osc', ports: [{ name: 'out', address: '/lfo.out', direction: 'out', shape: 'signal', kind: 'control', rate: 1 }], parameters: [] });
  graph.top.connections.push({ from: '/lfo.out', to: '/synth.cutoff', converters: [] });
  const owner = assignInstruments({ instances: graph.top.instances, connections: graph.top.connections, cards });
  owner.set('lfo', 'card:tape-echo');
  const binds = cards.find((c) => c.id === 'mono-synth').binds.filter((b) => b !== '/melody.position');
  // Three rows of knobs, as MONO SYNTH lays them out, the gap between rows its group headings'.
  const rect = { x: 0, y: 100, w: 1280, h: 700 };
  const controls = binds.map((address, i) => ({ id: `c${i}`, tag: 'signal-knob', binds: [address], writes: true, rect: { x: 40 + (i % 10) * 70, y: rect.y + 40 + Math.floor(i / 10) * 140, w: 56, h: 100 } }));
  const autos = automations({ entry: SYNTH, binds, owner, graph });
  const c = controlLayout({ rect, controls, engine: engineOf(SYNTH, owner), autos, edgeYs: new Map([['/synth.cutoff', 160]]), inner: new Map() });
  assert.ok(c.wires.length >= 5);
  const through = (a, b, r) => {
    const lo = { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y) };
    const hi = { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y) };
    return lo.x < r.x + r.w - 1e-6 && hi.x > r.x + 1e-6 && lo.y < r.y + r.h - 1e-6 && hi.y > r.y + 1e-6;
  };
  for (const w of c.wires) {
    for (let i = 1; i < w.points.length; i++) {
      for (const k of controls) {
        // Its last segment ends on its own control's top edge, from above: that is landing, not crossing.
        assert.ok(!through(w.points[i - 1], w.points[i], k.rect), `${w.id} runs over ${k.binds[0]}`);
      }
      assert.ok(!through(w.points[i - 1], w.points[i], c.cube) || i === 1, `${w.id} runs over the cube`);
    }
  }
  const external = c.wires.find((w) => w.auto.source === 'external');
  assert.equal(external.points[0].x, rect.x);
  assert.ok(external.points[1].x < Math.min(...controls.map((k) => k.rect.x)), 'along the left edge, in the margin');
});

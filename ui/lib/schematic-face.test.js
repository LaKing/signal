// A slot's face in the schematic (2026-10-30, ui/lib/schematic-face.js):
// its geometry in the box, its addresses rooted and skins resolved from the
// project's panel page, the panel's style sheets, what a hover or a
// selection lights, where it goes on screen.

import test from 'node:test';
import assert from 'node:assert/strict';
import { FACE_MIN_PX, FACE_STEP, FACE_W, estimateFace, faceAttribute, facePlacement, faceSkin, frameFace, hotAddresses, labelColumn, lit, measuredHeight, overlapping, placeFace, stylesheets } from './schematic-face.js';
import { HEADER } from './schematic-layout.js';

const BASE = 'http://127.0.0.1:7000/project/main.html';

test("a face sits under the box's title between its pins' names; the box grows to hold it, its pins where they were", () => {
  const node = { name: 'synth', w: 120, h: 100, slot: {}, ins: [{ name: 'gate' }, { name: 'a_very_long_parameter_name' }], outs: [{ name: 'out' }] };
  placeFace(node, { key: 'synth', file: 'parts/synth.html', error: null, count: 3 }, { w: FACE_W, h: 300 });
  assert.equal(node.face.dy, HEADER);
  assert.equal(node.face.h, 300);
  assert.ok(node.face.dx >= labelColumn(node.ins));
  assert.ok(node.face.dx + FACE_W <= node.w - labelColumn(node.outs));
  assert.ok(node.h >= HEADER + 300);
  // A name is cut at its column: a long one does not make the box huge.
  assert.ok(labelColumn(node.ins) <= labelColumn([{ name: 'x'.repeat(14) }]));
  // A small face never shrinks a box its pins make tall.
  const tall = { name: 's', w: 120, h: 900, ins: [], outs: [] };
  placeFace(tall, { count: 1 }, { w: FACE_W, h: 50 });
  assert.equal(tall.h, 900);
});

test('a face is estimated from its controls until measured; a measure is rounded up so a pixel lays nothing out again', () => {
  assert.ok(estimateFace(27).h > estimateFace(3).h);
  assert.equal(estimateFace(0).w, FACE_W);
  assert.equal(measuredHeight(0), null);
  assert.equal(measuredHeight(NaN), null);
  assert.equal(measuredHeight(1), FACE_STEP);
  assert.equal(measuredHeight(FACE_STEP * 10), FACE_STEP * 10);
  assert.equal(measuredHeight(FACE_STEP * 10 + 0.5), FACE_STEP * 11);
});

test("an opened slot's face is above its inside's nodes, centred on them", () => {
  const f = frameFace({ file: 'parts/synth.html', count: 2 }, { w: 400, h: 120 }, { x0: 0, x1: 600, y0: 10 });
  assert.equal(f.x, 100);
  assert.ok(f.y + f.h < 10);
  assert.equal(f.w, 400);
});

test("a face control's addresses are rooted at the slot drawn; a skin's relative URL is the project panel's", () => {
  assert.equal(faceAttribute('bind', '.cutoff', '/synth', BASE), '/synth.cutoff');
  assert.equal(faceAttribute('bind', '.a /vcf.b', '/rack1/fx', BASE), '/rack1/fx.a /rack1/fx/vcf.b');
  assert.equal(faceAttribute('label', '.cutoff', '/synth', BASE), '.cutoff', 'only address attributes');
  assert.equal(faceAttribute('skin', 'svg:assets/knob.svg', '/synth', BASE), 'svg:http://127.0.0.1:7000/project/assets/knob.svg');
  assert.equal(faceSkin('strip:assets/k.png;frames=64;2x=assets/k@2x.png', BASE), 'strip:http://127.0.0.1:7000/project/assets/k.png;frames=64;2x=http://127.0.0.1:7000/project/assets/k@2x.png');
  assert.equal(faceSkin('svg:/ui/assets/knob.svg', BASE), 'svg:/ui/assets/knob.svg', 'an absolute path is kept');
  assert.equal(faceSkin('svg:assets/knob.svg', null), 'svg:assets/knob.svg', 'no page to resolve from: as written');
  assert.equal(faceSkin('bogus', BASE), 'bogus');
  assert.equal(faceAttribute('skin', undefined, '/synth', BASE), undefined);
});

test("the project panel's style sheets style its faces, in order, comments skipped", () => {
  const page = `<!doctype html><head>
  <link rel="stylesheet" href="/ui/controls/theme.css">
  <!-- <link rel="stylesheet" href="old.css"> -->
  <link rel="icon" href="icon.png">
  <link href='studio.css' rel='stylesheet'>
  <link rel="stylesheet" href="studio.css">
  </head>`;
  assert.deepEqual(stylesheets(page, BASE), ['http://127.0.0.1:7000/ui/controls/theme.css', 'http://127.0.0.1:7000/project/studio.css']);
  assert.deepEqual(stylesheets('', BASE), []);
});

test("a wire into a parameter, or its pin, lights the face control bound to it", () => {
  const wire = { kind: 'wire', path: '', wire: { kind: 'connection', written: { from: 'lfo.out', to: 'synth.cutoff' } } };
  const hot = hotAddresses([wire]);
  assert.ok(hot.has('/synth.cutoff') && hot.has('/lfo.out'));
  assert.ok(lit('/synth.cutoff', hot));
  assert.ok(lit('/synth.drive /synth.cutoff', hot), 'one of several binds');
  assert.ok(!lit('/synth.drive', hot));
  assert.ok(!lit('/synth.cutoff', new Set()));
  // A pin of the box; in an opened slot, a frame's pin and an export wire from it.
  assert.ok(hotAddresses([{ kind: 'port', address: '/synth.cutoff' }]).has('/synth.cutoff'));
  const exported = { kind: 'wire', path: '/synth', wire: { kind: 'export', from: { node: '@inputs', port: 'cutoff' }, to: { node: 'vcf', port: 'cutoff' }, written: { from: 'cutoff', to: 'vcf.cutoff' } } };
  const inside = hotAddresses([exported]);
  assert.ok(inside.has('/synth.cutoff') && inside.has('/synth/vcf.cutoff'));
  const out = { kind: 'wire', path: '/synth', wire: { kind: 'export', from: { node: 'vcf', port: 'out' }, to: { node: '@outputs', port: 'out' }, written: { from: 'vcf.out', to: 'out' } } };
  assert.ok(hotAddresses([out]).has('/synth.out'));
  // Nothing and the background light nothing; nested wires keep their path.
  assert.equal(hotAddresses([null, { kind: 'background' }]).size, 0);
  assert.ok(hotAddresses([{ kind: 'wire', path: '/rack1', wire: { kind: 'connection', written: { from: 'a.out', to: 'fx.x' } } }]).has('/rack1/fx.x'));
});

test('a face goes over its area on screen, scaled with the view; too small, it is not shown', () => {
  const p = facePlacement({ x: 10, y: 20, w: FACE_W / 2, h: 100 });
  assert.equal(p.scale, 0.5);
  assert.equal(p.transform, 'translate(10px, 20px) scale(0.5)');
  assert.equal(facePlacement({ x: 0, y: 0, w: FACE_MIN_PX - 1, h: 10 }), null);
  assert.equal(facePlacement(null), null);
  assert.ok(overlapping({ x: 0, y: 0, w: 10, h: 10 }, { x: 5, y: 5, w: 10, h: 10 }));
  assert.ok(!overlapping({ x: 0, y: 0, w: 10, h: 10 }, { x: 10, y: 0, w: 10, h: 10 }));
});

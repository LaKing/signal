import test from 'node:test';
import assert from 'node:assert/strict';

import { bodiesList, chooseBackend, rgba, fitBox, gridSide, meshLines, meshProjection, pointsList, scaleSize, spriteFrame, spriteSource, toScreen, unit } from './canvas.js';

test('the box fits the canvas, centred, aspect kept, y up', () => {
  const fit = fitBox({ width: 400, height: 200 });
  assert.equal(fit.scale, 100);
  assert.deepEqual(toScreen(fit, 0, 0), [200, 100]);
  assert.deepEqual(toScreen(fit, -1, 1), [100, 0]);
  assert.deepEqual(toScreen(fit, 1, -1), [300, 200]);
});

test('device pixels and margins scale the fit', () => {
  const fit = fitBox({ width: 200, height: 200, ratio: 2, margin: 10 });
  assert.equal(fit.scale, 180);
  assert.deepEqual(toScreen(fit, 0, 0), [200, 200]);
  assert.deepEqual(toScreen(fit, 1, 1), [380, 20]);
  const off = fitBox({ width: 100, height: 100, box: [0, 0, 2, 1] });
  assert.equal(off.scale, 50);
  assert.deepEqual(toScreen(off, 1, 0.5), [50, 50]);
  assert.equal(fitBox({ width: 0, height: 100 }).scale, 0);
});

test('values scale to [0, 1] and to sizes, clamped', () => {
  assert.equal(unit(5, 0, 10), 0.5);
  assert.equal(unit(-3, 0, 10), 0);
  assert.equal(unit(30, 0, 10), 1);
  assert.equal(unit(NaN, 0, 10), 0);
  assert.equal(unit(1, 2, 2), 0);
  assert.equal(scaleSize(0.5, { min: 2, max: 6 }), 4);
  assert.equal(scaleSize(4, { min: 2, max: 6 }), 6);
  assert.equal(scaleSize(undefined), 2);
});

test('a sprite frame is chosen evenly over the life', () => {
  assert.equal(spriteFrame(0, 8), 0);
  assert.equal(spriteFrame(0.124, 8), 0);
  assert.equal(spriteFrame(0.125, 8), 1);
  assert.equal(spriteFrame(0.99, 8), 7);
  assert.equal(spriteFrame(1, 8), 7);
  assert.equal(spriteFrame(-1, 8), 0);
  assert.equal(spriteFrame(NaN, 8), 0);
  assert.equal(spriteFrame(0.7, 1), 0);
});

test('a sprite frame is found in its sheet', () => {
  // A horizontal strip of 8 frames of 32 × 32.
  assert.deepEqual(spriteSource({ index: 3, frames: 8, orientation: 'horizontal', imageWidth: 256, imageHeight: 32 }), { sx: 96, sy: 0, sw: 32, sh: 32 });
  // A 4 × 2 sheet.
  assert.deepEqual(spriteSource({ index: 5, frames: 8, columns: 4, imageWidth: 128, imageHeight: 64 }), { sx: 32, sy: 32, sw: 32, sh: 32 });
  assert.deepEqual(spriteSource({ index: 0, frames: 1, imageWidth: 16, imageHeight: 16 }), { sx: 0, sy: 0, sw: 16, sh: 16 });
});

test('only live particles are drawn', () => {
  const pos = Float32Array.from([0.5, -0.5, 1, 1, -1, 0]);
  const life = Float32Array.from([0.25, 0, 1]);
  assert.deepEqual(pointsList(pos, life), [
    { x: 0.5, y: -0.5, life: 0.25, age: 0.75 },
    { x: -1, y: 0, life: 1, age: 0 },
  ]);
  assert.equal(pointsList(pos).length, 3);
  assert.deepEqual(pointsList(null), []);
});

test('the mesh projects to a slanted grid with its edges', () => {
  assert.equal(gridSide(256), 16);
  assert.equal(gridSide(0), 1);
  const z = new Float32Array(9);
  z[4] = 1;
  const points = meshProjection(z, 3, { tilt: 0.5, height: 0.25 });
  assert.deepEqual(Array.from(points.slice(0, 2)), [-1, -0.5]);
  assert.deepEqual(Array.from(points.slice(8, 10)), [0, 0.25]);
  assert.deepEqual(Array.from(points.slice(16, 18)), [1, 0.5]);
  const lines = meshLines(3);
  assert.equal(lines.length, 2 * 12);
  assert.deepEqual(Array.from(lines.slice(0, 4)), [0, 1, 1, 2]);
});

test('bodies are read as circles and boxes', () => {
  const bodies = Float32Array.from([0.5, 0.25, 0.1, 0, -0.5, 0, 0.2, 1]);
  assert.deepEqual(bodiesList(bodies), [
    { x: 0.5, y: 0.25, size: Math.fround(0.1), box: false },
    { x: -0.5, y: 0, size: Math.fround(0.2), box: true },
  ]);
});

test('colours parse for WebGL', () => {
  assert.deepEqual(rgba('#fff'), [1, 1, 1, 1]);
  assert.deepEqual(rgba('#ff0000'), [1, 0, 0, 1]);
  assert.deepEqual(rgba('rgba(0, 255, 0, 0.5)'), [0, 1, 0, 0.5]);
  assert.deepEqual(rgba('nonsense', [0, 0, 0, 1]), [0, 0, 0, 1]);
});

test('WebGL draws unskinned points and meshes when there is one', () => {
  assert.equal(chooseBackend({ renderer: 'points', skinned: false, webgl: true }), 'webgl');
  assert.equal(chooseBackend({ renderer: 'mesh', skinned: false, webgl: false }), '2d');
  assert.equal(chooseBackend({ renderer: 'points', skinned: true, webgl: true }), '2d');
  assert.equal(chooseBackend({ renderer: 'bodies', skinned: false, webgl: true }), '2d');
  assert.equal(chooseBackend({ backend: '2d', renderer: 'mesh', skinned: false, webgl: true }), '2d');
});

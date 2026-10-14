import test from 'node:test';
import assert from 'node:assert/strict';

import { cleanCss, isSameOriginRef, parseSkin, resolveSkinUrl, sanitizeAttribute, stripFrame } from './skin.js';

test('skin attributes parse', () => {
  assert.equal(parseSkin(''), null);
  assert.equal(parseSkin(null), null);
  assert.deepEqual(parseSkin('svg:assets/knob.svg'), { kind: 'svg', url: 'assets/knob.svg' });
  assert.deepEqual(parseSkin('strip:assets/knob.png;frames=64'), {
    kind: 'strip', url: 'assets/knob.png', frames: 64, orientation: null, columns: null, scale: 1, hidpi: null,
  });
  const full = parseSkin('strip: a.png ; frames=8 ; orientation=horizontal ; 2x=a@2x.png');
  assert.equal(full.orientation, 'horizontal');
  assert.equal(full.hidpi, 'a@2x.png');
  assert.equal(parseSkin('strip:knob@2x.png;frames=4').scale, 2, '@2x in the name');
  assert.equal(parseSkin('strip:knob@2x.png;frames=4;scale=1').scale, 1);
  assert.equal(parseSkin('strip:sheet.png;frames=10;columns=4').columns, 4);
});

test('malformed skins are refused', () => {
  assert.throws(() => parseSkin('strip:a.png'), /frames=N/);
  assert.throws(() => parseSkin('strip:a.png;frames=0'), /at least 1/);
  assert.throws(() => parseSkin('strip:a.png;frames=2;orientation=diagonal'), /orientation/);
  assert.throws(() => parseSkin('strip:a.png;frames=2;size=3'), /unknown strip option/);
  assert.throws(() => parseSkin('png:a.png'), /unknown skin kind/);
  assert.throws(() => parseSkin('svg:'), /no URL/);
});

test('URLs stay as written: @2x is never percent-encoded', () => {
  assert.equal(parseSkin('strip:assets/knob-strip@2x.png;frames=2').url, 'assets/knob-strip@2x.png');
  assert.equal(resolveSkinUrl('assets/knob-strip@2x.png', 'http://127.0.0.1:8800/ui/gallery.html'), 'http://127.0.0.1:8800/ui/assets/knob-strip@2x.png');
  assert.equal(resolveSkinUrl('assets/knob@2x.png', 'file:///Users/me/signal/ui/gallery.html'), 'file:///Users/me/signal/ui/assets/knob@2x.png');
});

test('filmstrip frame selection: vertical and horizontal', () => {
  // 64 frames of 48 × 48, top to bottom.
  const vertical = { frames: 64, imageWidth: 48, imageHeight: 48 * 64 };
  assert.deepEqual(stripFrame({ ...vertical, position: 0 }), { index: 0, width: 48, height: 48, backgroundSize: '48px 3072px', backgroundPosition: '0px 0px' });
  assert.equal(stripFrame({ ...vertical, position: 1 }).index, 63);
  assert.equal(stripFrame({ ...vertical, position: 1 }).backgroundPosition, '0px -3024px');
  assert.equal(stripFrame({ ...vertical, position: 0.5 }).index, 32, '31.5 rounds up');
  assert.equal(stripFrame({ ...vertical, position: 0.49 }).index, 31);
  assert.equal(stripFrame({ ...vertical, position: -1 }).index, 0, 'clamped');
  assert.equal(stripFrame({ ...vertical, position: 7 }).index, 63, 'clamped');
  // Two frames side by side (a toggle), found from the image's shape.
  const horizontal = stripFrame({ frames: 2, imageWidth: 80, imageHeight: 24, position: 1 });
  assert.deepEqual([horizontal.index, horizontal.width, horizontal.height, horizontal.backgroundPosition], [1, 40, 24, '-40px 0px']);
  assert.equal(stripFrame({ frames: 4, orientation: 'horizontal', imageWidth: 40, imageHeight: 40, position: 1 }).backgroundPosition, '-30px 0px');
});

test('sprite sheet frame selection, row by row', () => {
  // 10 frames in 4 columns and 3 rows of 32 × 32.
  const sheet = { frames: 10, columns: 4, imageWidth: 128, imageHeight: 96 };
  const last = stripFrame({ ...sheet, position: 1 });
  assert.deepEqual([last.index, last.width, last.height, last.backgroundPosition], [9, 32, 32, '-32px -64px']);
  assert.equal(stripFrame({ ...sheet, position: 5 / 9 }).backgroundPosition, '-32px -32px');
});

test('an @2x image gives the same CSS geometry from twice the pixels', () => {
  const one = stripFrame({ frames: 64, imageWidth: 48, imageHeight: 3072, scale: 1, position: 0.3 });
  const two = stripFrame({ frames: 64, imageWidth: 96, imageHeight: 6144, scale: 2, position: 0.3 });
  assert.deepEqual(two, one);
});

test('SVG skin references stay on the host', () => {
  for (const ok of ['#pointer', 'parts.svg#p', 'assets/knob.png', '../assets/a.png', '/project/assets/a.png']) {
    assert.ok(isSameOriginRef(ok), ok);
  }
  for (const bad of ['http://example.com/a.png', 'https://cdn.example/x.svg', '//example.com/a.png', 'data:image/png;base64,AAAA', 'javascript:alert(1)', ' JavaScript:alert(1)', '', '\\\\host\\a']) {
    assert.ok(!isSameOriginRef(bad), bad);
  }
});

test('disguised schemes are refused (auditor probes)', () => {
  // A URL parser drops tabs and newlines: these are https: and javascript:.
  for (const bad of ['ht\ntps://example.com/a.png', 'java\tscript:alert(1)', 'java\rscript:alert(1)', '\u0001javascript:alert(1)', 'a:b/c.png']) {
    assert.ok(!isSameOriginRef(bad), JSON.stringify(bad));
  }
  // A CSS escape can spell url( (u\72l is url): such CSS is dropped whole.
  assert.equal(cleanCss('fill: u\\72l(https://example.com/a.svg)'), '');
  assert.equal(cleanCss('background: url(java\\73 cript:x)'), '');
  assert.equal(cleanCss('background: image-set("https://example.com/a.png" 1x)'), '');
  assert.equal(cleanCss('fill: u/**/rl(#ok)'), 'fill: url(#ok)', 'comments go first');
});

test('every SVG attribute is sanitized, presentation attributes included', () => {
  assert.equal(sanitizeAttribute('onload', 'alert(1)'), null);
  assert.equal(sanitizeAttribute('xml:base', 'https://example.com/'), null);
  assert.equal(sanitizeAttribute('href', 'java\tscript:alert(1)'), null);
  assert.equal(sanitizeAttribute('xlink:href', 'ht\ntps://example.com/a.png'), null);
  assert.equal(sanitizeAttribute('href', '#grad'), '#grad');
  assert.equal(sanitizeAttribute('fill', 'url(https://example.com/p.svg#g)'), 'none');
  assert.equal(sanitizeAttribute('filter', 'u\\72l(https://example.com/f.svg#f)'), null);
  assert.equal(sanitizeAttribute('mask', 'url(#m)'), 'url(#m)');
  assert.equal(sanitizeAttribute('style', 'fill: u\\72l(https://example.com/a.svg)'), null);
  assert.equal(sanitizeAttribute('fill', '#ff7043'), '#ff7043');
  assert.equal(sanitizeAttribute('data-travel', '180'), '180');
});

test('skin CSS loses imports and off-host url()s', () => {
  const css = '@import url("https://cdn.example/f.css"); @import "x.css"; .a { fill: url(#grad); background: url(https://example.com/a.png); mask: url("parts.svg#m"); }';
  const clean = cleanCss(css);
  assert.ok(!/@import/i.test(clean), clean);
  assert.ok(!/example/.test(clean), clean);
  assert.ok(clean.includes('url(#grad)'));
  assert.ok(clean.includes('url("parts.svg#m")'));
});

// M2.8-11 AC5: a panel is a grid of cards, and no text stands outside a
// card: the example projects' panels and the default panel (whose own text,
// the generated groups, the bar and the problems, are cards in
// controls/panel.js). Read from the panels' text, which they stay.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);

/** The text of `html`'s body that is not inside an element with the class `signal-card`. */
export function textOutsideCards(html) {
  const body = html.slice(html.indexOf('<body>') + 6, html.indexOf('</body>'));
  const source = body.replace(/<!--[\s\S]*?-->/g, '').replace(/<(script|style)\b[\s\S]*?<\/\1>/g, '');
  const stack = [];
  const outside = [];
  const tags = /<(\/?)([a-zA-Z][\w-]*)([^>]*)>/g;
  let at = 0;
  for (const m of source.matchAll(tags)) {
    const text = source.slice(at, m.index).trim();
    if (text && !stack.some((card) => card)) outside.push(text);
    at = m.index + m[0].length;
    const [, closing, name, attributes] = m;
    if (closing) stack.pop();
    else if (!VOID.has(name.toLowerCase()) && !attributes.trim().endsWith('/')) stack.push(/class="[^"]*\bsignal-card\b/.test(attributes));
  }
  const rest = source.slice(at).trim();
  if (rest) outside.push(rest);
  return outside;
}

test('the scan finds text outside a card', () => {
  assert.deepEqual(textOutsideCards('<body><header><h1>kick</h1></header><div class="signal-cards"><section class="signal-card"><h2>a</h2><br>b</section></div></body>'), ['kick']);
});

for (const panel of ['../default-panel.html', '../../projects/kick/ui/main.html', '../../projects/pulse/ui/main.html', '../../projects/physical-av/ui/main.html', '../../projects/studio/ui/main.html']) {
  test(`${panel.split('/').slice(-3).join('/')}: every text is in a card`, () => {
    const html = readFileSync(new URL(panel, import.meta.url), 'utf8');
    assert.deepEqual(textOutsideCards(html), []);
    if (!panel.includes('default-panel')) assert.match(html, /<div class="signal-cards">/, 'a grid of cards');
  });
}

test('the generated panel builds cards in a grid', () => {
  const generator = readFileSync(new URL('../controls/panel.js', import.meta.url), 'utf8');
  assert.match(generator, /\.bar, \.problems, \.generated section \{[^}]*background/, 'its bar, problems and groups are cards');
  assert.match(generator, /\.generated \{ display: grid;/);
});

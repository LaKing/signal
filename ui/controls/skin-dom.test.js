// The SVG skin sanitizer (ui/controls/skin-dom.js) on a stub tree of
// namespaced elements, as DOMParser gives them (review 2026-11-01,
// svg-sanitizer-prefixed-elements): elements and attributes are judged by
// namespace and local name, so a prefix hides nothing.
import test from 'node:test';
import assert from 'node:assert/strict';

import { sanitizeSvg } from './skin-dom.js';
import { SVG_NS, XLINK_NS, XML_NS, XMLNS_NS } from '../lib/skin.js';

const XHTML_NS = 'http://www.w3.org/1999/xhtml';

/** An element of `namespace` with qualified name `name` (`s:script`), its attributes `[namespace, qualified name, value]`. */
function element(namespace, name, attributes = [], children = []) {
  const node = {
    namespaceURI: namespace,
    localName: name.includes(':') ? name.slice(name.indexOf(':') + 1) : name,
    tagName: name,
    attributes: attributes.map(([ns, qualified, value]) => ({
      namespaceURI: ns,
      name: qualified,
      localName: qualified.includes(':') ? qualified.slice(qualified.indexOf(':') + 1) : qualified,
      value,
    })),
    children: [],
    parent: null,
    textContent: '',
    remove() {
      this.parent.children = this.parent.children.filter((c) => c !== this);
    },
    removeAttributeNode(attribute) {
      this.attributes = this.attributes.filter((a) => a !== attribute);
    },
  };
  for (const child of children) {
    child.parent = node;
    node.children.push(child);
  }
  return node;
}

const names = (node) => [node.tagName, ...node.children.flatMap(names)];
const attribute = (node, name) => node.attributes.find((a) => a.name === name)?.value;

test('prefixed script, iframe and foreignObject are removed, whatever the prefix', () => {
  const svg = element(SVG_NS, 'svg', [[XMLNS_NS, 'xmlns:s', SVG_NS], [XMLNS_NS, 'xmlns:h', XHTML_NS]], [
    element(SVG_NS, 's:script'),
    element(SVG_NS, 'script'),
    element(XHTML_NS, 'h:script'),
    element(XHTML_NS, 'h:iframe', [[null, 'srcdoc', '<script>parent.x=1</script>']]),
    element(SVG_NS, 's:foreignObject', [], [element(XHTML_NS, 'h:iframe', [[null, 'srcdoc', '<script>1</script>']])]),
    element(SVG_NS, 'foreignObject'),
    element(XHTML_NS, 'h:object'),
    element(XHTML_NS, 'h:embed'),
    element(SVG_NS, 's:animate', [[null, 'attributeName', 'href'], [null, 'to', 'javascript:alert(1)']]),
    element(SVG_NS, 'set'),
    element(SVG_NS, 'a', [[XLINK_NS, 'x:href', 'javascript:alert(1)']], [element(SVG_NS, 'circle')]),
    // A lookalike in no namespace or the wrong case is no SVG element either.
    element(null, 'circle'),
    element(SVG_NS, 'Circle'),
    element(SVG_NS, 's:g', [], [element(SVG_NS, 'circle', [[null, 'data-part', 'pointer']])]),
  ]);
  sanitizeSvg(svg);
  assert.deepEqual(names(svg), ['svg', 's:g', 'circle']);
  assert.equal(attribute(svg.children[0].children[0], 'data-part'), 'pointer');
  assert.equal(attribute(svg, 'xmlns:s'), SVG_NS, 'namespace declarations stay');
});

test('attributes are judged by namespace and local name: srcdoc, handlers and prefixed links go', () => {
  const use = element(SVG_NS, 'use', [
    [XLINK_NS, 'x:href', 'javascript:alert(1)'],
    [XLINK_NS, 'xlink:title', 'ok?'],
    [null, 'srcdoc', '<script>1</script>'],
    [null, 'onclick', 'alert(1)'],
    [null, 'ONload', 'alert(1)'],
    [XML_NS, 'xml:base', 'https://example.com/'],
    [XML_NS, 'xml:space', 'preserve'],
    ['http://www.inkscape.org/namespaces/inkscape', 'inkscape:label', 'knob'],
    ['http://evil.test/ns', 'e:href', 'javascript:alert(1)'],
    [null, 'fill', 'url(https://example.com/p.svg#g)'],
  ]);
  const local = element(SVG_NS, 'use', [[XLINK_NS, 'q:href', '#grad'], [null, 'href', '#grad']]);
  const svg = element(SVG_NS, 'svg', [], [use, local]);
  sanitizeSvg(svg);
  assert.deepEqual(
    use.attributes.map((a) => [a.name, a.value]),
    [['xml:space', 'preserve'], ['fill', 'none']],
  );
  assert.deepEqual(local.attributes.map((a) => [a.name, a.value]), [['q:href', '#grad'], ['href', '#grad']]);
});

test('only an image may name another file; every other reference is a fragment of the skin', () => {
  const use = element(SVG_NS, 'use', [[XLINK_NS, 'xlink:href', '/project/payload.svg#x'], [null, 'href', 'payload.svg#x']]);
  const fill = element(SVG_NS, 'rect', [[null, 'fill', 'url(other.svg#g)']]);
  const image = element(SVG_NS, 'image', [[null, 'href', 'assets/face.png']]);
  const fe = element(SVG_NS, 'feImage', [[XLINK_NS, 'xlink:href', 'assets/noise.png']]);
  const svg = element(SVG_NS, 'svg', [], [use, fill, image, fe]);
  sanitizeSvg(svg);
  assert.deepEqual(use.attributes, []);
  assert.equal(attribute(image, 'href'), 'assets/face.png');
  assert.equal(attribute(fe, 'xlink:href'), 'assets/noise.png');
});

test("a skin's style element keeps only same-host CSS", () => {
  const style = element(SVG_NS, 'style');
  style.textContent = '@import url(https://example.com/a.css); .k { fill: url(https://example.com/x.svg#g) }';
  const svg = element(SVG_NS, 'svg', [], [style]);
  sanitizeSvg(svg);
  assert.ok(!style.textContent.includes('example.com'), style.textContent);
});

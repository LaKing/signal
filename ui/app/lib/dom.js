// How Signal.app's pages set what they create an element with. Plain ES
// module, no DOM: it works on any object with `setAttribute`.

/** Whether `object[key] = …` would take: a data property, or one with a setter. */
export function assignable(object, key) {
  for (let o = object; o; o = Object.getPrototypeOf(o)) {
    const descriptor = Object.getOwnPropertyDescriptor(o, key);
    if (descriptor) return 'value' in descriptor ? descriptor.writable : Boolean(descriptor.set);
  }
  return false;
}

/**
 * Sets each of `props` on `element`: as a property where one can be set
 * (`className`, `textContent`, `value`, …), else as an attribute. A
 * read-only property (SignalMeter's `floor` has only a getter) gets the
 * attribute: assigning it throws in module code, and that once kept
 * Signal.app's parameter list empty while playing (M2.8-11).
 */
export function applyProps(element, props) {
  for (const [key, value] of Object.entries(props)) {
    if (assignable(element, key)) element[key] = value;
    else element.setAttribute(key, value);
  }
  return element;
}

import "@testing-library/jest-dom/vitest";

// jsdom does no layout, and some versions lack the Range measurement methods that xterm's link
// provider and CodeMirror call. Give them an empty answer instead of a TypeError.
if (typeof Range !== "undefined") {
  const emptyRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON: () => ({}) });
  const proto = Range.prototype as unknown as Record<string, unknown>;
  if (typeof proto.getClientRects !== "function") {
    proto.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} });
  }
  if (typeof proto.getBoundingClientRect !== "function") {
    proto.getBoundingClientRect = emptyRect;
  }
}

import { cleanup, render } from "@solidjs/testing-library";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import CodeEditor, { extensionToLanguage } from "./CodeEditor";

afterEach(() => {
  cleanup();
});

// No extra jsdom shims needed here. Older jsdom releases threw "not implemented" from
// Range.getClientRects/getBoundingClientRect during CM6's DOM-measuring pass, but this repo's
// jsdom (26.1.0, per src/test/setup.ts) already returns inert zeroed rects instead of throwing.
// CM6 core also never touches ResizeObserver itself - that's only used by TerminalPane/
// TerminalWorkspace to react to their own container resizes, not by the editor. Confirmed by
// running this suite with no shims at all: no console errors, all tests green.

function getView(container: HTMLElement): EditorView {
  const content = container.querySelector<HTMLElement>(".cm-content");
  if (!content) throw new Error("CodeEditor did not render a .cm-content node");
  const view = EditorView.findFromDOM(content);
  if (!view) throw new Error("EditorView.findFromDOM found no view for .cm-content");
  return view;
}

describe("extensionToLanguage", () => {
  it("resolves typescript", () => {
    expect(extensionToLanguage("src/app.ts")).toBe("typescript");
  });

  it("resolves tsx", () => {
    expect(extensionToLanguage("src/App.tsx")).toBe("tsx");
  });

  it("resolves javascript and its common aliases", () => {
    expect(extensionToLanguage("index.js")).toBe("javascript");
    expect(extensionToLanguage("index.mjs")).toBe("javascript");
    expect(extensionToLanguage("index.cjs")).toBe("javascript");
  });

  it("resolves jsx", () => {
    expect(extensionToLanguage("Widget.jsx")).toBe("jsx");
  });

  it("resolves rust", () => {
    expect(extensionToLanguage("crates/core/src/lib.rs")).toBe("rust");
  });

  it("resolves json", () => {
    expect(extensionToLanguage("package.json")).toBe("json");
  });

  it("resolves markdown", () => {
    expect(extensionToLanguage("README.md")).toBe("markdown");
  });

  it("resolves python", () => {
    expect(extensionToLanguage("scripts/build.py")).toBe("python");
  });

  it("resolves css", () => {
    expect(extensionToLanguage("src/index.css")).toBe("css");
  });

  it("resolves html", () => {
    expect(extensionToLanguage("public/index.html")).toBe("html");
  });

  it("falls back to plain for extensions with no language package (yaml, toml, shell)", () => {
    expect(extensionToLanguage("config.yaml")).toBe("plain");
    expect(extensionToLanguage("config.yml")).toBe("plain");
    expect(extensionToLanguage("Cargo.toml")).toBe("plain");
    expect(extensionToLanguage("scripts/deploy.sh")).toBe("plain");
  });

  it("falls back to plain for an unrecognized or missing extension", () => {
    expect(extensionToLanguage("Makefile")).toBe("plain");
    expect(extensionToLanguage("notes.xyz")).toBe("plain");
  });
});

describe("CodeEditor", () => {
  it("mounts with the given content", () => {
    const { container } = render(() => <CodeEditor value="const x = 1;" path="a.ts" />);
    expect(getView(container).state.doc.toString()).toBe("const x = 1;");
  });

  it("fires onChange with the updated content when the user types", () => {
    const onChange = vi.fn();
    const { container } = render(() => <CodeEditor value="abc" path="a.ts" onChange={onChange} />);
    const view = getView(container);

    // jsdom's contenteditable does not run a real input pipeline, so there is no DOM event that
    // reaches CM6's own beforeinput handler the way a real browser keystroke would. Dispatching a
    // transaction directly on the view exercises the exact same downstream path a real keystroke
    // takes (the transaction updates the doc, then EditorView.updateListener fires) - this is the
    // "dispatch via view" typing simulation the task calls for.
    view.dispatch({ changes: { from: 3, insert: "d" } });

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith("abcd");
  });

  it("does not fire onChange for the initial mount", () => {
    const onChange = vi.fn();
    render(() => <CodeEditor value="same" path="a.ts" onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("replaces the doc when the value prop changes externally, without firing onChange", () => {
    const onChange = vi.fn();
    let setValue!: (v: string) => void;
    function Harness() {
      const [value, set] = createSignal("first");
      setValue = set;
      return <CodeEditor value={value()} path="a.ts" onChange={onChange} />;
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);
    expect(view.state.doc.toString()).toBe("first");

    setValue("second, from outside");

    expect(view.state.doc.toString()).toBe("second, from outside");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not replace the doc (or move the cursor) when the value prop is set to the doc's own current content", () => {
    const onChange = vi.fn();
    let setValue!: (v: string) => void;
    function Harness() {
      const [value, set] = createSignal("abc");
      setValue = set;
      return <CodeEditor value={value()} path="a.ts" onChange={onChange} />;
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);

    // Simulate the app's normal round trip: user types, onChange updates the parent's signal,
    // which hands the exact same string straight back down as `value`. That echo must not be
    // re-dispatched as a doc replacement (it would needlessly reset undo grouping/selection).
    view.dispatch({ changes: { from: 3, insert: "d" } });
    expect(onChange).toHaveBeenCalledWith("abcd");
    setValue("abcd");

    expect(view.state.doc.toString()).toBe("abcd");
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("blocks edits when readOnly", () => {
    const onChange = vi.fn();
    const { container } = render(() => <CodeEditor value="locked" path="a.ts" onChange={onChange} readOnly />);
    const view = getView(container);

    // What actually stops a keystroke from editing the doc is `EditorState.readOnly`: every DOM
    // input pathway (typing, IME composition, paste, drop - see applyDOMChangeInner's callers in
    // @codemirror/view) checks `view.state.readOnly` before turning a browser edit event into a
    // transaction. jsdom doesn't run that real contenteditable input pipeline, so there is no
    // event we can fire here that would prove the block the way a real keystroke does; instead we
    // assert the two observable facts a real keystroke's guard is gated on: the facet is set, and
    // CM6 has published it to the DOM as `aria-readonly`.
    expect(view.state.readOnly).toBe(true);
    expect(view.contentDOM.getAttribute("aria-readonly")).toBe("true");
  });

  it("does not mark the editor read-only when the prop is omitted", () => {
    const { container } = render(() => <CodeEditor value="editable" path="a.ts" />);
    const view = getView(container);
    expect(view.state.readOnly).toBe(false);
    expect(view.contentDOM.getAttribute("aria-readonly")).toBeNull();
  });

  it("triggers onSave, and prevents the default browser action, for Mod-s", () => {
    const onSave = vi.fn();
    const { container } = render(() => <CodeEditor value="abc" path="a.ts" onSave={onSave} />);
    const content = container.querySelector(".cm-content") as HTMLElement;
    // CM6 resolves the platform-independent "Mod-" modifier from `navigator.platform` at keymap
    // build time (see `browser.mac` in @codemirror/view); jsdom reports an empty `platform`, so
    // it resolves "Mod" to Ctrl here rather than Cmd - only ctrlKey is set, to match exactly one
    // binding rather than an unintended Mod+Ctrl chord.
    const event = new KeyboardEvent("keydown", {
      key: "s",
      code: "KeyS",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    content.dispatchEvent(event);

    expect(onSave).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("does not trigger onSave for a plain 's' keypress", () => {
    const onSave = vi.fn();
    const { container } = render(() => <CodeEditor value="abc" path="a.ts" onSave={onSave} />);
    const content = container.querySelector(".cm-content") as HTMLElement;
    content.dispatchEvent(new KeyboardEvent("keydown", { key: "s", code: "KeyS", bubbles: true, cancelable: true }));
    expect(onSave).not.toHaveBeenCalled();
  });
});

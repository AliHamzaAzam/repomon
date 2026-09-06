import { readFileSync } from "node:fs";
import path from "node:path";
import { cleanup, render } from "@solidjs/testing-library";
import { CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { selectNextOccurrence } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import CodeEditor, {
  detectIndentUnit,
  documentWordCompletionSource,
  extensionToLanguage,
  matchSpecialFilename,
  sniffShebang,
} from "./CodeEditor";

afterEach(() => {
  cleanup();
});

function getView(container: HTMLElement): EditorView {
  const content = container.querySelector<HTMLElement>(".cm-content");
  if (!content) throw new Error("CodeEditor did not render a .cm-content node");
  const view = EditorView.findFromDOM(content);
  if (!view) throw new Error("EditorView.findFromDOM found no view for .cm-content");
  return view;
}

describe("extensionToLanguage and special files", () => {
  it("resolves typescript and tsx", () => {
    expect(extensionToLanguage("src/app.ts")).toBe("typescript");
    expect(extensionToLanguage("src/App.tsx")).toBe("tsx");
  });

  it("resolves javascript and its common aliases", () => {
    expect(extensionToLanguage("index.js")).toBe("javascript");
    expect(extensionToLanguage("index.mjs")).toBe("javascript");
    expect(extensionToLanguage("index.cjs")).toBe("javascript");
    expect(extensionToLanguage("Widget.jsx")).toBe("jsx");
  });

  it("resolves rust, json, markdown, python, css, html", () => {
    expect(extensionToLanguage("crates/core/src/lib.rs")).toBe("rust");
    expect(extensionToLanguage("package.json")).toBe("json");
    expect(extensionToLanguage("README.md")).toBe("markdown");
    expect(extensionToLanguage("scripts/build.py")).toBe("python");
    expect(extensionToLanguage("src/index.css")).toBe("css");
    expect(extensionToLanguage("public/index.html")).toBe("html");
  });

  it("resolves dynamic languages via language-data (yaml, toml, c, go)", () => {
    expect(extensionToLanguage("config.yaml")).toBe("yaml");
    expect(extensionToLanguage("config.yml")).toBe("yaml");
    expect(extensionToLanguage("Cargo.toml")).toBe("toml");
    expect(extensionToLanguage("main.go")).toBe("go");
    expect(extensionToLanguage("main.c")).toBe("c");
  });

  it("resolves special and extensionless filenames", () => {
    expect(matchSpecialFilename("Dockerfile")).toBe("Dockerfile");
    expect(matchSpecialFilename("docker/Containerfile")).toBe("Dockerfile");
    expect(matchSpecialFilename("Makefile")).toBe("Shell");
    expect(matchSpecialFilename("Justfile")).toBe("Shell");
    expect(matchSpecialFilename("Cargo.lock")).toBe("TOML");
    expect(matchSpecialFilename("package-lock.json")).toBe("JSON");
    expect(matchSpecialFilename("pnpm-lock.yaml")).toBe("YAML");
    expect(matchSpecialFilename(".gitignore")).toBe("Shell");
    expect(matchSpecialFilename(".env")).toBe("Shell");
    expect(matchSpecialFilename(".env.production")).toBe("Shell");

    expect(extensionToLanguage("Dockerfile")).toBe("dockerfile");
    expect(extensionToLanguage("Makefile")).toBe("shell");
    expect(extensionToLanguage("Cargo.lock")).toBe("toml");
  });

  it("sniffs shebangs from file content", () => {
    expect(sniffShebang("#!/usr/bin/env python3\nprint('hi')")).toBe("Python");
    expect(sniffShebang("#!/bin/bash\necho 1")).toBe("Shell");
    expect(sniffShebang("#!/usr/bin/env node\nconsole.log(1)")).toBe("JavaScript");
    expect(sniffShebang("#!/usr/bin/ruby\nputs 1")).toBe("Ruby");
    expect(sniffShebang("not a shebang")).toBeNull();

    expect(extensionToLanguage("script-without-ext", "#!/usr/bin/env python3\n")).toBe("python");
    expect(extensionToLanguage("script-without-ext", "#!/bin/sh\n")).toBe("shell");
  });

  it("respects language override", () => {
    expect(extensionToLanguage("script.py", undefined, "rust")).toBe("rust");
  });
});

describe("detectIndentUnit", () => {
  it("defaults based on file extension", () => {
    expect(detectIndentUnit("", "main.ts")).toBe("  ");
    expect(detectIndentUnit("", "lib.rs")).toBe("    ");
    expect(detectIndentUnit("", "main.go")).toBe("\t");
  });

  it("detects 2 spaces, 4 spaces, or tabs from content", () => {
    const twoSpaces = "function foo() {\n  const x = 1;\n  return x;\n}";
    expect(detectIndentUnit(twoSpaces, "unknown.txt")).toBe("  ");

    const fourSpaces = "def foo():\n    x = 1\n    return x\n";
    expect(detectIndentUnit(fourSpaces, "unknown.txt")).toBe("    ");

    const tabs = "func main() {\n\tprintln(1)\n}\n";
    expect(detectIndentUnit(tabs, "unknown.txt")).toBe("\t");
  });
});

describe("CodeEditor", () => {
  it("mounts with the given content", () => {
    const { container } = render(() => <CodeEditor value="const x = 1;" path="a.ts" />);
    expect(getView(container).state.doc.toString()).toBe("const x = 1;");
  });

  it("includes fold gutter and line numbers", () => {
    const { container } = render(() => <CodeEditor value="const x = 1;" path="a.ts" />);
    expect(container.querySelector(".cm-gutters")).toBeInTheDocument();
    expect(container.querySelector(".cm-lineNumbers")).toBeInTheDocument();
    expect(container.querySelector(".cm-foldGutter")).toBeInTheDocument();
  });

  it("fires onChange with the updated content when the user types", () => {
    const onChange = vi.fn();
    const { container } = render(() => <CodeEditor value="abc" path="a.ts" onChange={onChange} />);
    const view = getView(container);

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

  it("does not replace the doc when the value prop is set to the doc's own current content", () => {
    const onChange = vi.fn();
    let setValue!: (v: string) => void;
    function Harness() {
      const [value, set] = createSignal("abc");
      setValue = set;
      return <CodeEditor value={value()} path="a.ts" onChange={onChange} />;
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);

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

    expect(view.state.readOnly).toBe(true);
    expect(view.contentDOM.getAttribute("aria-readonly")).toBe("true");
  });

  it("triggers onSave, and prevents default, for Mod-s", () => {
    const onSave = vi.fn();
    const { container } = render(() => <CodeEditor value="abc" path="a.ts" onSave={onSave} />);
    const content = container.querySelector(".cm-content") as HTMLElement;
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

  it("toggles line wrapping dynamically", () => {
    let setWrap!: (w: boolean) => void;
    function Harness() {
      const [wrap, set] = createSignal(false);
      setWrap = set;
      return <CodeEditor value="hello world long text" path="a.ts" wrap={wrap()} />;
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);

    expect(view.contentDOM.classList.contains("cm-lineWrapping")).toBe(false);

    setWrap(true);
    expect(view.contentDOM.classList.contains("cm-lineWrapping")).toBe(true);
  });

  it("reports cursor activity and scroll on selection changes", () => {
    const onCursor = vi.fn();
    const { container } = render(() => <CodeEditor value="hello world" path="a.ts" onCursorActivity={onCursor} />);
    const view = getView(container);

    view.dispatch({ selection: { anchor: 5 } });
    expect(onCursor).toHaveBeenCalledWith(5, expect.any(Number), { rangeCount: 1, selectedChars: 0 });
  });

  it("reports multiple selection ranges and selected character count", () => {
    const onCursor = vi.fn();
    const { container } = render(() => (
      <CodeEditor value="hello world" path="a.ts" onCursorActivity={onCursor} />
    ));
    const view = getView(container);

    view.dispatch({
      selection: {
        anchor: 0,
        head: 5,
      },
    });
    expect(onCursor).toHaveBeenLastCalledWith(5, expect.any(Number), { rangeCount: 1, selectedChars: 5 });
  });

  it("supports multi-cursor: Mod-d (selectNextOccurrence) twice yields two selection ranges", () => {
    const { container } = render(() => <CodeEditor value="foo bar foo baz foo" path="a.ts" />);
    const view = getView(container);

    // Place a plain cursor inside the first "foo".
    view.dispatch({ selection: { anchor: 1 } });

    // First Mod-d selects the word under the cursor - still a single range.
    selectNextOccurrence(view);
    expect(view.state.selection.ranges.length).toBe(1);

    // Second Mod-d adds the next occurrence of "foo" as an additional range. Without
    // `EditorState.allowMultipleSelections.of(true)`, CodeMirror silently reduces this back to a
    // single range.
    selectNextOccurrence(view);
    expect(view.state.selection.ranges.length).toBe(2);
  });

  it("restores cursor and scroll position reactively when the active file (path) changes, not just on mount", () => {
    let setFile!: (f: { path: string; value: string; cursor?: number; scrollTop?: number }) => void;
    function Harness() {
      const [file, set] = createSignal({ path: "a.ts", value: "const a = 1;", cursor: 6, scrollTop: 0 });
      setFile = set;
      return (
        <CodeEditor
          value={file().value}
          path={file().path}
          initialCursor={file().cursor}
          initialScrollTop={file().scrollTop}
        />
      );
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);

    expect(view.state.selection.main.head).toBe(6);

    // Switching to a second file (same CodeEditor instance, as in the center workspace and the
    // rail panel after the non-keyed Show fix) must re-apply the new file's saved cursor - not
    // just the first file's, which only ever ran in onMount.
    setFile({ path: "b.ts", value: "function longer() { return 2; }", cursor: 20, scrollTop: 0 });
    expect(view.state.doc.toString()).toBe("function longer() { return 2; }");
    expect(view.state.selection.main.head).toBe(20);
  });
});

describe("documentWordCompletionSource (item 7: document-word completion)", () => {
  it("offers document words of 3+ characters, excluding shorter words", () => {
    const state = EditorState.create({ doc: "world worldwide hi wo" });
    const pos = state.doc.length;
    const context = new CompletionContext(state, pos, false);

    const result = documentWordCompletionSource(context) as CompletionResult | null;

    expect(result).not.toBeNull();
    const labels = result!.options.map((o) => o.label);
    expect(labels).toContain("world");
    expect(labels).toContain("worldwide");
    // "hi" is a real document word but shorter than the 3-character minimum.
    expect(labels).not.toContain("hi");
    expect(labels.every((l) => l.length >= 3)).toBe(true);
  });

  it("returns null with no content and no explicit request", () => {
    const state = EditorState.create({ doc: "" });
    const context = new CompletionContext(state, 0, false);
    expect(documentWordCompletionSource(context)).toBeNull();
  });
});

describe("CodeEditor theme (item 1: theme regression)", () => {
  it("uses no hard-coded hex color literals - every color is a CSS variable", () => {
    const src = readFileSync(path.resolve(process.cwd(), "src/components/CodeEditor.tsx"), "utf-8");
    expect(src).not.toMatch(/#[0-9a-fA-F]{3,6}\b/);
  });
});

describe("large files read-only mode (item F6)", () => {
  it("configures readOnly and disables fold and git gutters when large is true, and restores on file switch", () => {
    let setProps!: (p: { value: string; path: string; large?: boolean }) => void;
    function Harness() {
      const [props, set] = createSignal<{ value: string; path: string; large?: boolean }>({
        value: "line 1\nline 2",
        path: "big.txt",
        large: true,
      });
      setProps = set;
      return (
        <CodeEditor
          value={props().value}
          path={props().path}
          large={props().large}
        />
      );
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);

    expect(view.state.readOnly).toBe(true);
    expect(container.querySelector(".cm-foldGutter")).toBeNull();
    expect(container.querySelector(".cm-git-diff-gutter")).toBeNull();

    setProps({ value: "line 1\nline 2", path: "small.txt", large: false });
    expect(view.state.readOnly).toBe(false);
    expect(container.querySelector(".cm-foldGutter")).not.toBeNull();
    expect(container.querySelector(".cm-git-diff-gutter")).not.toBeNull();
  });
});

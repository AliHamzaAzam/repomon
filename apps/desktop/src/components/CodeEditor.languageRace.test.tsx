import { cleanup, render } from "@solidjs/testing-library";
import { language } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/// Regression coverage for item 4: the language-resolution effect in CodeEditor.tsx must depend
/// only on `path`/`languageOverride` (never re-running on every keystroke) and must drop an async
/// `resolveLanguageSupport` result that resolves after a newer request has already started - e.g.
/// the user switched files again before the first file's dynamic language package finished
/// loading. This mocks `@codemirror/language-data` with fake, externally-controlled language
/// descriptions so the test can resolve their loads in a deliberately out-of-order sequence and
/// assert the stale one never wins - hence its own file, isolated from CodeEditor.test.tsx's real
/// (unmocked) `@codemirror/language-data` usage.
///
/// Three distinct fake languages (not two) are used across the two tests below rather than
/// re-using extensions between tests: both CodeMirror's own `LanguageDescription` (caches
/// `support` once loaded) and CodeEditor.tsx's own module-level `languageCache` persist for the
/// life of this test file, so re-using an already-resolved language name in a later test would
/// resolve synchronously from cache instead of exercising the async path under test.
const langLoaders = vi.hoisted(() => {
  const resolvers: Record<string, () => void> = {};
  return { resolvers };
});

vi.mock("@codemirror/language-data", async () => {
  const { LanguageDescription } = await import("@codemirror/language");
  const { javascript } = await import("@codemirror/lang-javascript");
  const { python } = await import("@codemirror/lang-python");
  const { css } = await import("@codemirror/lang-css");
  type LanguageSupport = ReturnType<typeof javascript>;

  function fakeDescription(name: string, ext: string, support: () => LanguageSupport) {
    return LanguageDescription.of({
      name,
      extensions: [ext],
      load: () =>
        new Promise<LanguageSupport>((resolve) => {
          langLoaders.resolvers[name] = () => resolve(support());
        }),
    });
  }

  return {
    languages: [
      fakeDescription("FakeAlpha", "fakealpha", javascript),
      fakeDescription("FakeBeta", "fakebeta", python),
      fakeDescription("FakeGamma", "fakegamma", css),
    ],
  };
});

// Imported after the mock so CodeEditor.tsx picks up the fake `languages` array.
const { default: CodeEditor } = await import("./CodeEditor");

beforeEach(() => {
  for (const key of Object.keys(langLoaders.resolvers)) {
    delete langLoaders.resolvers[key];
  }
});

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

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("CodeEditor language resolution race (item 4)", () => {
  it("drops a stale async language load that resolves after a newer file was opened", async () => {
    let setPath!: (p: string) => void;
    function Harness() {
      const [path, set] = createSignal("first.fakealpha");
      setPath = set;
      return <CodeEditor value="" path={path()} />;
    }
    const { container } = render(() => <Harness />);
    const view = getView(container);
    await flush();

    // Mounting kicked off the (still-pending) FakeAlpha load.
    expect(langLoaders.resolvers.FakeAlpha).toBeDefined();

    // Switch files before FakeAlpha resolves - kicks off the (pending) FakeBeta load. This is
    // now the newer, "current" request.
    setPath("second.fakebeta");
    await flush();
    expect(langLoaders.resolvers.FakeBeta).toBeDefined();

    // Resolve the OLDER request (FakeAlpha) now, after the newer one has already started - out
    // of order. It must not win: the doc is showing "second.fakebeta" now, not "first.fakealpha".
    langLoaders.resolvers.FakeAlpha();
    await flush();
    expect(view.state.facet(language)?.name).not.toBe("javascript");

    // Resolving the newer, still-current request must apply its language.
    langLoaders.resolvers.FakeBeta();
    await flush();
    expect(view.state.facet(language)?.name).toBe("python");
  });

  it("does not re-run language resolution on every keystroke (depends on path, not content)", async () => {
    let setValue!: (v: string) => void;
    function Harness() {
      const [value, set] = createSignal("a");
      setValue = set;
      return <CodeEditor value={value()} path="only.fakegamma" />;
    }
    render(() => <Harness />);
    await flush();

    // Resolve the one-and-only load this file's path should ever trigger.
    expect(Object.keys(langLoaders.resolvers)).toEqual(["FakeGamma"]);
    langLoaders.resolvers.FakeGamma();
    await flush();

    // Typing (content changes, path does not) must not start a second language request - the
    // resolver table gains no new entries.
    setValue("ab");
    setValue("abc");
    await flush();

    expect(Object.keys(langLoaders.resolvers)).toEqual(["FakeGamma"]);
  });
});

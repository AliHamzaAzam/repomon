import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import MarkdownPreview from "./MarkdownPreview";
import MarkdownRenderer from "./MarkdownRenderer";
import { parseMarkdown } from "./parser";

const openUrlMock = vi.fn().mockResolvedValue(undefined);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (url: string) => openUrlMock(url),
}));

const daemonCallMock = vi.fn().mockResolvedValue({
  mime: "image/png",
  base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  size: 68,
});
vi.mock("../../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => daemonCallMock(method, params),
}));

afterEach(() => {
  cleanup();
  openUrlMock.mockReset();
  openUrlMock.mockResolvedValue(undefined);
  daemonCallMock.mockReset();
  daemonCallMock.mockResolvedValue({
    mime: "image/png",
    base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    size: 68,
  });
});

const FIXTURE_MARKDOWN = `
# Main Document Title

This is an introductory paragraph with **bold text**, *italic text*, ***bold and italic***, and ~~strikethrough text~~.
Here is some inline \`const answer = 42;\` code.

## Section Level 2

### Section Level 3

#### Section Level 4

##### Section Level 5

###### Section Level 6

> This is a blockquote.
> > And this is a nested blockquote.

---

### Lists and Tasks

- [ ] Incomplete task item
- [x] Completed task item
- Regular unordered item 1
- Regular unordered item 2

1. First ordered item
2. Second ordered item

### Fenced Code Blocks

\`\`\`rust
fn calculate_metrics(count: usize) -> usize {
    count * 2
}
\`\`\`

\`\`\`unknownlang
plain text code block
\`\`\`

### Links and Media

Visit [Repomon Project](https://github.com/example/repomon) or [Local Section](#section-level-2).

![Remote Graphic](https://example.com/logo.png "Remote Logo")
![Local Diagram](./assets/diagram.png "Local Diagram")

### Data Table

| Feature | Support | Priority |
| :--- | :---: | ---: |
| Editor | Full | High |
| Preview | Full | Medium |

### Security Edge Case

<script>alert('xss')</script>
<img src="x" onerror="window.pwned=true" />
`;

describe("MarkdownRenderer & MarkdownPreview", () => {
  it("renders all fixture elements without error", async () => {
    daemonCallMock.mockResolvedValue({
      mime: "image/png",
      base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
      size: 68,
    });

    const parsed = parseMarkdown(FIXTURE_MARKDOWN);
    const { container } = render(() => (
      <MarkdownRenderer
        ast={parsed.ast}
        filePath="docs/guide.md"
        laneId={1}
      />
    ));

    // Headings 1 through 6
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("Main Document Title");
    expect(screen.getByRole("heading", { level: 2 }).textContent).toContain("Section Level 2");
    expect(screen.getAllByRole("heading", { level: 3 }).length).toBeGreaterThan(0);
    expect(screen.getByRole("heading", { level: 4 }).textContent).toContain("Section Level 4");
    expect(screen.getByRole("heading", { level: 5 }).textContent).toContain("Section Level 5");
    expect(screen.getByRole("heading", { level: 6 }).textContent).toContain("Section Level 6");

    // Heading slug IDs for scroll sync
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.id).toBe("main-document-title");
    expect(h1.getAttribute("data-heading-line")).toBeTruthy();

    const h2 = screen.getByRole("heading", { level: 2 });
    expect(h2.id).toBe("section-level-2");

    // Inline styles: bold, italic, strikethrough, code
    expect(container.querySelector("strong")?.textContent).toBe("bold text");
    expect(container.querySelector("em")?.textContent).toBe("italic text");
    expect(container.querySelector("del")?.textContent).toBe("strikethrough text");
    expect(container.querySelector("code")?.textContent).toContain("const answer = 42;");

    // Blockquote
    const bq = container.querySelector("blockquote");
    expect(bq).toBeTruthy();
    expect(bq?.textContent).toContain("This is a blockquote");

    // Thematic break
    expect(container.querySelector("hr")).toBeTruthy();

    // Task list items with checkboxes
    const checkboxes = container.querySelectorAll<HTMLInputElement>("input[type='checkbox']");
    expect(checkboxes.length).toBe(2);
    expect(checkboxes[0].checked).toBe(false);
    expect(checkboxes[0].disabled).toBe(true);
    expect(checkboxes[1].checked).toBe(true);
    expect(checkboxes[1].disabled).toBe(true);

    // Lists: ordered and unordered
    expect(container.querySelector("ul")).toBeTruthy();
    expect(container.querySelector("ol")).toBeTruthy();

    // Table
    const table = container.querySelector("table");
    expect(table).toBeTruthy();
    const ths = table?.querySelectorAll("th");
    expect(ths?.length).toBe(3);
    expect(ths?.[0].textContent).toBe("Feature");
    expect(ths?.[0].className).toContain("text-left");
    expect(ths?.[1].className).toContain("text-center");
    expect(ths?.[2].className).toContain("text-right");

    // Links: external link click calls openUrl
    const extLink = screen.getByText("Repomon Project");
    fireEvent.click(extLink);
    expect(openUrlMock).toHaveBeenCalledWith("https://github.com/example/repomon");

    // Local image resolved through daemonCall("file.read_raw")
    await waitFor(() => {
      expect(daemonCallMock).toHaveBeenCalledWith("file.read_raw", {
        lane_id: 1,
        path: "docs/assets/diagram.png",
      });
    });

    // Sanitization: script and onerror image are plain text, not executable DOM elements
    expect(container.querySelectorAll("script").length).toBe(0);
    expect(container.textContent).toContain("<script>alert('xss')</script>");
    expect(container.textContent).toContain('<img src="x" onerror="window.pwned=true" />');
  });

  it("handles MarkdownPreview nearestHeading scrolling smoothly", () => {
    const scrollIntoViewMock = vi.fn();
    window.HTMLElement.prototype.scrollIntoView = scrollIntoViewMock;

    const { container } = render(() => (
      <MarkdownPreview
        content={FIXTURE_MARKDOWN}
        filePath="docs/guide.md"
        laneId={1}
        nearestHeading="section-level-2"
      />
    ));

    expect(container.querySelector("[data-testid='markdown-preview']")).toBeTruthy();
    expect(scrollIntoViewMock).toHaveBeenCalled();
  });
});

import { describe, expect, it } from "vitest";
import {
  findNearestHeading,
  parseInline,
  parseMarkdown,
  slugifyHeading,
} from "./parser";

describe("markdown parser", () => {
  it("slugifies headings correctly with deduplication", () => {
    const counts = new Map<string, number>();
    expect(slugifyHeading("Getting Started!", counts)).toBe("getting-started");
    expect(slugifyHeading("Getting Started!", counts)).toBe("getting-started-1");
    expect(slugifyHeading("`code` & [links](url)", counts)).toBe("code-links");
  });

  it("finds the nearest heading based on line numbers", () => {
    const doc = `
# Title

Paragraph here.

## Section 1

Some text.

### Subsection A

More text.

## Section 2
`;
    const { headings } = parseMarkdown(doc);
    expect(headings.length).toBe(4);
    expect(headings[0].slug).toBe("title");
    expect(headings[1].slug).toBe("section-1");
    expect(headings[2].slug).toBe("subsection-a");
    expect(headings[3].slug).toBe("section-2");

    expect(findNearestHeading(headings, 1)).toBe("title");
    expect(findNearestHeading(headings, 5)).toBe("title");
    expect(findNearestHeading(headings, 6)).toBe("section-1");
    expect(findNearestHeading(headings, 10)).toBe("subsection-a");
    expect(findNearestHeading(headings, 14)).toBe("section-2");
    expect(findNearestHeading(headings, 99)).toBe("section-2");
  });

  it("parses inline formatting including bold, italic, code, links, images, strikethrough", () => {
    const text = "Hello **bold** *italic* `code` ~~deleted~~ [link](https://example.com) ![alt](img.png)";
    const nodes = parseInline(text);
    expect(nodes).toEqual([
      { type: "text", text: "Hello " },
      { type: "bold", children: [{ type: "text", text: "bold" }] },
      { type: "text", text: " " },
      { type: "italic", children: [{ type: "text", text: "italic" }] },
      { type: "text", text: " " },
      { type: "inlineCode", code: "code" },
      { type: "text", text: " " },
      { type: "strikethrough", children: [{ type: "text", text: "deleted" }] },
      { type: "text", text: " " },
      {
        type: "link",
        url: "https://example.com",
        title: undefined,
        children: [{ type: "text", text: "link" }],
      },
      { type: "text", text: " " },
      { type: "image", src: "img.png", alt: "alt", title: undefined },
    ]);
  });

  it("sanitizes dangerous javascript: links", () => {
    const text = "[evil](javascript:alert(1))";
    const nodes = parseInline(text);
    expect(nodes[0]).toEqual({
      type: "link",
      url: "#",
      title: undefined,
      children: [{ type: "text", text: "evil" }],
    });
  });

  it("parses fenced code blocks, tables, blockquotes, lists, and task lists", () => {
    const doc = `
\`\`\`rust
fn hello() {
    println!("hi");
}
\`\`\`

> Quote block
> with continuation

- [ ] Task 1
- [x] Task 2
- Normal item

1. First
2. Second

| Header A | Header B |
| :--- | ---: |
| Left | Right |

---
`;
    const { ast } = parseMarkdown(doc);
    expect(ast.length).toBe(6);

    // Code block
    expect(ast[0].type).toBe("codeBlock");
    if (ast[0].type === "codeBlock") {
      expect(ast[0].language).toBe("rust");
      expect(ast[0].code).toContain("fn hello()");
    }

    // Blockquote
    expect(ast[1].type).toBe("blockquote");

    // Task list
    expect(ast[2].type).toBe("list");
    if (ast[2].type === "list") {
      expect(ast[2].ordered).toBe(false);
      expect(ast[2].items[0].task).toEqual({ checked: false });
      expect(ast[2].items[1].task).toEqual({ checked: true });
      expect(ast[2].items[2].task).toBeUndefined();
    }

    // Ordered list
    expect(ast[3].type).toBe("list");
    if (ast[3].type === "list") {
      expect(ast[3].ordered).toBe(true);
    }

    // Table
    expect(ast[4].type).toBe("table");
    if (ast[4].type === "table") {
      expect(ast[4].alignments).toEqual(["left", "right"]);
      expect(ast[4].rows.length).toBe(1);
    }

    // Thematic break
    expect(ast[5].type).toBe("thematicBreak");
  });
});

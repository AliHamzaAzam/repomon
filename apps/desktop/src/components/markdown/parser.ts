export type Alignment = "left" | "center" | "right" | null;

export interface HeadingNode {
  type: "heading";
  level: 1 | 2 | 3 | 4 | 5 | 6;
  text: string;
  slug: string;
  line: number;
  children: InlineNode[];
}

export interface ParagraphNode {
  type: "paragraph";
  children: InlineNode[];
}

export interface BlockquoteNode {
  type: "blockquote";
  children: BlockNode[];
}

export interface CodeBlockNode {
  type: "codeBlock";
  language: string;
  code: string;
}

export interface ListItemNode {
  task?: { checked: boolean };
  children: InlineNode[];
  subList?: ListNode;
}

export interface ListNode {
  type: "list";
  ordered: boolean;
  start?: number;
  items: ListItemNode[];
}

export interface TableNode {
  type: "table";
  headers: InlineNode[][];
  alignments: Alignment[];
  rows: InlineNode[][][];
}

export interface ThematicBreakNode {
  type: "thematicBreak";
}

export type BlockNode =
  | HeadingNode
  | ParagraphNode
  | BlockquoteNode
  | CodeBlockNode
  | ListNode
  | TableNode
  | ThematicBreakNode;

export interface TextNode {
  type: "text";
  text: string;
}

export interface BoldNode {
  type: "bold";
  children: InlineNode[];
}

export interface ItalicNode {
  type: "italic";
  children: InlineNode[];
}

export interface BoldItalicNode {
  type: "boldItalic";
  children: InlineNode[];
}

export interface StrikethroughNode {
  type: "strikethrough";
  children: InlineNode[];
}

export interface InlineCodeNode {
  type: "inlineCode";
  code: string;
}

export interface LinkNode {
  type: "link";
  url: string;
  title?: string;
  children: InlineNode[];
}

export interface ImageNode {
  type: "image";
  src: string;
  alt: string;
  title?: string;
}

export type InlineNode =
  | TextNode
  | BoldNode
  | ItalicNode
  | BoldItalicNode
  | StrikethroughNode
  | InlineCodeNode
  | LinkNode
  | ImageNode;

export function slugifyHeading(text: string, counts?: Map<string, number>): string {
  const clean = text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/[*_~]+/g, "")
    .trim();

  let slug = clean
    .toLowerCase()
    .replace(/[^\w\s-]/g, "")
    .replace(/[\s_-]+/g, "-")
    .replace(/^-+|-+$/g, "");

  if (!slug) slug = "heading";

  if (counts) {
    const existing = counts.get(slug) ?? 0;
    counts.set(slug, existing + 1);
    if (existing > 0) {
      return `${slug}-${existing}`;
    }
  }

  return slug;
}

export function findNearestHeading(headings: HeadingNode[], currentLine: number): string | null {
  if (headings.length === 0) return null;
  let nearest: HeadingNode = headings[0];
  for (const h of headings) {
    if (h.line <= currentLine) {
      nearest = h;
    } else {
      break;
    }
  }
  return nearest.slug;
}

export function sanitizeUrl(url: string): string {
  const trimmed = url.trim();
  const lower = trimmed.toLowerCase();
  if (
    lower.startsWith("javascript:") ||
    lower.startsWith("vbscript:") ||
    lower.startsWith("data:text/html")
  ) {
    return "#";
  }
  return trimmed;
}

export function parseInline(text: string): InlineNode[] {
  const nodes: InlineNode[] = [];
  let i = 0;
  const len = text.length;

  function appendText(str: string) {
    if (!str) return;
    const last = nodes[nodes.length - 1];
    if (last && last.type === "text") {
      last.text += str;
    } else {
      nodes.push({ type: "text", text: str });
    }
  }

  while (i < len) {

    if (text[i] === "`") {
      let backticks = 1;
      while (i + backticks < len && text[i + backticks] === "`") {
        backticks++;
      }
      const marker = "`".repeat(backticks);
      const closeIdx = text.indexOf(marker, i + backticks);
      if (closeIdx !== -1) {
        const code = text.slice(i + backticks, closeIdx);
        nodes.push({ type: "inlineCode", code });
        i = closeIdx + backticks;
        continue;
      }
    }

    if (text[i] === "!" && i + 1 < len && text[i + 1] === "[") {
      const altClose = text.indexOf("]", i + 2);
      if (altClose !== -1 && altClose + 1 < len && text[altClose + 1] === "(") {
        const parenClose = text.indexOf(")", altClose + 2);
        if (parenClose !== -1) {
          const alt = text.slice(i + 2, altClose);
          const fullSrc = text.slice(altClose + 2, parenClose).trim();
          let src = fullSrc;
          let title: string | undefined;
          const titleMatch = /^(.*?)\s+["'](.*)["']$/.exec(fullSrc);
          if (titleMatch) {
            src = titleMatch[1];
            title = titleMatch[2];
          }
          nodes.push({ type: "image", src: sanitizeUrl(src), alt, title });
          i = parenClose + 1;
          continue;
        }
      }
    }

    if (text[i] === "[") {
      const textClose = text.indexOf("]", i + 1);
      if (textClose !== -1 && textClose + 1 < len && text[textClose + 1] === "(") {
        const parenClose = text.indexOf(")", textClose + 2);
        if (parenClose !== -1) {
          const linkText = text.slice(i + 1, textClose);
          const fullUrl = text.slice(textClose + 2, parenClose).trim();
          let url = fullUrl;
          let title: string | undefined;
          const titleMatch = /^(.*?)\s+["'](.*)["']$/.exec(fullUrl);
          if (titleMatch) {
            url = titleMatch[1];
            title = titleMatch[2];
          }
          nodes.push({
            type: "link",
            url: sanitizeUrl(url),
            title,
            children: parseInline(linkText),
          });
          i = parenClose + 1;
          continue;
        }
      }
    }

    if (text.startsWith("~~", i)) {
      const closeIdx = text.indexOf("~~", i + 2);
      if (closeIdx !== -1) {
        const inner = text.slice(i + 2, closeIdx);
        nodes.push({ type: "strikethrough", children: parseInline(inner) });
        i = closeIdx + 2;
        continue;
      }
    }

    if (text.startsWith("***", i) || text.startsWith("___", i)) {
      const marker = text.slice(i, i + 3);
      const closeIdx = text.indexOf(marker, i + 3);
      if (closeIdx !== -1) {
        const inner = text.slice(i + 3, closeIdx);
        nodes.push({ type: "boldItalic", children: parseInline(inner) });
        i = closeIdx + 3;
        continue;
      }
    }

    if (text.startsWith("**", i) || text.startsWith("__", i)) {
      const marker = text.slice(i, i + 2);
      const closeIdx = text.indexOf(marker, i + 2);
      if (closeIdx !== -1) {
        const inner = text.slice(i + 2, closeIdx);
        nodes.push({ type: "bold", children: parseInline(inner) });
        i = closeIdx + 2;
        continue;
      }
    }

    if (text[i] === "*" || text[i] === "_") {
      const marker = text[i];
      // For underscore, only trigger if not inside a word
      const prevChar = i > 0 ? text[i - 1] : " ";
      const isWordChar = (c: string) => /\w/.test(c);
      if (marker === "_" && isWordChar(prevChar)) {
        appendText(text[i]);
        i++;
        continue;
      }

      const closeIdx = text.indexOf(marker, i + 1);
      if (closeIdx !== -1) {
        const nextChar = closeIdx + 1 < len ? text[closeIdx + 1] : " ";
        if (marker === "_" && isWordChar(nextChar)) {
          appendText(text[i]);
          i++;
          continue;
        }
        const inner = text.slice(i + 1, closeIdx);
        if (inner.length > 0) {
          nodes.push({ type: "italic", children: parseInline(inner) });
          i = closeIdx + 1;
          continue;
        }
      }
    }

    appendText(text[i]);
    i++;
  }

  return nodes;
}

export function parseMarkdown(content: string): { ast: BlockNode[]; headings: HeadingNode[] } {
  const lines = content.split(/\r?\n/);
  const ast: BlockNode[] = [];
  const headings: HeadingNode[] = [];
  const slugCounts = new Map<string, number>();

  let i = 0;
  const numLines = lines.length;

  while (i < numLines) {
    const line = lines[i];

    if (line.trim() === "") {
      i++;
      continue;
    }

    const codeBlockMatch = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (codeBlockMatch) {
      const fence = codeBlockMatch[1];
      const char = fence[0];
      const minLength = fence.length;
      const language = codeBlockMatch[2].trim().split(/\s+/)[0] || "";
      const codeLines: string[] = [];
      i++;
      while (i < numLines) {
        const curLine = lines[i];
        const endMatch = new RegExp(`^ {0,3}${char}{${minLength},}\\s*$`).exec(curLine);
        if (endMatch) {
          i++;
          break;
        }
        codeLines.push(curLine);
        i++;
      }
      ast.push({
        type: "codeBlock",
        language,
        code: codeLines.join("\n"),
      });
      continue;
    }

    const headingMatch = /^ {0,3}(#{1,6})\s+(.*)$/.exec(line);
    if (headingMatch) {
      const level = headingMatch[1].length as 1 | 2 | 3 | 4 | 5 | 6;

      const text = headingMatch[2].replace(/\s+#+\s*$/, "").trim();
      const slug = slugifyHeading(text, slugCounts);
      const node: HeadingNode = {
        type: "heading",
        level,
        text,
        slug,
        line: i + 1,
        children: parseInline(text),
      };
      ast.push(node);
      headings.push(node);
      i++;
      continue;
    }

    if (/^ {0,3}([-*_])(?:\s*\1){2,}\s*$/.test(line)) {
      ast.push({ type: "thematicBreak" });
      i++;
      continue;
    }

    if (/^ {0,3}>\s?/.test(line)) {
      const quoteLines: string[] = [];
      while (i < numLines) {
        const curLine = lines[i];
        const bqMatch = /^ {0,3}>\s?(.*)$/.exec(curLine);
        if (bqMatch) {
          quoteLines.push(bqMatch[1]);
          i++;
        } else if (curLine.trim() === "") {
          break;
        } else {
          // Lazy continuation line
          quoteLines.push(curLine);
          i++;
        }
      }
      const parsedQuote = parseMarkdown(quoteLines.join("\n"));
      ast.push({
        type: "blockquote",
        children: parsedQuote.ast,
      });
      continue;
    }

    if (
      line.includes("|") &&
      i + 1 < numLines &&
      /^ {0,3}\|?(\s*:?-+:?\s*\|)+\s*:?-+:?\s*\|?$/.test(lines[i + 1])
    ) {
      const headerLine = line;
      const delimiterLine = lines[i + 1];

      function parseCells(raw: string): string[] {
        let trimmed = raw.trim();
        if (trimmed.startsWith("|")) trimmed = trimmed.slice(1);
        if (trimmed.endsWith("|")) trimmed = trimmed.slice(0, -1);
        return trimmed.split("|").map((c) => c.trim());
      }

      const rawHeaders = parseCells(headerLine);
      const rawAligns = parseCells(delimiterLine);
      const alignments: Alignment[] = rawAligns.map((cell) => {
        const left = cell.startsWith(":");
        const right = cell.endsWith(":");
        if (left && right) return "center";
        if (left) return "left";
        if (right) return "right";
        return null;
      });

      const headers = rawHeaders.map((h) => parseInline(h));
      const rows: InlineNode[][][] = [];

      i += 2;
      while (i < numLines) {
        const rowLine = lines[i];
        if (!rowLine.includes("|") || rowLine.trim() === "") break;
        const cellStrings = parseCells(rowLine);
        const row = cellStrings.map((c) => parseInline(c));

        while (row.length < headers.length) {
          row.push([]);
        }
        rows.push(row);
        i++;
      }

      ast.push({
        type: "table",
        headers,
        alignments,
        rows,
      });
      continue;
    }

    const unorderedMatch = /^(\s*)([-*+])\s+(.*)$/.exec(line);
    const orderedMatch = /^(\s*)(\d+)\.\s+(.*)$/.exec(line);

    if (unorderedMatch || orderedMatch) {
      const isOrdered = Boolean(orderedMatch);
      const startNum = orderedMatch ? parseInt(orderedMatch[2], 10) : undefined;
      const items: ListItemNode[] = [];

      while (i < numLines) {
        const curLine = lines[i];
        if (curLine.trim() === "") {

          if (i + 1 < numLines && /^(\s*)([-*+]|\d+\.)\s+/.test(lines[i + 1])) {
            i++;
            continue;
          }
          break;
        }

        const match = isOrdered
          ? /^ {0,3}\d+\.\s+(.*)$/.exec(curLine)
          : /^ {0,3}[-*+]\s+(.*)$/.exec(curLine);

        if (match) {
          let itemText = match[1];
          let task: { checked: boolean } | undefined;

          const taskMatch = /^\[([ xX])\]\s+(.*)$/.exec(itemText);
          if (taskMatch) {
            task = { checked: taskMatch[1].toLowerCase() === "x" };
            itemText = taskMatch[2];
          }

          items.push({
            task,
            children: parseInline(itemText),
          });
          i++;
        } else if (/^\s{2,}/.test(curLine) && items.length > 0) {

          const subText = curLine.trim();
          const lastItem = items[items.length - 1];
          const subListMatch = /^([-*+]|\d+\.)\s+(.*)$/.exec(subText);
          if (subListMatch) {
            const subIsOrdered = /^\d+\./.test(subListMatch[1]);
            let subContent = subListMatch[2];
            let subTask: { checked: boolean } | undefined;
            const subTaskMatch = /^\[([ xX])\]\s+(.*)$/.exec(subContent);
            if (subTaskMatch) {
              subTask = { checked: subTaskMatch[1].toLowerCase() === "x" };
              subContent = subTaskMatch[2];
            }
            if (!lastItem.subList) {
              lastItem.subList = {
                type: "list",
                ordered: subIsOrdered,
                items: [],
              };
            }
            lastItem.subList.items.push({
              task: subTask,
              children: parseInline(subContent),
            });
          } else {

            lastItem.children.push({ type: "text", text: " " + subText });
          }
          i++;
        } else {
          break;
        }
      }

      ast.push({
        type: "list",
        ordered: isOrdered,
        start: startNum,
        items,
      });
      continue;
    }

    const paraLines: string[] = [];
    while (i < numLines) {
      const curLine = lines[i];
      if (curLine.trim() === "") break;
      if (/^ {0,3}(#{1,6})\s+/.test(curLine)) break;
      if (/^ {0,3}(`{3,}|~{3,})/.test(curLine)) break;
      if (/^ {0,3}>\s?/.test(curLine)) break;
      if (/^ {0,3}([-*_])(?:\s*\1){2,}\s*$/.test(curLine)) break;
      if (/^ {0,3}([-*+]|\d+\.)\s+/.test(curLine)) break;
      if (
        curLine.includes("|") &&
        i + 1 < numLines &&
        /^ {0,3}\|?(\s*:?-+:?\s*\|)+\s*:?-+:?\s*\|?$/.test(lines[i + 1])
      ) {
        break;
      }
      paraLines.push(curLine);
      i++;
    }

    if (paraLines.length > 0) {
      ast.push({
        type: "paragraph",
        children: parseInline(paraLines.join(" ")),
      });
    }
  }

  return { ast, headings };
}

/// Readers for the two repomind home documents the panel summarizes. They are plain markdown
/// written by agents and by the daemon's export, so every reader here is forgiving: a file that
/// does not follow the house layout still yields a usable line rather than an error.

/// One goal in flight, read from `plans/active/<slug>.md`.
export interface ActivePlan {
  /// Home-relative path, so clicking the entry can open the real file.
  path: string;
  /// The plan's own title: frontmatter `title:`, else its first heading, else the file name.
  title: string;
  /// What happens next, from a `next step` frontmatter field or a "Next step" section. Null when
  /// the plan does not say, which the panel shows as nothing rather than as a guess.
  nextStep: string | null;
}

/// The file name without its directory or `.md` suffix.
export function planSlug(path: string): string {
  const name = path.split("/").pop() ?? path;
  return name.replace(/\.md$/i, "");
}

function frontmatter(content: string): string | null {
  if (!content.startsWith("---")) return null;
  const end = content.indexOf("\n---", 3);
  return end === -1 ? null : content.slice(3, end);
}

/// Read one `key: value` line out of a frontmatter block, case-insensitively and tolerating the
/// `next_step` / `next step` / `next-step` spellings agents actually write.
function frontmatterField(block: string, keys: string[]): string | null {
  for (const line of block.split("\n")) {
    const separator = line.indexOf(":");
    if (separator === -1) continue;
    const key = line.slice(0, separator).trim().toLowerCase().replace(/[_-]/g, " ");
    if (!keys.includes(key)) continue;
    const value = line.slice(separator + 1).trim().replace(/^["']|["']$/g, "");
    if (value) return value;
  }
  return null;
}

/// The body with any frontmatter block removed, so heading and section scans do not match inside
/// it.
function body(content: string): string {
  const block = frontmatter(content);
  if (block === null) return content;
  const end = content.indexOf("\n---", 3);
  return content.slice(end + 4);
}

function firstHeading(text: string): string | null {
  for (const line of text.split("\n")) {
    const match = /^#{1,3}\s+(.+?)\s*$/.exec(line);
    if (match) return match[1];
  }
  return null;
}

/// The first non-empty line under a "Next step" (or "Next") heading, or the tail of a
/// "Next step: ..." line anywhere in the body.
function nextStepFrom(text: string): string | null {
  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const inline = /^\s*(?:[-*]\s*)?(?:\*\*)?next\s*step[s]?(?:\*\*)?\s*:\s*(.+?)\s*$/i.exec(line);
    if (inline) return inline[1];
    if (!/^#{1,6}\s+next(\s+step[s]?)?\s*$/i.test(line.trim())) continue;
    for (let next = index + 1; next < lines.length; next += 1) {
      const candidate = lines[next].trim().replace(/^[-*]\s+/, "");
      if (candidate) return candidate;
    }
  }
  return null;
}

/// Summarize one plan file for the panel's Active plans list.
export function readActivePlan(path: string, content: string): ActivePlan {
  const block = frontmatter(content);
  const rest = body(content);
  const title =
    (block && frontmatterField(block, ["title"])) ?? firstHeading(rest) ?? planSlug(path);
  const nextStep = (block && frontmatterField(block, ["next step"])) ?? nextStepFrom(rest);
  return { path, title, nextStep };
}

/// The home-relative path of today's journal digest, in the daemon's own `YYYY-MM-DD` naming.
export function journalPathFor(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `journal/${year}-${month}-${day}.md`;
}

/// The last entries of a journal digest, newest last, as the panel shows them.
///
/// The export writes one section per journal row, each starting at a heading. Anything before the
/// first heading is the file's own preamble and is not an entry.
export function journalTail(content: string, limit = 5): string[] {
  const entries: string[] = [];
  let current: string[] | null = null;
  for (const line of content.split("\n")) {
    if (/^#{2,6}\s+/.test(line)) {
      if (current) entries.push(current.join("\n").trimEnd());
      current = [line.replace(/^#{2,6}\s+/, "")];
    } else if (current) {
      current.push(line);
    }
  }
  if (current) entries.push(current.join("\n").trimEnd());
  return entries.slice(-limit);
}

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
    if (inline) return inline[1].trim();
    if (!/^#{1,6}\s+next(\s+step[s]?)?\s*$/i.test(line.trim())) continue;
    for (let next = index + 1; next < lines.length; next += 1) {
      const candidate = lines[next].trim().replace(/^[-*]\s+/, "");
      if (candidate) return candidate;
    }
  }
  return null;
}

/// Treat empty or period-only next steps as absent instead of displaying filler.
function isBlankNextStep(value: string): boolean {
  const trimmed = value.trim();
  return trimmed === "" || trimmed === ".";
}

/// Summarize one plan file for the panel's Active plans list.
export function readActivePlan(path: string, content: string): ActivePlan {
  const block = frontmatter(content);
  const rest = body(content);
  const title =
    (block && frontmatterField(block, ["title"])) ?? firstHeading(rest) ?? planSlug(path);
  const rawNextStep = (block && frontmatterField(block, ["next step"])) ?? nextStepFrom(rest);
  const nextStep = rawNextStep && !isBlankNextStep(rawNextStep) ? rawNextStep.trim() : null;
  return { path, title, nextStep };
}

/// The home-relative path of today's journal digest, in the daemon's own `YYYY-MM-DD` naming.
export function journalPathFor(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `journal/${year}-${month}-${day}.md`;
}

/// Returns the final journal sections in chronological order, excluding the preamble before the
/// first heading.
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

/// Everything the Plans board shows for one file, on top of [`ActivePlan`]: who owns it and when
/// it last moved. `owner` and `updated` are frontmatter fields the daemon's boot assembly already
/// reads, so the panel and the boot document describe a plan the same way.
export interface PlanSummary extends ActivePlan {
  /// Frontmatter `owner`, or null when the file does not say at all. Null is the only case the
  /// panel labels "unassigned" - a goal Add-goal itself wrote with that owner carries the word
  /// as real frontmatter, read back here the same way.
  owner: string | null;
  /// Frontmatter `updated`, else `created`, as written. Null when neither is there.
  updated: string | null;
}

/// Read one plan file into everything the board lists.
export function readPlanSummary(path: string, content: string): PlanSummary {
  const block = frontmatter(content);
  const rest = body(content);
  return {
    ...readActivePlan(path, content),
    owner: (block && frontmatterField(block, ["owner"])) ?? bodyField(rest, "owner"),
    updated:
      (block && frontmatterField(block, ["updated"])) ??
      (block && frontmatterField(block, ["created"])) ??
      null,
  };
}

/// A `Key: value` line in a body, with an optional list marker in front. The daemon reads plans
/// this way too, because operators write these by hand as often as agents do.
function bodyField(text: string, key: string): string | null {
  for (const raw of text.split("\n")) {
    const line = raw.trim().replace(/^[-*]\s+/, "");
    const separator = line.indexOf(":");
    if (separator === -1) continue;
    if (line.slice(0, separator).trim().toLowerCase() !== key) continue;
    const value = line.slice(separator + 1).trim();
    if (value) return value;
  }
  return null;
}

/// A title turned into the file name the home uses: lowercase words joined by hyphens, ASCII
/// only, capped so a rambling goal title cannot produce an unopenable path.
export function planSlugFor(title: string): string {
  const slug = title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/g, "");
  return slug || "goal";
}

/// A YAML frontmatter block in the home's own shape: the fences, one field per line, and the
/// blank line after. Mirrors the daemon's `md::frontmatter`, quoting the same values it quotes,
/// so a file written from the panel and one written by the daemon read identically.
function frontmatterBlock(fields: Array<[string, string]>): string {
  const scalar = (value: string) =>
    value === "" || value.trim() !== value || /[:#"'{}[\],\n]/.test(value)
      ? `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, " ")}"`
      : value;
  return `---\n${fields.map(([key, value]) => `${key}: ${scalar(value)}\n`).join("")}---\n\n`;
}

/// The date a home file stamps itself with, in the daemon's own `YYYY-MM-DD`.
function dayStamp(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/// Who owns a goal just added from the panel: `"repomind"` when the controller was actually told
/// about it, `"unassigned"` when Add goal found no controller running to tell.
export type NewPlanOwner = "repomind" | "unassigned";

/// Builds an active goal document in the boot reader’s format, using the title as the next step
/// when no intent is supplied.
export function newPlanDocument(
  title: string,
  intent: string,
  owner: NewPlanOwner,
  now = new Date(),
): string {
  const slug = planSlugFor(title);
  const nextStep = intent.trim() || title.trim();
  const fields: Array<[string, string]> = [
    ["title", title],
    ["type", "plan"],
    ["permalink", `repomind/plans/active/${slug}`],
    ["status", "active"],
    ["owner", owner],
    ["source", `repomon desktop ${dayStamp(now)}`],
    ["created", now.toISOString()],
  ];
  return `${frontmatterBlock(fields)}# ${title}\n\nNext step: ${nextStep}\n`;
}

/// The same plan closed out: `status: done`, a `closed` stamp, and the operator's one-line
/// outcome appended to the body. Everything else in the file is left exactly as it was, because
/// the plan's own history is the point of keeping it.
export function donePlanDocument(content: string, outcome: string, now = new Date()): string {
  const block = frontmatter(content);
  const rest = body(content);
  const fields = new Map<string, string>();
  const order: string[] = [];
  for (const line of (block ?? "").split("\n")) {
    const separator = line.indexOf(":");
    if (separator === -1) continue;
    const key = line.slice(0, separator).trim();
    if (!key) continue;
    if (!fields.has(key)) order.push(key);
    fields.set(key, line.slice(separator + 1).trim().replace(/^["']|["']$/g, ""));
  }
  fields.set("status", "done");
  if (!order.includes("status")) order.push("status");
  fields.set("closed", now.toISOString());
  if (!order.includes("closed")) order.push("closed");
  const rebuilt = order.map((key) => [key, fields.get(key) ?? ""] as [string, string]);
  const tail = rest.replace(/\s*$/, "");
  return `${frontmatterBlock(rebuilt)}${tail}\n\nOutcome: ${outcome}\n`;
}

/// Every entry in a journal digest, oldest first. [`journalTail`] is the same reader bounded to
/// the last few; the day browser wants the whole day.
export function journalEntries(content: string): string[] {
  return journalTail(content, Number.MAX_SAFE_INTEGER);
}

/// One day file in `journal/`, or one month file in `journal/archive/`.
export interface JournalDay {
  /// Home-relative path, so a day can be opened in the editor.
  path: string;
  /// `YYYY-MM-DD` for a day, `YYYY-MM` for an archived month.
  label: string;
  archived: boolean;
}

/// Separates current days and archived months, sorting ISO-formatted filenames newest-first.
export function journalDays(paths: string[]): { days: JournalDay[]; archive: JournalDay[] } {
  const days: JournalDay[] = [];
  const archive: JournalDay[] = [];
  for (const path of paths) {
    const name = path.split("/").pop() ?? path;
    if (!name.toLowerCase().endsWith(".md") || name.toLowerCase() === "readme.md") continue;
    const label = name.replace(/\.md$/i, "");
    const archived = path.includes("/archive/");
    (archived ? archive : days).push({ path, label, archived });
  }
  const newestFirst = (a: JournalDay, b: JournalDay) => b.label.localeCompare(a.label);
  return { days: days.sort(newestFirst), archive: archive.sort(newestFirst) };
}

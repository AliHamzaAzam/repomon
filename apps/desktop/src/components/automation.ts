import type { ApprovalRule, PendingDialog, Playbook } from "../bindings";
import { DaemonRpcError } from "../ipc/rpc";

export const JOURNAL_LIMIT = 200;

export function journalQueryParams(query: string): { query?: string; limit: number } {
  const trimmed = query.trim();
  return trimmed ? { query: trimmed, limit: JOURNAL_LIMIT } : { limit: JOURNAL_LIMIT };
}

export function playbookState(book: Playbook): { label: string; awaitingApproval: boolean } {
  if (book.status !== "approved") return { label: "draft", awaitingApproval: true };
  if (book.draft_content !== null) return { label: "approved · revision pending", awaitingApproval: true };
  return { label: "approved", awaitingApproval: false };
}

export function scheduleAddParams(
  spec: string,
  prompt: string,
  cap: string,
): { spec: string; prompt: string; max_actions?: number } {
  const parsed = Number.parseInt(cap.trim(), 10);
  const base = { spec: spec.trim(), prompt: prompt.trim() };
  return Number.isFinite(parsed) && parsed > 0 ? { ...base, max_actions: parsed } : base;
}

export function groupApprovalRules(rules: ApprovalRule[]): Array<{ repo: string; rules: ApprovalRule[] }> {
  const byRepo = new Map<string, ApprovalRule[]>();
  for (const rule of rules) {
    const bucket = byRepo.get(rule.repo);
    if (bucket) bucket.push(rule);
    else byRepo.set(rule.repo, [rule]);
  }
  return [...byRepo.entries()]
    .map(([repo, group]) => ({ repo, rules: [...group].sort((a, b) => a.pattern.localeCompare(b.pattern)) }))
    .sort((a, b) => a.repo.localeCompare(b.repo));
}

export function replacementDialog(error: unknown): PendingDialog | null | undefined {
  if (!(error instanceof DaemonRpcError) || error.code !== -32010) return undefined;
  const data = error.data as { dialog?: PendingDialog | null } | null;
  return data?.dialog;
}

export function formatTime(value: string): string {
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(value));
}

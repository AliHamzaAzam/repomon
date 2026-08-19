import type { ApprovalRule, DialogClass, PendingDialog, Playbook, PolicyAction, SupervisionConfig } from "../bindings";
import { DaemonRpcError } from "../ipc/rpc";
import type { SelectOption } from "./controls/Select";

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

/** Dialog classes shown in the global supervision defaults grid, in a fixed display order. */
export const SUPERVISION_DIALOG_CLASSES: Array<{ id: DialogClass; label: string; description: string }> = [
  { id: "command_exec", label: "Command execution", description: "Terminal bash and shell execution" },
  { id: "file_write", label: "File modification", description: "Creating, editing, or replacing files" },
  { id: "deletion", label: "File deletion", description: "Removing files or worktrees" },
  { id: "network_access", label: "Network access", description: "Outbound network requests and fetches" },
  { id: "credential_access", label: "Credential access", description: "Reading secrets, auth tokens, and keys" },
  { id: "push_remote", label: "Push to remote", description: "Git push and remote branch mutations" },
  { id: "install", label: "Package installation", description: "Installing package dependencies" },
  { id: "device_access", label: "Device access", description: "Interacting with hardware or external devices" },
  { id: "unknown", label: "Other dialogs", description: "Unclassified or ambiguous prompts" },
];

export const SUPERVISION_ACTION_OPTIONS: SelectOption[] = [
  { value: "auto_approve", label: "Auto-approve" },
  { value: "auto_deny", label: "Auto-deny" },
  { value: "hold", label: "Hold for human" },
];

export const SUPERVISION_MAIL_MODE_OPTIONS: SelectOption[] = [
  { value: "nudge", label: "Nudge" },
  { value: "full_body", label: "Full body" },
];

export function supervisionClassActionColor(action: PolicyAction): string {
  switch (action) {
    case "auto_approve":
      return "text-signal";
    case "auto_deny":
      return "text-fault";
    case "hold":
      return "text-attention";
    default:
      return "text-muted";
  }
}

/** Returns a copy of a supervision class map with a single dialog class's action changed, all others preserved. */
export function updatedSupervisionClasses(
  current: SupervisionConfig["classes"] | undefined,
  cls: DialogClass,
  action: PolicyAction,
): SupervisionConfig["classes"] {
  return { ...current, [cls]: action };
}

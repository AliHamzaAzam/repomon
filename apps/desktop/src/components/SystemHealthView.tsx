import { For, Show, createEffect, createSignal } from "solid-js";

import type { SystemDoctorResult } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import { isMac } from "../keymap";
import { AgentIcon, IconCheck, IconCopy, IconGitBranch, IconRefresh, IconTerminal } from "./icons";

export function getSystemInstallCommand(tool: "tmux" | "git"): string {
  if (isMac()) {
    return `brew install ${tool}`;
  }
  return `sudo apt install ${tool}`;
}

export function getAgentInstallInfo(kind: string, command: string): { command: string; guide?: string } | null {
  const k = kind.toLowerCase();
  const c = command.toLowerCase();
  if (k.includes("claude") || c === "claude") {
    return {
      command: "npm install -g @anthropic-ai/claude-code",
      guide: "Install Claude Code CLI via npm",
    };
  }
  if (k.includes("antigravity") || k === "agy" || c === "agy") {
    return {
      command: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
      guide: "Install Antigravity CLI via official install script",
    };
  }
  if (k.includes("codex") || c === "codex") {
    return {
      command: "npm install -g @openai/codex",
      guide: "Install OpenAI Codex CLI via npm",
    };
  }
  if (k.includes("opencode") || c === "opencode") {
    return {
      command: "npm install -g opencode-ai",
      guide: "Install OpenCode CLI via npm",
    };
  }
  if (k.includes("cursor") || c === "cursor-agent") {
    return {
      command: "curl https://cursor.com/install -fsS | bash",
      guide: "Install Cursor CLI agent via official install script",
    };
  }
  if (k.includes("aider") || c === "aider") {
    return {
      command: "pip install aider-chat",
      guide: "Install Aider CLI via pip",
    };
  }
  return null;
}

export interface SystemHealthViewProps {
  onConfigureCustomAgents?: () => void;
  showTitle?: boolean;
}

export default function SystemHealthView(props: SystemHealthViewProps) {
  const showTitle = () => props.showTitle ?? true;
  const [doctorResult, setDoctorResult] = createSignal<SystemDoctorResult | null>(null);
  const [doctorLoading, setDoctorLoading] = createSignal(false);
  const [doctorError, setDoctorError] = createSignal<string | null>(null);
  const [copiedKey, setCopiedKey] = createSignal<string | null>(null);

  async function fetchDoctor() {
    setDoctorLoading(true);
    setDoctorError(null);
    try {
      const res = await daemonCall("system.doctor");
      setDoctorResult(res);
    } catch (err) {
      setDoctorError(err instanceof Error ? err.message : String(err));
    } finally {
      setDoctorLoading(false);
    }
  }

  createEffect(() => {
    void fetchDoctor();
  });

  async function copyToClipboard(key: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedKey(key);
      setTimeout(() => {
        if (copiedKey() === key) setCopiedKey(null);
      }, 2000);
    } catch (e) {
      console.error("Failed to copy", e);
    }
  }

  return (
    <section class="space-y-5">
      {/* Header with Title, Status & Refresh */}
      <Show when={showTitle()}>
        <div class="flex items-center justify-between border-b border-line pb-3">
          <div>
            <h3 class="text-sm font-semibold text-foreground">System Health</h3>
            <p class="text-xs text-muted mt-0.5">
              Daemon runtime dependencies, terminal multiplexer, and detected agent CLI tools.
            </p>
          </div>
          <button
            type="button"
            class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs font-medium text-foreground hover:bg-line/40 transition-colors disabled:opacity-60 cursor-pointer"
            disabled={doctorLoading()}
            onClick={() => void fetchDoctor()}
            title="Refresh system dependency status"
            aria-label="Refresh system health status"
          >
            <IconRefresh
              size={13}
              class={doctorLoading() ? "animate-spin text-accent" : "text-muted"}
            />
            <span>{doctorLoading() ? "Checking…" : "Refresh"}</span>
          </button>
        </div>
      </Show>

      {/* Error Banner */}
      <Show when={doctorError()}>
        {(err) => (
          <div class="rounded-xl border border-fault/30 bg-fault/10 p-3.5 text-xs text-fault flex items-start justify-between gap-3">
            <div>
              <p class="font-semibold">Unable to run system diagnostics</p>
              <p class="mt-0.5 text-fault/80">{err()}</p>
            </div>
            <button
              type="button"
              class="focus-ring shrink-0 rounded-lg border border-fault/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground hover:bg-fault/20 transition-colors"
              onClick={() => void fetchDoctor()}
            >
              Retry Check
            </button>
          </div>
        )}
      </Show>

      {/* Loading Skeleton */}
      <Show when={doctorLoading() && !doctorResult()}>
        <div class="space-y-4 animate-pulse">
          <div class="rounded-xl border border-line bg-surface/40 p-4 space-y-3">
            <div class="h-4 w-32 rounded bg-line/60" />
            <div class="h-12 rounded-lg bg-line/30" />
            <div class="h-12 rounded-lg bg-line/30" />
          </div>
          <div class="rounded-xl border border-line bg-surface/40 p-4 space-y-3">
            <div class="h-4 w-44 rounded bg-line/60" />
            <div class="grid gap-2 sm:grid-cols-2">
              <div class="h-16 rounded-lg bg-line/30" />
              <div class="h-16 rounded-lg bg-line/30" />
            </div>
          </div>
        </div>
      </Show>

      {/* Doctor Data */}
      <Show when={doctorResult()}>
        {(doc) => {
          const tmuxInfo = () => doc().tmux;
          const gitInfo = () => doc().git;
          const agentsList = () => doc().agents;
          const detectedCount = () => agentsList().filter((a) => a.detected).length;

          return (
            <div class="space-y-4">
              {/* Section 1: Core Runtime (tmux + git) */}
              <div class="rounded-xl border border-line bg-surface p-3.5 space-y-3">
                <div class="flex items-center justify-between">
                  <span class="section-label">Core Runtime Dependencies</span>
                  <span class="text-[11px] font-mono text-muted">
                    {tmuxInfo().available && gitInfo().available ? (
                      <span class="text-emerald-500 font-medium">✓ Ready for sessions</span>
                    ) : (
                      <span class="text-amber-500 font-medium">Attention needed</span>
                    )}
                  </span>
                </div>

                <div class="divide-y divide-line/60 rounded-lg border border-line/70 bg-background/50">
                  {/* tmux Row */}
                  <div class="p-3 space-y-1.5">
                    <div class="flex items-start justify-between gap-3">
                      <div class="flex items-center gap-2.5 min-w-0">
                        <div class="flex size-6 items-center justify-center rounded-md border border-line bg-surface text-foreground shrink-0">
                          <IconTerminal size={13} />
                        </div>
                        <div class="min-w-0">
                          <div class="flex items-center gap-2">
                            <span class="font-medium text-xs text-foreground">tmux</span>
                            <span class="text-[11px] text-muted truncate">Terminal Multiplexer</span>
                          </div>
                          <p class="font-mono text-[10.5px] text-muted truncate mt-0.5" title={tmuxInfo().path || undefined}>
                            {tmuxInfo().path ? tmuxInfo().path : "Not found on PATH or bundle"}
                          </p>
                        </div>
                      </div>

                      <div class="flex items-center gap-2 shrink-0">
                        <Show when={tmuxInfo().version}>
                          {(ver) => (
                            <span class="font-mono text-[10.5px] text-muted bg-surface px-2 py-0.5 rounded border border-line">
                              {ver()}
                            </span>
                          )}
                        </Show>
                        <Show
                          when={tmuxInfo().available}
                          fallback={
                            <span class="rounded bg-amber-500/10 border border-amber-500/30 px-2 py-0.5 text-[10.5px] font-medium text-amber-500">
                              Missing
                            </span>
                          }
                        >
                          <Show
                            when={tmuxInfo().source === "bundled"}
                            fallback={
                              <span class="flex items-center gap-1.5 rounded bg-emerald-500/10 border border-emerald-500/30 px-2 py-0.5 text-[10.5px] font-medium text-emerald-500">
                                <span class="size-1.5 rounded-full bg-emerald-500" />
                                System PATH
                              </span>
                            }
                          >
                            <span
                              class="flex items-center gap-1.5 rounded bg-accent/15 border border-accent/30 px-2 py-0.5 text-[10.5px] font-medium text-accent font-mono"
                              title="Using Repomon's standalone built-in tmux binary"
                            >
                              <span class="size-1.5 rounded-full bg-accent" />
                              Repomon Built-in
                            </span>
                          </Show>
                        </Show>
                      </div>
                    </div>

                    {/* Bundled Reassurance Note */}
                    <Show when={tmuxInfo().available && tmuxInfo().source === "bundled"}>
                      <div class="flex items-center gap-1.5 text-[10.5px] text-accent/90 bg-accent/8 rounded px-2 py-0.5 border border-accent/20">
                        <span>Using Repomon's built-in standalone tmux — no separate Homebrew or system installation needed.</span>
                      </div>
                    </Show>

                    {/* Missing tmux helper */}
                    <Show when={!tmuxInfo().available}>
                      <div class="rounded-lg border border-amber-500/20 bg-amber-500/5 p-2.5 text-xs space-y-2">
                        <p class="text-foreground text-[11px]">
                          Repomon needs tmux to run agent sessions and live terminal attachments.
                        </p>
                        <div class="flex items-center gap-2">
                          <code class="flex-1 font-mono text-[10.5px] bg-surface px-2 py-0.5 rounded border border-line select-all text-foreground">
                            {getSystemInstallCommand("tmux")}
                          </code>
                          <button
                            type="button"
                            class="focus-ring flex items-center gap-1 rounded-md border border-line bg-surface px-2 py-0.5 text-xs font-medium text-foreground hover:bg-line/40 transition-colors"
                            onClick={() => void copyToClipboard("tmux-install", getSystemInstallCommand("tmux"))}
                            aria-label="Copy tmux install command"
                          >
                            <Show when={copiedKey() === "tmux-install"} fallback={<IconCopy size={11} class="text-muted" />}>
                              <IconCheck size={11} class="text-emerald-500" />
                            </Show>
                            <span>{copiedKey() === "tmux-install" ? "Copied" : "Copy"}</span>
                          </button>
                        </div>
                      </div>
                    </Show>
                  </div>

                  {/* git Row */}
                  <div class="p-3 space-y-1.5">
                    <div class="flex items-start justify-between gap-3">
                      <div class="flex items-center gap-2.5 min-w-0">
                        <div class="flex size-6 items-center justify-center rounded-md border border-line bg-surface text-foreground shrink-0">
                          <IconGitBranch size={13} />
                        </div>
                        <div class="min-w-0">
                          <div class="flex items-center gap-2">
                            <span class="font-medium text-xs text-foreground">git</span>
                            <span class="text-[11px] text-muted truncate">Version Control & Worktrees</span>
                          </div>
                          <p class="font-mono text-[10.5px] text-muted truncate mt-0.5" title={gitInfo().path || undefined}>
                            {gitInfo().path ? gitInfo().path : "Not found on PATH"}
                          </p>
                        </div>
                      </div>

                      <div class="flex items-center gap-2 shrink-0">
                        <Show when={gitInfo().version}>
                          {(ver) => (
                            <span class="font-mono text-[10.5px] text-muted bg-surface px-2 py-0.5 rounded border border-line">
                              {ver()}
                            </span>
                          )}
                        </Show>
                        <Show
                          when={gitInfo().available}
                          fallback={
                            <span class="rounded bg-amber-500/10 border border-amber-500/30 px-2 py-0.5 text-[10.5px] font-medium text-amber-500">
                              Missing
                            </span>
                          }
                        >
                          <span class="flex items-center gap-1.5 rounded bg-emerald-500/10 border border-emerald-500/30 px-2 py-0.5 text-[10.5px] font-medium text-emerald-500">
                            <span class="size-1.5 rounded-full bg-emerald-500" />
                            Available
                          </span>
                        </Show>
                      </div>
                    </div>

                    {/* Missing git helper */}
                    <Show when={!gitInfo().available}>
                      <div class="rounded-lg border border-amber-500/20 bg-amber-500/5 p-2.5 text-xs space-y-2">
                        <p class="text-foreground text-[11px]">
                          Repomon requires git to manage worktree lanes, branches, and commit tracking.
                        </p>
                        <div class="flex items-center gap-2">
                          <code class="flex-1 font-mono text-[10.5px] bg-surface px-2 py-0.5 rounded border border-line select-all text-foreground">
                            {getSystemInstallCommand("git")}
                          </code>
                          <button
                            type="button"
                            class="focus-ring flex items-center gap-1 rounded-md border border-line bg-surface px-2 py-0.5 text-xs font-medium text-foreground hover:bg-line/40 transition-colors"
                            onClick={() => void copyToClipboard("git-install", getSystemInstallCommand("git"))}
                            aria-label="Copy git install command"
                          >
                            <Show when={copiedKey() === "git-install"} fallback={<IconCopy size={11} class="text-muted" />}>
                              <IconCheck size={11} class="text-emerald-500" />
                            </Show>
                            <span>{copiedKey() === "git-install" ? "Copied" : "Copy"}</span>
                          </button>
                        </div>
                      </div>
                    </Show>
                  </div>
                </div>
              </div>

              {/* Section 2: Agent CLIs */}
              <div class="rounded-xl border border-line bg-surface p-3.5 space-y-3">
                <div class="flex items-center justify-between">
                  <div>
                    <span class="section-label">Coding Agents & Tooling</span>
                    <p class="text-[11px] text-muted mt-0.5">
                      CLI executables detected on PATH for spawning agent sessions.
                    </p>
                  </div>
                  <span class="text-[10.5px] font-mono text-muted bg-surface-raised px-2 py-0.5 rounded border border-line">
                    {detectedCount()} / {agentsList().length} detected
                  </span>
                </div>

                <div class="grid gap-2 sm:grid-cols-2">
                  <For each={agentsList()}>
                    {(agent) => {
                      const installInfo = () => getAgentInstallInfo(agent.kind, agent.command);
                      const agentKey = `agent-${agent.kind}-${agent.command}`;

                      return (
                        <div class="flex flex-col justify-between rounded-lg border border-line/70 bg-background/50 p-2.5 space-y-1.5">
                          <div class="flex items-start justify-between gap-2">
                            <div class="flex items-center gap-2 min-w-0">
                              <div class="flex size-6 items-center justify-center rounded-md border border-line bg-surface text-foreground shrink-0">
                                <AgentIcon agent={agent.kind} size={13} />
                              </div>
                              <div class="min-w-0">
                                <p class="font-medium text-xs text-foreground truncate">{agent.name}</p>
                                <p class="font-mono text-[10.5px] text-muted truncate">
                                  {agent.command}
                                </p>
                              </div>
                            </div>

                            <span
                              class={`shrink-0 rounded px-1.5 py-0.2 text-[9.5px] font-medium ${
                                agent.detected
                                  ? "bg-emerald-500/10 border border-emerald-500/30 text-emerald-500"
                                  : "bg-surface text-muted border border-line"
                              }`}
                            >
                              {agent.detected ? "✓ Detected" : "Not Found"}
                            </span>
                          </div>

                          {/* Actionable guidance if missing */}
                          <Show when={!agent.detected}>
                            <div class="border-t border-line/40 pt-1.5 text-[10.5px] text-muted space-y-1">
                              <Show
                                when={installInfo()}
                                fallback={
                                  <p class="text-muted/80">
                                    Install binary or configure custom command in{" "}
                                    <Show
                                      when={props.onConfigureCustomAgents}
                                      fallback={<span>Agents & Icons</span>}
                                    >
                                      <button
                                        type="button"
                                        class="underline hover:text-foreground cursor-pointer"
                                        onClick={() => props.onConfigureCustomAgents?.()}
                                      >
                                        Agents & Icons
                                      </button>
                                    </Show>
                                    .
                                  </p>
                                }
                              >
                                {(info) => (
                                  <div class="space-y-1">
                                    <p class="text-muted/90 truncate text-[10px]">{info().guide}</p>
                                    <div class="flex items-center gap-1.5">
                                      <code class="flex-1 font-mono text-[9.5px] bg-surface px-1.5 py-0.5 rounded border border-line select-all text-foreground truncate">
                                        {info().command}
                                      </code>
                                      <button
                                        type="button"
                                        class="focus-ring flex items-center gap-1 rounded border border-line bg-surface px-1.5 py-0.5 text-[9.5px] font-medium text-foreground hover:bg-line/40 transition-colors shrink-0 cursor-pointer"
                                        onClick={() => void copyToClipboard(agentKey, info().command)}
                                        aria-label={`Copy install command for ${agent.name}`}
                                      >
                                        <Show when={copiedKey() === agentKey} fallback={<IconCopy size={10} class="text-muted" />}>
                                          <IconCheck size={10} class="text-emerald-500" />
                                        </Show>
                                        <span>{copiedKey() === agentKey ? "Copied" : "Copy"}</span>
                                      </button>
                                    </div>
                                  </div>
                                )}
                              </Show>
                            </div>
                          </Show>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </div>
            </div>
          );
        }}
      </Show>
    </section>
  );
}

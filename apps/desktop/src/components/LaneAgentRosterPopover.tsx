import { For, Show } from "solid-js";
import { Portal } from "solid-js/web";

import type { AgentSession, Lane } from "../bindings";
import { slotOf } from "./agentLabel";
import { AgentIcon, IconGitBranch } from "./icons";

export interface AgentStatusDetails {
  label: string;
  toneClass: string;
  dotClass: string;
  badgeClass: string;
}

export function getSessionStatusDetails(session: AgentSession): AgentStatusDetails {
  if (session.pending_dialog || session.pending_prompt) {
    return {
      label: "Decision",
      toneClass: "text-attention",
      dotClass: "bg-attention animate-pulse ring-1 ring-attention/40",
      badgeClass: "bg-attention/15 text-attention border border-attention/30 font-medium",
    };
  }
  if (!session.external && !session.inferred && session.status === "running" && session.stale) {
    return {
      label: "Stalled",
      toneClass: "text-fault",
      dotClass: "bg-fault ring-1 ring-fault/40",
      badgeClass: "bg-fault/15 text-fault border border-fault/30",
    };
  }
  if (session.status === "rate-limited") {
    return {
      label: "Rate limited",
      toneClass: "text-fault",
      dotClass: "bg-fault ring-1 ring-fault/40",
      badgeClass: "bg-fault/15 text-fault border border-fault/30",
    };
  }
  if (!session.external && !session.inferred && session.status === "waiting") {
    return {
      label: "Needs attention",
      toneClass: "text-attention",
      dotClass: "bg-attention ring-1 ring-attention/40",
      badgeClass: "bg-attention/15 text-attention border border-attention/30",
    };
  }
  if (session.external) {
    return {
      label: "External",
      toneClass: "text-muted",
      dotClass: "bg-muted/60",
      badgeClass: "bg-raised text-muted border border-line",
    };
  }
  if (session.status === "running") {
    return {
      label: "Running",
      toneClass: "text-signal",
      dotClass: "bg-signal ring-1 ring-signal/40",
      badgeClass: "bg-signal/15 text-signal border border-signal/30",
    };
  }
  if (session.inferred) {
    return {
      label: "Inferred",
      toneClass: "text-signal",
      dotClass: "bg-signal/60",
      badgeClass: "bg-signal/10 text-signal/80 border border-signal/20",
    };
  }
  return {
    label: "Idle",
    toneClass: "text-muted",
    dotClass: "bg-muted/40",
    badgeClass: "bg-raised/80 text-muted border border-line/60",
  };
}

export function agentKindDisplayName(agent?: string | null): string {
  const raw = agent?.toLowerCase().trim() ?? "";
  if (raw === "claude-code" || raw === "claude") return "Claude Code";
  if (raw === "antigravity" || raw === "agy") return "Antigravity";
  if (raw === "codex") return "Codex";
  if (raw === "opencode") return "OpenCode";
  if (raw === "cursor") return "Cursor";
  if (raw === "aider") return "Aider";
  if (!raw || raw === "unknown") return "Agent";
  return raw.charAt(0).toUpperCase() + raw.slice(1);
}

export function agentSessionTitle(session: AgentSession): string {
  if (session.custom_label) return session.custom_label;
  if (session.generated_label) return session.generated_label;
  const kind = agentKindDisplayName(session.agent);
  const slot = slotOf(session.tmux_window);
  if (slot !== null) return `${kind} #${slot}`;
  if (session.external) return `${kind} (External)`;
  return kind;
}

export interface LaneAgentRosterPopoverProps {
  lane: Lane;
  anchorRect: DOMRect | null;
  visible: boolean;
  onSelectAgent?: (lane: Lane, session: AgentSession) => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

export function LaneAgentRosterPopover(props: LaneAgentRosterPopoverProps) {
  const position = () => {
    const rect = props.anchorRect;
    if (!rect) return { top: 0, left: 0 };
    const gap = 10;
    const viewportHeight = typeof window !== "undefined" ? window.innerHeight : 800;
    const left = rect.right + gap;
    const sessionCount = props.lane.agent_sessions?.length ?? 0;
    const estimatedHeight = 64 + sessionCount * 58;
    let top = rect.top;
    if (top + estimatedHeight > viewportHeight - 16) {
      top = Math.max(16, viewportHeight - estimatedHeight - 16);
    }
    return { top, left };
  };

  const laneTitle = () => props.lane.worktree.name || "Lane";
  const branchName = () => props.lane.worktree.branch ?? "detached";
  const sessionCount = () => props.lane.agent_sessions?.length ?? 0;

  return (
    <Show when={Boolean(props.visible && props.anchorRect && sessionCount() > 0)}>
      <Portal>
        <div
          class="fixed z-50 w-76 rounded-xl border border-line/90 bg-surface/95 dark:bg-surface/90 backdrop-blur-md shadow-2xl p-3 text-foreground transition-opacity duration-150 animate-in fade-in select-none pointer-events-auto"
          style={{
            top: `${position().top}px`,
            left: `${position().left}px`,
          }}
          onMouseEnter={props.onMouseEnter}
          onMouseLeave={props.onMouseLeave}
          role="tooltip"
          aria-label={`Active agents in ${laneTitle()}`}
        >
          {/* Header: Lane Name & Agent Count */}
          <div class="flex items-center justify-between gap-2">
            <div class="min-w-0 flex-1">
              <div class="truncate text-xs font-semibold text-foreground">
                {laneTitle()}
              </div>
              <div class="mt-0.5 flex items-center gap-1 font-mono text-[10px] text-muted">
                <IconGitBranch size={10} class="shrink-0 text-muted/70" />
                <span class="truncate">{branchName()}</span>
              </div>
            </div>
            <span class="shrink-0 inline-flex items-center gap-1 rounded border border-line/80 bg-raised/80 px-1.5 py-0.5 font-mono text-[10px] font-medium text-muted">
              <span>{sessionCount()} {sessionCount() === 1 ? "agent" : "agents"}</span>
            </span>
          </div>

          <div class="my-2.5 h-px bg-line/60" />

          {/* Roster List */}
          <div class="space-y-1.5">
            <For each={props.lane.agent_sessions}>
              {(agent) => {
                const status = () => getSessionStatusDetails(agent);
                const title = () => agentSessionTitle(agent);
                const kind = () => agentKindDisplayName(agent.agent);

                return (
                  <button
                    type="button"
                    class="group/roster-row flex w-full items-start gap-2.5 rounded-lg border border-line/40 bg-raised/40 p-2 text-left transition-all duration-150 hover:bg-raised hover:border-line hover:shadow-xs focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-signal active:scale-[0.99] cursor-pointer"
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onSelectAgent?.(props.lane, agent);
                    }}
                    aria-label={`Switch to ${title()} terminal`}
                  >
                    {/* Brand / Agent Icon */}
                    <div class="relative flex size-7 shrink-0 items-center justify-center rounded-md border border-line/60 bg-surface shadow-xs transition-colors group-hover/roster-row:border-line">
                      <AgentIcon
                        agent={agent.agent}
                        size={14}
                        class={status().toneClass}
                      />
                      <span
                        class={`absolute -top-0.5 -right-0.5 size-2 rounded-full border border-surface ${status().dotClass}`}
                        aria-hidden="true"
                      />
                    </div>

                    {/* Content */}
                    <div class="min-w-0 flex-1">
                      <div class="flex items-center justify-between gap-1.5">
                        <span class="truncate text-xs font-medium text-foreground group-hover/roster-row:font-semibold transition-colors" title={title()}>
                          {title()}
                        </span>
                        <span
                          class={`inline-flex shrink-0 items-center px-1.5 py-0.5 rounded text-[9px] font-mono leading-none ${status().badgeClass}`}
                        >
                          {status().label}
                        </span>
                      </div>

                      <div class="mt-0.5 flex items-center gap-1 font-mono text-[10px] text-muted">
                        <span class="truncate">{kind()}</span>
                        <Show when={agent.tmux_window}>
                          <span class="text-muted/40">·</span>
                          <span class="text-muted/80">{agent.tmux_window}</span>
                        </Show>
                        <Show when={agent.external}>
                          <span class="text-muted/40">·</span>
                          <span class="text-attention/90">external</span>
                        </Show>
                      </div>

                      {/* Pending prompt or message preview */}
                      <Show when={agent.pending_prompt}>
                        <div class="mt-1 truncate rounded border border-attention/20 bg-attention/10 px-1.5 py-0.5 text-[10px] italic text-attention">
                          {agent.pending_prompt}
                        </div>
                      </Show>
                      <Show when={!agent.pending_prompt && agent.status === "waiting" && agent.last_message}>
                        <div class="mt-1 truncate text-[10px] italic text-muted/80">
                          {`"${agent.last_message}"`}
                        </div>
                      </Show>
                    </div>
                  </button>
                );
              }}
            </For>
          </div>

          {/* Subtle footer */}
          <div class="mt-2 text-[10px] text-muted/60 font-mono text-center">
            Click an agent to open terminal
          </div>
        </div>
      </Portal>
    </Show>
  );
}

export default LaneAgentRosterPopover;

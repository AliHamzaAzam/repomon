import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Lane, PullRequestSummary } from "../bindings";
import { agentStateIn, agentStateReason, stateIndicator } from "../stores/fleet";
import { daemonCall } from "../ipc/rpc";
import { IconGitBranch, IconChevronRight, AgentIcon } from "./icons";

// Mirrors the fleet sidebar's own status dot exactly (FleetSidebar.tsx's LaneRow): Tailwind's
// scanner cannot see a template-interpolated class name, so the tone maps to a literal class here
// too rather than `bg-${tone}`.
function agentDotClass(tone: string): string {
  return tone === "signal" ? "bg-signal" : tone === "attention" ? "bg-attention" : tone === "fault" ? "bg-fault" : "bg-muted/50";
}

export default function ConversationContext(props: { lane: Lane; visible: boolean; onChanges?: () => void; onFocusAgent?: (window: string) => void }) {
  const [prs, setPrs] = createSignal<PullRequestSummary[]>([]);
  createEffect(() => {
    if (!props.visible) return;
    const repoId = props.lane.repo.id;
    let disposed = false;
    void daemonCall("repo.pull_requests").then((items) => { if (!disposed) setPrs(items.filter((pr) => pr.repo_id === repoId)); }).catch(() => undefined);
    onCleanup(() => { disposed = true; });
  });
  const dirty = () => props.lane.state.dirty;
  return <aside class="context-rail conversation-context" aria-label="Lane context">
    <section><h2>Environment</h2><p class="context-repo">{props.lane.repo.label ?? props.lane.repo.name}</p>
      <p class="context-branch"><IconGitBranch size={14} /><span>{props.lane.worktree.branch ?? "Detached HEAD"}</span></p>
      <p class="context-path" title={props.lane.worktree.path}>{props.lane.worktree.path}</p>
      <div class="context-changes"><span>{dirty().staged + dirty().unstaged + dirty().untracked ? "Working changes" : "Working tree clean"}</span>
        <Show when={dirty().staged + dirty().unstaged + dirty().untracked}><dl><div><dt>Staged</dt><dd>{dirty().staged}</dd></div><div><dt>Unstaged</dt><dd>{dirty().unstaged}</dd></div><div><dt>Untracked</dt><dd>{dirty().untracked}</dd></div></dl></Show>
      </div>
      <Show when={props.lane.state.ahead || props.lane.state.behind}><p class="text-xs text-muted">{props.lane.state.ahead} ahead · {props.lane.state.behind} behind</p></Show>
      <Show when={props.onChanges}><button type="button" class="context-link focus-ring" onClick={props.onChanges}>Open files <IconChevronRight size={12} /></button></Show>
    </section>
    <section><h2>Agents <span>{props.lane.agent_sessions.length}</span></h2>
      <Show when={props.lane.agent_sessions.length} fallback={<p class="text-xs text-muted">No active agents in this lane.</p>}><For each={props.lane.agent_sessions}>{(session) => {
        const label = () => session.custom_label || session.generated_label || session.agent;
        const interactive = () => !!session.tmux_window;
        const body = <>
          <span class={`context-agent-dot ${agentDotClass(stateIndicator(agentStateIn(props.lane, session)).tone)}`} aria-hidden="true" />
          <AgentIcon agent={session.agent} size={16} />
          <div><p>{label()}</p><span>{agentStateIn(props.lane, session).replace(/-/g, " ")}</span><Show when={session.subagent_running}><p class="text-xs text-muted">{session.subagent_running}</p></Show></div>
        </>;
        return interactive()
          ? <button type="button" class="context-agent context-agent-interactive focus-ring" title={agentStateReason(session) ?? undefined} aria-label={`Focus ${label()}'s pane`} onClick={() => props.onFocusAgent?.(session.tmux_window!)}>{body}</button>
          : <div class="context-agent" title={agentStateReason(session) ?? undefined}>{body}</div>;
      }}</For></Show>
    </section>
    <Show when={prs().length}><section><h2>Repository pull requests</h2><For each={prs()}>{(pr) => <button class="context-pr focus-ring" onClick={() => void openUrl(pr.url)}><span>#{pr.number} {pr.title}</span><IconChevronRight size={12} /></button>}</For></section></Show>
  </aside>;
}

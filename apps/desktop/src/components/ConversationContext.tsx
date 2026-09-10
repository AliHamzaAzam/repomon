import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Lane, PullRequestSummary } from "../bindings";
import { agentStateIn, agentStateReason } from "../stores/fleet";
import { daemonCall } from "../ipc/rpc";
import { IconGitBranch, IconChevronRight, AgentIcon } from "./icons";

export default function ConversationContext(props: { lane: Lane; visible: boolean; onChanges?: () => void }) {
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
      <Show when={props.lane.agent_sessions.length} fallback={<p class="text-xs text-muted">No active agents in this lane.</p>}><For each={props.lane.agent_sessions}>{(session) => <div class="context-agent" title={agentStateReason(session) ?? undefined}>
        <AgentIcon agent={session.agent} size={16} /><div><p>{session.custom_label || session.generated_label || session.agent}</p><span>{agentStateIn(props.lane, session).replace(/-/g, " ")}</span><Show when={session.subagent_running}><p class="text-xs text-muted">{session.subagent_running}</p></Show></div>
      </div>}</For></Show>
    </section>
    <Show when={prs().length}><section><h2>Repository pull requests</h2><For each={prs()}>{(pr) => <button class="context-pr focus-ring" onClick={() => void openUrl(pr.url)}><span>#{pr.number} {pr.title}</span><IconChevronRight size={12} /></button>}</For></section></Show>
  </aside>;
}

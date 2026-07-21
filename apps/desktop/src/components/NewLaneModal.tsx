import { For, Show, createSignal } from "solid-js";

import type { Repo } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

export default function NewLaneModal(props: {
  repos: Repo[];
  initialRepoId?: number;
  onClose: () => void;
  onDone: (laneId: number) => Promise<void>;
}) {
  const [repoId, setRepoId] = createSignal(props.initialRepoId ?? props.repos[0]?.id ?? 0);
  const [branch, setBranch] = createSignal("");
  const [source, setSource] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function create() {
    if (!branch().trim() || !repoId()) return;
    setBusy(true);
    setError(null);
    try {
      const lane = await daemonCall("lane.create", {
        repo_id: repoId(),
        branch: branch().trim(),
        source_branch: source().trim() || undefined,
      });
      await props.onDone(lane.id);
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const footer = (
    <>
      <button type="button" class="focus-ring rounded border border-line px-3 py-2 text-xs text-muted" onClick={props.onClose}>Cancel</button>
      <button type="button" class="focus-ring rounded bg-signal px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50" disabled={busy() || !branch().trim() || !repoId()} onClick={() => void create()}>
        {busy() ? "Creating…" : "Create lane"}
      </button>
    </>
  );

  return (
    <Modal title="New lane" subtitle="Creates a git worktree and branch for a new agent to work in." onClose={props.onClose} footer={footer}>
      <div class="space-y-4">
        <label class="block">
          <span class="section-label">Repository</span>
          <select class="settings-input" value={repoId()} onChange={(event) => setRepoId(Number(event.currentTarget.value))}>
            <For each={props.repos}>{(repo) => <option value={repo.id}>{repo.name}</option>}</For>
          </select>
        </label>
        <label class="block">
          <span class="section-label">New branch</span>
          <input class="settings-input" value={branch()} placeholder="feature/my-change" onInput={(event) => setBranch(event.currentTarget.value)} />
        </label>
        <label class="block">
          <span class="section-label">Source branch (optional)</span>
          <input class="settings-input" value={source()} placeholder="defaults to the repo's current branch" onInput={(event) => setSource(event.currentTarget.value)} />
        </label>
        <Show when={error()}>
          <p class="rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
        </Show>
      </div>
    </Modal>
  );
}

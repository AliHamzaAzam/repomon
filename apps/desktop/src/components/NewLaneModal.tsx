import { Show, createSignal } from "solid-js";

import type { Repo } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import Select from "./controls/Select";
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
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Cancel
      </button>
      <button
        type="button"
        class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
        disabled={busy() || !branch().trim() || !repoId()}
        onClick={() => void create()}
      >
        {busy() ? "Creating…" : "Create Lane"}
      </button>
    </>
  );

  return (
    <Modal
      title="New lane"
      subtitle="Creates an isolated git worktree and branch for development."
      onClose={props.onClose}
      footer={footer}
    >
      <div class="space-y-4">
        <Select
          label="Repository"
          value={String(repoId())}
          options={props.repos.map((r) => ({ value: String(r.id), label: r.name }))}
          onChange={(val) => setRepoId(Number(val))}
        />
        <label class="block">
          <span class="section-label">New Branch Name</span>
          <input
            class="focus-ring mt-1.5 h-9 w-full rounded-lg border border-line bg-surface px-3 font-mono text-xs text-foreground outline-none placeholder:text-muted/60"
            value={branch()}
            placeholder="feature/my-change"
            onInput={(event) => setBranch(event.currentTarget.value)}
          />
        </label>
        <label class="block">
          <span class="section-label">Source Branch (Optional)</span>
          <input
            class="focus-ring mt-1.5 h-9 w-full rounded-lg border border-line bg-surface px-3 font-mono text-xs text-foreground outline-none placeholder:text-muted/60"
            value={source()}
            placeholder="defaults to current branch"
            onInput={(event) => setSource(event.currentTarget.value)}
          />
        </label>
        <Show when={error()}>
          <p class="rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
        </Show>
      </div>
    </Modal>
  );
}

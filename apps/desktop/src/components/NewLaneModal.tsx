import { Show, createSignal } from "solid-js";

import type { Repo } from "../bindings";
import { translateError, type TranslatedError } from "../ipc/errors";
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
  const [error, setError] = createSignal<TranslatedError | null>(null);

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
      setError(translateError(cause, { binary: "git" }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title="New lane"
      subtitle="Creates an isolated git worktree and branch for development."
      onClose={props.onClose}
      footer={
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
      }
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
          {(err) => (
            <div class="rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault space-y-1.5">
              <div class="flex items-start gap-2">
                <span class="mt-0.5 inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-fault" />
                <p class="flex-1 font-medium leading-snug">{err().friendly}</p>
              </div>
              <Show when={err().raw && err().raw !== err().friendly}>
                <details class="group mt-1 pt-1 border-t border-fault/20">
                  <summary class="cursor-pointer select-none text-[11px] font-normal text-fault/75 hover:text-fault transition-colors outline-none">
                    Technical details
                  </summary>
                  <pre class="mt-1 max-h-24 overflow-x-auto whitespace-pre-wrap rounded bg-background/50 p-2 font-mono text-[10px] text-muted">
                    {err().raw}
                  </pre>
                </details>
              </Show>
            </div>
          )}
        </Show>
      </div>
    </Modal>
  );
}

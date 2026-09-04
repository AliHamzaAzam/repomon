import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

import type { Playbook } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import { RowShell, Section, SectionButton, SectionNote, sinceLabel } from "./RepomindSection";

/// The approval gate, as a surface rather than a promise: what repomind has written and is
/// waiting on, and what it is allowed to follow.
///
/// The two lists are deliberately not one list with a status column. A draft is a decision the
/// operator owes; an approved playbook is settled. Sorting them together would bury the first
/// under the second the moment the home has more than a few.
export interface RepomindPlaybooksProps {
  /// Opens a home-relative path in the editor on the home lane.
  onOpen: (path: string) => void;
  /// The home's counts moved, so the status poll should catch up now.
  onChanged?: () => void;
  /// Bumped by the panel when the draft or playbook count moves under us.
  revision?: number;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/// A playbook with a revision waiting is listed on both sides: the approved text is what agents
/// get, and the revision is still a decision somebody owes.
function drafts(books: Playbook[]): Playbook[] {
  return books.filter((book) => book.status === "draft" || book.draft_content !== null);
}

function approved(books: Playbook[]): Playbook[] {
  return books.filter((book) => book.status === "approved");
}

export default function RepomindPlaybooks(props: RepomindPlaybooksProps) {
  const [books, setBooks] = createSignal<Playbook[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);

  let live = true;
  // Newest read wins.
  let token = 0;
  onCleanup(() => {
    live = false;
  });

  async function load() {
    const mine = ++token;
    try {
      const result = await daemonCall("playbook.list");
      if (!live || mine !== token) return;
      setBooks(result.playbooks);
      setError(null);
    } catch (cause) {
      if (!live || mine !== token) return;
      setBooks([]);
      setError(errorMessage(cause));
    }
  }

  createEffect(() => {
    props.revision;
    void load();
  });

  async function decide(name: string, verdict: "approve" | "reject") {
    setBusy(`${verdict}:${name}`);
    setError(null);
    try {
      if (verdict === "approve") await daemonCall("playbook.approve", { name });
      else await daemonCall("playbook.reject", { name });
      await load();
      props.onChanged?.();
    } catch (cause) {
      if (live) setError(errorMessage(cause));
    } finally {
      if (live) setBusy(null);
    }
  }

  const pending = () => drafts(books());
  const settled = () => approved(books());

  return (
    <Section
      title="Playbooks"
      detail={pending().length ? `${pending().length} waiting on you` : String(settled().length)}
    >
      <Show when={error()}>{(message) => <SectionNote tone="fault">{message()}</SectionNote>}</Show>

      <Show when={pending().length}>
        <ul class="mb-2 space-y-0.5">
          <For each={pending()}>
            {(book) => (
              <RowShell title={`Draft, written ${sinceLabel(book.updated_at)}`}>
                <span class="size-1.5 shrink-0 rounded-full bg-attention" aria-hidden="true" />
                <span class="min-w-0 flex-1 truncate text-xs text-foreground">{book.name}</span>
                <Show when={book.status === "approved"}>
                  <span class="shrink-0 font-mono text-[10px] text-muted/70">revision</span>
                </Show>
                <SectionButton
                  label="Approve"
                  busy={busy() === `approve:${book.name}`}
                  title={`Move ${book.name} into playbooks and let repomind follow it`}
                  onClick={() => void decide(book.name, "approve")}
                />
                <SectionButton
                  label="Reject"
                  tone="fault"
                  busy={busy() === `reject:${book.name}`}
                  title={`Move ${book.name} into playbooks/rejected, keeping the text`}
                  onClick={() => void decide(book.name, "reject")}
                />
              </RowShell>
            )}
          </For>
        </ul>
      </Show>

      <Show
        when={settled().length}
        fallback={
          <Show when={!pending().length && !error()}>
            <SectionNote>
              No playbooks yet. Repomind drafts one into playbooks/drafts when it finishes a
              multi-lane goal, and it stays inert until you approve it here.
            </SectionNote>
          </Show>
        }
      >
        <ul class="space-y-0.5">
          <For each={settled()}>
            {(book) => (
              <RowShell title={`Approved ${sinceLabel(book.approved_at)}`}>
                <span class="min-w-0 flex-1 truncate text-xs text-foreground">{book.name}</span>
                <button
                  type="button"
                  class="focus-ring shrink-0 rounded px-1 py-0.5 font-mono text-[10px] text-muted transition-colors hover:text-foreground"
                  onClick={() => props.onOpen(`playbooks/${book.name}.md`)}
                  title={`Open playbooks/${book.name}.md`}
                >
                  Open
                </button>
              </RowShell>
            )}
          </For>
        </ul>
      </Show>
    </Section>
  );
}

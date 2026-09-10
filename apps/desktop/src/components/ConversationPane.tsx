import { ErrorBoundary, For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { PendingDialog } from "../bindings";
import { daemonCall, type TranscriptTarget } from "../ipc/rpc";
import { createTranscript, type ConversationRow } from "../stores/transcript";
import DiffView, { parseDiff } from "./DiffView";
import { MarkdownRenderer, parseMarkdown } from "./markdown";
import { IconChevronDown, IconChevronRight, IconArrowUp } from "./icons";
import type { TranscriptDetail } from "./controls/TranscriptDetailToggle";
import "./conversation.css";

export function dialogSummary(dialog: PendingDialog): string {
  const text = dialog.title != null ? `${dialog.title} \u2014 ${dialog.question}` : dialog.question;
  const chars = [...text];
  return chars.length > 120 ? chars.slice(0, 119).join("") + "…" : text;
}
function validDialog(value: unknown): value is PendingDialog {
  if (!value || typeof value !== "object") return false;
  const dialog = value as PendingDialog;
  return typeof dialog.question === "string" && Array.isArray(dialog.options) && dialog.options.every((option) => typeof option.text === "string");
}
function TextBody(props: { text: string; laneId: number }) {
  const parsed = createMemo(() => { try { return parseMarkdown(props.text); } catch { return null; } });
  return <ErrorBoundary fallback={<pre class="conversation-raw">{props.text}</pre>}><Show when={parsed()} fallback={<pre class="conversation-raw">{props.text}</pre>}>{(value) => <MarkdownRenderer ast={value().ast} laneId={props.laneId} />}</Show></ErrorBoundary>;
}
function LedgerRow(props: { row: ConversationRow; laneId: number; detail: string; kind: string }) {
  const item = () => props.row.item;
  const [expanded, setExpanded] = createSignal<boolean>();
  const open = () => expanded() ?? props.detail === "verbose";
  const tool = () => item().kind === "tool_call";
  const time = () => { const date = new Date(item().at ?? ""); return Number.isNaN(date.valueOf()) ? "" : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }); };
  const speaker = () => tool() ? "tool" : item().kind === "terminal_block" ? "terminal" : item().role === "user" ? "you" : item().kind === "status" ? "status" : props.kind === "claude-code" ? "claude" : props.kind;
  return <article class={`conversation-row ${tool() ? "conversation-tool-row" : ""}`} data-transcript-id={props.row.key} data-partial={item().partial ? "true" : undefined}>
    <div class="conversation-gutter"><Show when={!tool()}><time>{time()}</time><span title={item().model ? `${speaker()} · ${item().model}` : speaker()}>{speaker()}</span></Show></div>
    <div class="conversation-body">
      <Show when={tool()} fallback={<Show when={!props.row.fallback && item().kind !== "status" && item().kind !== "dialog"} fallback={<pre class="conversation-raw">{item().text || (validDialog(item().dialog) ? item().dialog?.question : "No text in this entry.")}</pre>}><TextBody text={item().text} laneId={props.laneId} /></Show>}>
        <button class="conversation-tool focus-ring" aria-expanded={open()} onClick={() => setExpanded(!open())}>
          <span class="shrink-0 text-muted">{open() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}</span>
          <span class="font-semibold">{item().name ?? "Tool"}</span><span class="min-w-0 flex-1 truncate text-muted" title={item().input_summary}>{item().input_summary ?? item().result_summary ?? item().text}</span>
          <span class={item().status === "error" ? "text-fault" : "text-muted"}>{item().status === "error" ? "failed" : item().status ?? "result"}</span>
        </button>
        <Show when={open()}><div class="conversation-tool-detail">
          <Show when={item().diff && parseDiff(item().diff!).length} fallback={<pre class="conversation-raw">{item().result_summary ?? item().text ?? item().input_summary}</pre>}>
            <DiffView patch={item().diff!} focusPath={parseDiff(item().diff!)[0]?.path} />
          </Show>
        </div></Show>
      </Show>
      <Show when={item().model && !tool()}><span class="conversation-model">{item().model}</span></Show>
      <Show when={item().partial}><span class="conversation-streaming" role="status">Writing<span aria-hidden="true">…</span></span></Show>
      <Show when={item().cost_usd != null}><span class="font-mono text-[10px] text-muted">${item().cost_usd!.toFixed(4)}</span></Show>
    </div>
  </article>;
}

export default function ConversationPane(props: { target: TranscriptTarget; visible: boolean; kind: string; detail?: TranscriptDetail; onTerminal: () => void }) {
  const transcript = createTranscript(() => props.visible ? props.target : null);
  const detail = () => props.detail ?? "normal";
  const [tail, setTail] = createSignal("");
  const [dialog, setDialog] = createSignal<PendingDialog | null>(null);
  const [reply, setReply] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [following, setFollowing] = createSignal(true);
  let scroll!: HTMLDivElement;
  let pendingScroll = 0;
  let refreshPrompt: (() => Promise<void>) | undefined;
  createEffect(() => {
    if (!props.visible) return;
    const target = { lane_id: props.target.lane_id, window: props.target.window };
    let disposed = false;
    let polling = false;
    const poll = async () => {
      if (polling || disposed) return;
      polling = true;
      const results = await Promise.allSettled([daemonCall("agent.capture", { ...target, lines: 3 }), daemonCall("agent.prompt", target)]);
      if (!disposed) {
        if (results[0].status === "fulfilled") setTail(results[0].value.content.split("\n").filter((line) => line.trim()).slice(-3).join("\n"));
        if (results[1].status === "fulfilled") setDialog(validDialog(results[1].value.dialog) ? results[1].value.dialog : null);
      }
      polling = false;
    };
    refreshPrompt = poll;
    void poll();
    const interval = setInterval(() => void poll(), 1000);
    onCleanup(() => { disposed = true; clearInterval(interval); refreshPrompt = undefined; });
  });
  createEffect(() => {
    transcript.revision();
    if (props.visible && following()) {
      cancelAnimationFrame(pendingScroll);
      pendingScroll = requestAnimationFrame(() => { if (scroll) scroll.scrollTop = scroll.scrollHeight; });
    }
  });
  onCleanup(() => cancelAnimationFrame(pendingScroll));
  async function older() {
    const height = scroll.scrollHeight;
    const top = scroll.scrollTop;
    setFollowing(false);
    await transcript.loadOlder();
    requestAnimationFrame(() => { scroll.scrollTop = top + scroll.scrollHeight - height; });
  }
  async function answer(choice: number) {
    const current = dialog();
    if (!current || busy()) return;
    setBusy(true); setError(null);
    try {
      await daemonCall("agent.answer", { lane_id: props.target.lane_id, window: props.target.window, choice, expect_summary: dialogSummary(current) });
      setDialog(null);
    } catch (cause) { setError(String(cause)); await refreshPrompt?.(); }
    finally { setBusy(false); }
  }
  async function send() {
    const text = reply();
    if (!text.trim() || busy() || dialog()) return;
    setBusy(true); setError(null);
    try { await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text, enter: true }); setReply(""); setFollowing(true); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  return <section class="conversation" aria-label="Conversation">
    <div class="conversation-scroll" ref={scroll} onScroll={() => setFollowing(scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 48)}>
      <div class="conversation-ledger">
        <Show when={transcript.nextBefore() !== null}><button class="focus-ring conversation-older" disabled={transcript.loading()} onClick={() => void older()}>Load earlier messages</button></Show>
        <Show when={transcript.error()}><p class="text-fault text-xs" role="alert">{transcript.error()} <button class="underline focus-ring" onClick={props.onTerminal}>Open terminal</button></p></Show>
        <Show when={!transcript.rows().length}><div class="conversation-empty"><p>{transcript.loading() ? "Opening conversation…" : "No conversation yet"}</p><p class="text-xs text-muted">{transcript.loading() ? "Connecting to this agent's output." : "Replies will appear here as the agent writes. The terminal is available below."}</p></div></Show>
        <For each={transcript.rows().filter((row) => detail() !== "summary" || row.item.kind !== "tool_call")}>
          {(row) => <LedgerRow row={row} laneId={props.target.lane_id} detail={detail()} kind={props.kind} />}
        </For>
      </div>
    </div>
    <Show when={!following()}><button class="conversation-latest focus-ring" onClick={() => { setFollowing(true); scroll.scrollTop = scroll.scrollHeight; }}>Latest output</button></Show>
    <footer class="conversation-footer" classList={{"is-pending": !!dialog()}}>
      <Show when={dialog()} fallback={<button class="conversation-tail focus-ring" onClick={props.onTerminal} aria-label="Expand terminal"><span class="conversation-footer-label">Terminal <IconChevronRight size={12} /></span><pre>{tail() || "No terminal output yet"}</pre></button>}>{(pending) => <div class="conversation-dialog">
        <div class="min-w-0 flex-1">
          <p class="conversation-question"><Show when={pending().title}><span>{pending().title}: </span></Show>{pending().question}</p>
          <Show when={pending().body?.length}><pre class="conversation-raw">{pending().body?.join("\n")}</pre></Show>
        </div>
        <div class="flex flex-wrap gap-2"><For each={pending().options}>{(option, index) => <button class="focus-ring rounded border border-line bg-surface px-3 py-1.5 text-xs hover:border-attention disabled:opacity-50" disabled={busy()} onClick={() => void answer(index())}>{option.text}</button>}</For></div>
      </div>}</Show>
      <Show when={error()}><p class="text-xs text-fault px-5 py-2" role="alert">{error()}</p></Show>
      <form class="conversation-compose" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <div class="conversation-reply">
        <textarea class="rounded" aria-label={`Reply to ${props.kind}`} placeholder={dialog() ? "Answer the prompt first" : `Reply to ${props.kind}…`} disabled={!!dialog() || busy()} value={reply()} rows={1} onInput={(event) => setReply(event.currentTarget.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); } }} />
        <button class="focus-ring rounded p-2 text-muted hover:text-foreground disabled:opacity-50" type="submit" aria-label="Send reply" disabled={!reply().trim() || !!dialog() || busy()}><IconArrowUp size={16} /></button>
        </div>
      </form>
    </footer>
  </section>;
}

import { ErrorBoundary, For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { Lane, PendingDialog } from "../bindings";
import { daemonCall, type ActivitySnapshot, type TranscriptTarget } from "../ipc/rpc";
import { createTranscript, type ConversationRow } from "../stores/transcript";
import DiffView, { parseDiff } from "./DiffView";
import { MarkdownRenderer, parseMarkdown } from "./markdown";
import { IconChevronDown, IconChevronRight } from "./icons";
import type { TranscriptDetail } from "./controls/TranscriptDetailToggle";
import AttachmentComposer from "./controls/AttachmentComposer";
import ConversationContext from "./ConversationContext";
import AttachmentChip, { isImageAttachment } from "./controls/AttachmentChip";
import AttachmentPreview from "./controls/AttachmentPreview";
import { attachmentTextParts } from "./attachmentText";
import { agentKindDisplayName, hasTranscriptSource, statusRowsFor } from "../stores/agentViews";
import { formatTokens } from "./usageMetrics";
import "./conversation.css";

// Session-level activity ("Whisking... (33s, 1.1k tokens)"), distinct from the per-message
// streaming marker: this belongs to the session and stays pinned above the composer regardless
// of scroll position, while "Writing..." stays attached to the streaming row it describes.
// Structured fields from the daemon's watch/event payload (never scraped from pane text).
function formatElapsed(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "";
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return rest ? `${minutes}m ${rest}s` : `${minutes}m`;
}
// A Codex idle footer can report model/effort with verb and elapsed both null - that reads as
// "nothing active" for this row's purpose, not as a label-less parenthetical, so it renders
// nothing rather than leaving a stray "(gpt-6-astra, high)" with no verb in front of it.
export function activityLabel(activity: ActivitySnapshot): string | null {
  if (!activity.verb) return null;
  const parts = [
    activity.elapsed_seconds != null ? formatElapsed(activity.elapsed_seconds) : null,
    activity.token_count != null ? `${formatTokens(activity.token_count)} tokens` : null,
    activity.thought_seconds != null ? `thought ${formatElapsed(activity.thought_seconds)}` : null,
    activity.model,
    activity.effort,
  ].filter((part): part is string => !!part);
  return parts.length ? `${activity.verb}… (${parts.join(", ")})` : `${activity.verb}…`;
}

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
function MessageBody(props: { row: ConversationRow; laneId: number; onResize?: () => void }) {
  const [expanded, setExpanded] = createSignal(false);
  const parts = createMemo(() => attachmentTextParts(props.row.item.text));
  const pane = () => props.row.paneExcerpt;
  const images = createMemo(() => parts().flatMap((part) => "attachment" in part && isImageAttachment(part.attachment) ? [part.attachment] : []));
  return <Show when={props.row.fallback} fallback={
    <>
      <Show when={images().length}><div class="conversation-images"><For each={images()}>{(file, index) => <AttachmentPreview file={file} number={index() + 1} onResize={props.onResize} />}</For></div></Show>
      <div class="conversation-message rounded"><For each={parts()}>{(part) => "attachment" in part
        ? isImageAttachment(part.attachment)
          ? <span class="attachment-reference" title={part.attachment.path}>[Image #{images().indexOf(part.attachment) + 1}]</span>
          : <AttachmentChip file={part.attachment} />
        : <Show when={part.text.trim()}><TextBody text={part.text} laneId={props.laneId} /></Show>}</For></div>
    </>
  }>
    <button type="button" class="conversation-excerpt-toggle focus-ring" aria-expanded={expanded()} onClick={() => setExpanded(!expanded())}>
      {expanded() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
      <span>{pane() ? "Terminal excerpt" : "Unformatted entry"}</span>
      <span class="text-muted">{props.row.item.text.split("\n").length} {props.row.item.text.includes("\n") ? "lines" : "line"}</span>
    </button>
    <Show when={expanded()}><pre class="conversation-raw conversation-excerpt focus-ring" tabindex="0" aria-label={pane() ? "Terminal excerpt content" : "Unformatted entry content"}>{props.row.item.text || "No text in this entry."}</pre></Show>
  </Show>;
}
// Codex's grammar-matched tool-rollup summaries ("Ran 1 shell command") arrive as an ordinary
// tool_call with name "tool_summary" - the summary text itself already reads as a complete
// sentence, so the raw internal name would be redundant chrome rather than useful identification.
function toolLabel(name: string | undefined): string | null {
  return name && name !== "tool_summary" ? name : null;
}
function LedgerRow(props: { row: ConversationRow; laneId: number; detail: string; kind: string; pending?: "sent" | "queued"; onResize?: () => void }) {
  const item = () => props.row.item;
  const [expanded, setExpanded] = createSignal<boolean>();
  const open = () => expanded() ?? props.detail === "verbose";
  const tool = () => item().kind === "tool_call";
  const time = () => { const date = new Date(item().at ?? ""); return Number.isNaN(date.valueOf()) ? "" : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }); };
  const speaker = () => tool() ? "tool" : props.row.fallback ? props.row.paneExcerpt ? "terminal" : "entry" : item().role === "user" ? "you" : item().kind === "status" ? "status" : props.kind === "claude-code" ? "claude" : props.kind;
  return <article class={`conversation-row ${tool() ? "conversation-tool-row" : ""} ${item().role === "user" ? "conversation-user" : ""} ${props.row.fallback ? "conversation-fallback" : ""}`} data-transcript-id={props.row.key} data-partial={item().partial ? "true" : undefined}>
    <div class="conversation-gutter"><Show when={!tool()}><time>{time()}</time><Show when={item().role !== "user"}><span title={item().model ? `${speaker()} · ${item().model}` : speaker()}>{speaker()}</span></Show></Show></div>
    <div class="conversation-body rounded">
      <Show when={tool()} fallback={<Show when={item().kind !== "status" && item().kind !== "dialog"} fallback={<pre class="conversation-raw">{item().text || (validDialog(item().dialog) ? item().dialog?.question : "No text in this entry.")}</pre>}><MessageBody row={props.row} laneId={props.laneId} onResize={props.onResize} /></Show>}>
        <button class="conversation-tool focus-ring" aria-expanded={open()} onClick={() => setExpanded(!open())}>
          <span class="shrink-0 text-muted">{open() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}</span>
          <Show when={toolLabel(item().name)}>{(name) => <span class="font-semibold">{name()}</span>}</Show><span class="min-w-0 flex-1 truncate text-muted" title={item().input_summary}>{item().input_summary ?? item().result_summary ?? item().text}</span>
          <span class={item().status === "error" ? "text-fault" : "text-muted"}>{item().status === "error" ? "failed" : item().status ?? "result"}</span>
        </button>
        <Show when={open()}><div class="conversation-tool-detail">
          <Show when={item().diff && parseDiff(item().diff!).length} fallback={<pre class="conversation-raw">{item().result_summary ?? item().text ?? item().input_summary}</pre>}>
            <DiffView patch={item().diff!} focusPath={parseDiff(item().diff!)[0]?.path} />
          </Show>
        </div></Show>
      </Show>
      <Show when={item().partial && !props.row.fallback}>
        <Show when={item().role === "user"} fallback={<span class="conversation-streaming" role="status">Writing<span aria-hidden="true">…</span></span>}>
          <span class="conversation-pending" role="status">{props.pending === "queued" ? "Queued" : "Sent"}</span>
        </Show>
      </Show>
    </div>
  </article>;
}

// A turn starts at a user message. Keep each original row object so streamed assistant
// upserts retain their DOM node, while all the turn's work shares one disclosure.
export function groupTurnWork(rows: ConversationRow[]) {
  const groups = new Map<string, ConversationRow[]>();
  const hidden = new Set<string>();
  let first: string | undefined;
  for (const row of rows) {
    if ((row.item.role === "user" && row.item.kind !== "status") || row.item.status_kind === "turn_started") first = undefined;
    if (!row.fallback && (row.item.kind === "tool_call" || row.item.kind === "status")) {
      if (!first) { first = row.key; groups.set(first, []); }
      else hidden.add(row.key);
      groups.get(first)!.push(row);
    }
  }
  return { groups, hidden };
}
function TurnWork(props: { rows: ConversationRow[]; detail: string; kind: string; laneId: number }) {
  const [expanded, setExpanded] = createSignal<boolean>();
  const open = () => expanded() ?? props.detail === "verbose";
  const tools = () => props.rows.filter((row) => row.item.kind === "tool_call");
  const notices = () => props.rows.filter((row) => row.item.kind === "status" && statusRowsFor(props.kind, props.detail).includes(row.item.status_kind ?? ""));
  const failed = () => tools().filter((row) => row.item.status === "error").length;
  const running = () => tools().some((row) => row.item.status === "running" || row.item.partial);
  return <Show when={tools().length || notices().length}><div class="conversation-work">
    <button type="button" class="work-summary focus-ring" aria-expanded={open()} onClick={() => setExpanded(!open())}>
      {open() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
      <span>{tools().length ? `${running() ? "Using" : "Used"} ${tools().length} ${tools().length === 1 ? "tool" : "tools"}` : "Turn details"}</span>
      <Show when={failed()}><span class="text-fault">{failed()} failed</span></Show>
      <Show when={notices().some((row) => row.item.status_kind === "rate_limit" || row.item.status_kind === "usage_limit")}><span class="text-attention">Limit reached</span></Show>
    </button>
    <Show when={open()}><div class="work-details"><For each={tools()}>{(row) => <LedgerRow row={row} laneId={props.laneId} kind={props.kind} detail="verbose" />}</For><For each={notices()}>{(row) => <p class="work-notice" data-transcript-id={row.key}>{row.item.text}</p>}</For></div></Show>
  </div></Show>;
}

// Caps how many loaded rows mount as DOM/Solid components at once, so a long-running lane or a
// history built from many "Load earlier messages" pages does not keep growing the live render
// tree forever. The window grows with what the user actually asks to see: loading an older page,
// or revealing already-loaded rows that fell outside the cap, both extend it by exactly what
// became visible so the ledger never mounts more than what is either recent or requested.
const RENDER_CAP = 250;

export default function ConversationPane(props: { target: TranscriptTarget; visible: boolean; kind: string; lane?: Lane; onFiles?: () => void; detail?: TranscriptDetail; onTerminal: () => void }) {
  const transcript = createTranscript(() => props.visible ? props.target : null);
  const detail = () => props.detail ?? "normal";
  const [dialog, setDialog] = createSignal<PendingDialog | null>(null);
  const [revealed, setRevealed] = createSignal(0);
  const visibleRows = createMemo(() => {
    const all = transcript.rows();
    const limit = RENDER_CAP + revealed();
    return all.length > limit ? all.slice(all.length - limit) : all;
  });
  const hiddenLoaded = () => transcript.rows().length - visibleRows().length;
  const hasOlder = () => hiddenLoaded() > 0 || transcript.nextBefore() !== null;
  const olderLabel = () => {
    if (transcript.loading()) return "Loading earlier messages…";
    const hidden = hiddenLoaded();
    if (hidden > 0) return `Load ${hidden} earlier message${hidden === 1 ? "" : "s"}`;
    const remaining = transcript.remaining();
    return remaining ? `Load ${remaining} earlier message${remaining === 1 ? "" : "s"}` : "Load earlier messages";
  };
  const work = createMemo(() => groupTurnWork(visibleRows()));
  const model = () => [...transcript.rows()].reverse().find((row) => row.item.model)?.item.model;
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
      try {
        const result = await daemonCall("agent.prompt", target);
        if (!disposed) setDialog(validDialog(result.dialog) ? result.dialog : null);
      } catch { /* Keep the last known prompt until the next refresh. */ }
      polling = false;
    };
    refreshPrompt = poll;
    void poll();
    const interval = setInterval(() => void poll(), 1000);
    onCleanup(() => { disposed = true; clearInterval(interval); refreshPrompt = undefined; });
  });
  const followLatest = () => {
    if (props.visible && following()) {
      cancelAnimationFrame(pendingScroll);
      pendingScroll = requestAnimationFrame(() => { if (scroll) scroll.scrollTop = scroll.scrollHeight; });
    }
  };
  createEffect(() => { transcript.revision(); followLatest(); });
  onCleanup(() => cancelAnimationFrame(pendingScroll));
  // Claude TUI's own behaviour: a text selection over transcript content copies itself, no
  // explicit copy step. Scoped to the ledger and the pending-decision body so it never fires for
  // the composer textarea, whose own selection is not part of window.getSelection() anyway.
  function copySelectionToClipboard() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed) return;
    const text = selection.toString();
    if (!text) return;
    const anchor = selection.anchorNode;
    const element = anchor instanceof Element ? anchor : anchor?.parentElement;
    if (!element?.closest(".conversation-ledger, .conversation-dialog")) return;
    void navigator.clipboard.writeText(text).catch(() => undefined);
  }
  async function older() {
    const height = scroll.scrollHeight;
    const top = scroll.scrollTop;
    setFollowing(false);
    const hidden = hiddenLoaded();
    if (hidden > 0) setRevealed((n) => n + hidden);
    else { const fetched = await transcript.loadOlder(); if (fetched) setRevealed((n) => n + fetched); }
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
  async function send(text: string): Promise<boolean> {
    if (!text.trim() || busy() || dialog()) return false;
    setBusy(true); setError(null);
    try { await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text, enter: true }); setFollowing(true); return true; }
    catch (cause) { setError(String(cause)); return false; }
    finally { setBusy(false); }
  }
  return <section class="conversation" aria-label="Conversation" onMouseUp={copySelectionToClipboard} onKeyUp={copySelectionToClipboard}>
    <div class="conversation-layout">
    <div class="conversation-main">
    <div class="conversation-scroll" ref={scroll} onScroll={() => setFollowing(scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 48)}>
      <div class="conversation-ledger">
        <Show when={!hasTranscriptSource(props.kind)}><p class="conversation-source-note">{agentKindDisplayName(props.kind)} sends its output straight to the terminal; there is no saved chat transcript to read here. A live excerpt appears below, or open the live terminal to follow along.</p></Show>
        <Show when={hasOlder()} fallback={<Show when={transcript.pagedOnce()}><p class="conversation-history-boundary">Beginning of conversation</p></Show>}>
          <button class="focus-ring conversation-older" disabled={transcript.loading()} onClick={() => void older()}>{olderLabel()}</button>
        </Show>
        <Show when={transcript.error()}><p class="text-fault text-xs" role="alert">{transcript.error()} <button class="underline focus-ring" onClick={props.onTerminal}>Open terminal</button></p></Show>
        <Show when={!transcript.rows().length}><div class="conversation-empty"><p>{transcript.loading() ? "Opening conversation…" : "No conversation yet"}</p><p class="text-xs text-muted">{transcript.loading() ? "Connecting to this agent's output." : "Replies will appear here as the agent writes. The terminal is available below."}</p></div></Show>
        <For each={visibleRows().filter((row) => !work().hidden.has(row.key))}>
          {(row) => <Show when={work().groups.has(row.key)} fallback={<LedgerRow row={row} laneId={props.target.lane_id} detail={detail()} kind={props.kind} pending={transcript.inputStates()[row.key]} onResize={followLatest} />}><TurnWork rows={work().groups.get(row.key) ?? []} laneId={props.target.lane_id} detail={detail()} kind={props.kind} /></Show>}
        </For>
      </div>
    </div>
    <Show when={!following()}><button class="conversation-latest focus-ring" onClick={() => { setFollowing(true); scroll.scrollTop = scroll.scrollHeight; }}>Latest output</button></Show>
    <footer class="conversation-footer" classList={{"is-pending": !!dialog()}}>
      <Show when={dialog()} fallback={<div class="conversation-terminal-line"><Show when={transcript.activity() && activityLabel(transcript.activity()!)}>{(label) => <span class="conversation-activity">{label()}</span>}</Show><Show when={props.lane}><span class="conversation-compact-context"><strong>{props.lane!.repo.label ?? props.lane!.repo.name}</strong><span>{props.lane!.worktree.branch ?? "Detached HEAD"}</span><Show when={props.lane!.state?.dirty}><span>{props.lane!.state.dirty.staged} staged · {props.lane!.state.dirty.unstaged} unstaged</span></Show></span></Show><button class="conversation-tail focus-ring" onClick={props.onTerminal} aria-label="Expand terminal">Open live terminal <IconChevronRight size={12} /></button></div>}>{(pending) => <div class="conversation-dialog">
        <div class="min-w-0 flex-1">
          <p class="conversation-question"><Show when={pending().title}><span>{pending().title}: </span></Show>{pending().question}</p>
          <Show when={pending().body?.length}><pre class="conversation-raw">{pending().body?.join("\n")}</pre></Show>
        </div>
        <div class="flex flex-wrap gap-2"><For each={pending().options}>{(option, index) => <button class="focus-ring rounded border border-line bg-surface px-3 py-1.5 text-xs hover:border-attention disabled:opacity-50" disabled={busy()} onClick={() => void answer(index())}>{option.text}</button>}</For></div>
      </div>}</Show>
      <Show when={error()}><p class="text-xs text-fault px-5 py-2" role="alert">{error()}</p></Show>
      <AttachmentComposer kind={props.kind} model={model()} disabled={!!dialog()} busy={busy()} onSend={send} />
    </footer>
    </div>
    <Show when={props.lane}>{(lane) => <ConversationContext lane={lane()} visible={props.visible} onChanges={props.onFiles} />}</Show>
    </div>
  </section>;
}

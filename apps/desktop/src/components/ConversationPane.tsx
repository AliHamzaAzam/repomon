import { ErrorBoundary, For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
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
import { statusRowsFor } from "../stores/agentViews";
import { formatTokens } from "./usageMetrics";
import { isAgentCommand, resolveCommand } from "./agentCommands";
import { createCommandCatalog } from "../stores/commandCatalog";
import { createInputHistory } from "../stores/inputHistory";
import { markChatLatency } from "../ipc/chatLatency";
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

function optionLabel(option: PendingDialog["options"][number]): string {
  return option.text.replace(/\.\s*$/, "");
}
function PendingDecision(props: { dialog: PendingDialog; busy: boolean; onAnswer: (index: number) => void }) {
  const optionRefs: (HTMLButtonElement | undefined)[] = [];
  const [focusedIndex, setFocusedIndex] = createSignal(0);
  let lastQuestion: string | undefined;
  createEffect(() => {
    if (props.dialog.question === lastQuestion) return;
    lastQuestion = props.dialog.question;
    const start = props.dialog.selected ?? 0;
    setFocusedIndex(start);
    optionRefs[start]?.focus();
  });
  const move = (delta: number) => {
    const count = props.dialog.options.length;
    const next = (focusedIndex() + delta + count) % count;
    setFocusedIndex(next);
    optionRefs[next]?.focus();
  };
  function onKeyDown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowRight") { event.preventDefault(); move(1); }
    else if (event.key === "ArrowUp" || event.key === "ArrowLeft") { event.preventDefault(); move(-1); }
    else if (event.key === "Enter") { event.preventDefault(); props.onAnswer(focusedIndex()); }
    else if (/^[1-9]$/.test(event.key)) {
      const index = Number(event.key) - 1;
      if (index < props.dialog.options.length) { event.preventDefault(); setFocusedIndex(index); props.onAnswer(index); }
    }
  }
  return <div class="conversation-dialog">
    <p class="conversation-question"><Show when={props.dialog.title}>{(title) => <span class="conversation-question-title">{title()}: </span>}</Show>{props.dialog.question}</p>
    <Show when={props.dialog.body?.length}><pre class="conversation-raw conversation-excerpt">{props.dialog.body?.join("\n")}</pre></Show>
    <div class="conversation-dialog-options" role="group" aria-label="Choose one" onKeyDown={onKeyDown}>
      <For each={props.dialog.options}>{(option, index) => <button
        ref={(el) => { optionRefs[index()] = el; }}
        type="button"
        tabIndex={focusedIndex() === index() ? 0 : -1}
        class="conversation-dialog-option focus-ring"
        disabled={props.busy}
        onFocus={() => setFocusedIndex(index())}
        onClick={() => props.onAnswer(index())}
      >
        <span class="conversation-dialog-option-key" aria-hidden="true">{index() + 1}</span>
        <span class="conversation-dialog-option-text">
          <span class="conversation-dialog-option-label">{optionLabel(option)}</span>
          <Show when={option.description}>{(description) => <span class="conversation-dialog-option-description">{description()}</span>}</Show>
        </span>
      </button>}</For>
    </div>
  </div>;
}
function TextBody(props: { text: string; laneId: number }) {
  const parsed = createMemo(() => { try { return parseMarkdown(props.text); } catch { return null; } });
  return <ErrorBoundary fallback={<pre class="conversation-raw">{props.text}</pre>}><Show when={parsed()} fallback={<pre class="conversation-raw">{props.text}</pre>}>{(value) => <MarkdownRenderer ast={value().ast} laneId={props.laneId} />}</Show></ErrorBoundary>;
}
// Coalesces a rapidly-changing streamed string to at most one update per animation frame. Both
// attachmentTextParts and parseMarkdown below re-scan the whole accumulated text from scratch on
// every call, so calling them on every token re-parses a growing string from zero each time.
// Measured (see qa report): an ~8.4k-character reply re-parsed on every 4-character token spiked
// a single call past 35ms, well over one frame's budget, and over a second of total CPU time
// across the turn. This bounds how often that full-length parse actually runs without changing
// the settled result once a frame lands - a normal, slower-than-60fps stream never notices it.
function useThrottledText(source: () => string): () => string {
  const [display, setDisplay] = createSignal(source());
  let frame: number | undefined;
  createEffect(() => {
    source();
    if (frame !== undefined) return;
    frame = requestAnimationFrame(() => { frame = undefined; setDisplay(source()); });
  });
  onCleanup(() => { if (frame !== undefined) cancelAnimationFrame(frame); });
  return display;
}
function MessageBody(props: { row: ConversationRow; laneId: number; onResize?: () => void; clampWhenTall?: boolean }) {
  const [expanded, setExpanded] = createSignal(false);
  const throttledText = useThrottledText(() => props.row.item.text);
  const parts = createMemo(() => attachmentTextParts(throttledText()));
  const pane = () => props.row.paneExcerpt;
  const images = createMemo(() => parts().flatMap((part) => "attachment" in part && isImageAttachment(part.attachment) ? [part.attachment] : []));
  // Pending-row content sizes to itself up to --pending-message-max-height rather than a fixed
  // clamp; when it genuinely overflows that, this shows an explicit toggle instead of handing the
  // row its own scrollbar nested inside the pending queue's. Measured, not assumed: a short
  // message never gets a pointless "Show more" it doesn't need.
  const [overflowing, setOverflowing] = createSignal(false);
  let messageRef: HTMLDivElement | undefined;
  const checkOverflow = () => {
    if (!props.clampWhenTall || !messageRef) return;
    setOverflowing(messageRef.scrollHeight - messageRef.clientHeight > 1);
  };
  onMount(checkOverflow);
  createEffect(() => { parts(); checkOverflow(); });
  if (props.clampWhenTall) {
    window.addEventListener("resize", checkOverflow);
    onCleanup(() => window.removeEventListener("resize", checkOverflow));
  }
  return <Show when={props.row.fallback} fallback={
    <>
      <Show when={props.row.item.mail}>{(mail) => <p class="conversation-mail-header">Mail from {mail().sender}<Show when={props.row.item.at}> · <time dateTime={props.row.item.at!}>{new Date(props.row.item.at!).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })}</time></Show></p>}</Show>
      <Show when={images().length}><div class="conversation-images"><For each={images()}>{(file, index) => <AttachmentPreview file={file} number={index() + 1} onResize={props.onResize} />}</For></div></Show>
      <div ref={messageRef} class="conversation-message rounded" classList={{ "conversation-message-clamped": !!props.clampWhenTall && !expanded() }}><For each={parts()}>{(part) => "attachment" in part
        ? isImageAttachment(part.attachment)
          ? <span class="attachment-reference" title={part.attachment.path}>[Image #{images().indexOf(part.attachment) + 1}]</span>
          : <AttachmentChip file={part.attachment} />
        : <Show when={part.text.trim()}><TextBody text={part.text} laneId={props.laneId} /></Show>}</For></div>
      <Show when={props.clampWhenTall && (overflowing() || expanded())}>
        <button type="button" class="conversation-excerpt-toggle focus-ring" aria-expanded={expanded()} onClick={() => setExpanded((value) => !value)}>
          {expanded() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
          <span>{expanded() ? "Show less" : "Show more"}</span>
        </button>
      </Show>
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
// Inbound fleet mail is typed into the agent's pane, so it travels the input path and carries
// role "user" like something the operator wrote. It is not his, and `mail.sender` already names
// who sent it, so attribution reads from there rather than from the role it had to borrow.
function isOperatorInput(row: ConversationRow): boolean {
  return row.item.role === "user" && !row.item.mail;
}
// Every address on a fleet starts "lane-", so in a 68px gutter (44px, then 32px, further down)
// that prefix is the only part that survives ellipsis and it identifies nobody. Drop the shared
// prefix and let .truncate-tail clip from the start, so what is left is the part that differs.
// The full address stays in the title and in the "Mail from ..." header beside it.
function senderLabel(sender: string): string {
  return sender.replace(/^lane-/, "");
}
function LedgerRow(props: { row: ConversationRow; laneId: number; detail: string; kind: string; delivered?: boolean; onResize?: () => void }) {
  const item = () => props.row.item;
  const [expanded, setExpanded] = createSignal<boolean>();
  const open = () => expanded() ?? props.detail === "verbose";
  const tool = () => item().kind === "tool_call";
  const time = () => { const date = new Date(item().at ?? ""); return Number.isNaN(date.valueOf()) ? "" : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }); };
  const speaker = () => tool() ? "tool" : props.row.fallback ? props.row.paneExcerpt ? "terminal" : "entry" : item().mail ? item().mail!.sender : item().role === "user" ? "you" : item().kind === "status" ? item().status_kind === "command_result" ? "command" : "status" : props.kind === "claude-code" ? "claude" : props.kind;
  return <article class={`conversation-row ${tool() ? "conversation-tool-row" : ""} ${isOperatorInput(props.row) ? "conversation-user" : ""} ${props.row.fallback ? "conversation-fallback" : ""}`} data-transcript-id={props.row.key} data-partial={item().partial ? "true" : undefined}>
    <div class="conversation-gutter"><Show when={!tool()}><time dateTime={item().at ?? undefined}>{time()}</time><span class={item().mail ? "truncate-tail" : undefined} title={item().model ? `${speaker()} · ${item().model}` : speaker()}>{item().mail ? senderLabel(item().mail!.sender) : speaker()}</span><Show when={props.delivered}><span class="conversation-delivered" role="status" title={`Sent to the agent. ${props.kind} does not report back which messages it has read, so whether it has been picked up cannot be shown here.`}>Delivered</span></Show></Show></div>
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
      {/* A still-partial user row reaching this point is mid-transition to "consumed" (or
         untracked) - the contract is explicit that consumed shows neither Sent nor Queued, just
         an ordinary seated line, so only the assistant case gets a marker here. Sent/Queued rows
         never reach this component: they render in the pinned queue below instead. */}
      <Show when={item().partial && !props.row.fallback && item().role !== "user"}>
        <span class="conversation-streaming" role="status">Writing<span aria-hidden="true">…</span></span>
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
    if (!row.fallback && !["source_unavailable", "command_result"].includes(row.item.status_kind ?? "") && (row.item.kind === "tool_call" || row.item.kind === "status")) {
      if (!first) { first = row.key; groups.set(first, []); }
      else hidden.add(row.key);
      groups.get(first)!.push(row);
    }
  }
  return { groups, hidden };
}
function TurnWork(props: { rows: ConversationRow[]; detail: string; kind: string; laneId: number }) {
  const [expanded, setExpanded] = createSignal<boolean>();
  const open = () => expanded() ?? (props.detail === "verbose" || notices().some((row) => row.item.status_kind === "error"));
  const tools = () => props.rows.filter((row) => row.item.kind === "tool_call");
  const notices = () => props.rows.filter((row) => row.item.kind === "status" && (row.item.status_kind === "error" || statusRowsFor(props.kind, props.detail).includes(row.item.status_kind ?? "")));
  const failed = () => tools().filter((row) => row.item.status === "error").length;
  const running = () => tools().some((row) => row.item.status === "running" || row.item.partial);
  return <Show when={tools().length || notices().length}><div class="conversation-work">
    <button type="button" class="work-summary focus-ring" aria-expanded={open()} onClick={() => setExpanded(!open())}>
      {open() ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
      <span>{tools().length ? `${running() ? "Using" : "Used"} ${tools().length} ${tools().length === 1 ? "tool" : "tools"}` : "Turn details"}</span>
      <Show when={failed()}><span class="text-fault">{failed()} failed</span></Show><Show when={notices().some((row) => row.item.status_kind === "error")}><span class="text-fault">Agent error</span></Show>
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

export default function ConversationPane(props: { target: TranscriptTarget; visible: boolean; shown?: boolean; kind: string; lane?: Lane; onFiles?: () => void; onFocusAgent?: (window: string) => void; detail?: TranscriptDetail; onTerminal: () => void; onCommand?: (text?: string) => Promise<void> }) {
  // `visible` is pane-level (is this window still relevant at all); `shown` (defaulting to
  // visible for callers that don't distinguish the two) is specifically "chat is the displayed
  // view right now" and gates cosmetic, display-only work that would otherwise run against an
  // invisible DOM tree. Neither alone should drive the transcript subscription: gating on
  // `visible` alone would page in a conversation for a pane the user has never actually opened
  // Chat on; gating on `shown` alone is the round 6 regression this fixes - it tears the watch
  // down and re-pages from scratch on every Terminal/Chat toggle even though the pane never
  // unmounted. So subscribing is lazy (first happens when chat is actually shown) but then
  // latches for the pane's lifetime, tracking only `visible` from then on - once opened, Chat
  // stays subscribed across toggles and only tears down when the pane itself goes away.
  const displayed = () => props.shown ?? props.visible;
  const [everShown, setEverShown] = createSignal(false);
  createEffect(() => { if (displayed()) setEverShown(true); });
  const subscribed = () => props.visible && everShown();
  const transcript = createTranscript(() => subscribed() ? props.target : null);
  // Fetched once the pane is genuinely shown, same gate as the transcript watch - the palette
  // opens on a keystroke and needs this to already be sitting there, not to fetch it fresh.
  const { catalog, loading: catalogLoading, error: catalogError } = createCommandCatalog(() => subscribed() ? { lane_id: props.target.lane_id, window: props.target.window } : null);
  // Same gate as the catalog above: fetched once the pane is genuinely shown, so Up/Down feel
  // instant once the operator actually reaches for them. The agent's own history file, not a
  // desktop-local list - see stores/inputHistory.ts.
  const { history: inputHistory, error: inputHistoryError } = createInputHistory(() => subscribed() ? { lane_id: props.target.lane_id, window: props.target.window } : null);
  const detail = () => props.detail ?? "normal";
  const [dialog, setDialog] = createSignal<PendingDialog | null>(null);
  const [revealed, setRevealed] = createSignal(0);
  const visibleRows = createMemo(() => {
    const all = transcript.rows();
    const limit = RENDER_CAP + revealed();
    return all.length > limit ? all.slice(all.length - limit) : all;
  });
  // Something the agent has not read yet is not "seated in the transcript as though it were
  // delivered" - matching the TUI's own split between consumed `>` lines and the highlighted
  // still-waiting block at the bottom. "consumed" (and untracked/durable rows) stay in the
  // ordinary ledger; only sent/queued move to the pinned queue below the scroll area.
  const pendingKeys = createMemo(() => {
    const states = transcript.inputStates();
    const keys = new Set<string>();
    for (const key in states) if (states[key] === "sent" || states[key] === "queued") keys.add(key);
    return keys;
  });
  const pendingRows = createMemo(() => transcript.rows().filter((row) => pendingKeys().has(row.key)));
  const hiddenLoaded = () => transcript.rows().length - visibleRows().length;
  const hasOlder = () => hiddenLoaded() > 0 || transcript.nextBefore() !== null;
  const olderLabel = () => {
    if (transcript.loading()) return "Loading earlier messages…";
    const hidden = hiddenLoaded();
    if (hidden > 0) return `Load ${hidden} earlier message${hidden === 1 ? "" : "s"}`;
    const remaining = transcript.remaining();
    return remaining ? `Load ${remaining} earlier message${remaining === 1 ? "" : "s"}` : "Load earlier messages";
  };
  const sourceNote = createMemo(() => transcript.rows().find((row) => row.item.status_kind === "source_unavailable")?.item.text);
  const ledgerRows = createMemo(() => visibleRows().filter((row) => !pendingKeys().has(row.key) && row.item.status_kind !== "source_unavailable"));
  const work = createMemo(() => groupTurnWork(ledgerRows()));
  // Chat-mode first-open latency breakdown (round 9): the last of the six marks - the moment
  // real content actually lands in the ledger, not just the moment the data arrived. Fires once
  // per mount; a live stream adding more rows afterward isn't a second "first" anything.
  let markedFirstContent = false;
  createEffect(() => {
    if (markedFirstContent || ledgerRows().length === 0) return;
    markedFirstContent = true;
    markChatLatency("first_content_visible", props.target);
  });
  const model = () => [...transcript.rows()].reverse().find((row) => row.item.model)?.item.model;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [following, setFollowing] = createSignal(true);
  let scroll!: HTMLDivElement;
  let pendingScroll = 0;
  let refreshPrompt: (() => Promise<void>) | undefined;
  // The daemon already folds a synthesized "dialog" item into this same transcript watch (see
  // conversation.rs's detect_dialog pass), so the latest one in the loaded rows is this window's
  // current pending prompt with no extra round trip. `agent.prompt` below only backs that up for
  // whatever gap remains (the moment before any watch event has landed, and any source the scan
  // pass cannot reach), polled far less eagerly than every visible pane once a second used to.
  const transcriptDialog = createMemo<PendingDialog | null>(() => {
    const rows = transcript.rows();
    for (let i = rows.length - 1; i >= 0; i--) {
      const row = rows[i];
      if (row.item.kind === "dialog" && validDialog(row.item.dialog)) return row.item.dialog;
    }
    return null;
  });
  // Additive only: a transcript-sourced dialog latches immediately, but its absence never clears
  // one - that stays the poll's job (and `answer()`'s own optimistic clear), since a scan pass
  // that has not run yet is not evidence the prompt is gone.
  createEffect(() => { const found = transcriptDialog(); if (found) setDialog(found); });
  createEffect(() => {
    // Only a pane that is both visible and actually on the chat view is worth a live RPC poll; a
    // mounted-but-backgrounded Terminal-view or off-screen pane relies on the transcript-derived
    // dialog above alone.
    if (!props.visible || !displayed()) return;
    const target = { lane_id: props.target.lane_id, window: props.target.window };
    let disposed = false;
    let polling = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      if (disposed) return;
      // Nothing running means a new dialog can only appear alongside transcript activity this
      // effect will already react to; back off to an occasional safety-net poll instead of once
      // a second so a quiet, fully idle pane stops costing the daemon's single connection.
      timer = setTimeout(() => void poll(), transcript.activity() ? 1000 : 8000);
    };
    const poll = async () => {
      if (polling || disposed) return;
      polling = true;
      try {
        // The transcript already has an authoritative answer; do not let a slower RPC's result
        // land afterwards and clobber it back to null.
        if (!transcriptDialog()) {
          const result = await daemonCall("agent.prompt", target);
          if (!disposed && !transcriptDialog()) setDialog(validDialog(result.dialog) ? result.dialog : null);
        }
      } catch { /* Keep the last known prompt until the next refresh. */ }
      polling = false;
      schedule();
    };
    refreshPrompt = poll;
    void poll();
    onCleanup(() => { disposed = true; if (timer !== undefined) clearTimeout(timer); refreshPrompt = undefined; });
  });
  const followLatest = () => {
    if (!displayed() || !following()) return;
    cancelAnimationFrame(pendingScroll);
    // Re-check on arrival, not just at schedule time: the operator can scroll away (disengaging
    // following) in the gap between this rAF being requested and it actually firing, and an
    // unconditional write here would yank the view back down anyway once it does.
    pendingScroll = requestAnimationFrame(() => {
      if (scroll && displayed() && following()) scroll.scrollTop = scroll.scrollHeight;
    });
  };
  createEffect(() => { transcript.revision(); followLatest(); });
  onCleanup(() => cancelAnimationFrame(pendingScroll));
  // A short, freshly opened ledger can never become scrollable, so the near-top scroll gesture in
  // onLedgerScroll below can never fire - "loading messages" then requires a manual click every
  // time instead of the same auto-load a longer conversation gets for free. Back it in eagerly
  // instead, the same page a real near-top scroll would ask for, until either older history is
  // exhausted or the ledger grows past its own viewport height (the scroll-triggered path then
  // takes over, same as always).
  createEffect(() => {
    if (!displayed() || !props.visible) return;
    if (transcript.loading() || loadingOlder) return;
    if (!hasOlder()) return;
    let cancelled = false;
    let frame: number | undefined;
    // clientHeight === 0 means the pane hasn't been laid out/measured yet (e.g. this exact render
    // tick, right after the initial page resolves and before its layout has settled, or a test
    // with no real layout at all) - not proof there is nothing to scroll. Retry across a few
    // frames rather than deciding once and never looking again, the same way TerminalPane retries
    // a zero-size warm pane until its grid is measurable.
    const tryBackfill = (framesLeft: number) => {
      if (cancelled || !scroll) return;
      if (scroll.clientHeight === 0 && framesLeft > 0) {
        frame = requestAnimationFrame(() => tryBackfill(framesLeft - 1));
        return;
      }
      if (scroll.clientHeight === 0 || scroll.scrollHeight > scroll.clientHeight) return;
      void older();
    };
    tryBackfill(30);
    onCleanup(() => {
      cancelled = true;
      if (frame !== undefined) cancelAnimationFrame(frame);
    });
  });
  // Counts ledger rows that arrived while scrolled away from the bottom, for the Latest output
  // pill's badge. Keyed, not counted per revision: a streamed partial's repeated token upserts
  // reuse one key and must not inflate the count, only a genuinely new row should. History the
  // operator explicitly paged in with older() is excluded via loadingOlder - that is older output
  // they asked for, not new output that arrived, even though it lands while scrolled away.
  const [unreadKeys, setUnreadKeys] = createSignal<ReadonlySet<string>>(new Set());
  let seenKeys = new Set<string>();
  let loadingOlder = false;
  createEffect(() => {
    const keys = new Set(ledgerRows().map((row) => row.key));
    if (following() || loadingOlder) {
      if (!loadingOlder && unreadKeys().size) setUnreadKeys(new Set<string>());
    } else {
      const added = [...keys].filter((key) => !seenKeys.has(key));
      if (added.length) setUnreadKeys((prev) => new Set<string>([...prev, ...added]));
    }
    seenKeys = keys;
  });
  // Claude TUI's own behaviour: a text selection over transcript content copies itself, no
  // explicit copy step. Scoped to the ledger and the pending-decision body so it never fires for
  // the composer textarea, whose own selection is not part of window.getSelection() anyway.
  function copySelectionToClipboard() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed) return;
    const text = selection.toString();
    if (!text) return;
    const scope = ".conversation-ledger, .conversation-dialog";
    const inScope = (node: Node | null) => {
      const element = node instanceof Element ? node : node?.parentElement;
      return !!element?.closest(scope);
    };
    // A drag that starts in the scroll container's own empty space below the last message (or in
    // one of the gutter-track wrapper elements around the ledger) and ends inside real text fails
    // a closest() check on the anchor alone, even though the selection plainly covers ledger
    // content - the anchor is the drag's start point, not necessarily where the text is. Check
    // both ends before giving up.
    if (!inScope(selection.anchorNode) && !inScope(selection.focusNode)) return;
    // A permissions or focus failure here was previously invisible (.catch(() => undefined)); it
    // now surfaces through the same error banner other footer actions use.
    void navigator.clipboard.writeText(text).catch((cause) => setError(`Could not copy selection: ${String(cause)}`));
  }
  async function older() {
    const height = scroll.scrollHeight;
    const top = scroll.scrollTop;
    setFollowing(false);
    loadingOlder = true;
    const hidden = hiddenLoaded();
    if (hidden > 0) setRevealed((n) => n + hidden);
    else { const fetched = await transcript.loadOlder(); if (fetched) setRevealed((n) => n + fetched); }
    loadingOlder = false;
    // Synchronous, not requestAnimationFrame: Solid has already patched the DOM by the time
    // setRevealed/loadOlder above returns (fine-grained reactivity applies a signal write's
    // dependent DOM updates in the same tick, not on a later frame), so reading scrollHeight and
    // writing scrollTop right here lands before the browser's next paint. Deferring this one
    // frame with rAF let the browser paint the newly-taller content at the *old* scrollTop first
    // - a real, visible one-frame jump to unrelated content, confirmed with a real browser (this
    // sequence is invisible in jsdom, which never paints).
    scroll.scrollTop = top + scroll.scrollHeight - height;
  }
  // Re-armed only by leaving the near-top zone (older()'s own anchor-preserving scroll jump does
  // this on a successful load, since the newly prepended content pushes scrollTop back down) - so
  // a continuous scroll-up gesture that stalls right at the top cannot fire a second auto-load.
  let autoLoadArmed = true;
  const NEAR_TOP_PX = 64;
  // Tracks the ledger's own scrollTop across events so a genuine upward move can be told apart
  // from the ledger growing underneath a stationary viewport (see onLedgerScroll).
  let lastScrollTop = 0;
  function onLedgerScroll() {
    const top = scroll.scrollTop;
    const scrolledUp = top < lastScrollTop - 1;
    lastScrollTop = top;
    // While a turn streams, the ledger's own height keeps growing under a stationary scrollTop,
    // which can hold the bottom gap inside the 48px threshold indefinitely - the operator scrolls
    // up, the gap never clears 48px because new content is filling in just as fast, and the very
    // next token's followLatest() yanks the view straight back to the bottom mid-gesture. A
    // genuine upward move (scrollTop actually decreasing, which only ever happens from a real
    // user gesture or paging in older history - programmatic follow only ever increases it)
    // disengages follow immediately regardless of the gap; only scrolling back down, the Latest
    // output pill, or sending a new message re-engages it.
    if (scrolledUp) setFollowing(false);
    else setFollowing(scroll.scrollHeight - top - scroll.clientHeight < 48);
    const scrollable = scroll.scrollHeight > scroll.clientHeight;
    const nearTop = scrollable && top < NEAR_TOP_PX;
    if (!nearTop) { autoLoadArmed = true; return; }
    if (!autoLoadArmed || transcript.loading() || !hasOlder()) return;
    autoLoadArmed = false;
    void older();
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
    try {
      // Never drive the agent's interactive picker blind: a command the catalog marks one_shot
      // sends as a single fully specified line, same RPC and same transcript as any other
      // message - no terminal overlay involved. Anything the catalog doesn't vouch for takes the
      // existing terminal route instead, same as before this catalog existed.
      if (isAgentCommand(text)) {
        const resolution = resolveCommand(text, catalog());
        if (resolution.oneShotLine) {
          await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text: resolution.oneShotLine, enter: true });
        } else if (props.onCommand) {
          await props.onCommand(resolution.terminalText);
        } else {
          await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text, enter: true });
        }
      } else {
        await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text, enter: true });
      }
      setFollowing(true); return true;
    }
    catch (cause) { setError(String(cause)); return false; }
    finally { setBusy(false); }
  }
  // Model selection is always a one-shot line too: `model_command` only ever appears in the
  // catalog when the daemon considers it safe to send with an argument and no interactive state.
  async function selectModel(id: string) {
    const command = catalog().model_command;
    if (!command) return;
    setBusy(true); setError(null);
    try {
      await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text: `${command} ${id}`, enter: true });
      setFollowing(true);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  // Same one-shot rule as the model: `effort_command` only appears when the daemon has a form it
  // can send with an argument on one line.
  async function selectEffort(id: string) {
    const command = catalog().effort_command;
    if (!command) return;
    setBusy(true); setError(null);
    try {
      await daemonCall("agent.send_input", { lane_id: props.target.lane_id, window: props.target.window, text: `${command} ${id}`, enter: true });
      setFollowing(true);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  async function controls(text?: string) {
    setError(null);
    try { await props.onCommand?.(text); }
    catch (cause) { setError(String(cause)); }
  }
  return <section class="conversation" aria-label="Conversation" onMouseUp={copySelectionToClipboard} onKeyUp={copySelectionToClipboard}>
    <div class="conversation-layout">
    <div class="conversation-main">
    <div class="conversation-scroll-area">
    <div class="conversation-scroll" ref={scroll} onScroll={onLedgerScroll}>
      <div class="conversation-ledger">
        <Show when={sourceNote()}>{(note) => <p class="conversation-source-note">{note()}</p>}</Show>
        <Show when={hasOlder()} fallback={<Show when={transcript.pagedOnce() && !sourceNote()}><p class="conversation-history-boundary">Beginning of conversation</p></Show>}>
          <button class="focus-ring conversation-older" disabled={transcript.loading()} onClick={() => void older()}>{olderLabel()}</button>
        </Show>
        <Show when={transcript.error()}><p class="text-fault text-xs" role="alert">{transcript.error()} <button class="underline focus-ring" onClick={props.onTerminal}>Open terminal</button></p></Show>
        <Show when={!transcript.rows().length}>
          <Show when={transcript.loading()} fallback={<div class="conversation-empty"><p>No conversation yet</p><p class="text-xs text-muted">Replies will appear here as the agent writes. The terminal is available below.</p></div>}>
            <div class="conversation-skeleton" role="status" aria-label="Opening conversation">
              <For each={[0, 1, 2]}>{() => <div class="conversation-skeleton-row"><div class="conversation-skeleton-line" style={{ width: "35%" }} /><div class="conversation-skeleton-line" style={{ width: "88%" }} /><div class="conversation-skeleton-line" style={{ width: "62%" }} /></div>}</For>
            </div>
          </Show>
        </Show>
        <For each={ledgerRows().filter((row) => !work().hidden.has(row.key))}>
          {(row) => <Show when={work().groups.has(row.key)} fallback={<LedgerRow row={row} laneId={props.target.lane_id} detail={detail()} kind={props.kind} delivered={transcript.inputStates()[row.key] === "delivered"} onResize={followLatest} />}><TurnWork rows={work().groups.get(row.key) ?? []} laneId={props.target.lane_id} detail={detail()} kind={props.kind} /></Show>}
        </For>
      </div>
    </div>
    <Show when={!following()}>
      <div class="conversation-latest-anchor">
        <button class="conversation-latest focus-ring" onClick={() => { setFollowing(true); scroll.scrollTop = scroll.scrollHeight; }}>
          <IconChevronDown size={12} />
          <span>Latest output</span>
          <Show when={unreadKeys().size}>{(count) => <span class="conversation-latest-count">{count()}</span>}</Show>
        </button>
      </div>
    </Show>
    </div>
    <Show when={pendingRows().length}>
      <div class="conversation-pending-queue" aria-label="Not yet read by the agent">
        <div class="conversation-pending-inner">
        <For each={pendingRows()}>{(row) => <div class="conversation-row conversation-pending-row" classList={{"conversation-user": isOperatorInput(row)}} data-transcript-id={row.key}>
          <div class="conversation-gutter"><span class={row.item.mail ? "truncate-tail" : undefined} title={row.item.mail ? row.item.mail.sender : "you"}>{row.item.mail ? senderLabel(row.item.mail.sender) : "you"}</span><span class="conversation-pending" role="status">{transcript.inputStates()[row.key] === "queued" ? "Queued" : "Sent"}</span></div>
          <div class="conversation-body rounded"><MessageBody row={row} laneId={props.target.lane_id} clampWhenTall /></div>
        </div>}</For>
        </div>
      </div>
    </Show>
    <footer class="conversation-footer" classList={{"is-pending": !!dialog()}}>
      <Show when={dialog()} fallback={<div class="conversation-terminal-line"><Show when={transcript.activity() && activityLabel(transcript.activity()!)}>{(label) => <span class="conversation-activity">{label()}</span>}</Show><Show when={props.lane}><span class="conversation-compact-context"><strong>{props.lane!.repo.label ?? props.lane!.repo.name}</strong><span>{props.lane!.worktree.branch ?? "Detached HEAD"}</span><Show when={props.lane!.state?.dirty}><span>{props.lane!.state.dirty.staged} staged · {props.lane!.state.dirty.unstaged} unstaged</span></Show></span></Show><Show when={props.onCommand}><button class="conversation-controls focus-ring" onClick={() => void controls()}>Agent controls</button></Show><button class="conversation-tail focus-ring" onClick={props.onTerminal} aria-label="Expand terminal">Open live terminal <IconChevronRight size={12} /></button></div>}>{(pending) => <PendingDecision dialog={pending()} busy={busy()} onAnswer={(index) => void answer(index)} />}</Show>
      <Show when={error()}><p class="text-xs text-fault px-5 py-2" role="alert">{error()}</p></Show>
      <AttachmentComposer kind={props.kind} model={transcript.activity()?.model ?? model()} disabled={!!dialog()} busy={busy()} onSelectEffort={(id) => void selectEffort(id)} onSend={send}
        catalog={catalog()} catalogError={!!catalogError()} catalogLoading={catalogLoading()} onSelectModel={(id) => void selectModel(id)}
        displayed={displayed}
        history={inputHistory().entries.map((entry) => entry.text)} historyUnavailable={inputHistory().source === "none"} historyError={inputHistoryError()} />
    </footer>
    </div>
    <Show when={props.lane}>{(lane) => <ConversationContext lane={lane()} visible={displayed()} onChanges={props.onFiles} onFocusAgent={props.onFocusAgent} />}</Show>
    </div>
  </section>;
}

//! Volatile delivery state for successful daemon submissions. Transcript consumption is authoritative.
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const INPUT_SENT: &str = "event.agent.input_sent";
#[derive(Default)]
pub struct Inputs {
    serial: AtomicU64,
    windows: Mutex<HashMap<String, WindowInputs>>,
}
#[derive(Default)]
struct WindowInputs {
    pending: Vec<Ticket>,
    aliases: HashMap<String, String>,
    /// How far a catch-up read has already looked, and in which source. A transcript only grows,
    /// so ground already covered cannot start holding a ticket's evidence later. Without this a
    /// ticket whose evidence never arrives re-reads the whole range on every sweep for as long as
    /// it stays pending, which on a long session is megabytes every few seconds, forever.
    swept: Option<(PathBuf, u64)>,
}
pub struct Ticket {
    source: Option<Source>,
    floor: u64,
    item: TranscriptItem,
    prior_prompts: Option<Vec<String>>,
    consumed: bool,
    submitted: String,
    /// STALL INSTRUMENTATION. One diagnosis per stuck ticket, so a brief that never leaves the
    /// pinned queue names the conjunct that refused it instead of being argued about.
    diagnosed: bool,
}

/// The three independent conditions consumption requires, evaluated once so the matcher and its
/// diagnosis can never disagree about why a ticket did or did not pair.
struct Conjuncts {
    same_source: bool,
    same_mail: bool,
    after_send: bool,
    text_equal: bool,
}

impl Conjuncts {
    fn consumed(&self) -> bool {
        self.same_source && (self.same_mail || (self.after_send && self.text_equal))
    }
}

/// How long a ticket may sit pending before it is worth a line. A consumed input normally pairs
/// within a tick or two; ten seconds means something refused it.
const STUCK_MS: i64 = 10_000;

/// Row ids carry the source path once they reach a page, and `conjuncts` reads the byte offset
/// back out of that shape. A catch-up read works on raw scanner ids, so it restores the prefix.
fn qualified_id(source: &Source, id: &str) -> String {
    match source.path.as_ref() {
        Some(path) => format!("{}:{id}", path.display()),
        None => id.into(),
    }
}

/// Evaluate consumption for one pending ticket against one durable row. Lifted out of the matcher
/// so the stuck-ticket diagnosis reports exactly what the matcher decided, not a paraphrase.
fn conjuncts(pending: &Ticket, row: &TranscriptItem, id: &str, source: &Source) -> Conjuncts {
    let same_source = pending
        .source
        .as_ref()
        .is_none_or(|old| old.path.is_none() || old == source);
    let offset = if matches!(source.kind.as_str(), "opencode" | "hermes") {
        row.at.map(|t| t.timestamp_millis().max(0) as u64)
    } else {
        source
            .path
            .as_ref()
            .and_then(|p| id.strip_prefix(&format!("{}:", p.display())))
            .and_then(|id| id.split(':').next())
            .and_then(|v| v.parse::<u64>().ok())
    };
    let after_send = if pending.source.as_ref().is_some_and(|s| s.path.is_some()) {
        offset.is_some_and(|offset| offset >= pending.floor)
    } else {
        row.at.zip(pending.item.at).is_some_and(|(row, sent)| {
            if source.kind == "antigravity" {
                row.timestamp() >= sent.timestamp()
            } else {
                row >= sent
            }
        })
    };
    // A globally unique mail ID is authoritative consumption evidence even if the provider
    // records receipt time before injection completes or collapses whitespace.
    let same_mail = row
        .mail
        .as_ref()
        .zip(pending.item.mail.as_ref())
        .is_some_and(|(row, sent)| row.id == sent.id);
    Conjuncts {
        same_source,
        same_mail,
        after_send,
        text_equal: row.text.trim() == pending.item.text.trim(),
    }
}

/// Report, once, why a ticket that should have been consumed is still pinned. Runs only for
/// tickets older than [`STUCK_MS`], so a healthy window never writes a line. Names the conjunct
/// that refused the best available candidate rather than guessing at a cause.
fn diagnose_stuck(pending: &mut [Ticket], rows: &[TranscriptItem], source: &Source) {
    let now = chrono::Utc::now();
    for ticket in pending.iter_mut() {
        if ticket.diagnosed {
            continue;
        }
        let age = ticket
            .item
            .at
            .map_or(0, |sent| (now - sent).num_milliseconds());
        if age < STUCK_MS {
            continue;
        }
        ticket.diagnosed = true;
        let users = rows.iter().filter(|r| r.role == "user").count();
        // The row that should have paired is the one whose text matches; report its other two
        // conjuncts. With no text match at all, the texts themselves are the story.
        let best = rows
            .iter()
            .filter(|r| r.role == "user")
            .filter_map(|r| {
                let id = r.id.clone()?;
                let c = conjuncts(ticket, r, &id, source);
                c.text_equal.then_some(c)
            })
            .next();
        match best {
            Some(c) => crate::chat_open_trace::input_stuck(
                age,
                users,
                c.same_source,
                c.after_send,
                true,
                ticket.submitted.chars().count(),
            ),
            None => crate::chat_open_trace::input_stuck(
                age,
                users,
                ticket.source.is_none(),
                false,
                false,
                ticket.submitted.chars().count(),
            ),
        }
    }
}

pub async fn prepare_input(ctx: &Ctx, lane: LaneId, window: &str, text: &str) -> Option<Ticket> {
    if repomon_core::usage_ledger::scan::strip_injected_blocks(text)
        .trim()
        .is_empty()
    {
        return None;
    }
    let backend = ctx.backend.clone();
    let target = window.to_string();
    let pane =
        tokio::task::spawn_blocking(move || backend.capture_named(&target, CaptureOpts::last(100)))
            .await
            .ok()
            .and_then(Result::ok);
    prepare_input_from_pane(ctx, lane, window, text, pane.as_deref()).await
}

/// Verified injection already captured the pane; do not add a second observation to its sequence.
pub async fn prepare_input_from_pane(
    ctx: &Ctx,
    lane: LaneId,
    window: &str,
    text: &str,
    pane: Option<&str>,
) -> Option<Ticket> {
    let cleaned = repomon_core::usage_ledger::scan::strip_injected_blocks(text);
    if cleaned.trim().is_empty() || repomon_core::agent::conversation::is_slash_command(&cleaned) {
        return None;
    }
    let p = Params {
        lane_id: lane,
        session_id: None,
        window: Some(window.into()),
        kind: None,
        before: None,
        on: true,
    };
    let source = resolve_source(ctx, &p, false, None).await.ok();
    let floor = source.as_ref().map_or(0, |s| {
        if matches!(s.kind.as_str(), "opencode" | "hermes") {
            chrono::Utc::now().timestamp_millis().max(0) as u64
        } else {
            s.path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len())
        }
    });
    // The baseline is what makes a later echo countable as new. It needs the kind, so it is `None`
    // exactly when the source did not resolve, and that stays distinct from an empty baseline: an
    // empty one asserts the pane held no earlier prompts, which would retire the next echo seen
    // whether or not it was ours. `append` reads the `None` as "nothing can confirm this ticket"
    // rather than leaving it pinned as unread forever.
    let prior_prompts = source
        .as_ref()
        .map(|s| s.kind.as_str())
        .zip(pane)
        .map(|(kind, pane)| {
            repomon_core::agent::conversation_queue::pane_inputs(kind, pane).consumed
        });
    let now = chrono::Utc::now();
    let mut parsed = repomon_core::agent::repomail::split(cleaned.trim(), Some(now));
    let mut item = if parsed.len() == 1 {
        parsed.remove(0)
    } else {
        TranscriptItem::new("user", cleaned.trim(), Some(now))
    };
    item.partial = Some(true);
    Some(Ticket {
        source,
        floor,
        item,
        prior_prompts,
        consumed: false,
        submitted: cleaned.trim().into(),
        diagnosed: false,
    })
}
impl Inputs {
    pub fn sent(&self, ctx: &Ctx, window: &str, mut ticket: Ticket) {
        ticket.item.id = Some(format!(
            "sent:{}",
            self.serial.fetch_add(1, Ordering::Relaxed)
        ));
        self.windows
            .lock()
            .unwrap()
            .entry(window.into())
            .or_default()
            .pending
            .push(ticket);
        ctx.broadcast(INPUT_SENT, json!({"window": window}));
    }
    /// The byte offset a catch-up read must start from to find evidence the display page cannot
    /// reach, or `None` when every pending ticket is still young enough to pair from the page.
    ///
    /// `reconcile` only ever sees the trailing page, which is a display bound, not an evidence
    /// one. On a session writing faster than the page is wide, a row passes out of that window
    /// between two passes and the ticket can never pair again. A ticket records the source length
    /// at send time, so its evidence, if it exists at all, lies at or after that offset.
    pub(super) fn catch_up_floor(&self, window: &str, source: &Source) -> Option<u64> {
        let now = chrono::Utc::now();
        let windows = self.windows.lock().unwrap();
        let state = windows.get(window)?;
        let floor = state
            .pending
            .iter()
            .filter(|ticket| {
                ticket
                    .source
                    .as_ref()
                    .is_none_or(|s| s.path.is_none() || s == source)
            })
            .filter(|ticket| {
                ticket
                    .item
                    .at
                    .is_none_or(|sent| (now - sent).num_milliseconds() >= STUCK_MS)
            })
            .map(|ticket| ticket.floor)
            .min()?;
        // Resume where the last read stopped, and only for the source it read.
        let swept = state
            .swept
            .as_ref()
            .filter(|(path, _)| source.path.as_ref() == Some(path))
            .map_or(0, |(_, offset)| *offset);
        Some(floor.max(swept))
    }

    /// Record how far a catch-up read got, so the next one starts there instead of repeating it.
    pub(super) fn mark_swept(&self, window: &str, source: &Source, next: u64) {
        let Some(path) = source.path.clone() else {
            return;
        };
        let mut windows = self.windows.lock().unwrap();
        if let Some(state) = windows.get_mut(window) {
            state.swept = Some((path, next));
        }
    }

    /// Retire tickets whose evidence a catch-up read found outside the display page. The rows are
    /// not seated: they are already older than the page, so the ledger will show them when the
    /// operator pages back, and the ticket has no further claim to make.
    pub(super) fn retire_confirmed(
        &self,
        window: &str,
        source: &Source,
        rows: &[TranscriptItem],
    ) -> usize {
        let mut windows = self.windows.lock().unwrap();
        let Some(state) = windows.get_mut(window) else {
            return 0;
        };
        let before = state.pending.len();
        state.pending.retain(|ticket| {
            !rows.iter().any(|row| {
                row.role == "user"
                    && row.id.as_ref().is_some_and(|id| {
                        conjuncts(ticket, row, &qualified_id(source, id), source).consumed()
                    })
            })
        });
        before - state.pending.len()
    }

    pub(super) fn reconcile(
        &self,
        window: &str,
        source: &Source,
        rows: &mut Vec<TranscriptItem>,
    ) -> Vec<String> {
        let mut replaced = Vec::new();
        let mut windows = self.windows.lock().unwrap();
        let Some(state) = windows.get_mut(window) else {
            return replaced;
        };
        if source.path.is_none() {
            // A pane echo alone is not proof of consumption. The pending ticket supplies this
            // mail row until the provider's consumed-prompt region confirms it.
            rows.retain(|row| {
                !row.mail.as_ref().is_some_and(|mail| {
                    state.pending.iter().any(|ticket| {
                        ticket
                            .item
                            .mail
                            .as_ref()
                            .is_some_and(|pending| pending.id == mail.id)
                    })
                })
            });
        }
        for row in rows.iter_mut() {
            let Some(id) = row.id.clone() else {
                continue;
            };
            if let Some(alias) = state.aliases.get(&id) {
                replaced.push(id);
                row.id = Some(alias.clone());
                continue;
            }
            if row.role != "user" {
                continue;
            }
            let index = state
                .pending
                .iter()
                .position(|pending| conjuncts(pending, row, &id, source).consumed());
            if let Some(index) = index {
                let pending = state.pending.remove(index);
                let alias = pending.item.id.unwrap();
                replaced.push(id.clone());
                state.aliases.insert(id, alias.clone());
                row.id = Some(alias);
            }
        }
        diagnose_stuck(&mut state.pending, rows, source);
        replaced
    }
    pub(super) fn append(
        &self,
        window: &str,
        source: &Source,
        pane: &str,
        items: &mut Vec<TranscriptItem>,
        order: &mut Vec<String>,
    ) -> Value {
        let mut windows = self.windows.lock().unwrap();
        let mut states = serde_json::Map::new();
        let observed = repomon_core::agent::conversation_queue::pane_inputs(&source.kind, pane);
        let mut matched = HashMap::<String, usize>::new();
        let mut queued_text = observed.queued.clone();
        if let Some(window) = windows.get_mut(window) {
            for ticket in &mut window.pending {
                if ticket
                    .source
                    .as_ref()
                    .is_some_and(|s| s.path.is_some() && s != source)
                {
                    continue;
                }
                let id = ticket.item.id.clone().unwrap();
                let text = repomon_core::agent::conversation_queue::normalized(&ticket.submitted);
                let queued = if let Some(index) = queued_text.iter().position(|row| row == &text) {
                    queued_text.remove(index);
                    true
                } else {
                    false
                };
                let prior = ticket
                    .prior_prompts
                    .as_ref()
                    .map(|prompts| prompts.iter().filter(|s| *s == &text).count());
                let current = observed.consumed.iter().filter(|s| *s == &text).count();
                let used = matched.entry(text).or_default();
                let mail_consumed = ticket.item.mail.as_ref().is_some_and(|mail| {
                    observed.consumed.iter().any(|text| {
                        repomon_core::agent::repomail::split(text, None)
                            .iter()
                            .any(|row| row.mail.as_ref().is_some_and(|row| row.id == mail.id))
                    })
                });
                if !queued && (mail_consumed || prior.is_some_and(|prior| current > prior + *used))
                {
                    ticket.consumed = true;
                    *used += 1;
                }
                // "sent" is a claim that the agent has not read this yet, and the pinned queue
                // shows it as such. Only make that claim while some channel could still withdraw
                // it: this kind's pane region with a baseline to count against, or a bound
                // transcript for `reconcile` to pair against. With neither, all we know is that
                // the keystrokes landed, and saying more would make an unreadable agent
                // indistinguishable from one that is ignoring the operator.
                let watched = (repomon_core::agent::conversation_queue::observable(&source.kind)
                    && ticket.prior_prompts.is_some())
                    || source.path.is_some();
                let state = if ticket.consumed {
                    "consumed"
                } else if queued || (source.kind == "codex" && observed.queue_reported) {
                    "queued"
                } else if watched {
                    "sent"
                } else {
                    "delivered"
                };
                states.insert(id.clone(), json!(state));
                order.push(id);
                items.push(ticket.item.clone());
            }
        }
        Value::Object(states)
    }
}

#[cfg(test)]
mod tests {

    /// The consumption predicate, pinned. A brief of the operator's that the agent demonstrably
    /// acted on stayed pinned, and the recorded row's text was verbatim, so the text conjunct was
    /// not the refusal. This documents the other two and, in particular, that `same_source`
    /// demands full equality of `Source` between the ticket and the row being reconciled.
    #[test]
    fn consumption_conjuncts_are_independent_and_same_source_demands_full_equality() {
        let sent = chrono::Utc::now();
        let path = std::path::PathBuf::from("/db/session.jsonl");
        let bound = |session: &str| Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(path.clone()),
            session: Some(session.into()),
        };
        let ticket = |source: Option<Source>, floor: u64| Ticket {
            source,
            floor,
            item: TranscriptItem::new("user", "the brief text", Some(sent)),
            prior_prompts: None,
            consumed: false,
            submitted: "the brief text".into(),
            diagnosed: false,
        };
        let mut row = TranscriptItem::new("user", "the brief text", Some(sent));
        row.id = Some("/db/session.jsonl:4096:0".into());
        let id = row.id.clone().unwrap();

        // The ordinary case: same source, the row sits past the floor, texts equal.
        let c = conjuncts(&ticket(Some(bound("s1")), 1024), &row, &id, &bound("s1"));
        assert!(c.consumed(), "a plain consumption must pair");

        // The suspect: the window's source changed between send and reconcile. Text still equal,
        // offset still past the floor, and the ticket can never pair again.
        let c = conjuncts(&ticket(Some(bound("s1")), 1024), &row, &id, &bound("s2"));
        assert!(!c.same_source, "a changed session must break same_source");
        assert!(c.text_equal && c.after_send, "the other two still hold");
        assert!(!c.consumed(), "so the ticket stays pinned");

        // A ticket created before the window had a resolved source is exempt from same_source,
        // which is why `discover = false` at prepare time changes which rule applies.
        let c = conjuncts(&ticket(None, 0), &row, &id, &bound("s2"));
        assert!(
            c.same_source,
            "an unresolved ticket is not held to source equality"
        );
        assert!(c.consumed());

        // A row written before the input was sent is not evidence of consuming it.
        let c = conjuncts(&ticket(Some(bound("s1")), 8192), &row, &id, &bound("s1"));
        assert!(
            !c.after_send,
            "offset below the floor is older than the send"
        );
        assert!(!c.consumed());

        // Different text never pairs, whatever else holds.
        let mut other = row.clone();
        other.text = "a different brief".into();
        let c = conjuncts(&ticket(Some(bound("s1")), 1024), &other, &id, &bound("s1"));
        assert!(!c.text_equal && !c.consumed());
    }
    use super::*;

    /// A claude-code transcript holding one mail envelope, followed by enough unrelated records to
    /// push it further behind the tail than a page is wide. This is the operator's controller pane
    /// in miniature: the evidence is on disk and outside every page the watch will ever load.
    fn transcript_with_mail_behind_the_page(
        dir: &std::path::Path,
        id: &str,
        body: &str,
    ) -> PathBuf {
        let path = dir.join("session.jsonl");
        let envelope =
            format!("[REPOMAIL id={id} from=lane-2/1 reply_to=none] {body} [{id}] [END REPOMAIL]");
        let mut text = format!(
            "{}\n",
            json!({"type":"user","message":{"content":envelope}})
        );
        let filler = json!({"type":"assistant","message":{"content":"x".repeat(2048)}}).to_string();
        while text.len() < 256 * 1024 {
            text.push_str(&filler);
            text.push('\n');
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    fn mail_ticket(
        source: &Source,
        id: &str,
        body: &str,
        sent: chrono::DateTime<chrono::Utc>,
    ) -> Ticket {
        let envelope =
            format!("[REPOMAIL id={id} from=lane-2/1 reply_to=none] {body} [{id}] [END REPOMAIL]");
        let mut parsed = repomon_core::agent::repomail::split(envelope.trim(), Some(sent));
        assert_eq!(
            parsed.len(),
            1,
            "a bare envelope is one row, so it keeps its mail id"
        );
        Ticket {
            source: Some(source.clone()),
            floor: 0,
            item: parsed.remove(0),
            prior_prompts: Some(Vec::new()),
            consumed: false,
            submitted: envelope,
            diagnosed: false,
        }
    }

    /// The round's defect, measured on the operator's own pane: `reconcile` only ever sees the
    /// trailing page, so once a mail row falls behind it no conjunct is ever evaluated and the
    /// ticket is pinned as unread for good. A read from the ticket's own floor finds it.
    #[tokio::test]
    async fn mail_behind_the_display_page_is_retired_by_a_read_from_the_tickets_own_floor() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let id = "2dd190b818e0323bb958c845ab7d2f0b";
        let body = "Ordered-chain work is committed on the branch.";
        let path = transcript_with_mail_behind_the_page(dir.path(), id, body);
        let source = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(path.clone()),
            session: Some("s1".into()),
        };
        let sent = chrono::Utc::now() - chrono::Duration::milliseconds(STUCK_MS + 1_000);
        ctx.transcript_inputs
            .sent(&ctx, "lane-1", mail_ticket(&source, id, body, sent));

        // The page the watch actually loads. The mail row is not in it, so nothing pairs.
        let page_start =
            repomon_core::usage_ledger::scan::jsonl_page_start(&path, u64::MAX, PAGE_BYTES)
                .unwrap();
        assert!(page_start > 0, "the fixture is wider than one page");
        let page = scan_range(&source, page_start, None).unwrap();
        let mut rows: Vec<_> = page
            .transcript
            .into_iter()
            .map(|entry| {
                let mut item = entry.item;
                item.id = item.id.map(|id| format!("{}:{id}", path.display()));
                item
            })
            .collect();
        assert!(
            !rows.iter().any(|row| row.mail.is_some()),
            "the evidence is behind the page, which is the whole defect"
        );
        assert!(
            ctx.transcript_inputs
                .reconcile("lane-1", &source, &mut rows)
                .is_empty(),
            "a page without the row cannot retire the ticket"
        );

        // The catch-up read knows where to look because the ticket recorded its floor.
        let floor = ctx
            .transcript_inputs
            .catch_up_floor("lane-1", &source)
            .expect("a stuck ticket asks for a read");
        let scan = scan_range(&source, floor, None).unwrap();
        let behind: Vec<_> = scan.transcript.into_iter().map(|e| e.item).collect();
        assert_eq!(
            ctx.transcript_inputs
                .retire_confirmed("lane-1", &source, &behind),
            1,
            "the mail id is in the transcript, so the ticket has nothing left to claim"
        );
        let mut items = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append("lane-1", &source, "", &mut items, &mut order);
        assert!(
            states.as_object().unwrap().is_empty(),
            "a retired ticket leaves the pinned queue entirely"
        );
    }

    /// A young ticket is not swept: the row may simply not be written yet, and reading megabytes
    /// on every tick for a mail that is about to pair normally would be the wrong trade.
    #[tokio::test]
    async fn a_ticket_younger_than_the_stuck_threshold_asks_for_no_catch_up_read() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let source = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(dir.path().join("session.jsonl")),
            session: Some("s1".into()),
        };
        ctx.transcript_inputs.sent(
            &ctx,
            "lane-1",
            mail_ticket(&source, "abc", "just sent", chrono::Utc::now()),
        );
        assert!(
            ctx.transcript_inputs
                .catch_up_floor("lane-1", &source)
                .is_none()
        );
    }

    /// The same mail body recurs in a long transcript, because the agent quotes it back. One
    /// envelope is one ticket, so a sweep that sees several matching rows must retire exactly the
    /// one ticket and never reach past it into a second, still-unread mail.
    #[tokio::test]
    async fn repeated_occurrences_of_one_mail_retire_one_ticket_and_leave_the_others_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let source = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(dir.path().join("session.jsonl")),
            session: Some("s1".into()),
        };
        let sent = chrono::Utc::now() - chrono::Duration::milliseconds(STUCK_MS + 1_000);
        let delivered = "2dd190b818e0323bb958c845ab7d2f0b";
        let unread = "7af6dadef35efab8a7c239ddb73cf4a6";
        ctx.transcript_inputs.sent(
            &ctx,
            "lane-1",
            mail_ticket(&source, delivered, "same body", sent),
        );
        ctx.transcript_inputs.sent(
            &ctx,
            "lane-1",
            mail_ticket(&source, unread, "same body", sent),
        );

        // The delivered mail appears several times over; the second mail is nowhere yet.
        let envelope = format!(
            "[REPOMAIL id={delivered} from=lane-2/1 reply_to=none] same body [{delivered}] [END REPOMAIL]"
        );
        let rows: Vec<_> = (0..5)
            .map(|n| {
                let mut row = repomon_core::agent::repomail::split(&envelope, Some(sent)).remove(0);
                row.id = Some(format!(
                    "{}:{}:0",
                    source.path.as_ref().unwrap().display(),
                    n * 100
                ));
                row
            })
            .collect();
        assert_eq!(
            ctx.transcript_inputs
                .retire_confirmed("lane-1", &source, &rows),
            1,
            "five echoes of one mail are still one delivered mail"
        );
        let mut items = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append("lane-1", &source, "", &mut items, &mut order);
        let states = states.as_object().unwrap().clone();
        assert_eq!(states.len(), 1, "the mail with no row stays pinned");
        assert_eq!(states.values().next().unwrap(), "sent");
    }

    /// A ticket whose evidence never arrives is never abandoned, so the sweep that looks for it
    /// must not cost more each time it runs. Measured before this cursor existed: one stuck ticket
    /// on a 26 MB session re-read up to 26 MB every five seconds, about 5 MB/s sustained and
    /// unbounded. A transcript only grows, so ground already covered cannot start holding the
    /// evidence later, and re-reading it buys nothing.
    #[tokio::test]
    async fn a_ticket_that_can_never_pair_rereads_only_what_the_source_has_newly_written() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        // The ticket's own mail id is never written, so no sweep can ever retire it.
        let path = transcript_with_mail_behind_the_page(dir.path(), "a-different-mail", "other");
        let source = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(path.clone()),
            session: Some("s1".into()),
        };
        let sent = chrono::Utc::now() - chrono::Duration::milliseconds(STUCK_MS + 1_000);
        ctx.transcript_inputs.sent(
            &ctx,
            "lane-1",
            mail_ticket(&source, "never-written", "absent", sent),
        );

        let sweep = |source: &Source| {
            let from = ctx
                .transcript_inputs
                .catch_up_floor("lane-1", source)
                .expect("a stuck ticket still asks to be looked for");
            let len = std::fs::metadata(source.path.as_ref().unwrap())
                .unwrap()
                .len();
            let read = len.saturating_sub(from);
            if len > from {
                let scan = scan_range(source, from, None).unwrap();
                ctx.transcript_inputs
                    .mark_swept("lane-1", source, scan.next_offset);
            }
            read
        };

        let first = sweep(&source);
        assert!(first > 128 * 1024, "the first read covers the whole range");
        // What remains is the trailing assistant group, which the scanner deliberately rewinds to
        // so a message still being streamed is re-read until it is complete. That is one message,
        // not one transcript, and it does not grow with the file.
        let repeat = sweep(&source);
        assert!(
            repeat * 10 < first,
            "a source that has not grown must not cost another full read: {repeat} vs {first}"
        );
        assert_eq!(sweep(&source), repeat, "and it stays flat, tick after tick");

        // Only what the agent has newly written is read, which is what tailing costs anyway.
        let appended = format!(
            "{}\n",
            json!({"type":"assistant","message":{"content":"y"}})
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, appended.as_bytes()).unwrap();
        drop(file);
        let after = sweep(&source);
        assert!(
            after >= appended.len() as u64 && after <= repeat + appended.len() as u64,
            "the read covers the new bytes and no more than the re-read group: {after}"
        );

        // The ticket is still pending and still reads as unread: nothing was silently dropped and
        // nothing was marked paired on no evidence.
        let mut items = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append("lane-1", &source, "", &mut items, &mut order);
        assert_eq!(states.as_object().unwrap().values().next().unwrap(), "sent");
    }

    /// A rewritten source is shorter than the cursor that was reading it, so the cursor no longer
    /// describes ground that exists. The next read starts over rather than reading past the end.
    #[tokio::test]
    async fn a_source_rewritten_shorter_than_the_cursor_is_swept_from_the_start_again() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let path = transcript_with_mail_behind_the_page(dir.path(), "a-different-mail", "other");
        let source = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: Some(path.clone()),
            session: Some("s1".into()),
        };
        let sent = chrono::Utc::now() - chrono::Duration::milliseconds(STUCK_MS + 1_000);
        let id = "2dd190b818e0323bb958c845ab7d2f0b";
        ctx.transcript_inputs
            .sent(&ctx, "lane-1", mail_ticket(&source, id, "body", sent));
        let long = std::fs::metadata(&path).unwrap().len();
        ctx.transcript_inputs.mark_swept("lane-1", &source, long);
        assert_eq!(
            ctx.transcript_inputs
                .catch_up_floor("lane-1", &source)
                .unwrap(),
            long
        );

        // The provider rewrites the session shorter, and the evidence now sits below the cursor.
        let rewritten = transcript_with_mail_behind_the_page(dir.path(), id, "body");
        assert_eq!(rewritten, path);
        std::fs::write(
            &path,
            format!(
                "{}\n",
                json!({"type":"user","message":{"content":format!("[REPOMAIL id={id} from=lane-2/1 reply_to=none] body [{id}] [END REPOMAIL]")}})
            ),
        )
        .unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let floor = ctx
            .transcript_inputs
            .catch_up_floor("lane-1", &source)
            .unwrap();
        assert!(len < floor, "the cursor is past the end of the new source");
        let from = if len < floor { 0 } else { floor };
        let scan = scan_range(&source, from, None).unwrap();
        let rows: Vec<_> = scan.transcript.into_iter().map(|e| e.item).collect();
        assert_eq!(
            ctx.transcript_inputs
                .retire_confirmed("lane-1", &source, &rows),
            1,
            "starting over finds the evidence the stale cursor would have skipped"
        );
    }

    fn ticket_for(
        kind: &str,
        text: &str,
        pane: Option<&str>,
        sent: chrono::DateTime<chrono::Utc>,
    ) -> Ticket {
        Ticket {
            source: None,
            floor: 0,
            item: TranscriptItem::new("user", text, Some(sent)),
            prior_prompts: pane.map(|pane| {
                repomon_core::agent::conversation_queue::pane_inputs(kind, pane).consumed
            }),
            consumed: false,
            submitted: text.into(),
            diagnosed: false,
        }
    }

    fn state_of(ctx: &Ctx, window: &str, source: &Source, pane: &str) -> String {
        let mut items = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append(window, source, pane, &mut items, &mut order);
        states
            .as_object()
            .unwrap()
            .values()
            .next()
            .and_then(|v| v.as_str())
            .unwrap()
            .into()
    }

    /// A kind whose pane has no prompt region to read, on a window whose transcript never bound.
    /// Nothing can ever report that the agent picked this up, so pinning it under "Not yet read by
    /// the agent" states something we do not know and can never withdraw. It must read as
    /// delivered instead, which is the whole of what we do know.
    #[tokio::test]
    async fn an_input_no_channel_can_confirm_is_delivered_not_pinned_as_unread() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let sent = chrono::Utc::now();
        let pane = "hermes> hh\nhermes> \n";
        for (kind, expected) in [("hermes", "delivered"), ("aider", "delivered")] {
            let window = format!("lane-{kind}");
            ctx.transcript_inputs
                .sent(&ctx, &window, ticket_for(kind, "hh", Some(pane), sent));
            let unbound = Source {
                window: window.clone(),
                kind: kind.into(),
                path: None,
                session: None,
            };
            assert_eq!(
                state_of(&ctx, &window, &unbound, pane),
                expected,
                "{kind} offers no pane region and no bound transcript"
            );
            // Binding the transcript hands `reconcile` a channel that can still retire the
            // ticket, so the unread claim becomes ours to make again.
            let bound = Source {
                path: Some(dir.path().join("session.jsonl")),
                session: Some("s1".into()),
                ..unbound
            };
            assert_eq!(state_of(&ctx, &window, &bound, pane), "sent");
        }
    }

    /// The operator's reproduction: send, then clear the agent's chat. Clearing destroys the pane
    /// echo, and a managed TUI on the alternate screen keeps no scrollback to recover it from, so
    /// the evidence is gone for good. Consumption already observed must survive that, and a kind
    /// that never had a channel must not be left pinned by it.
    #[tokio::test]
    async fn clearing_the_chat_after_the_send_neither_unconsumes_nor_strands_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let sent = chrono::Utc::now();
        let cleared = "\n  Antigravity CLI 1.2.2\n\n────\n>\n────\n? for shortcuts\n";
        let echoed = "────\n> hh\n\n▸ Thought for 5s\n────\n>\n────\n? for shortcuts\n";
        let before = "────\n>\n────\n? for shortcuts\n";

        ctx.transcript_inputs.sent(
            &ctx,
            "lane-agy",
            ticket_for("antigravity", "hh", Some(before), sent),
        );
        let agy = Source {
            window: "lane-agy".into(),
            kind: "antigravity".into(),
            path: None,
            session: None,
        };
        assert_eq!(
            state_of(&ctx, "lane-agy", &agy, echoed),
            "consumed",
            "antigravity echoes the prompt it has read"
        );
        assert_eq!(
            state_of(&ctx, "lane-agy", &agy, cleared),
            "consumed",
            "clearing the chat destroys the echo but not what it already proved"
        );

        // The same clear, on a kind with no pane region, where the echo was never readable.
        ctx.transcript_inputs.sent(
            &ctx,
            "lane-hermes",
            ticket_for("hermes", "hh", Some(before), sent),
        );
        let hermes = Source {
            window: "lane-hermes".into(),
            kind: "hermes".into(),
            path: None,
            session: None,
        };
        assert_eq!(state_of(&ctx, "lane-hermes", &hermes, cleared), "delivered");
    }

    /// Lock 2. A ticket prepared without a resolved source has no baseline, because without the
    /// kind there is no way to read the pane and an empty baseline would be a different claim: it
    /// asserts the pane held no earlier prompts, which would retire the next echo seen whether or
    /// not it was ours. What must not happen is the old outcome, where that ticket sat pinned as
    /// unread for the life of the window with nothing able to release it.
    #[tokio::test]
    async fn a_ticket_with_no_baseline_reports_what_it_knows_instead_of_pinning_forever() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let p = Params {
            lane_id: 1,
            session_id: None,
            window: Some("lane-1".into()),
            kind: None,
            before: None,
            on: true,
        };
        assert!(
            resolve_source(&ctx, &p, false, None).await.is_err(),
            "this window has no lane behind it, so the source cannot resolve"
        );
        let ticket = prepare_input_from_pane(&ctx, p.lane_id, "lane-1", "hh", Some("❯ hh\n"))
            .await
            .unwrap();
        assert!(ticket.prior_prompts.is_none(), "no kind, so no baseline");
        ctx.transcript_inputs.sent(&ctx, "lane-1", ticket);
        let unbound = Source {
            window: "lane-1".into(),
            kind: "claude-code".into(),
            path: None,
            session: None,
        };
        assert_eq!(
            state_of(&ctx, "lane-1", &unbound, "❯ hh\n\n────\n❯ \n────\n"),
            "delivered",
            "an observable kind with no baseline still has nothing that could confirm it"
        );
    }

    #[tokio::test]
    async fn antigravity_second_precision_consumes_only_the_new_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let sent = chrono::DateTime::parse_from_rfc3339("2026-09-11T16:39:12.900Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ticket = Ticket {
            source: None,
            floor: 0,
            item: TranscriptItem::new("user", "Write a poem", Some(sent)),
            prior_prompts: None,
            consumed: false,
            submitted: "Write a poem".into(),
            diagnosed: false,
        };
        ctx.transcript_inputs.sent(&ctx, "lane-1", ticket);
        let src = Source {
            window: "lane-1".into(),
            kind: "antigravity".into(),
            path: Some(dir.path().join("source.jsonl")),
            session: Some("session".into()),
        };
        let mut old = TranscriptItem::new(
            "user",
            "Write a poem",
            Some(sent - chrono::Duration::seconds(2)),
        );
        old.id = Some("old".into());
        let mut rows = vec![old];
        assert!(
            ctx.transcript_inputs
                .reconcile("lane-1", &src, &mut rows)
                .is_empty()
        );
        let mut current = TranscriptItem::new(
            "user",
            "Write a poem",
            chrono::DateTime::from_timestamp(sent.timestamp(), 0),
        );
        current.id = Some("current".into());
        rows.push(current);
        assert_eq!(
            ctx.transcript_inputs.reconcile("lane-1", &src, &mut rows),
            vec!["current"]
        );
        assert_eq!(rows[1].id.as_deref(), Some("sent:0"));
    }
}

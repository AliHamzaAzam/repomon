# Brief U1: Usage ledger, token and cost tracking across all agent kinds

Goal: Repomon knows how many tokens every managed agent burns, what it costs, and shows it
the way codeburn (github.com/getagentseal/codeburn) does, but fleet-aware: per agent kind,
per repo, per lane, per session, per day, with cache hit rates and a clear "what this cost"
answer. Everything local; no proxy, no API keys required; nothing leaves the machine.

Branch: `feat/usage-ledger` in a fresh worktree at `/private/tmp/repomon-feat-usage-ledger`
from current main. Daemon-heavy first, then a desktop view. Frontend work runs
`/frontend-design`, `/impeccable`, and the `dataviz` skill (charts must be theme-aware, no hex).

## Sources (read-only, per agent kind)

- **Claude Code**: `~/.claude/projects/<path>/<session>.jsonl`; each assistant message carries
  `message.usage` with `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
  `cache_read_input_tokens`, plus `model`; the transcript already parsed by
  `crates/repomon-core/src/agent/claude.rs` for status, so extend that reader rather than adding
  a second one. Also honor multiple accounts (`~/.claude-work` style roots the daemon already
  knows for the Extensions panel).
- **Codex**: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` token events (find the exact event
  shape by reading a real file on this machine; the model name is in the session header).
- **Antigravity**: the conversation store under `~/.gemini/antigravity-cli/` (the daemon already
  reads `cache/last_conversations` and the SQLite `-wal` manifest; find where per-turn token
  counts live, `brain/<id>/.system_generated/logs/transcript*.jsonl` carries usage per
  response; if no counts exist, estimate from content length at 4 chars per token and mark the
  row `estimated: true`).
- **OpenCode**: `~/.local/share/opencode/opencode*.db` (read-only SQLite; messages carry token
  counts).
- Unknown kinds: estimate from pane output length, flagged estimated.

Map every record to the fleet: session id or window -> lane -> repo (the daemon knows which
session ran in which window; for sessions outside Repomon's control, attribute to the repo by
the transcript's cwd and mark `external: true`).

## Ledger (SQLite, new migration next in sequence)

`usage_events(id, at, agent_kind, model, account, lane_id, repo_id, session_id, window,
input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, thinking_tokens, estimated,
source_path, source_offset)` with a unique key on `(source_path, source_offset)` so re-reads
are idempotent; an `ingest_cursors(source_path, offset, mtime)` table; daily rollups
`usage_daily(day, agent_kind, model, repo_id, ...)` maintained on ingest. Ingest runs from
`notify-debouncer-full` watchers on the source directories plus a full scan at daemon start and
every 10 minutes; bounded work per tick; never blocks the RPC loop (spawn_blocking).

## Pricing

`crates/repomon-core/src/pricing.rs`: a built-in table for the models in use (Claude 5 family,
Opus/Sonnet/Haiku, GPT-5 and Codex models, Gemini 3.x) with input, output, cache-read, and
cache-write rates per million tokens and an `effective_from` date; a `[usage] price_overrides`
config table; optional daily refresh from LiteLLM's public JSON (`model_prices_and_context_window.json`)
cached under the data dir, off by default, on by a config flag. Cost is computed at query time
from the ledger, so a price change re-prices history. Subscription accounts (Claude Max, Google
AI Pro) show "equivalent API cost" labeled as such, next to the existing quota percentages
from `usage_watch`.

## RPCs (local-only; read RPCs allowed over the bridge)

- `usage.summary { range: today|week|month|custom, group_by: kind|model|repo|lane|account }`
  -> totals, per-group rows, cache hit rate, estimated share.
- `usage.timeline { range, bucket: 15m|hour|day, group_by }` -> series for charts.
- `usage.sessions { range, lane_id?, limit }` -> per-session rows with task headline (from the
  transcript), tokens, cost, duration, tool calls, retries.
- `usage.export { range, format: csv|json }` -> file path under the data dir.
- `usage.ingest_now` and `usage.status` (cursors, last scan, errors).

## Desktop

A "Usage" view (toolbar entry next to Supervision, keymap entry, documented): headline tiles
(today, this week, this month; cost and tokens; cache hit rate; estimated share), a timeline
chart with bucket and group toggles, breakdown tables by kind, model, repo, lane, account, a
sessions table with search and an open-in-lane action, an "optimize" panel with plain findings
(top three cost drivers, cache miss hotspots, sessions with many retries, models that could be
cheaper for the task category), and Export. The sidebar's rate-limits card gains a one-line
cost-today figure. All colors via CSS variables through the dataviz palette guidance.

## CLI

`repomon usage today|week|month|report --group-by ... --csv`.

## Tests and gate

Fixture transcripts (redacted real files from this machine for each kind), idempotent ingest,
attribution to lanes, pricing math, rollups, RPC shapes, chart data reducers, the view's
empty and populated states. Gate: `cargo test -p repomon-core -p repomon-daemon -p repomon-tui`;
`bun run check`, `bun run test`, `bun run bindings:check`. Usual rules: worktree only, isolated
daemon for live checks (point the ingest at copied fixtures, never at the real transcripts from
a test), no hex colors, no emoji, no em-dashes, 1-line Conventional Commits, no merge or push.
Report with a before/after on this machine: totals per kind for the last 7 days from the real
transcripts (read-only), so the numbers can be sanity-checked against codeburn if the operator
installs it.

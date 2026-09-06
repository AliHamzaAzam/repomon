# Brief U3-fixes: review findings on feat/model-rates-settings

Same worktree `/private/tmp/repomon-feat-model-rates`, same branch, build on top of 5189670 (no
history rewrite). Rules as before. One commit per item, 1-line Conventional Commits, no co-author
trailer, no merge/push/bundle.

1. **CRITICAL, `crates/repomon-core/src/pricing.rs::apply_overrides` (about lines 194-206) and
   `crates/repomon-daemon/src/usage_ingest.rs::price_table` (about line 314).** An undated
   override now takes `effective_from` from the base row it inherits rates from; when that base is
   a LiteLLM snapshot row, `price_table` has stamped it with `Utc::now()`, so the override row is
   dated at table-build time and `cost(model, event.at)` never selects it for any stored event
   (all `at < now`). The Settings table resolves at `now` and shows "override" with the new
   numbers, while usage.summary keeps the old cost. Under the default `refresh_prices = true` this
   is the common case for dated ids like `claude-sonnet-5-20260901`; it is also a regression from
   main for undated overrides on dated ids. Fix both halves: (a) inherit RATES from the
   exact/prefix/alias base but inherit the DATE only from an exact-id row, else the epoch; (b) stop
   stamping snapshot rows with `Utc::now()`; use a fixed floor just after `builtin_effective_from()`
   so LiteLLM rates beat the built-in row and also price history. Regression test with
   `refresh_prices = true` and a cached snapshot fixture: override a dated Claude id, assert
   `usage.summary` cost_usd changes, not only `usage.models` source. Then a live check on an
   isolated daemon (apps/desktop/e2e/isolated.sh recipe) and quote the before/after cost.
2. **`crates/repomon-core/src/store/mod.rs` (about lines 2375-2397), the `usage.models` 30-day
   aggregation** is a full scan of usage_events with GROUP BY model and no index on model. Add
   migration `0028_usage_events_model.sql` with `CREATE INDEX idx_usage_events_model ON
   usage_events(model, at)` (or read tokens_30d from usage_daily and keep only MAX(at) on
   usage_events), and in `UsageSettingsView.tsx` memoize on `usage_refresh_prices` specifically so
   unrelated config patches do not re-issue `usage.models`. Test: migration applies on a fresh and
   an existing DB; EXPLAIN QUERY PLAN on a fixture DB shows the index.
3. Small: `RateInput`'s `placeholder:text-foreground` makes a blank "keep resolved" field look
   typed; use the muted placeholder color. Reject `usage_price_override_upsert` together with
   `usage_price_override_reset` in one `config.set` in the validation block.

Gate as before; report commit hashes, tests, gate tails, and the live before/after cost.

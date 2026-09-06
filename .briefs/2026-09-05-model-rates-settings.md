# Brief U3: Model rates in Settings (manual price overrides with a UI)

Run `/frontend-design` and `/impeccable` before touching UI and apply them throughout.

Context: usage pricing resolves overrides from `[usage.price_overrides."<model>"]` in config.toml,
then the daily LiteLLM snapshot, then the built-in table (`crates/repomon-core/src/pricing.rs`,
`crates/repomon-daemon/src/usage_ingest.rs::price_table`, `usage_rates.rs`). The only way to set
or correct a rate today is a hand edit of config.toml plus a daemon restart. The Usage view's
"No published rate for X ... set a price in Settings" warning links to Settings > System, which
has no price editor. Operator decision (2026-09-05): a Settings surface for model rates.

Branch: `feat/model-rates-settings` in a fresh worktree at `/private/tmp/repomon-feat-model-rates`
from current main (5095fcb or later). This brief is the complete scope.

## Scope

1. **Per-model rates RPC.** Extend `usage.rates` (or add `usage.models` if extending would bloat
   the status payload) so the daemon returns one row per model the ledger has ever seen plus every
   model that has an override: `model`, `input_per_mtok`, `output_per_mtok`, `cache_read_per_mtok`,
   `cache_write_per_mtok`, `source` (RateSource, plus `unpriced` when nothing matched and the row
   is on the generic fallback), `override` (the sparse PriceOverride if one exists), `last_seen`
   (from usage_events), `tokens_30d` (so the table can sort by what matters). ts-rs types, bindings
   regenerated, RpcMap entry, `docs/protocol.md`. Unit tests on the resolver: exact id, family
   prefix, override beating LiteLLM, an override with only `output_per_mtok` set keeping the other
   three from the snapshot.
2. **Overrides take effect without a restart.** Confirm that `config.set` with a patched
   `usage.price_overrides` updates the config the daemon's `price_table` reads (Ctx config) and
   that the next `usage.summary`/`usage.rates` reflects it. If it does not, make it so. Test:
   set an override through the RPC, query, see the new cost and `source: override`.
3. **Settings > Usage tab** in `SettingsModal.tsx` (new tab between the existing ones where it
   reads naturally; keep `openSettingsTab("usage")` working): the `[usage]` toggles that already
   exist in config (`enabled`, `refresh_prices`) with one-line explanations, the rates provenance
   line and Refresh (reuse the Usage view footnote's data), then a **Model rates** table: columns
   model, input, output, cache read, cache write (all per million tokens, money formatter from
   `usageMetrics.ts`), source badge (LiteLLM, override, built-in, unpriced), last seen, 30-day
   tokens; sortable; a filter box; unpriced rows first by default. Inline edit: clicking a rate
   cell (or an Edit action per row) edits the four fields with validation (non-negative number,
   blank means "keep the resolved value"), Save writes the sparse override through `config.set`
   (only the fields the user typed), Reset removes the override for that row. "Add model" lets the
   user type a model id or family prefix that the ledger has not seen yet (for future runs). Keep
   it keyboard reachable; empty state explains where rates come from. Tests with mocked RPCs:
   save writes only the typed fields, reset removes the key, sort and filter, the unpriced badge.
4. **Point the warning at it.** The Usage view's unpriced warning opens Settings > Usage with the
   filter prefilled to the first unpriced model. Test.
5. **CLI parity (small).** `repomon usage rates` already lists rates; add `repomon usage rates set
   <model> [--input N] [--output N] [--cache-read N] [--cache-write N]` and `repomon usage rates
   reset <model>` that write the same sparse override via `config.set`. Tests on argument parsing
   and the patch shape.
6. **Docs.** `docs/desktop.md` usage section: Settings > Usage, the override precedence, the
   family-prefix behavior; README one line.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock`
or copy the production database; never kill processes by name pattern; never `git add -A`; tests
use fixtures and mocked RPCs; live checks use an isolated daemon (see `apps/desktop/e2e/isolated.sh`).
Zero hex colors (CSS variables only), no emoji (SVG icons), no em-dashes in code, comments, or copy,
no `<Show keyed>` around anything expensive, async results token guarded. Gate: `cargo test -p
repomon-core -p repomon-daemon -p repomon-tui`; in `apps/desktop` `bun run check`, `bun run test`,
`bun run bindings:check`. Commits: 1-line Conventional Commits per numbered item where sensible,
no co-author trailer. Do not merge, push, or build the bundle. Report commit hashes, per-item
summary, test names, gate tails, and what could not be verified without a screenshot.

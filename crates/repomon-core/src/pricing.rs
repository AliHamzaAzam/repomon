//! Model prices and cost arithmetic for the usage ledger.
//!
//! Cost is never stored: the ledger keeps token counts, and every query re-prices them through a
//! [`PriceTable`]. A price change therefore re-prices history, and an operator who disagrees with
//! a built-in rate can correct it from `[usage.price_overrides]` without a rebuild.
//!
//! Rates are US dollars per million tokens. Cache-read and cache-write rates are the discounted
//! and surcharged input rates the providers publish for prompt caching. Every row carries an
//! `effective_from` instant; [`PriceTable::lookup`] picks the newest row not after the event, so
//! a price cut applies from its date forward and older events keep the price they were billed at.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Token counts for one priced unit of work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounts {
    /// Uncached input tokens.
    pub input: u64,
    /// Generated tokens, including any thinking tokens the provider bills as output.
    pub output: u64,
    /// Tokens served from the prompt cache.
    pub cache_read: u64,
    /// Tokens written into the prompt cache.
    pub cache_write: u64,
}

/// Where a resolved rate came from. Config overrides win over a LiteLLM snapshot, which wins over
/// the built-in table — the built-in table is the floor every model can fall back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum RateSource {
    /// Shipped with repomon, dated the day it was last checked against a published price list.
    Builtin,
    /// Parsed from a cached LiteLLM `model_prices_and_context_window.json` snapshot.
    Litellm,
    /// An operator correction from `[usage.price_overrides]`.
    Override,
}

impl std::fmt::Display for RateSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RateSource::Builtin => "builtin",
            RateSource::Litellm => "litellm",
            RateSource::Override => "override",
        })
    }
}

/// One model's rates, in dollars per million tokens, from `effective_from` onward.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPrice {
    /// The model id, or the family prefix a dated model id falls back to.
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
    /// The instant this row starts applying. Events before it fall to an older row, or to nothing.
    pub effective_from: DateTime<Utc>,
    /// Where this row came from, so a resolved price carries its own provenance.
    pub source: RateSource,
}

/// A `[usage.price_overrides."model"]` entry. Every field is optional: a partial override edits
/// only the rates it names and inherits the rest from the built-in row for that model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceOverride {
    pub input_per_mtok: Option<f64>,
    pub output_per_mtok: Option<f64>,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    /// When the override starts applying. Defaults to the date of the built-in row it replaces,
    /// so an undated override supersedes that row rather than sitting behind it in history.
    pub effective_from: Option<DateTime<Utc>>,
}

/// The prices the ledger reads. Rows are matched by exact model id first, then by the longest
/// model prefix, so `claude-haiku-4-5-20251001` prices off the `claude-haiku-4-5` row.
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    rows: Vec<ModelPrice>,
}

/// The date the built-in rates were last checked against the published price lists.
fn builtin_effective_from() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

/// UTC midnight on the day the table is built, so a placeholder rate dated "today" still applies
/// to every event from earlier today rather than only ones after this exact instant.
fn today_utc_midnight() -> DateTime<Utc> {
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .unwrap_or(now)
}

impl PriceTable {
    /// A table with no rows. Every lookup misses.
    pub fn empty() -> Self {
        PriceTable::default()
    }

    /// The rates shipped with repomon: the Claude 5 family and its Opus, Sonnet and Haiku
    /// siblings, the GPT-5 and Codex models, and Gemini 3.x. Provider list prices as published
    /// for first-party API access; subscription plans bill differently, which is why the desktop
    /// labels a subscription account's number "equivalent API cost".
    pub fn builtin() -> Self {
        let from = builtin_effective_from();
        // (model or family prefix, input, output, cache read, cache write)
        const ROWS: &[(&str, f64, f64, f64, f64)] = &[
            // Anthropic.
            ("claude-fable-5-1", 10.0, 50.0, 0.25, 12.5),
            ("claude-fable-5", 10.0, 50.0, 1.0, 12.5),
            ("claude-mythos-5-1", 10.0, 50.0, 0.25, 12.5),
            ("claude-mythos-5", 10.0, 50.0, 1.0, 12.5),
            ("claude-opus-5", 5.0, 25.0, 0.5, 6.25),
            ("claude-opus-4-8", 5.0, 25.0, 0.5, 6.25),
            ("claude-opus-4-7", 5.0, 25.0, 0.5, 6.25),
            ("claude-opus-4-6", 5.0, 25.0, 0.5, 6.25),
            ("claude-opus-4-5", 5.0, 25.0, 0.5, 6.25),
            ("claude-sonnet-5", 2.0, 10.0, 0.2, 2.5),
            ("claude-sonnet-4-6", 3.0, 15.0, 0.3, 3.75),
            ("claude-sonnet-4-5", 3.0, 15.0, 0.3, 3.75),
            ("claude-haiku-4-5", 1.0, 5.0, 0.1, 1.25),
            // OpenAI. The Codex CLI reports its own internal model ids, which share the GPT-5
            // rate card.
            ("gpt-5", 1.25, 10.0, 0.125, 1.25),
            ("codex-", 1.25, 10.0, 0.125, 1.25),
            // Google.
            ("gemini-3-pro", 2.0, 12.0, 0.2, 2.5),
            ("gemini-3-flash", 0.3, 2.5, 0.03, 0.375),
            ("gemini-3", 2.0, 12.0, 0.2, 2.5),
        ];
        let mut table = PriceTable::empty();
        for (model, input, output, cache_read, cache_write) in ROWS {
            table.insert(ModelPrice {
                model: (*model).to_string(),
                input_per_mtok: *input,
                output_per_mtok: *output,
                cache_read_per_mtok: *cache_read,
                cache_write_per_mtok: *cache_write,
                effective_from: from,
                source: RateSource::Builtin,
            });
        }
        // GPT-6 has no published rate card yet. This copies the GPT-5 row as a best-effort
        // placeholder rather than leaving the family unpriced, dated from today so it never
        // reprices something billed before the family existed. Replace it the moment a real rate
        // is published, with `[usage.price_overrides."gpt-6"]`.
        table.insert(ModelPrice {
            model: "gpt-6".to_string(),
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cache_read_per_mtok: 0.125,
            cache_write_per_mtok: 1.25,
            effective_from: today_utc_midnight(),
            source: RateSource::Builtin,
        });
        table
    }

    /// Add one row. Rows for the same model at different dates coexist.
    pub fn insert(&mut self, price: ModelPrice) {
        self.rows.push(price);
    }

    /// How many rows the table holds.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the table has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Apply operator overrides. A partial override inherits the rates it omits from the newest
    /// existing row for that model, so correcting one rate does not blank the others.
    pub fn apply_overrides<I>(&mut self, overrides: I)
    where
        I: IntoIterator<Item = (String, PriceOverride)>,
    {
        for (model, over) in overrides {
            let base = self
                .rows
                .iter()
                .filter(|r| r.model == model)
                .max_by_key(|r| r.effective_from)
                .cloned();
            let effective_from = over
                .effective_from
                .or_else(|| base.as_ref().map(|b| b.effective_from))
                .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
            let row = ModelPrice {
                model: model.clone(),
                input_per_mtok: over
                    .input_per_mtok
                    .or(base.as_ref().map(|b| b.input_per_mtok))
                    .unwrap_or(0.0),
                output_per_mtok: over
                    .output_per_mtok
                    .or(base.as_ref().map(|b| b.output_per_mtok))
                    .unwrap_or(0.0),
                cache_read_per_mtok: over
                    .cache_read_per_mtok
                    .or(base.as_ref().map(|b| b.cache_read_per_mtok))
                    .unwrap_or(0.0),
                cache_write_per_mtok: over
                    .cache_write_per_mtok
                    .or(base.as_ref().map(|b| b.cache_write_per_mtok))
                    .unwrap_or(0.0),
                effective_from,
                source: RateSource::Override,
            };
            // An override at the same instant replaces the row it overrides rather than racing it.
            self.rows
                .retain(|r| !(r.model == model && r.effective_from == effective_from));
            self.rows.push(row);
        }
    }

    /// The rates for `model` as of `at`.
    ///
    /// Three steps, in order, and the first that hits wins:
    /// 1. An exact row for `model` itself (newest one not after `at`).
    /// 2. `model`'s alias (see [`resolve_alias`]), matched the same way (exact, then that alias's
    ///    own longest family prefix). This runs before step 3 on purpose: an alias exists
    ///    precisely because `model`'s own name is not what LiteLLM published it under, so its
    ///    resolved row is a better rate than falling through to a generic family prefix of
    ///    `model`'s own (unaliased) name.
    /// 3. `model`'s longest family prefix among the table's own rows.
    pub fn lookup(&self, model: &str, at: DateTime<Utc>) -> Option<&ModelPrice> {
        if let Some(hit) = self.exact_match(model, at) {
            return Some(hit);
        }
        if let Some(alias) = resolve_alias(model) {
            // Guard against a self-mapped alias (kept for a couple of entries as a documented,
            // pinned no-op — see `ALIASES`): re-running the exact match on the same string would
            // just repeat step 1's miss, so only take the alias branch when it names somewhere new.
            if alias != model {
                if let Some(hit) = self.exact_match(alias, at).or_else(|| self.prefix_match(alias, at)) {
                    return Some(hit);
                }
            }
        }
        self.prefix_match(model, at)
    }

    /// The newest row not after `at` whose model id is exactly `model`.
    fn exact_match(&self, model: &str, at: DateTime<Utc>) -> Option<&ModelPrice> {
        self.rows
            .iter()
            .filter(|r| r.effective_from <= at && r.model == model)
            .max_by_key(|r| r.effective_from)
    }

    /// The row whose model id is the longest prefix of `model`, among rows not after `at`. Ties
    /// on length go to the newer row.
    fn prefix_match(&self, model: &str, at: DateTime<Utc>) -> Option<&ModelPrice> {
        let mut best: Option<&ModelPrice> = None;
        let mut best_len = 0usize;
        for row in &self.rows {
            if row.effective_from > at || !model.starts_with(&row.model) {
                continue;
            }
            let len = row.model.len();
            let better = match best {
                None => true,
                Some(b) => {
                    len > best_len || (len == best_len && row.effective_from > b.effective_from)
                }
            };
            if better {
                best = Some(row);
                best_len = len;
            }
        }
        best
    }

    /// The count of distinct model rows currently priced by each source, keyed by model id: for
    /// each name in the table, whichever row is newest as of now decides that model's source. An
    /// override always wins its model's slot the moment it's applied, since `apply_overrides`
    /// dates it at or after the row it replaces.
    pub fn source_counts(&self) -> RateSourceCounts {
        let mut newest: HashMap<&str, &ModelPrice> = HashMap::new();
        for row in &self.rows {
            newest
                .entry(row.model.as_str())
                .and_modify(|cur| {
                    if row.effective_from >= cur.effective_from {
                        *cur = row;
                    }
                })
                .or_insert(row);
        }
        let mut counts = RateSourceCounts::default();
        for row in newest.values() {
            match row.source {
                RateSource::Builtin => counts.builtin += 1,
                RateSource::Litellm => counts.litellm += 1,
                RateSource::Override => counts.overrides += 1,
            }
        }
        counts
    }

    /// What `tokens` cost on `model` at `at`, in dollars. `None` when the model has no price.
    ///
    /// A free-tier model id costs zero rather than nothing-known: it has a published rate, and
    /// that rate is zero, so it is priced instead of being reported as a gap in the table.
    pub fn cost(&self, model: &str, at: DateTime<Utc>, tokens: &TokenCounts) -> Option<f64> {
        let p = match self.lookup(model, at) {
            Some(p) => p,
            None if is_free_tier(model) => return Some(0.0),
            None => return None,
        };
        const PER: f64 = 1_000_000.0;
        Some(
            tokens.input as f64 / PER * p.input_per_mtok
                + tokens.output as f64 / PER * p.output_per_mtok
                + tokens.cache_read as f64 / PER * p.cache_read_per_mtok
                + tokens.cache_write as f64 / PER * p.cache_write_per_mtok,
        )
    }
}

/// Whether `model` is a provider's free tier, whose published rate is zero.
///
/// OpenCode names these `<model>-free`. They are priced rather than reported as unpriced, so the
/// "no published rate" warning stays about models that really would cost money.
pub fn is_free_tier(model: &str) -> bool {
    model.ends_with("-free")
}

/// Model ids a CLI logs that do not (or might not) match a LiteLLM key by name or family prefix,
/// mapped to the closest LiteLLM key to price them off instead.
///
/// This is a name fix, not a rate invention: every target here is a real LiteLLM entry. When
/// LiteLLM renames or drops one of these, the alias just stops matching and the model falls back
/// to the family-prefix rule, then to the built-in table, then to unpriced — it never fabricates
/// a number.
const ALIASES: &[(&str, &str)] = &[
    // The mythos-5-1 codename has no LiteLLM entry of its own yet; its family's current release
    // (mythos-5) is the closest published rate.
    ("claude-mythos-5-1", "claude-mythos-5"),
    // The Codex CLI's auto-review model id is internal; it shares the Codex rate card.
    ("codex-auto-review", "gpt-5-codex"),
    // The tiered-pricing label the desktop reads from Antigravity has no LiteLLM entry itself;
    // the underlying model does.
    ("gemini-3.8-flash-tiered", "gemini-3.8-flash"),
    // Pinned rather than left to an exact-match coincidence: the target is the id this codebase
    // already expects LiteLLM to publish for it.
    ("claude-fable-5-1", "claude-fable-5-1"),
    ("gpt-5.6-sol", "gpt-5.6-sol"),
    ("gpt-5.6-luna", "gpt-5.6-luna"),
];

/// The LiteLLM key `model` should price off when neither its exact id nor its family prefix is in
/// the table, or `None` when `model` has no known alias.
pub fn resolve_alias(model: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(from, _)| *from == model)
        .map(|(_, to)| *to)
}

/// How many of the table's priced models come from each source, for the pricing footnote and
/// `usage.rates`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct RateSourceCounts {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub builtin: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub litellm: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub overrides: u64,
}

impl RateSourceCounts {
    /// Every priced model the table currently resolves, across all three sources.
    pub fn total(&self) -> u64 {
        self.builtin + self.litellm + self.overrides
    }
}

/// What the ledger knows about its price rates: where they came from, how fresh the LiteLLM
/// snapshot is, and whether the last fetch failed. Read by `usage.rates` and printed by
/// `repomon usage rates` and the Usage view's pricing footnote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct RatesStatus {
    pub source_counts: RateSourceCounts,
    /// When the cached LiteLLM snapshot was last fetched (a 304 counts as a fetch: it confirmed
    /// the cache is current). `None` until the first successful fetch.
    pub fetched_at: Option<DateTime<Utc>>,
    /// The cached snapshot's ETag, so a repeat fetch can ask LiteLLM for only what changed.
    pub etag: Option<String>,
    /// When the daily refresh next runs, given `fetched_at` and the 24h cadence. `None` when
    /// refreshing is off or nothing has been fetched yet.
    pub next_refresh_at: Option<DateTime<Utc>>,
    /// The last fetch's error, if it failed. Cleared by the next successful fetch.
    pub last_error: Option<String>,
    /// Whether `[usage] refresh_prices` is on. Off means the counts above are builtin/override
    /// only, and `fetched_at`/`next_refresh_at` stay `None`.
    pub enabled: bool,
}

/// The Usage view's pricing footnote and the CLI's `repomon usage rates` summary line: one string
/// stating where rates came from, so both surfaces agree on the wording.
///
/// `now` is threaded through rather than read internally so the "updated Nh ago" phrasing is
/// deterministic in tests.
pub fn format_rates_footnote(status: &RatesStatus, now: DateTime<Utc>) -> String {
    if !status.enabled {
        return format!(
            "Rates: built-in only ({} models). LiteLLM refresh is off ([usage] refresh_prices).",
            status.source_counts.total()
        );
    }
    if let Some(err) = &status.last_error {
        return format!(
            "Rates: LiteLLM fetch failed ({err}); using {} cached/built-in model(s).",
            status.source_counts.total()
        );
    }
    let mut line = match status.fetched_at {
        Some(at) => {
            let age = (now - at).max(chrono::Duration::zero());
            format!(
                "Rates: LiteLLM, updated {} ({} models)",
                humanize_age(age),
                status.source_counts.litellm
            )
        }
        None => "Rates: LiteLLM not fetched yet".to_string(),
    };
    if status.source_counts.overrides > 0 {
        line.push_str(&format!(", {} from overrides", status.source_counts.overrides));
    }
    if status.source_counts.builtin > 0 {
        line.push_str(&format!(", {} built-in", status.source_counts.builtin));
    }
    line
}

/// A short "3h ago" / "2d ago" / "just now" phrase for a non-negative duration.
fn humanize_age(age: chrono::Duration) -> String {
    let mins = age.num_minutes();
    if mins < 1 {
        return "just now".to_string();
    }
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = age.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", age.num_days())
}

/// Parse a LiteLLM `model_prices_and_context_window.json` snapshot into price rows.
///
/// LiteLLM quotes dollars per token; repomon quotes dollars per million, so every rate is scaled
/// by a million. Entries without both an input and an output cost are skipped: they are
/// embeddings, rerankers and audio models, none of which a coding agent bills against.
pub fn parse_litellm_snapshot(
    json: &str,
    effective_from: DateTime<Utc>,
) -> Result<Vec<ModelPrice>> {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for (model, entry) in raw {
        let obj = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };
        let num = |key: &str| obj.get(key).and_then(serde_json::Value::as_f64);
        let (input, output) = match (num("input_cost_per_token"), num("output_cost_per_token")) {
            (Some(i), Some(o)) => (i, o),
            _ => continue,
        };
        const PER: f64 = 1_000_000.0;
        out.push(ModelPrice {
            model,
            input_per_mtok: input * PER,
            output_per_mtok: output * PER,
            cache_read_per_mtok: num("cache_read_input_token_cost").unwrap_or(0.0) * PER,
            cache_write_per_mtok: num("cache_creation_input_token_cost").unwrap_or(input) * PER,
            effective_from,
            source: RateSource::Litellm,
        });
    }
    if out.is_empty() {
        return Err(Error::Other("price snapshot held no priced models".into()));
    }
    out.sort_by(|a, b| a.model.cmp(&b.model));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    #[test]
    fn builtin_table_prices_a_claude_sonnet_call() {
        let table = PriceTable::builtin();
        let price = table.lookup("claude-sonnet-5", at(2026, 9, 1)).unwrap();
        assert!(price.input_per_mtok > 0.0);
        assert!(price.output_per_mtok > price.input_per_mtok);
        assert!(price.cache_read_per_mtok < price.input_per_mtok);
    }

    #[test]
    fn cost_scales_linearly_with_tokens() {
        let table = PriceTable::builtin();
        let one = table
            .cost(
                "claude-sonnet-5",
                at(2026, 9, 1),
                &TokenCounts {
                    input: 1_000_000,
                    ..Default::default()
                },
            )
            .unwrap();
        let two = table
            .cost(
                "claude-sonnet-5",
                at(2026, 9, 1),
                &TokenCounts {
                    input: 2_000_000,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!((two - one * 2.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_has_no_price() {
        let table = PriceTable::builtin();
        assert!(
            table
                .lookup("totally-made-up-model", at(2026, 9, 1))
                .is_none()
        );
    }

    #[test]
    fn override_wins_over_builtin() {
        let mut table = PriceTable::builtin();
        table.apply_overrides([(
            "claude-sonnet-5".to_string(),
            PriceOverride {
                input_per_mtok: Some(1.0),
                output_per_mtok: Some(2.0),
                cache_read_per_mtok: Some(0.1),
                cache_write_per_mtok: Some(1.25),
                effective_from: None,
            },
        )]);
        let price = table.lookup("claude-sonnet-5", at(2026, 9, 1)).unwrap();
        assert_eq!(price.input_per_mtok, 1.0);
        assert_eq!(price.output_per_mtok, 2.0);
    }

    #[test]
    fn newest_effective_row_not_after_the_event_wins() {
        let mut table = PriceTable::empty();
        table.insert(ModelPrice {
            model: "m".into(),
            input_per_mtok: 1.0,
            output_per_mtok: 1.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            effective_from: at(2026, 1, 1),
            source: RateSource::Builtin,
        });
        table.insert(ModelPrice {
            model: "m".into(),
            input_per_mtok: 9.0,
            output_per_mtok: 9.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            effective_from: at(2026, 6, 1),
            source: RateSource::Builtin,
        });
        assert_eq!(
            table.lookup("m", at(2026, 3, 1)).unwrap().input_per_mtok,
            1.0
        );
        assert_eq!(
            table.lookup("m", at(2026, 9, 1)).unwrap().input_per_mtok,
            9.0
        );
        assert!(table.lookup("m", at(2025, 1, 1)).is_none());
    }

    #[test]
    fn dated_model_ids_fall_back_to_the_family_prefix() {
        let table = PriceTable::builtin();
        assert!(
            table
                .lookup("claude-haiku-4-5-20251001", at(2026, 9, 1))
                .is_some()
        );
    }

    #[test]
    fn cache_write_is_charged_above_input() {
        let table = PriceTable::builtin();
        let write = table
            .cost(
                "claude-sonnet-5",
                at(2026, 9, 1),
                &TokenCounts {
                    cache_write: 1_000_000,
                    ..Default::default()
                },
            )
            .unwrap();
        let input = table
            .cost(
                "claude-sonnet-5",
                at(2026, 9, 1),
                &TokenCounts {
                    input: 1_000_000,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(write > input);
    }

    #[test]
    fn gpt_6_prices_off_the_placeholder_family_row() {
        let table = PriceTable::builtin();
        let price = table.lookup("gpt-6-astra", Utc::now()).unwrap();
        assert_eq!(price.input_per_mtok, 1.25);
        assert_eq!(price.output_per_mtok, 10.0);
        assert!(
            table.cost("gpt-6-astra", Utc::now(), &TokenCounts::default()).is_some(),
            "a gpt-6 id should price rather than read as a gap in the table"
        );
    }

    #[test]
    fn a_free_tier_id_costs_zero_rather_than_nothing_known() {
        let table = PriceTable::builtin();
        let tokens = TokenCounts {
            input: 1_000,
            output: 2_000,
            cache_read: 0,
            cache_write: 0,
        };
        assert!(is_free_tier("kimi-k2-thinking-free"));
        assert_eq!(
            table.cost("kimi-k2-thinking-free", at(2026, 9, 1), &tokens),
            Some(0.0)
        );
        assert_eq!(
            table.cost("kimi-k2-thinking", at(2026, 9, 1), &tokens),
            None,
            "a paid model with no row is still a gap in the table"
        );
    }

    #[test]
    fn litellm_snapshot_parses_into_price_rows() {
        let json = r#"{
          "sample-model": {"input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015,
            "cache_read_input_token_cost": 0.0000003, "cache_creation_input_token_cost": 0.00000375},
          "sample-embedding": {"input_cost_per_token": 0.0000001}
        }"#;
        let rows = parse_litellm_snapshot(json, at(2026, 9, 1)).unwrap();
        let m = rows.iter().find(|r| r.model == "sample-model").unwrap();
        assert!((m.input_per_mtok - 3.0).abs() < 1e-9);
        assert!((m.output_per_mtok - 15.0).abs() < 1e-9);
        assert!((m.cache_read_per_mtok - 0.3).abs() < 1e-9);
        assert!((m.cache_write_per_mtok - 3.75).abs() < 1e-9);
        assert!(rows.iter().all(|r| r.model != "sample-embedding"));
    }

    #[test]
    fn an_alias_prices_a_model_that_has_no_row_of_its_own() {
        let mut table = PriceTable::empty();
        table.insert(ModelPrice {
            model: "claude-mythos-5".into(),
            input_per_mtok: 4.0,
            output_per_mtok: 20.0,
            cache_read_per_mtok: 0.4,
            cache_write_per_mtok: 5.0,
            effective_from: at(2026, 8, 1),
            source: RateSource::Litellm,
        });
        // "claude-mythos-5-1" has no row of its own here, and is not a prefix match for
        // "claude-mythos-5" (prefix matching only runs the other direction: the table's row must
        // prefix the query, not the reverse) — only the alias resolves it.
        let price = table.lookup("claude-mythos-5-1", at(2026, 9, 1)).unwrap();
        assert_eq!(price.source, RateSource::Litellm);
        assert_eq!(price.input_per_mtok, 4.0);
    }

    #[test]
    fn an_alias_beats_a_generic_family_prefix_on_the_unaliased_name() {
        // The built-in table prices anything starting with "codex-" off the generic GPT-5 rate
        // card. "codex-auto-review" also has a specific LiteLLM entry via its alias
        // ("gpt-5-codex") — that should win over the generic built-in prefix.
        let mut table = PriceTable::builtin();
        table.insert(ModelPrice {
            model: "gpt-5-codex".into(),
            input_per_mtok: 9.0,
            output_per_mtok: 90.0,
            cache_read_per_mtok: 0.9,
            cache_write_per_mtok: 9.0,
            effective_from: at(2026, 8, 1),
            source: RateSource::Litellm,
        });
        let price = table.lookup("codex-auto-review", at(2026, 9, 1)).unwrap();
        assert_eq!(price.source, RateSource::Litellm);
        assert_eq!(price.input_per_mtok, 9.0);
    }

    #[test]
    fn a_self_mapped_alias_does_not_infinite_loop_or_change_the_result() {
        // "gpt-5.6-sol" aliases to itself (pinned defensively). With no matching row anywhere it
        // must still terminate and report unpriced, not loop.
        let table = PriceTable::empty();
        assert!(table.lookup("gpt-5.6-sol", at(2026, 9, 1)).is_none());
    }

    #[test]
    fn an_unaliased_unmatched_model_stays_unpriced() {
        let table = PriceTable::builtin();
        assert!(table.lookup("some-unknown-model-9000", at(2026, 9, 1)).is_none());
    }

    #[test]
    fn source_counts_report_the_active_row_per_model() {
        let mut table = PriceTable::empty();
        table.insert(ModelPrice {
            model: "a".into(),
            input_per_mtok: 1.0,
            output_per_mtok: 1.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            effective_from: at(2026, 1, 1),
            source: RateSource::Builtin,
        });
        table.insert(ModelPrice {
            model: "b".into(),
            input_per_mtok: 2.0,
            output_per_mtok: 2.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            effective_from: at(2026, 1, 1),
            source: RateSource::Litellm,
        });
        table.apply_overrides([(
            "a".to_string(),
            PriceOverride {
                input_per_mtok: Some(9.0),
                ..Default::default()
            },
        )]);
        let counts = table.source_counts();
        assert_eq!(counts.overrides, 1, "the override replaces \"a\"'s slot");
        assert_eq!(counts.litellm, 1);
        assert_eq!(counts.builtin, 0);
        assert_eq!(counts.total(), 2);
    }

    #[test]
    fn footnote_reports_litellm_freshness_and_the_other_sources() {
        let status = RatesStatus {
            source_counts: RateSourceCounts {
                builtin: 4,
                litellm: 12,
                overrides: 2,
            },
            fetched_at: Some(at(2026, 9, 1) - chrono::Duration::hours(3)),
            etag: Some("etag-1".into()),
            next_refresh_at: Some(at(2026, 9, 2)),
            last_error: None,
            enabled: true,
        };
        let line = format_rates_footnote(&status, at(2026, 9, 1));
        assert!(line.contains("LiteLLM"), "{line}");
        assert!(line.contains("3h"), "{line}");
        assert!(line.contains("12 models"), "{line}");
        assert!(line.contains("2 from overrides"), "{line}");
        assert!(line.contains("4 built-in"), "{line}");
    }

    #[test]
    fn footnote_surfaces_a_failed_fetch_rather_than_hiding_it() {
        let status = RatesStatus {
            source_counts: RateSourceCounts::default(),
            fetched_at: None,
            etag: None,
            next_refresh_at: None,
            last_error: Some("connection timed out".into()),
            enabled: true,
        };
        let line = format_rates_footnote(&status, at(2026, 9, 1));
        assert!(line.contains("failed"), "{line}");
        assert!(line.contains("connection timed out"), "{line}");
    }

    #[test]
    fn footnote_says_refresh_is_off_when_it_is() {
        let status = RatesStatus {
            source_counts: RateSourceCounts {
                builtin: 20,
                litellm: 0,
                overrides: 0,
            },
            enabled: false,
            ..Default::default()
        };
        let line = format_rates_footnote(&status, at(2026, 9, 1));
        assert!(line.contains("off"), "{line}");
        assert!(line.contains("built-in"), "{line}");
    }
}

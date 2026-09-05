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
            };
            // An override at the same instant replaces the row it overrides rather than racing it.
            self.rows
                .retain(|r| !(r.model == model && r.effective_from == effective_from));
            self.rows.push(row);
        }
    }

    /// The rates for `model` as of `at`: the newest row not after `at`, matching the model id
    /// exactly if possible and otherwise by its longest family prefix.
    pub fn lookup(&self, model: &str, at: DateTime<Utc>) -> Option<&ModelPrice> {
        let mut best: Option<&ModelPrice> = None;
        let mut best_len = 0usize;
        for row in &self.rows {
            if row.effective_from > at {
                continue;
            }
            let matched = if row.model == model {
                true
            } else {
                model.starts_with(&row.model)
            };
            if !matched {
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
        });
        table.insert(ModelPrice {
            model: "m".into(),
            input_per_mtok: 9.0,
            output_per_mtok: 9.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            effective_from: at(2026, 6, 1),
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
}

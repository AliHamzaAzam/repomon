//! One test in its own process so its fixture cache environment cannot race other test cases.
use chrono::{TimeZone, Utc};
use repomon_core::{Config, Store};
use repomon_core::usage_ledger::UsageEvent;
use repomon_daemon::{Ctx, conn::{ConnSession, ConnKind}, rpc::dispatch};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn cached_snapshot_and_undated_overrides_reprice_historical_summary() {
    let dir = tempfile::tempdir().unwrap();
    // This integration-test binary contains only this test; no other test shares its environment.
    unsafe { std::env::set_var("REPOMON_DATA_DIR", dir.path()); }
    let cache = dir.path().join("prices");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("litellm.json"), include_str!("fixtures/model_rates_snapshot.json")).unwrap();
    let store = Store::open_in_memory().unwrap();
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
    let models = ["claude-sonnet-5-20260901", "claude-haiku-4-5-20251001"];
    store.record_usage_events(models.iter().enumerate().map(|(offset, model)| UsageEvent {
        at, agent_kind: "claude-code".into(), model: (*model).into(), account: "fixture".into(),
        lane_id: None, repo_id: None, session_id: None, window: None, cwd: None,
        input_tokens: 1_000_000, output_tokens: 1_000_000, cache_read_tokens: 0,
        cache_write_tokens: 0, thinking_tokens: 0, estimated: false, subagent: false,
        external: true, source_path: "fixture.jsonl".into(), source_offset: offset as i64,
    }).collect()).await.unwrap();
    let config = Config::default();
    assert!(config.usage.refresh_prices);
    let ctx = Ctx::new_with_config_path(store, config, None, dir.path().join("config.toml"));
    let sess = Arc::new(ConnSession::new(1, ConnKind::Local));
    let params = json!({ "range": "custom", "since": "2026-09-01T00:00:00Z",
        "until": "2026-09-02T00:00:00Z", "group_by": "model" });
    let before = dispatch(&ctx, &sess, "usage.summary", Some(params.clone())).await.unwrap();
    assert_eq!(before["totals"]["cost_usd"], 36.0, "snapshot must price stored events before overrides");
    for model in models {
        dispatch(&ctx, &sess, "config.set", Some(json!({ "usage_price_override_upsert": {
            "model": model, "output_per_mtok": 8.0
        }}))).await.unwrap();
    }
    let after = dispatch(&ctx, &sess, "usage.summary", Some(params)).await.unwrap();
    assert_eq!(after["totals"]["cost_usd"], 22.0);
    let groups = after["groups"].as_array().unwrap();
    assert_eq!(groups.iter().find(|r| r["key"] == models[0]).unwrap()["totals"]["cost_usd"], 12.0);
    assert_eq!(groups.iter().find(|r| r["key"] == models[1]).unwrap()["totals"]["cost_usd"], 10.0);
    let rates = dispatch(&ctx, &sess, "usage.models", None).await.unwrap();
    assert!(rates.as_array().unwrap().iter().all(|r| r["source"] == "override"));
}

//! Supervision policy snapshot and resolution for the daemon.

use std::collections::HashMap;

use repomon_core::agent::supervision::{SupervisionPolicy, resolve};
use repomon_core::model::LaneId;

use crate::Ctx;

/// Cached in-memory snapshot of effective supervision policies for all enabled lanes.
#[derive(Debug, Clone, Default)]
pub struct PolicySnapshot {
    /// Global supervision master switch (`config.supervision.enabled`).
    pub master: bool,
    /// Effective policies for lanes where supervision is active (both master and lane enabled).
    pub lanes: HashMap<LaneId, SupervisionPolicy>,
}

impl PolicySnapshot {
    /// Return the effective policy for `id` if supervision is active on that lane.
    pub fn lane(&self, id: LaneId) -> Option<&SupervisionPolicy> {
        self.lanes.get(&id)
    }

    /// True if the master switch is on and at least one lane is actively supervised.
    pub fn any_enabled(&self) -> bool {
        self.master && !self.lanes.is_empty()
    }
}

/// Rebuild a fresh [`PolicySnapshot`] by reading user config and DB overrides.
pub async fn rebuild_snapshot(ctx: &Ctx) -> PolicySnapshot {
    let defaults = ctx.config.read().await.supervision.clone();
    let master = defaults.enabled;
    let mut lanes = HashMap::new();
    if master {
        if let Ok(policies) = ctx.store.lane_policies().await {
            for p in policies {
                let effective = resolve(&defaults, Some(&p));
                if effective.enabled {
                    lanes.insert(p.lane_id, effective);
                }
            }
        }
    }
    PolicySnapshot { master, lanes }
}

/// Rebuild and update the cached [`PolicySnapshot`] on `ctx.supervision`.
pub async fn refresh(ctx: &Ctx) {
    let snapshot = rebuild_snapshot(ctx).await;
    *ctx.supervision.write().await = snapshot;
}

/// Get the current effective [`SupervisionPolicy`] for a lane, if actively supervised.
pub async fn supervised(ctx: &Ctx, lane: LaneId) -> Option<SupervisionPolicy> {
    ctx.supervision.read().await.lane(lane).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use repomon_core::agent::supervision::{
        DialogClass, MailDeliveryMode, PolicyAction, SupervisionOverrides,
    };
    use repomon_core::{Config, Store};
    use std::sync::Arc;

    async fn test_ctx() -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.supervision.enabled = true;
        Ctx::new(store, config, None)
    }

    #[tokio::test]
    async fn snapshot_only_contains_enabled_lanes() {
        let ctx = test_ctx().await;

        // Lane 1: enabled
        let p1 = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: HashMap::from([(DialogClass::CommandExec, PolicyAction::AutoApprove)])
                .into_iter()
                .collect(),
            mail_mode: Some(MailDeliveryMode::Nudge),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p1).await.unwrap();

        // Lane 2: disabled
        let p2 = SupervisionOverrides {
            lane_id: 2,
            enabled: false,
            classes: std::collections::BTreeMap::new(),
            mail_mode: None,
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p2).await.unwrap();

        let snapshot = rebuild_snapshot(&ctx).await;
        assert!(snapshot.master);
        assert_eq!(snapshot.lanes.len(), 1);
        assert!(snapshot.lane(1).is_some());
        assert!(snapshot.lane(2).is_none());
        assert!(snapshot.any_enabled());
    }

    #[tokio::test]
    async fn snapshot_empty_when_master_off() {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.supervision.enabled = false; // master OFF
        let ctx = Ctx::new(store, config, None);

        // Lane 1: explicitly enabled in DB, but master is OFF
        let p1 = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: std::collections::BTreeMap::new(),
            mail_mode: None,
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p1).await.unwrap();

        let snapshot = rebuild_snapshot(&ctx).await;
        assert!(!snapshot.master);
        assert!(snapshot.lanes.is_empty());
        assert!(!snapshot.any_enabled());
        assert!(snapshot.lane(1).is_none());
    }

    #[tokio::test]
    async fn supervised_reads_refreshed_state() {
        let ctx = test_ctx().await;

        assert_eq!(supervised(&ctx, 10).await, None);

        let p = SupervisionOverrides {
            lane_id: 10,
            enabled: true,
            classes: HashMap::from([(DialogClass::Deletion, PolicyAction::AutoDeny)])
                .into_iter()
                .collect(),
            mail_mode: None,
            nudge_text: Some("nudge 10".into()),
            stall_mins: Some(20),
            nudge_retries: Some(2),
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p).await.unwrap();

        // Before refresh, cache doesn't have it
        assert_eq!(supervised(&ctx, 10).await, None);

        // After refresh, cache is updated
        refresh(&ctx).await;
        let pol = supervised(&ctx, 10).await.expect("supervised");
        assert!(pol.enabled);
        assert_eq!(pol.nudge_text, "nudge 10");
        assert_eq!(pol.stall_mins, 20);
        assert_eq!(pol.nudge_retries, 2);
    }
}

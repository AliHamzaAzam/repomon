//! APNs push — how agent alerts reach the phone when the companion app is closed.
//!
//! The daemon talks to Apple directly over HTTP/2 (`a2`), signing with the `.p8` key from
//! `[push]` in the config; no relay in between. `notify_watch` calls [`send_all`] with the
//! same title/body it broadcast as `event.notification`. Pushes carry the lane/session/prompt
//! as custom data so the app can deep-link, and a category — `AGENT_PROMPT` when there's a
//! pending question to act on (the app attaches an Approve action), `AGENT_ALERT` otherwise.
//! Tokens APNs reports dead (`Unregistered`/`BadDeviceToken`) are evicted from the store.

use std::sync::Arc;

use a2::{
    Client, ClientConfig, CollapseId, DefaultNotificationBuilder, Endpoint, ErrorReason,
    NotificationBuilder, NotificationOptions, Priority,
};
use repomon_core::config::PushConfig;
use serde_json::Value;

use crate::Ctx;

/// Notification categories the app registers actions for.
pub const CATEGORY_PROMPT: &str = "AGENT_PROMPT";
pub const CATEGORY_ALERT: &str = "AGENT_ALERT";

/// A ready APNs sender, built from a complete `[push]` config. `None` when push isn't
/// (fully) configured — callers just skip sending.
pub struct Push {
    client: Client,
    topic: String,
}

impl Push {
    /// Build a sender from the config: needs team id, key id, p8 path, and bundle id.
    pub fn from_config(cfg: &PushConfig) -> Option<Push> {
        let (team, key, p8, topic) = (
            cfg.team_id.as_deref()?,
            cfg.key_id.as_deref()?,
            cfg.p8_path.as_deref()?,
            cfg.bundle_id.as_deref()?,
        );
        let pem = match std::fs::File::open(p8) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("push: cannot open p8 key {}: {e}", p8.display());
                return None;
            }
        };
        let endpoint = if cfg.sandbox {
            Endpoint::Sandbox
        } else {
            Endpoint::Production
        };
        match Client::token(pem, key, team, ClientConfig::new(endpoint)) {
            Ok(client) => Some(Push {
                client,
                topic: topic.to_string(),
            }),
            Err(e) => {
                tracing::warn!("push: APNs client init failed: {e}");
                None
            }
        }
    }

    /// Send one alert to one device. Returns `Ok(false)` when APNs says the token is dead
    /// (the caller should evict it), `Ok(true)` on success.
    pub async fn send(
        &self,
        device_token: &str,
        title: &str,
        body: &str,
        category: &str,
        data: &Value,
    ) -> Result<bool, a2::Error> {
        let builder = DefaultNotificationBuilder::new()
            .set_title(title)
            .set_body(body)
            .set_sound("default")
            .set_category(category);
        // Collapse duplicates of the same alert on the lock screen: the daemon stamps each payload
        // with a stable `id` (lane:session:kind:activity) that only changes on real new activity,
        // so a flapped re-send replaces rather than stacks. APNs caps the value at 64 bytes; our
        // ids are ASCII, so a byte slice is a safe truncation.
        let collapse = data
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| {
                // Cap at 64 bytes (APNs limit), backing up to a char boundary so a future
                // multibyte id can't panic the slice (today's ids are ASCII).
                let mut end = s.len().min(64);
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                &s[..end]
            })
            .and_then(|s| CollapseId::new(s).ok());
        let options = NotificationOptions {
            apns_topic: Some(&self.topic),
            apns_priority: Some(Priority::High),
            apns_collapse_id: collapse,
            ..Default::default()
        };
        let mut payload = builder.build(device_token, options);
        let _ = payload.add_custom_data("repomon", data);
        match self.client.send(payload).await {
            Ok(_) => Ok(true),
            Err(a2::Error::ResponseError(resp)) => {
                let dead = resp.error.as_ref().is_some_and(|e| {
                    matches!(
                        e.reason,
                        ErrorReason::Unregistered | ErrorReason::BadDeviceToken
                    )
                });
                if dead {
                    Ok(false)
                } else {
                    Err(a2::Error::ResponseError(resp))
                }
            }
            Err(e) => Err(e),
        }
    }
}

/// Push `title`/`body` to every registered device, evicting tokens APNs reports dead.
/// Builds the sender fresh per call — alerts are rare and the key parse is cheap, and this
/// way `[push]` config changes apply immediately.
pub async fn send_all(ctx: &Arc<Ctx>, title: &str, body: &str, category: &str, data: &Value) {
    let devices = ctx.store.list_devices().await.unwrap_or_default();
    if devices.is_empty() {
        return;
    }
    let cfg = ctx.config.read().await.push.clone();
    let Some(push) = Push::from_config(&cfg) else {
        return;
    };
    for token in devices {
        match push.send(&token, title, body, category, data).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    "push: evicting dead device token {}…",
                    &token[..8.min(token.len())]
                );
                let _ = ctx.store.unregister_device(token).await;
            }
            Err(e) => tracing::warn!("push: send failed: {e}"),
        }
    }
}

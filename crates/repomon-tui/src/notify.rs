//! Maintains in-app notification history, sharing transition classification and native delivery
//! with the core notification module.

use chrono::{DateTime, Local};

use repomon_core::model::LaneId;

// Everything reusable - kinds, edge detection, text composition, and the local desktop delivery -
// lives in core, shared with the daemon's notification engine; this module keeps only the in-app
// feed event below.
pub use repomon_core::notify::{NotifKind, compose, compose_burst, play_chime, send_native};

/// A fired notification, kept in the in-app history feed.
#[derive(Debug, Clone)]
pub struct NotifEvent {
    pub when: DateTime<Local>,
    pub kind: NotifKind,
    /// The lane the alert was about - lets the feed jump straight to it.
    pub lane_id: LaneId,
    /// The session that fired (Claude transcript id) - lets the feed open/attach the exact
    /// agent in a multi-agent lane. `None` when the session couldn't be identified.
    pub session_id: Option<String>,
    /// False until the user opens the Notifications view; drives the unread unread badge.
    pub read: bool,
    pub title: String,
    pub body: String,
}

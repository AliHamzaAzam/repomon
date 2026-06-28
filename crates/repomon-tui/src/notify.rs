//! In-app notification feed (the scrollable history) for agent state changes.
//!
//! The TUI watches each agent session's status across refreshes (see `App::detect_notifications`)
//! and, on a meaningful transition, fires a native desktop popup plus an in-app banner and this
//! history feed. The pure parts — kinds, edge detection, text composition — AND the desktop popup
//! delivery ([`send_native`]) live in `repomon_core::notify`, shared with the daemon (which fires
//! the same popup when this TUI is parked in an attach or closed). The config toggles live in `app`.

use chrono::{DateTime, Local};

use repomon_core::model::LaneId;

// Everything reusable — kinds, edge detection, text composition, and the local desktop delivery —
// lives in core, shared with the daemon's notification engine; this module keeps only the in-app
// feed event below.
pub use repomon_core::notify::{NotifKind, compose, compose_burst, play_chime, send_native};

/// A fired notification, kept in the in-app history feed.
#[derive(Debug, Clone)]
pub struct NotifEvent {
    pub when: DateTime<Local>,
    pub kind: NotifKind,
    /// The lane the alert was about — lets the feed jump straight to it.
    pub lane_id: LaneId,
    /// The session that fired (Claude transcript id) — lets the feed open/attach the exact
    /// agent in a multi-agent lane. `None` when the session couldn't be identified.
    pub session_id: Option<String>,
    /// False until the user opens the Notifications view; drives the ⚑ unread badge.
    pub read: bool,
    pub title: String,
    pub body: String,
}

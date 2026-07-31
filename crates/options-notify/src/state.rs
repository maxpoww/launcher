//! The notification data model.
//!
//! [`NotificationEvent`] is the canonical, serializable shape every notification
//! takes once received over D-Bus — rich enough to carry action buttons, inline
//! replies, images, and provenance. The struct layout is the project's canonical
//! model; parsing from the raw `Notify` arguments lives in [`crate::server`].

use serde::{Deserialize, Serialize};

/// FreeDesktop urgency levels (the `urgency` hint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UrgencyLevel {
    Low = 0,
    #[default]
    Normal = 1,
    Critical = 2,
}

impl UrgencyLevel {
    /// Map the raw `urgency` hint byte; anything unexpected is Normal.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => UrgencyLevel::Low,
            2 => UrgencyLevel::Critical,
            _ => UrgencyLevel::Normal,
        }
    }
}

/// One action button offered by a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    /// The identifier passed back in `ActionInvoked`.
    pub key: String,
    /// Human-readable button label.
    pub label: String,
}

/// A received desktop notification, fully parsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub id: u32,
    pub app_name: String,
    pub app_icon: Option<String>,
    pub summary: String,
    pub body: String,
    pub urgency: UrgencyLevel,
    pub actions: Vec<NotificationAction>,
    pub supports_inline_reply: bool,
    pub inline_reply_action_key: Option<String>,
    pub category: Option<String>,
    pub desktop_entry: Option<String>,
    /// Raw pixel buffer from the `image-data` hint (see [`RawImage`] for the
    /// dimensions, which travel alongside).
    pub image_data: Option<Vec<u8>>,
    /// Width/height of `image_data` (needed to actually render it — the raw
    /// `Vec<u8>` alone is un-renderable).
    pub image_dims: Option<(u32, u32)>,
    pub timestamp: i64,
    pub replaces_id: u32,
    pub is_read: bool,
}

//! The notification data model.
//!
//! [`NotificationEvent`] is the canonical, serializable shape every notification
//! takes once received over D-Bus — rich enough to carry action buttons, inline
//! replies, images, and provenance. The struct layout is the project's canonical
//! model; parsing from the raw `Notify` arguments lives in [`crate::server`].

use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

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

/// The over-the-wire notification the frontend renders — the render-relevant
/// subset of [`NotificationEvent`], marshalled as a D-Bus dict (`a{sv}`) on the
/// `org.options.Notifications` control interface.
///
/// A **dict** (rather than a positional struct) is deliberate: new fields can be
/// added later without changing the D-Bus signature or breaking an older peer,
/// which matters across two independently-deployed processes (daemon + shell).
///
/// Rendering intentionally does *not* ship raw pixels: `app_icon` /
/// `desktop_entry` let waverunner resolve the icon through its own FreeDesktop
/// theme chain, so the hot path stays a small typed message. `actions` is the
/// flat `[key, label, key, label, …]` form (same shape the wire `Notify` uses);
/// the UI pairs them, and passes a `key` back to `InvokeAction`.
#[derive(Debug, Clone, PartialEq, Eq, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "a{sv}")]
pub struct ActiveNotification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub desktop_entry: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub urgency: u8,
    pub supports_inline_reply: bool,
    pub timestamp_ms: u64,
}

impl From<&NotificationEvent> for ActiveNotification {
    fn from(e: &NotificationEvent) -> Self {
        let mut actions = Vec::with_capacity(e.actions.len() * 2);
        for a in &e.actions {
            actions.push(a.key.clone());
            actions.push(a.label.clone());
        }
        Self {
            id: e.id,
            app_name: e.app_name.clone(),
            app_icon: e.app_icon.clone().unwrap_or_default(),
            desktop_entry: e.desktop_entry.clone().unwrap_or_default(),
            summary: e.summary.clone(),
            body: e.body.clone(),
            actions,
            urgency: e.urgency as u8,
            supports_inline_reply: e.supports_inline_reply,
            timestamp_ms: e.timestamp.max(0) as u64,
        }
    }
}

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
    /// The `transient` hint (or a synchronous OSD notification): bypass the
    /// server's persistence — show it live, but never keep it in the durable
    /// history. True for volume/brightness OSD and similar throwaway toasts.
    pub transient: bool,
    /// The `x-canonical-private-synchronous` tag, if present. Synchronous
    /// notifications with the same tag (a volume or brightness bar being nudged
    /// repeatedly) replace each other in place instead of stacking. Internal —
    /// never sent to the UI.
    pub sync_tag: Option<String>,
    /// The `resident` hint: the notification should stay after one of its actions
    /// is invoked (persistent controls, e.g. a media player's prev/next), rather
    /// than closing as notifications normally do on activation.
    pub resident: bool,
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
    /// Tightly-packed premultiplied-*straight* RGBA8 for the notification's own
    /// image (a messaging avatar, media art, …), captured at `Notify` time —
    /// empty when the notification carries no image. Unlike `app_icon` /
    /// `desktop_entry` (which the shell resolves through its icon theme), these
    /// pixels can't be re-fetched later: apps like Chrome point their image
    /// hints at scoped-temp files that are deleted seconds after `Notify`, so
    /// the daemon reads them immediately and ships the bytes.
    pub image_rgba: Vec<u8>,
    /// Dimensions of `image_rgba` (`0×0` when absent). `image_rgba.len()` is
    /// always `width * height * 4`.
    pub image_width: u32,
    pub image_height: u32,
    /// The `transient` hint — the notification should be shown but not kept in
    /// the durable history. `Option` (not bare `bool`) so this stays wire-
    /// compatible with an older daemon that predates the field: a dict missing
    /// the key deserializes to `None` (treated as not-transient) instead of
    /// failing. `None` and `Some(false)` mean the same thing to the UI.
    pub transient: Option<bool>,
    /// The `resident` hint — the notification stays after an action is invoked
    /// (persistent controls) rather than closing. `Option` for the same wire-
    /// compatibility reason as `transient`: an older peer omits the key → `None`
    /// (treated as not-resident).
    pub resident: Option<bool>,
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
            image_rgba: e.image_data.clone().unwrap_or_default(),
            image_width: e.image_dims.map_or(0, |(w, _)| w),
            image_height: e.image_dims.map_or(0, |(_, h)| h),
            transient: Some(e.transient),
            resident: Some(e.resident),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{serialized::Context, to_bytes, SerializeDict, Type, LE};

    /// A copy of the wire dict **without** the `transient` field — stands in for
    /// an older daemon that predates it.
    #[derive(SerializeDict, Type)]
    #[zvariant(signature = "a{sv}")]
    struct OldWire {
        id: u32,
        app_name: String,
        app_icon: String,
        desktop_entry: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        urgency: u8,
        supports_inline_reply: bool,
        timestamp_ms: u64,
        image_rgba: Vec<u8>,
        image_width: u32,
        image_height: u32,
    }

    /// Forward compatibility: a dict from an older peer (no `transient` key) must
    /// still deserialize into the current [`ActiveNotification`], with the missing
    /// field defaulting to `None` (treated as not-transient) rather than erroring.
    /// This is why the field is `Option<bool>`, and it guards against the wire
    /// skew that a bare `bool` would crash on.
    #[test]
    fn missing_transient_key_deserializes_to_none() {
        let old = OldWire {
            id: 7,
            app_name: "App".into(),
            app_icon: String::new(),
            desktop_entry: String::new(),
            summary: "S".into(),
            body: "B".into(),
            actions: vec![],
            urgency: 1,
            supports_inline_reply: false,
            timestamp_ms: 100,
            image_rgba: vec![],
            image_width: 0,
            image_height: 0,
        };
        let ctxt = Context::new_dbus(LE, 0);
        let bytes = to_bytes(ctxt, &old).expect("serialize old wire");
        let (decoded, _): (ActiveNotification, _) =
            bytes.deserialize().expect("deserialize into current type");
        assert_eq!(decoded.id, 7);
        // Both fields added after the old schema default to `None`.
        assert_eq!(decoded.transient, None);
        assert_eq!(decoded.resident, None);
    }

    /// A round-trip through the current schema preserves `transient`.
    #[test]
    fn transient_round_trips() {
        let ev = NotificationEvent {
            id: 1,
            app_name: "OSD".into(),
            app_icon: None,
            summary: "Volume".into(),
            body: String::new(),
            urgency: UrgencyLevel::Normal,
            actions: vec![],
            supports_inline_reply: false,
            inline_reply_action_key: None,
            category: None,
            desktop_entry: None,
            image_data: None,
            image_dims: None,
            timestamp: 5,
            replaces_id: 0,
            is_read: false,
            transient: true,
            sync_tag: None,
            resident: false,
        };
        let wire = ActiveNotification::from(&ev);
        assert_eq!(wire.transient, Some(true));
        let ctxt = Context::new_dbus(LE, 0);
        let bytes = to_bytes(ctxt, &wire).expect("serialize");
        let (decoded, _): (ActiveNotification, _) = bytes.deserialize().expect("deserialize");
        assert_eq!(decoded.transient, Some(true));
    }
}

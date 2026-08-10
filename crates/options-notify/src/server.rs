//! The `org.freedesktop.Notifications` D-Bus interface implementation.
//!
//! Owns the live notification store and republishes it on a `watch` whenever it
//! changes, so the surface (the notification OPTION box) always sees the current
//! list. Parses the raw `Notify` arguments — actions, urgency, category, images,
//! inline-reply — into the rich [`NotificationEvent`] model.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::state::{NotificationAction, NotificationEvent, UrgencyLevel};

/// Capabilities advertised via `GetCapabilities`.
const CAPABILITIES: &[&str] = &[
    "actions",
    "body",
    "body-hyperlinks",
    "body-markup",
    "icon-static",
    "inline-reply",
    "x-canonical-private-synchronous",
];

/// The served interface state: the id counter, the active store, and the
/// publish handle.
pub(crate) struct Notifications {
    next_id: u32,
    store: Vec<NotificationEvent>,
    tx: watch::Sender<Vec<NotificationEvent>>,
}

impl Notifications {
    pub(crate) fn new(tx: watch::Sender<Vec<NotificationEvent>>) -> Self {
        Self {
            next_id: 0,
            store: Vec::new(),
            tx,
        }
    }

    /// Next non-zero id (0 is reserved by the spec for "server allocates").
    fn alloc_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        self.next_id
    }

    fn publish(&self) {
        let _ = self.tx.send(self.store.clone());
    }

    /// Remove a notification from the store (UI-driven dismiss/action/reply),
    /// republishing the list. Returns whether one was actually removed.
    pub(crate) fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.store.iter().position(|n| n.id == id) {
            self.store.remove(pos);
            self.publish();
            true
        } else {
            false
        }
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    /// The core entry point: an app posts a notification. `replaces_id != 0`
    /// updates an existing one in place (atomic replace — no jitter).
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &mut self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.alloc_id()
        };
        let event = build_event(id, replaces_id, app_name, app_icon, summary, body, &actions, &hints);
        // Replace in place if the id already exists, else append.
        if let Some(slot) = self.store.iter_mut().find(|n| n.id == id) {
            *slot = event;
        } else {
            self.store.push(event);
        }
        self.publish();
        id
    }

    /// Close a notification by id (app- or server-initiated), emitting the
    /// `NotificationClosed` signal (reason 3 = closed via this call).
    async fn close_notification(
        &mut self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(pos) = self.store.iter().position(|n| n.id == id) {
            self.store.remove(pos);
            self.publish();
            let _ = emitter.notification_closed(id, 3).await;
        }
        Ok(())
    }

    /// Advertise what this server supports.
    fn get_capabilities(&self) -> Vec<String> {
        CAPABILITIES.iter().map(|s| s.to_string()).collect()
    }

    /// (name, vendor, version, spec_version).
    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "OPTIONS".to_string(),
            "Golem".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    /// Emitted when a notification is closed (1 expired, 2 dismissed, 3 via
    /// CloseNotification, 4 undefined).
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    /// Emitted when the user activates one of a notification's actions.
    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    /// Emitted when the user submits an inline reply (the text goes back to the
    /// originating app). Non-standard but the de-facto inline-reply signal.
    #[zbus(signal)]
    async fn notification_replied(
        emitter: &SignalEmitter<'_>,
        id: u32,
        text: &str,
    ) -> zbus::Result<()>;
}

/// Current unix time in milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Assemble a [`NotificationEvent`] from the raw `Notify` arguments.
#[allow(clippy::too_many_arguments)]
fn build_event(
    id: u32,
    replaces_id: u32,
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    actions: &[String],
    hints: &HashMap<String, OwnedValue>,
    ) -> NotificationEvent {
    let (actions, supports_inline_reply, inline_reply_action_key) = parse_actions(actions);
    // Resolve the notification's own image, richest source first. The `*-data`
    // hints carry raw pixels; the `*-path` hints and (for Chrome-style web
    // notifications) the `app_icon` param point at image *files* that must be
    // read now — they live in scoped-temp dirs the app deletes moments later.
    let (image_data, image_dims) = hints
        .get("image-data")
        .or_else(|| hints.get("image_data"))
        .or_else(|| hints.get("icon_data"))
        .and_then(parse_image)
        .or_else(|| {
            hint_string(hints, "image-path")
                .or_else(|| hint_string(hints, "image_path"))
                .as_deref()
                .and_then(load_image_file)
        })
        .or_else(|| load_image_file(&app_icon))
        .map(|(bytes, dims)| (Some(bytes), Some(dims)))
        .unwrap_or((None, None));

    NotificationEvent {
        id,
        app_name,
        app_icon: (!app_icon.is_empty()).then_some(app_icon),
        summary,
        body,
        urgency: hint_u8(hints, "urgency")
            .map(UrgencyLevel::from_u8)
            .unwrap_or_default(),
        actions,
        supports_inline_reply,
        inline_reply_action_key,
        category: hint_string(hints, "category"),
        desktop_entry: hint_string(hints, "desktop-entry"),
        image_data,
        image_dims,
        timestamp: now_millis(),
        replaces_id,
        is_read: false,
    }
}

/// Split the flat `[key, label, key, label, …]` actions array into pairs, and
/// detect inline-reply support (an action keyed `inline-reply`).
fn parse_actions(actions: &[String]) -> (Vec<NotificationAction>, bool, Option<String>) {
    let mut out = Vec::new();
    let mut inline = false;
    let mut inline_key = None;
    for pair in actions.chunks(2) {
        if let [key, label] = pair {
            if key == "inline-reply" {
                inline = true;
                inline_key = Some(key.clone());
            }
            out.push(NotificationAction {
                key: key.clone(),
                label: label.clone(),
            });
        }
    }
    (out, inline, inline_key)
}

fn hint_u8(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<u8> {
    u8::try_from(hints.get(key)?.try_clone().ok()?).ok()
}

fn hint_string(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    String::try_from(hints.get(key)?.try_clone().ok()?).ok()
}

/// Decode the `image-data` hint `(iiibiiay)` into a tight RGBA buffer plus
/// `(width, height)`. Best-effort — returns `None` on any shape mismatch.
fn parse_image(v: &OwnedValue) -> Option<(Vec<u8>, (u32, u32))> {
    let (w, h, rowstride, _has_alpha, _bits, channels, data): (
        i32,
        i32,
        i32,
        bool,
        i32,
        i32,
        Vec<u8>,
    ) = v.try_clone().ok()?.try_into().ok()?;
    let (w, h) = (w.max(0) as usize, h.max(0) as usize);
    let ch = channels.clamp(1, 4) as usize;
    let rs = (rowstride.max(0) as usize).max(w * ch);
    if w == 0 || h == 0 || data.len() < h * rs {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &data[y * rs..y * rs + w * ch];
        for x in 0..w {
            let px = &row[x * ch..x * ch + ch];
            let a = if ch == 4 { px[3] } else { 255 };
            rgba.extend_from_slice(&[px[0], px[1], px[2], a]);
        }
    }
    Some((rgba, (w as u32, h as u32)))
}

/// Load an image *file* referenced by a notification (an `image-path` hint or a
/// `file://` / absolute `app_icon`) into tight RGBA8 plus `(w, h)`, downscaled
/// so the wire payload stays small. Returns `None` for empty strings, themed
/// icon names (which are not files), missing files, or anything that won't
/// decode. This is the capture that makes per-service imagery (WhatsApp / etc.
/// avatars Chrome writes to scoped-temp files) survive past `Notify`.
fn load_image_file(reference: &str) -> Option<(Vec<u8>, (u32, u32))> {
    if reference.is_empty() {
        return None;
    }
    // Accept `file://` URIs and bare absolute paths; a themed name isn't a file.
    let path = reference.strip_prefix("file://").unwrap_or(reference);
    if !path.starts_with('/') {
        return None;
    }
    let img = image::open(path).ok()?;
    // Cap the long edge: the card tile is small, and this bounds the D-Bus
    // payload (every `ActiveChanged` re-sends the whole active list).
    const MAX_EDGE: u32 = 96;
    let img = if img.width().max(img.height()) > MAX_EDGE {
        img.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    (w > 0 && h > 0).then(|| (rgba.into_raw(), (w, h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_split_into_pairs() {
        let (acts, inline, _) = parse_actions(&[
            "default".into(),
            "Open".into(),
            "archive".into(),
            "Archive".into(),
        ]);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].key, "default");
        assert_eq!(acts[1].label, "Archive");
        assert!(!inline);
    }

    #[test]
    fn inline_reply_action_is_detected() {
        let (_, inline, key) = parse_actions(&["inline-reply".into(), "Reply".into()]);
        assert!(inline);
        assert_eq!(key.as_deref(), Some("inline-reply"));
    }

    #[test]
    fn odd_action_arrays_dont_panic() {
        let (acts, _, _) = parse_actions(&["lonely".into()]);
        assert!(acts.is_empty());
    }

    #[test]
    fn urgency_maps_from_byte() {
        assert_eq!(UrgencyLevel::from_u8(0), UrgencyLevel::Low);
        assert_eq!(UrgencyLevel::from_u8(2), UrgencyLevel::Critical);
        assert_eq!(UrgencyLevel::from_u8(9), UrgencyLevel::Normal);
    }
}

//! The `org.freedesktop.Notifications` D-Bus interface implementation.
//!
//! Owns the live notification store and republishes it on a `watch` whenever it
//! changes, so the surface (the notification OPTION box) always sees the current
//! list. Parses the raw `Notify` arguments — actions, urgency, category, images,
//! transient/synchronous — into the rich [`NotificationEvent`] model.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::Connection;

use crate::state::{NotificationAction, NotificationEvent, UrgencyLevel};

/// Capabilities advertised via `GetCapabilities`.
const CAPABILITIES: &[&str] = &[
    "actions",
    "body",
    "body-hyperlinks",
    "body-markup",
    "icon-static",
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

    /// Expire a notification: remove it only if it's still the *same* one that
    /// armed the timer (matched by id **and** its arm-time `timestamp`), so a
    /// replace/close before the deadline doesn't wrongly close its successor.
    /// Returns whether one was actually expired.
    pub(crate) fn expire(&mut self, id: u32, armed_ts: i64) -> bool {
        if let Some(pos) = self
            .store
            .iter()
            .position(|n| n.id == id && n.timestamp == armed_ts)
        {
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
        expire_timeout: i32,
        #[zbus(connection)] conn: &Connection,
    ) -> u32 {
        // Id resolution, in priority order: an explicit `replaces_id`; else a
        // synchronous notification replacing a prior one with the same tag
        // (volume/brightness bars update in place, never stack); else a fresh id.
        let id = if replaces_id != 0 {
            replaces_id
        } else if let Some(id) = sync_tag(&hints).and_then(|tag| find_sync_id(&self.store, &tag)) {
            id
        } else {
            self.alloc_id()
        };
        let event = build_event(id, replaces_id, app_name, app_icon, summary, body, &actions, &hints);
        // Capture the fields the expiry timer needs before the event moves into
        // the store. `timestamp` is refreshed on every post/replace, so it doubles
        // as the arm token that ties this timer to this exact notification.
        let armed_ts = event.timestamp;
        // Phone messages bridged in over KDE Connect are persistent unread
        // indicators — they must stay until the phone marks them read (which
        // closes them), never auto-expire out from under the user.
        let expire = if is_persistent(&event) {
            None
        } else {
            effective_expire(expire_timeout, event.urgency)
        };
        // Replace in place if the id already exists, else append.
        if let Some(slot) = self.store.iter_mut().find(|n| n.id == id) {
            *slot = event;
        } else {
            self.store.push(event);
        }
        self.publish();
        // Arm the auto-expiry off the interface loop: after the delay it reaches
        // back through the object server to remove the notification (if still the
        // same one) and emit `NotificationClosed` reason 1.
        if let Some(delay_ms) = expire {
            let conn = conn.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                if let Err(e) = expire_notification(&conn, id, armed_ts).await {
                    tracing::warn!("expire notification {id}: {e}");
                }
            });
        }
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
}

/// Current unix time in milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Auto-expiry defaults for `expire_timeout == -1` ("server default"), by
/// urgency — generous so the active set (and the amber bell) doesn't accumulate
/// notifications an app never explicitly closes.
const DEFAULT_EXPIRE_MS: u64 = 10_000;
const LOW_EXPIRE_MS: u64 = 5_000;

/// Resolve the FreeDesktop `expire_timeout` (ms) into an actual delay, or `None`
/// for "never auto-expire":
/// - `> 0`: honour it exactly (the app asked for this lifetime).
/// - `0`: never expire (spec — stays until dismissed/replaced).
/// - `< 0` (`-1` = "server default"): our choice — Critical is sticky, Normal/Low
///   get a generous default.
///
/// Expiry only clears the *active* notification (Closed reason 1); the UI keeps
/// its own durable history, so nothing browsable is lost. Pure, so it's tested.
fn effective_expire(expire_timeout: i32, urgency: UrgencyLevel) -> Option<u64> {
    use std::cmp::Ordering;
    match expire_timeout.cmp(&0) {
        Ordering::Greater => Some(expire_timeout as u64),
        Ordering::Equal => None,
        Ordering::Less => match urgency {
            UrgencyLevel::Critical => None,
            UrgencyLevel::Low => Some(LOW_EXPIRE_MS),
            UrgencyLevel::Normal => Some(DEFAULT_EXPIRE_MS),
        },
    }
}

/// Fire the auto-expiry: reach the standard interface through the object server,
/// remove the notification if it's still the same one that armed the timer, and
/// emit `NotificationClosed` reason 1 (expired) so the originating app learns it.
async fn expire_notification(conn: &Connection, id: u32, armed_ts: i64) -> zbus::Result<()> {
    let iface = conn
        .object_server()
        .interface::<_, Notifications>(crate::NOTIFY_PATH)
        .await?;
    let expired = iface.get_mut().await.expire(id, armed_ts);
    if expired {
        iface.signal_emitter().notification_closed(id, 1).await?;
    }
    Ok(())
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
    let actions = parse_actions(actions);
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
        category: hint_string(hints, "category"),
        desktop_entry: hint_string(hints, "desktop-entry"),
        image_data,
        image_dims,
        timestamp: now_millis(),
        replaces_id,
        is_read: false,
        transient: is_transient(hints),
        sync_tag: sync_tag(hints),
    }
}

/// The `x-canonical-private-synchronous` tag, if the hint is present. Its value
/// is a string (`"volume"`, `"brightness"`, …); a present-but-non-string value
/// collapses to an empty tag so such notifications still share one slot.
fn sync_tag(hints: &HashMap<String, OwnedValue>) -> Option<String> {
    let v = hints.get("x-canonical-private-synchronous")?;
    Some(
        v.try_clone()
            .ok()
            .and_then(|c| String::try_from(c).ok())
            .unwrap_or_default(),
    )
}

/// The id of an existing synchronous notification carrying the same `tag`, so a
/// repeat (a volume/brightness bar nudged again) replaces it in place instead of
/// stacking a new card. Pure, so it's unit-tested.
fn find_sync_id(store: &[NotificationEvent], tag: &str) -> Option<u32> {
    store
        .iter()
        .find(|n| n.sync_tag.as_deref() == Some(tag))
        .map(|n| n.id)
}

/// Whether a notification should never auto-expire (stays until explicitly
/// closed). Phone notifications bridged in over KDE Connect are live unread
/// indicators — the phone owns their lifetime, closing them when read.
fn is_persistent(event: &NotificationEvent) -> bool {
    event
        .desktop_entry
        .as_deref()
        .is_some_and(|d| d.starts_with("org.kde.kdeconnect"))
        || event.app_name.eq_ignore_ascii_case("KDE Connect")
}

/// Whether a notification should bypass durable persistence: the explicit
/// `transient` hint, or a synchronous OSD notification
/// (`x-canonical-private-synchronous`), which is throwaway by nature (volume /
/// brightness bars that replace each other and shouldn't be logged).
fn is_transient(hints: &HashMap<String, OwnedValue>) -> bool {
    hint_flag(hints, "transient") || hints.contains_key("x-canonical-private-synchronous")
}

/// Read a boolean hint `key` (accepting the bool or byte forms apps send).
fn hint_flag(hints: &HashMap<String, OwnedValue>, key: &str) -> bool {
    hints.get(key).is_some_and(hint_truthy)
}

/// Interpret a hint value as a boolean flag — apps send `transient` as either a
/// real bool or a byte (`0`/`1`), so accept either.
fn hint_truthy(v: &OwnedValue) -> bool {
    if let Some(b) = v.try_clone().ok().and_then(|c| bool::try_from(c).ok()) {
        return b;
    }
    v.try_clone()
        .ok()
        .and_then(|c| u8::try_from(c).ok())
        .is_some_and(|n| n != 0)
}

/// Split the flat `[key, label, key, label, …]` actions array into pairs.
fn parse_actions(actions: &[String]) -> Vec<NotificationAction> {
    actions
        .chunks_exact(2)
        .map(|pair| NotificationAction {
            key: pair[0].clone(),
            label: pair[1].clone(),
        })
        .collect()
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
        let acts = parse_actions(&[
            "default".into(),
            "Open".into(),
            "archive".into(),
            "Archive".into(),
        ]);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].key, "default");
        assert_eq!(acts[1].label, "Archive");
    }

    #[test]
    fn odd_action_arrays_dont_panic() {
        let acts = parse_actions(&["lonely".into()]);
        assert!(acts.is_empty());
    }

    #[test]
    fn urgency_maps_from_byte() {
        assert_eq!(UrgencyLevel::from_u8(0), UrgencyLevel::Low);
        assert_eq!(UrgencyLevel::from_u8(2), UrgencyLevel::Critical);
        assert_eq!(UrgencyLevel::from_u8(9), UrgencyLevel::Normal);
    }

    fn ev(id: u32, sync_tag: Option<&str>) -> NotificationEvent {
        NotificationEvent {
            id,
            app_name: "App".into(),
            app_icon: None,
            summary: String::new(),
            body: String::new(),
            urgency: UrgencyLevel::Normal,
            actions: vec![],
            category: None,
            desktop_entry: None,
            image_data: None,
            image_dims: None,
            timestamp: 0,
            replaces_id: 0,
            is_read: false,
            transient: sync_tag.is_some(),
            sync_tag: sync_tag.map(Into::into),
        }
    }

    #[test]
    fn synchronous_replaces_by_tag_not_others() {
        let store = vec![
            ev(1, Some("volume")),
            ev(2, None),
            ev(3, Some("brightness")),
        ];
        // A repeat volume bar reuses id 1; brightness reuses 3; an unknown tag or
        // a plain notification gets no match (→ a fresh id at the call site).
        assert_eq!(find_sync_id(&store, "volume"), Some(1));
        assert_eq!(find_sync_id(&store, "brightness"), Some(3));
        assert_eq!(find_sync_id(&store, "battery"), None);
        assert_eq!(find_sync_id(&[], "volume"), None);
    }

    #[test]
    fn sync_tag_extraction() {
        use zbus::zvariant::Value;
        let mut h: HashMap<String, OwnedValue> = HashMap::new();
        assert_eq!(sync_tag(&h), None); // absent
        h.insert(
            "x-canonical-private-synchronous".into(),
            Value::from("volume").try_into().unwrap(),
        );
        assert_eq!(sync_tag(&h), Some("volume".to_string()));
    }

    #[test]
    fn transient_hint_detection() {
        use zbus::zvariant::Value;
        let mut h: HashMap<String, OwnedValue> = HashMap::new();
        assert!(!is_transient(&h)); // nothing set

        h.insert("transient".into(), Value::Bool(true).try_into().unwrap());
        assert!(is_transient(&h)); // bool form

        let mut h2: HashMap<String, OwnedValue> = HashMap::new();
        h2.insert("transient".into(), Value::U8(1).try_into().unwrap());
        assert!(is_transient(&h2)); // byte form

        let mut h3: HashMap<String, OwnedValue> = HashMap::new();
        h3.insert("transient".into(), Value::Bool(false).try_into().unwrap());
        assert!(!is_transient(&h3)); // explicitly false

        // A synchronous OSD notification is transient by nature.
        let mut h4: HashMap<String, OwnedValue> = HashMap::new();
        h4.insert(
            "x-canonical-private-synchronous".into(),
            Value::from("volume").try_into().unwrap(),
        );
        assert!(is_transient(&h4));
    }

    #[test]
    fn expire_timeout_resolution() {
        // An explicit positive timeout is honoured verbatim, regardless of urgency.
        assert_eq!(effective_expire(3000, UrgencyLevel::Normal), Some(3000));
        assert_eq!(effective_expire(3000, UrgencyLevel::Critical), Some(3000));
        // 0 means never expire.
        assert_eq!(effective_expire(0, UrgencyLevel::Normal), None);
        // -1 (server default): critical is sticky, others get a default, low the
        // shortest.
        assert_eq!(effective_expire(-1, UrgencyLevel::Critical), None);
        assert_eq!(effective_expire(-1, UrgencyLevel::Normal), Some(DEFAULT_EXPIRE_MS));
        assert_eq!(effective_expire(-1, UrgencyLevel::Low), Some(LOW_EXPIRE_MS));
        assert!(
            effective_expire(-1, UrgencyLevel::Low).unwrap()
                < effective_expire(-1, UrgencyLevel::Normal).unwrap()
        );
    }
}

//! The track an ACTION leaves behind.
//!
//! Golem speaks first: an offer arrives on the bar, you answer it, and it goes.
//! That is right for the bar — the surface has to clear — but it means the
//! answered offer leaves *nothing*. You can't see what Golem asked, when it
//! asked, or what you said; and having said "not now" at 20:31 you have no way
//! back to it except waiting for the machine to ask again.
//!
//! So an answered offer files a **track** into the notification OPTION, which is
//! already the place Golem keeps what happened while you were elsewhere. The
//! card carries the offer in the words it asked ("Turn on eye protection"), your
//! answer ("Turned on" / "Not now"), and the time — and it keeps the offer's own
//! [`AffordanceAction`] whole, so clicking the card runs the offer again. A
//! record you can act on, not a receipt.
//!
//! Tracks live beside the history rather than inside it: the notification
//! history is a mirror of what the notify daemon sent us (`ActiveNotification`,
//! a D-Bus wire type), and a track is Golem's own memory of a conversation it
//! had with the user. The card in the list is keyed to its track by
//! `timestamp_ms` — the same stable-across-reboot key the read state uses, since
//! notification ids restart at 1 every boot.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use options_engine::AffordanceAction;
use serde::{Deserialize, Serialize};

use crate::App;

const TRACKS_FILE: &str = "action-tracks.json";

/// The app name every track is filed under — it was Golem that spoke, not an
/// application. Also what the card's monogram tile shows.
pub(crate) const TRACK_APP: &str = "Golem";

/// One answered offer.
#[derive(Clone, Serialize, Deserialize)]
pub struct ActionTrack {
    /// The affordance's stable id (`"sunset.eye_protection"`), so a later pass
    /// can group or reason about tracks by which offer they came from.
    pub id: String,
    /// What was offered, in the words it was offered in.
    pub offered: String,
    /// The answer, as the card shows it ("Turned on", "Not now").
    pub answer: String,
    /// Whether the offer was accepted. Kept apart from `answer` so the meaning
    /// survives a rewording of the text.
    pub taken: bool,
    /// When it was answered (unix ms) — the key the card is found by.
    pub at_ms: u64,
    /// The offer's action, kept whole so the card can run it again. This is why
    /// `AffordanceAction` round-trips (it derives `Deserialize` where the rest
    /// of the affordance does not).
    pub action: AffordanceAction,
}

/// Read the tracks left by earlier sessions. A missing or corrupt file simply
/// yields none: the cards would then be inert records, which is a survivable
/// degradation, not an error worth a word.
pub fn load() -> HashMap<u64, ActionTrack> {
    crate::persist::read_json(&crate::persist::data_path(TRACKS_FILE)).unwrap_or_default()
}

impl App {
    /// File an answered offer: remember it, and put its card in the
    /// notification list.
    ///
    /// `action` is the offer's own action — pass it even when the answer was
    /// "no", because the whole point of the record is that you can change your
    /// mind from it later.
    pub(crate) fn track_action(
        &mut self,
        id: &str,
        offered: &str,
        answer: &str,
        taken: bool,
        action: AffordanceAction,
    ) {
        let at_ms = now_ms();
        let track = ActionTrack {
            id: id.to_owned(),
            offered: offered.to_owned(),
            answer: answer.to_owned(),
            taken,
            at_ms,
            action,
        };
        tracing::info!(
            "action track: {} — {} ({})",
            track.id,
            track.answer,
            if track.taken { "taken" } else { "declined" }
        );
        self.file_notif_record(TRACK_APP, &track.offered, &track.answer, at_ms);
        self.action_tracks.insert(at_ms, track);
        self.save_action_tracks();
    }

    /// Open an answered offer again (a click on its card). Returns whether the
    /// card was a track at all — the caller falls through to the ordinary
    /// "open the app that notified you" routing when it wasn't.
    ///
    /// The click brings the OFFER BACK to the bar rather than quietly doing the
    /// thing: the record is a way back into the conversation, not a shortcut
    /// past it. You get asked again, in the same place, and answer the same way
    /// — and that answer files its own record. An offer with no surface of its
    /// own (nothing that could pop up) runs its action outright instead.
    ///
    /// The track itself is left as it stands. It records the moment Golem asked
    /// and you answered; calling it back is not that moment happening again.
    pub(crate) fn replay_action_track(&mut self, at_ms: u64) -> bool {
        let Some(track) = self.action_tracks.get(&at_ms) else {
            return false;
        };
        let (id, action) = (track.id.clone(), track.action.clone());
        if self.recall_offer(&id) {
            tracing::info!("action track: recalled {id} to the bar");
            // Hand off to the bar: the offer is up there now, so the box that
            // sent you there gets out of the way.
            self.force_collapse_notif();
            self.sync_options_input();
        } else {
            tracing::info!("action track: replaying {id}");
            self.run_affordance_action(&action);
        }
        true
    }

    pub(crate) fn save_action_tracks(&self) {
        crate::persist::write_json(
            "action-tracks",
            &crate::persist::data_path(TRACKS_FILE),
            &self.action_tracks,
        );
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

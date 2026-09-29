//! Golem's configuration panel: the main card, opened empty.
//!
//! OPTIONS offers the right control at the right moment, but it can't guess
//! every one (a screen resolution, a scale). The panel is the place to go for
//! the rest: a gear among the apps ([`crate::apps::SETTINGS_ID`]) that opens
//! the same card the apps live in, with the sections and search cleared away
//! so the card itself is the surface. The dock band stays, so the gear that
//! opened the panel is the one that closes it.

use tracing::info;

use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;

impl App {
    /// One-shot: pin the Settings gear to the dock, just before the Recycle
    /// Bin so the bin keeps its place. Same marker rule as `pin_trash_once`: a
    /// user who later unpins or moves it isn't overruled on the next start.
    pub(crate) fn pin_settings_once(&mut self) {
        let marker = crate::persist::data_path("settings-pinned");
        if marker.exists() {
            return;
        }
        if !self.pins.is_pinned(apps::SETTINGS_ID) {
            let trash = format!("group:{}", groups::TRASH_ID);
            let slot = self
                .pins
                .pins()
                .iter()
                .position(|p| *p == trash)
                .unwrap_or(self.pins.pins().len());
            self.pins.pin_at(apps::SETTINGS_ID, slot);
        }
        crate::persist::write_text("settings-pin-marker", &marker, "1\n");
    }

    /// The gear was clicked: open the card as the panel, or close it if the
    /// panel is what's showing. With the card already open on the apps, the
    /// sections clear in place.
    pub(crate) fn toggle_settings_panel(&mut self) {
        self.close_group();
        if self.ui.target() == Target::Open {
            if self.settings_panel {
                info!("settings: closing the panel");
                self.handle_command(Command::Collapse);
            } else {
                info!("settings: panel in place of the apps");
                self.settings_panel = true;
                self.search.open = false;
                self.schedule_frame();
            }
            return;
        }
        info!("settings: opening the panel");
        self.settings_opening = true;
        self.handle_command(Command::Toggle);
        // Refused (e.g. the dock is suppressed): don't let a later open
        // come up as the panel.
        self.settings_opening = false;
    }
}

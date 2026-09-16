//! STAGE mode — one thing alone on screen, the deck of everything else below.
//!
//! Bound to Super+Enter. On: one task sits alone at the stage rect with a strip
//! of thumbnails in the gap beneath it; clicking one puts that on the stage.
//! Off: every window's geometry is exactly as it was, and you are left on
//! whichever task was on the stage — so the mode doubles as a way to travel
//! rather than only a place to look.
//!
//! ## Two modes: a task, or a desk
//!
//! What "one thing" means is [`Mode`], switched from the bar's stage-mode pill
//! (or `waverunner-ctl stage-mode`) and remembered between sessions:
//!
//! * [`Mode::Task`] — one **window**, maximized so it covers its workspace's
//!   siblings; the deck holds every window.
//! * [`Mode::Desk`] — one **workspace**, whole, its own tiling laid out inside
//!   the stage rect; the deck holds every occupied workspace.
//!
//! They are the same machine with two settings, not two features. A desk needs
//! *less*: nothing is maximized, nothing is parked, and showing one is the same
//! focus that shows a task. The one thing it needs that a task does not is that
//! the inset reach a workspace of several windows, which the smart-gaps
//! selectors do not cover — see [`hypr::set_general_gaps`].
//!
//! ## Nothing moves
//!
//! The obvious implementation — carry each window to a staging spot and carry it
//! home again from a recorded map — cannot be built on this compositor: its Lua
//! API has **no absolute-position dispatcher** (see `docs/hypr-api.md`). That
//! turned out to be a gift. Instead:
//!
//! - a **workspace rule** opens the stage inset via `gaps_out`, and the
//!   compositor lays the window into it — the rect is computed, never set, so it
//!   cannot drift from the design (measured `[10,31] 1980×1019`);
//! - the staged window is **maximized**, which covers any siblings, so "one task
//!   alone" holds on a workspace with three windows as well as one;
//! - showing another task is just **focusing** it.
//!
//! So **the tiling tree is never modified**, and there is no desktop to
//! reconstruct on the way out: un-maximizing is the compositor's own operation
//! and it restores the previous layout itself. What we record is not the
//! desktop but *our own changes* — what we put on the stage, the floating
//! windows we parked out of the way (the one case where a window does change
//! workspace, exactly reversible), and the compositor settings we silenced.
//!
//! ## If the daemon dies mid-stage
//!
//! The gap rules and the maximized window would outlive it, leaving the desktop
//! visibly wrong with nothing left running to explain why. So entering writes a
//! breadcrumb to `$XDG_RUNTIME_DIR`; [`recover_if_stranded`] runs at startup and
//! undoes a stage that no longer has an owner. The runtime dir is cleared on
//! reboot, which is the right lifetime for it.

use std::path::PathBuf;

use tracing::{debug, info, warn};

use crate::hypr;

/// The bottom gap the stage leaves for the deck, in logical pixels. The tiles
/// are drawn into this band; it is the "bigger gap at the bottom" of the design.
pub const BAND: i32 = 155;
/// The air on each side of the staged window.
///
/// Together with [`BAND`] and [`GAP_TOP`] this is the stage rect — the numbers
/// [`set_stage_gaps`](crate::hypr::set_stage_gaps) hands the compositor, which
/// then computes the rectangle itself.
///
/// Read against the *whole* distance from the screen's top edge down to the
/// window — the OPTIONS bar's reserved 28px plus [`GAP_TOP`], so 38px — rather
/// than against the top gap alone: what reads as the stage's inset is the band
/// of screen around it, and at the top that band includes the bar. Set a little
/// under that total rather than equal to it, which is the value Max landed on
/// by eye.
pub const GAP_SIDE: i32 = 30;
/// The air between the OPTIONS bar and the top of the staged window.
pub const GAP_TOP: i32 = 10;

/// How long the opening is given to play before the compositor's animation is
/// taken away (see [`Stage::silence`]).
///
/// Covers the `windows` leaf, which is the `easy` spring at speed 4.79 in
/// `/etc/nixos/hyprland.lua` — a little under half a second in practice. Erring
/// long costs only that a task switch made inside the first half-second animates
/// like an ordinary resize; erring short would cut the entrance in half, which
/// is the thing this exists to stop.
pub const ANIM_SETTLE: std::time::Duration = std::time::Duration::from_millis(520);

/// What the stage puts in the rect — one task, or one whole desk.
///
/// The mechanism is the same either way (the inset is a workspace rule, and
/// showing something is a focus), so the difference is small in code and large
/// in use: [`Mode::Task`] maximizes one window so it covers its siblings and the
/// deck holds every window; [`Mode::Desk`] maximizes nothing, so the workspace's
/// own tiling lays itself out inside the stage rect and the deck holds one tile
/// per workspace.
///
/// Chosen from the bar (the stage-mode pill) or `waverunner-ctl stage-mode`, and
/// remembered across sessions — it is a preference about how you work, not a
/// per-session mood.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    /// One task alone on the stage; a tile per window.
    #[default]
    Task,
    /// A whole workspace, tiling intact, inside the stage rect; a tile per
    /// workspace.
    Desk,
}

impl Mode {
    /// The other one — the whole of what the bar's pill does.
    pub fn flipped(self) -> Self {
        match self {
            Mode::Task => Mode::Desk,
            Mode::Desk => Mode::Task,
        }
    }
}

/// Where the chosen mode is remembered between sessions.
fn mode_path() -> std::path::PathBuf {
    crate::persist::data_path("stage-mode.json")
}

/// The mode the stage should open in, from the last session's choice.
pub fn remembered_mode() -> Mode {
    crate::persist::read_json(&mode_path()).unwrap_or_default()
}

/// Stage-mode state. Inert while `on` is false — the mode costs nothing when
/// it is not running.
///
/// There is no record of where the user came from: leaving keeps them on the
/// staged task, so there is nowhere to travel back to.
#[derive(Default)]
pub struct Stage {
    on: bool,
    /// One task, or one whole desk. Survives the mode being off — it is the
    /// mode the stage will open in next time.
    mode: Mode,
    /// The window currently on the stage.
    staged: Option<String>,
    /// Every window whose fullscreen state the stage has changed, mapped to the
    /// state it was found in (0/1/2). This is the whole record of what the mode
    /// owes the desktop, and it is settled in one go on the way out.
    ///
    /// Two things put a window in here:
    ///
    /// * **Taking the stage.** A task that leaves the stage **keeps the stage
    ///   shape** rather than being handed back immediately — taking it away
    ///   would make the client relayout its content, and give it back on the way
    ///   back, a visible hitch on every switch, twice. Held once, the second
    ///   visit costs nothing.
    /// * **The entry sweep.** The stage shape is itself a fullscreen state and a
    ///   workspace holds exactly one, so a window that was already fullscreen
    ///   when the mode opened would block the stage from ever shaping a sibling
    ///   on its workspace. Those come out of fullscreen on the way in.
    ///
    /// Either way the debt is the same — a window and the state to put it back
    /// into — so it is one map rather than two.
    ///
    /// Ordered rather than hashed so the breadcrumb it is written to is stable.
    shaped: std::collections::BTreeMap<String, i64>,
    /// Floating windows moved out of the way, mapped to the workspace each came
    /// from.
    ///
    /// The stage covers its workspace's other tasks by **maximizing**, and a
    /// maximized window covers only *tiled* ones — floating windows render above
    /// the whole tiling layer, so a floating sibling sat on top of the staged
    /// task and broke the one promise the mode makes. They cannot simply be
    /// un-floated either: re-floating restores a window's size but not its
    /// position, and this mode returns every window exactly as it found it.
    ///
    /// Moving them to another workspace does preserve both, exactly (verified),
    /// so that is what this is: out of sight while the stage owns the screen,
    /// back on the way out. See [`hypr::PARK_WS`] for why it is an ordinary
    /// workspace rather than a special one.
    parked: std::collections::BTreeMap<String, i64>,
    /// The animation leaves as they were before the stage silenced them, so
    /// leaving restores exactly what it found.
    anim_snapshot: Vec<hypr::AnimLeaf>,
    /// Whether the silence has landed yet — it arrives a beat after the mode
    /// opens, so the opening itself still animates. See [`Stage::silence`].
    silenced: bool,
    /// The deck, left to right: EVERY task, including the staged one, ordered by
    /// workspace. Tiles never change place — see `deck_order`.
    deck: Vec<String>,
    /// The deck in [`Mode::Desk`]: every occupied workspace, ascending. The
    /// same promise as `deck` and for the same reason — a desk is always where
    /// you last saw it, and the order is the one `Super+N` already taught.
    desks: Vec<i64>,
    /// The workspace on the stage in [`Mode::Desk`].
    desk: Option<i64>,
    /// The desktop's general gaps as the mode found them.
    ///
    /// [`Mode::Task`] leaves them alone: the one window on the stage is
    /// maximized, so it matches the smart-gaps selectors the mode re-points. A
    /// desk holds however many windows the user put there, which matches neither
    /// selector, so it is the general gaps that lay it out — and moving them is
    /// how the desk gets its inset (see [`hypr::set_general_gaps`]).
    ///
    /// This is the only record of what they were, so it rides in the breadcrumb:
    /// a desktop left with a 155px hole along the bottom of every workspace is
    /// exactly the kind of wrong a dead daemon must not leave behind.
    gaps: hypr::GapSnapshot,
    /// The lone window currently wearing the stage frame in [`Mode::Desk`].
    ///
    /// A desk showing one window is the same picture as a staged task, and the
    /// smart-gaps window rules would strip that window's rounding — so it gets
    /// the frame, exactly as a staged task does. A desk showing several does
    /// not: dimming *around* each of them would dim them against each other,
    /// and a borderless window on a desk of three is one you can no longer tell
    /// is focused.
    desk_tagged: Option<String>,
}

impl Stage {
    /// Off, and opening in whichever mode was last chosen.
    pub fn new() -> Self {
        Stage {
            mode: remembered_mode(),
            ..Default::default()
        }
    }

    // Read by the deck's renderer, which lands in the next step; the mode is
    // driven by `stage-toggle` / `stage-show` until then.
    #[allow(dead_code)]
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// The deck as addresses, left to right — what the tile strip renders.
    #[allow(dead_code)]
    pub fn deck(&self) -> &[String] {
        &self.deck
    }

    /// The address on the stage, if any. In [`Mode::Desk`] this is the task
    /// focus sits on within the shown desk — what the mode leaves you on.
    #[allow(dead_code)]
    pub fn staged(&self) -> Option<&str> {
        self.staged.as_deref()
    }

    /// One task, or one whole desk.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The deck in [`Mode::Desk`]: every occupied workspace, ascending.
    pub fn desks(&self) -> &[i64] {
        &self.desks
    }

    /// The workspace on the stage in [`Mode::Desk`].
    pub fn desk(&self) -> Option<i64> {
        self.desk
    }

    /// Switch between showing one task and showing one desk.
    ///
    /// Off the stage this only records the preference for next time. On it, the
    /// screen changes under the user, so the compositor work happens here: the
    /// two modes differ by what is maximized, what is parked, and which
    /// workspaces carry the inset.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        crate::persist::write_json("stage", &mode_path(), &mode);
        if !self.on {
            self.mode = mode;
            info!("stage: mode {mode:?} (for next time)");
            return;
        }
        let tasks = hypr::stage_tasks();
        match mode {
            Mode::Desk => {
                // Nothing is maximized on a desk. The debt itself stays —
                // `shaped` still records what each window was found in, which is
                // what exit hands back — but every stage shape is let go now, so
                // the workspace lays out as the user built it.
                for addr in self.shaped.keys() {
                    hypr::set_fullscreen_of(addr, 0);
                }
                if let Some(a) = self.staged.clone() {
                    hypr::set_stage_tag(&a, false);
                }
                // Floating windows are part of the desk — they were only in the
                // way of a maximized window covering its siblings.
                for (addr, ws) in std::mem::take(&mut self.parked) {
                    hypr::unpark_window(&addr, ws);
                }
                self.mode = Mode::Desk;
                self.desks = desk_order(&tasks);
                self.desk = self
                    .staged
                    .as_ref()
                    .and_then(|a| tasks.iter().find(|t| &t.address == a))
                    .map(|t| t.workspace)
                    .or_else(|| self.desks.first().copied());
                // The inset every desk is laid out into. The snapshot to put
                // back was taken on the way into the mode, so flipping modes
                // repeatedly can never record the stage's own gaps as the
                // desktop's.
                hypr::set_general_gaps(BAND);
                self.retag_desk(&tasks);
            }
            Mode::Task => {
                self.mode = Mode::Task;
                // The desktop's own gaps come back: a staged task is maximized
                // and takes its rect from the smart-gaps selectors instead, so
                // leaving the general ones moved would only be felt by the
                // workspaces nobody is looking at.
                hypr::restore_general_gaps(&self.gaps);
                if let Some(a) = self.desk_tagged.take() {
                    hypr::set_stage_tag(&a, false);
                }
                self.deck = deck_order(&tasks, &self.parked);
                // The task you were looking at takes the stage. Cleared first so
                // the switch runs its full course — parking the floating
                // siblings, evicting whatever holds the workspace's fullscreen
                // slot, maximizing and tagging — rather than returning early on
                // "already staged".
                let addr = self
                    .staged
                    .take()
                    .or_else(|| tasks.first().map(|t| t.address.clone()));
                if let Some(a) = addr {
                    self.show(&a);
                }
            }
        }
        write_breadcrumb(&self.anim_snapshot, &self.shaped, &self.parked, &self.gaps);
        info!("stage: mode {mode:?}");
    }

    /// Hand the stage frame to the shown desk's window when it is the only one
    /// there, and take it off whoever had it.
    fn retag_desk(&mut self, tasks: &[hypr::StageTask]) {
        let on_desk: Vec<&hypr::StageTask> = tasks
            .iter()
            .filter(|t| Some(t.workspace) == self.desk)
            .collect();
        // Only a lone TILED window: a lone floating one keeps its own frame
        // (the smart-gaps rules never touched it), and dimming around it would
        // darken the desk it is floating over.
        let alone = match on_desk.as_slice() {
            [t] if !t.floating => Some(t.address.clone()),
            _ => None,
        };
        if alone == self.desk_tagged {
            return;
        }
        if let Some(prev) = self.desk_tagged.take() {
            hypr::set_stage_tag(&prev, false);
        }
        if let Some(a) = alone {
            hypr::set_stage_tag(&a, true);
            self.desk_tagged = Some(a);
        }
    }

    /// Take the compositor's animation away, once the opening has played.
    ///
    /// The mode wants both things at different moments: the way **in** is a
    /// movement and should be seen, while switching tasks inside it must be an
    /// instant cut (Max: "too much animation on the window switching… only the
    /// ones on the deck"). So the silence arrives late, on the daemon's timer,
    /// rather than before the window has moved.
    ///
    /// Idempotent, and a no-op once the mode is off: the timer can always fire
    /// after a stage that has already closed, and a silence landing then would
    /// leave the *desktop* un-animated with nothing left to restore it.
    pub fn silence(&mut self) {
        if !self.on || self.silenced {
            return;
        }
        // Silencing wipes every leaf's speed and curve, and the snapshot is the
        // only thing that can put them back — a bare `enabled = true` does not
        // even re-enable a leaf. So an empty snapshot (socket error, parse
        // failure) means silencing would leave the desktop permanently
        // un-animated, with the breadcrumb recording nothing to recover from
        // either. A stage that animates its switches is much the lesser evil.
        if self.anim_snapshot.is_empty() {
            return;
        }
        hypr::silence_animations();
        self.silenced = true;
    }

    pub fn toggle(&mut self) {
        if self.on {
            self.exit();
        } else {
            self.enter();
        }
    }

    /// Enter: open the stage inset, silence the compositor's animation, and
    /// maximize the entry task — the focused window, or on an **empty
    /// workspace** the most recently focused task anywhere (the mode is a view
    /// of every task, so an empty space is a fine door into it; refusing to
    /// open there just read as a dead keybind).
    fn enter(&mut self) {
        if self.on {
            return;
        }
        let focused_live = hypr::active_window();
        let tasks = hypr::stage_tasks();
        // `stage_tasks` is sorted by focus recency, so on an empty workspace
        // the first entry IS the last focused task.
        let Some(focused) = focused_live
            .clone()
            .or_else(|| tasks.first().map(|t| t.address.clone()))
        else {
            // No windows anywhere: nothing to stage, and entering would strand
            // the gap rules for no benefit.
            debug!("stage: no tasks at all, not entering");
            return;
        };
        // Entering from an empty workspace means the entry task is somewhere
        // else — focus has to travel to it before it can be maximized there.
        let travelled = focused_live.is_none();
        self.deck = deck_order(&tasks, &self.parked);
        self.desks = desk_order(&tasks);
        for (i, addr) in self.deck.iter().enumerate() {
            if let Some(t) = tasks.iter().find(|t| &t.address == addr) {
                debug!(
                    "stage: deck[{i}] ws{} {} — {}{}",
                    t.workspace,
                    t.class,
                    t.title,
                    if *addr == focused { "  (on stage)" } else { "" }
                );
            }
        }

        // The task list already carries fullscreen and floating, so the ONE
        // `j/clients` read above serves the deck, the entry sweep and the
        // parking decision. It used to be two reads — two snapshots of the same
        // desktop that a window closing in between could make disagree.
        self.shaped.clear();
        for t in &tasks {
            if t.fullscreen != 0 {
                self.shaped.insert(t.address.clone(), t.fullscreen);
            }
        }
        // The task taking the stage has a state to be put back into even when
        // that state is plain windowed — and if the sweep already recorded it,
        // that reading is the true one and must not be overwritten.
        self.shaped.entry(focused.clone()).or_insert(0);
        // Floating windows sharing the opening task's workspace go out of sight:
        // a maximized stage covers tiled siblings only, and a floating one would
        // sit on top of it. See [`Stage::parked`]. A desk has nothing to cover —
        // its windows are all meant to be on screen — so nothing is parked there.
        self.parked.clear();
        let focused_ws = tasks
            .iter()
            .find(|t| t.address == focused)
            .map(|t| t.workspace);
        if self.mode == Mode::Task {
            for t in &tasks {
                if t.floating && t.address != focused && Some(t.workspace) == focused_ws {
                    if let Some(ws) = focused_ws {
                        self.parked.insert(t.address.clone(), ws);
                    }
                }
            }
        }
        self.staged = Some(focused);
        self.desk = focused_ws;
        self.on = true;

        // Snapshot the animation leaves BEFORE silencing them, and write it into
        // the breadcrumb — silencing wipes each leaf's speed and curve, so this
        // record is the only way back if the daemon dies while staged.
        self.anim_snapshot = hypr::snapshot_animations();
        // Read before anything is moved, and in both modes: a mid-session switch
        // to desks must find the desktop's own numbers here, not the stage's.
        self.gaps = hypr::snapshot_general_gaps();
        if self.mode == Mode::Desk {
            hypr::set_general_gaps(BAND);
        }
        write_breadcrumb(&self.anim_snapshot, &self.shaped, &self.parked, &self.gaps);
        // Silencing wipes every leaf's speed and curve, and the snapshot is the
        // only thing that can put them back — a bare `enabled = true` does not
        // even re-enable a leaf. So an empty snapshot (socket error, parse
        // failure) means silencing would leave the desktop permanently
        // un-animated, with the breadcrumb recording nothing to recover from
        // either. A stage that animates its switches is much the lesser evil.
        if self.anim_snapshot.is_empty() {
            warn!("stage: no animation snapshot; leaving compositor animation alone");
        }
        // NOT silenced here: the opening itself is a movement, and the whole of
        // it is the compositor carrying the task into the stage rect. Silencing
        // before that made the mode arrive as a cut. [`Stage::silence`] takes the
        // animation away once the opening has played — see `ANIM_SETTLE`.
        hypr::set_stage_gaps(BAND);
        hypr::assert_stage_frame();
        // The overview stays reachable from inside the mode (Max, 2026-09-12):
        // it is the map you use to find the task you want on the stage, and it
        // hands whatever you land on straight to it. Super+R therefore keeps its
        // binding; the stage submap carries its own copy, since a submap hides
        // the ordinary keymap.
        hypr::set_plugin_stage(true);
        hypr::set_stage_submap(true);
        // The entry sweep: everything that arrived fullscreen comes out of it,
        // so no pre-existing fullscreen window can hold its workspace's one slot
        // against a task the user later puts on the stage. The task taking the
        // stage is skipped — it goes straight to the stage shape rather than
        // through plain windowed, which would be a visible bounce. On a desk
        // nothing is maximized at all, so it is swept with the rest: a
        // fullscreen window there would cover the whole screen, gaps and deck
        // included.
        //
        // One dispatch each, deliberately: a single eval aborts at the first
        // window that has closed since the read above, leaving the rest
        // fullscreen with no record that they were missed.
        let staged = self.staged.clone();
        let keep_shaped = (self.mode == Mode::Task).then(|| staged.clone()).flatten();
        for addr in self
            .shaped
            .keys()
            .filter(|a| Some(a.as_str()) != keep_shaped.as_deref())
        {
            hypr::set_fullscreen_of(addr, 0);
        }
        for addr in self.parked.keys() {
            hypr::park_window(addr);
        }
        if let Some(a) = staged {
            // Entered from an empty workspace: travel to the entry task first —
            // the focus is what brings its workspace forward, exactly as in a
            // deck switch.
            if travelled {
                hypr::focus_window_no_warp(&a);
            }
            match self.mode {
                Mode::Task => {
                    hypr::set_fullscreen_of(&a, 1);
                    hypr::set_stage_tag(&a, true);
                }
                // The desk needs no shaping: the workspace rule is the whole of
                // it, and the compositor has already laid the windows into the
                // inset. All that is left is the frame for a desk of one.
                Mode::Desk => self.retag_desk(&tasks),
            }
        }
        info!(
            "stage: on, {:?} ({} tasks, {} desks)",
            self.mode,
            self.deck.len(),
            self.desks.len()
        );
    }

    /// Leave: un-maximize, put the smart-gaps rules and the animation back, and
    /// **stay on the task that was on stage**.
    ///
    /// You leave the mode where you were working, not where you entered from —
    /// so the stage doubles as a way to travel. (It used to return to an
    /// "anchor" recorded on entry; Max reversed that on 2026-09-05, which is why
    /// the anchor is gone entirely rather than left unused.)
    ///
    /// Every window's *geometry* is still restored exactly; it is only which
    /// task you are looking at that now differs.
    fn exit(&mut self) {
        if !self.on {
            return;
        }
        // The animation comes back BEFORE anything moves, so the way out is the
        // way in played backwards: the task settles out of the stage rect
        // instead of snapping there. Everything below this line is a resize the
        // user should see happen. (It used to be the last thing restored, which
        // is exactly why leaving read as a cut.)
        hypr::restore_animations(&std::mem::take(&mut self.anim_snapshot));
        self.silenced = false;
        // Focus the staged task FIRST — both because that is where we are
        // deliberately leaving the user, and because focusing a window kicks
        // fullscreen off its same-workspace siblings: done after the restore,
        // it silently undid a just-repaid fullscreen debt on the staged task's
        // own workspace (measured — a window restored to fs=2 read fs=0 one
        // focus later).
        if let Some(addr) = self.staged.clone() {
            if hypr::window_exists(&addr) {
                hypr::focus_window_no_warp(&addr);
            }
        }
        // Settle the whole debt: the tasks still holding the stage shape since
        // they were last staged, and the windows the entry sweep took out of
        // fullscreen. This is the moment the user expects the desktop to move,
        // so every resize lands here rather than being spread across the
        // session.
        restore_shaped(&std::mem::take(&mut self.shaped));
        // Floating windows come home to the workspace they were taken from, with
        // the position and size they had — that round trip is exact, which is
        // why parking was chosen over un-floating them.
        for (addr, ws) in std::mem::take(&mut self.parked) {
            hypr::unpark_window(&addr, ws);
        }
        if let Some(addr) = self.desk_tagged.take() {
            hypr::set_stage_tag(&addr, false);
        }
        hypr::clear_stage_gaps();
        // Unconditionally, whichever mode was up: it writes back what was read
        // on the way in, so on a stage that never showed a desk it writes the
        // values that are already there.
        hypr::restore_general_gaps(&self.gaps);
        hypr::set_stage_submap(false);
        hypr::set_plugin_stage(false);

        // Nothing to travel back to: focus is already on the staged task, which
        // is exactly where the user asked to be left.

        self.on = false;
        self.staged = None;
        self.desk = None;
        self.deck.clear();
        self.desks.clear();
        clear_breadcrumb();
        info!("stage: off");
    }

    /// Put `addr` on the stage.
    ///
    /// The task leaving does **not** go back to plain tiling: it keeps the stage
    /// shape (see [`Stage::shaped`]), so returning to it later costs its client
    /// no relayout. The only tasks handed back here are the ones that have to be:
    /// a workspace holds one fullscreen window, so anything already shaped on the
    /// incoming task's workspace must let go first.
    ///
    /// Returns whether `addr` is now the task on stage, so the deck can decline
    /// to mark a tile it failed to stage.
    pub fn show(&mut self, addr: &str) -> bool {
        if !self.on {
            debug!("stage: show({addr}) ignored, not staged");
            return false;
        }
        if self.staged.as_deref() == Some(addr) {
            return true;
        }
        // ONE compositor read for the whole switch: whether both windows are
        // still there, and what state the incoming one was in. Asking these
        // separately cost three round-trips between the click and the first
        // frame of the tile animation, which is exactly where a stall shows.
        let states = hypr::window_states();
        if !states.contains_key(addr) {
            // The deck can outlive a window that was closed behind our back.
            warn!("stage: {addr} is gone, dropping it from the deck");
            self.deck.retain(|a| a != addr);
            self.shaped.remove(addr);
            return false;
        }

        // The task leaving keeps its shape; it only loses the frame.
        let untag = self
            .staged
            .clone()
            .filter(|prev| states.contains_key(prev) && prev != addr);

        // The arriving task may itself be parked — it has to come home before it
        // can take a stage on the workspace it belongs to.
        let unpark: Vec<(String, i64)> = self
            .parked
            .remove(addr)
            .map(|ws| vec![(addr.to_owned(), ws)])
            .unwrap_or_default();
        let target_ws = unpark
            .first()
            .map(|(_, ws)| *ws)
            .or_else(|| states.get(addr).map(|s| s.workspace));
        // Every floating window sharing that workspace goes out of sight.
        let park: Vec<String> = states
            .iter()
            .filter(|(a, s)| {
                s.floating
                    && a.as_str() != addr
                    && Some(s.workspace) == target_ws
                    && !self.parked.contains_key(*a)
            })
            .map(|(a, _)| a.clone())
            .collect();
        for a in &park {
            if let Some(ws) = target_ws {
                self.parked.insert(a.clone(), ws);
            }
        }
        // Moving a user's window is the most consequential thing this mode does,
        // so every move it makes is on the record.
        if !park.is_empty() || !unpark.is_empty() {
            debug!(
                "stage: park {park:?} -> ws{}, unpark {unpark:?}",
                hypr::PARK_WS
            );
        }

        // Whoever holds the fullscreen slot on the workspace we are moving to
        // has to let go — the compositor allows exactly one per workspace and
        // refuses the second, silently. This is the one case where a task's
        // content is resized twice, and it is unavoidable: two tasks sharing a
        // workspace cannot both hold the stage shape.
        //
        // Judged by the window's **actual** state, not by membership in
        // `shaped`: the stage submap blocks binds but not a client's own
        // fullscreen request, so a video player can take the slot mid-session
        // all by itself — and an eviction list built from our own bookkeeping
        // would walk right past it, leaving the arriving task's maximize to be
        // refused.
        //
        // Evictees go to plain **windowed**, not to the state they were found
        // in. Restoring one to fullscreen here would simply retake the slot,
        // and then the incoming task's maximize would be the one silently
        // refused — which is exactly how a task that arrived true-fullscreen
        // once came out of the mode merely windowed. The real state is handed
        // back at exit, the one moment nothing is competing for the slot; the
        // debt entry below is what remembers it.
        let restore: Vec<(String, i64)> = states
            .iter()
            .filter(|(a, s)| {
                a.as_str() != addr && Some(s.workspace) == target_ws && s.fullscreen != 0
            })
            .map(|(a, _)| (a.clone(), 0))
            .collect();
        for (a, _) in &restore {
            let found = states.get(a).map(|s| s.fullscreen).unwrap_or(0);
            // `or_insert`: a window already carrying a debt keeps its original
            // one — its current state is the stage shape, not what it is owed.
            self.shaped.entry(a.clone()).or_insert(found);
        }

        // Remembered the first time this task is staged, and never overwritten:
        // once it holds the stage shape its reported state is "maximized", which
        // is not what it should be handed back to.
        if let Some(state) = states.get(addr) {
            self.shaped
                .entry(addr.to_owned())
                .or_insert(state.fullscreen);
        }
        // Recorded before the compositor is touched, so anything that reacts to
        // the workspace change already sees the new stage rather than the old.
        // The deck itself does not change — tiles keep their places. Only which
        // one is raised.
        self.staged = Some(addr.to_owned());
        // Kept current even here, so switching to desks lands on the desk of the
        // task you were just looking at rather than the one you opened on.
        self.desk = target_ws.or(self.desk);
        write_breadcrumb(&self.anim_snapshot, &self.shaped, &self.parked, &self.gaps);

        // The focus bounce's neighbour, from the read already made — a lookup
        // inside the swap cost a second `j/clients` round-trip at click time.
        // Not one being parked in this same eval: it would have left the
        // workspace by the time the bounce runs, and bouncing off it would drag
        // the whole eval through the park workspace.
        let neighbor = states
            .iter()
            .find(|(a, s)| {
                a.as_str() != addr && Some(s.workspace) == target_ws && !park.contains(*a)
            })
            .map(|(a, _)| a.clone());

        // The whole hand-over in one eval, so no frame is ever drawn mid-swap —
        // see `swap_stage_no_warp` for why each part is where it is.
        hypr::swap_stage_no_warp(hypr::Handover {
            untag: untag.as_deref(),
            restore: &restore,
            park: &park,
            unpark: &unpark,
            addr,
            neighbor: neighbor.as_deref(),
        });
        true
    }

    /// Put workspace `ws` on the stage — the desk-mode switch.
    ///
    /// Nothing is moved, shaped or parked: a desk is shown by **focusing** a
    /// task on it, which is what brings its workspace forward. Focus lands on
    /// that desk's most recently used task, so a desk you come back to hands you
    /// the window you left off in rather than an arbitrary one.
    ///
    /// Returns whether the desk is now on the stage, so the deck can decline to
    /// raise a tile it failed to show.
    pub fn show_desk(&mut self, ws: i64) -> bool {
        if !self.on || self.mode != Mode::Desk {
            debug!("stage: show_desk({ws}) ignored, not on a desk");
            return false;
        }
        if self.desk == Some(ws) {
            return true;
        }
        // Focus-recency ordered, so the first match IS the task last used there.
        let tasks = hypr::stage_tasks();
        let Some(task) = tasks.iter().find(|t| t.workspace == ws) else {
            // The deck can outlive a desk whose last window closed behind our
            // back.
            warn!("stage: desk {ws} is empty, dropping it from the deck");
            self.desks.retain(|d| *d != ws);
            return false;
        };
        // Nothing to set up: the inset is the desktop's general gaps, so a desk
        // that did not exist when the mode opened already has it.
        self.desk = Some(ws);
        // What the mode will leave the user on, exactly as `staged` means in
        // task mode.
        self.staged = Some(task.address.clone());
        hypr::focus_window_no_warp(&task.address);
        self.retag_desk(&tasks);
        write_breadcrumb(&self.anim_snapshot, &self.shaped, &self.parked, &self.gaps);
        true
    }

    /// Re-derive the deck from the live task list, keeping whatever is on stage.
    ///
    /// The deck is a map of the desktop, and the desktop changes while the mode
    /// is up: windows open and close under it. Rebuilding from
    /// [`deck_order`] keeps every existing tile in its slot — the sort is by
    /// workspace and stable — and moves only what an arrival or a departure
    /// genuinely moves.
    pub fn resync(&mut self, tasks: &[hypr::StageTask]) {
        if !self.on {
            return;
        }
        self.deck = deck_order(tasks, &self.parked);
        self.desks = desk_order(tasks);
        if self.mode == Mode::Desk {
            // The window that appeared (or the one that closed) may have changed
            // whether the shown desk is a desk of one.
            self.retag_desk(tasks);
        }
    }

    /// Drop a closed window from the deck (called on a compositor close event).
    /// If the *staged* one closed, the mode has nothing in front of it, so fall
    /// back to the leftmost task rather than leaving a hole.
    pub fn forget(&mut self, addr: &str) {
        if !self.on {
            return;
        }
        self.deck.retain(|a| a != addr);
        // A closed window has no shape to hand back and nowhere to be brought
        // home to; leaving it in either set would have `exit` dispatch at an
        // address that no longer exists.
        self.shaped.remove(addr);
        self.parked.remove(addr);
        if self.desk_tagged.as_deref() == Some(addr) {
            self.desk_tagged = None;
        }
        if self.mode == Mode::Desk {
            let tasks = hypr::stage_tasks();
            self.desks = desk_order(&tasks);
            // Only an emptied desk needs anything done: the shown desk survives
            // its other windows closing, and the compositor hands focus on to a
            // sibling by itself.
            if self.desk.is_some_and(|ws| !self.desks.contains(&ws)) {
                self.desk = None;
                self.staged = None;
                let candidates = self.desks.clone();
                for ws in candidates {
                    if self.show_desk(ws) {
                        break;
                    }
                }
            } else {
                if self.staged.as_deref() == Some(addr) {
                    self.staged = tasks
                        .iter()
                        .find(|t| Some(t.workspace) == self.desk)
                        .map(|t| t.address.clone());
                }
                self.retag_desk(&tasks);
            }
            return;
        }
        if self.staged.as_deref() == Some(addr) {
            self.staged = None;
            // Walk the deck rather than trying only the leftmost: several
            // windows can close at once (an app quitting takes its whole set
            // with it), and the events arrive one at a time, so the first
            // candidate may already be gone too.
            let candidates = self.deck.clone();
            for next in candidates {
                if self.show(&next) {
                    break;
                }
            }
        }
    }
}

/// The deck's order: **every** task, including the one on stage, sorted by
/// workspace and stable within it.
///
/// A tile never changes place. Switching tasks only raises a different one, so
/// a task is always where you last saw it — the deck becomes a map you can learn
/// rather than a list that reshuffles under you. Sorting by workspace also keeps
/// it aligned with `Super+N`.
///
/// (This replaced a recency order in which the staged task left the deck and the
/// one it displaced rejoined at the far left. Max moved away from it precisely
/// because the tiles moved.)
fn deck_order(
    tasks: &[hypr::StageTask],
    parked: &std::collections::BTreeMap<String, i64>,
) -> Vec<String> {
    let mut ordered: Vec<&hypr::StageTask> = tasks.iter().collect();
    // By the workspace a task belongs to, not where it currently sits: a parked
    // floating window is off on the park workspace, and sorting by that would
    // fling its tile to the end of the row.
    ordered.sort_by_key(|t| parked.get(&t.address).copied().unwrap_or(t.workspace));
    ordered.into_iter().map(|t| t.address.clone()).collect()
}

/// The desk deck's order: every **occupied** workspace, ascending.
///
/// Empty workspaces are left out — a desk with nothing on it is not a place you
/// travel to, and the row would carry a tile you could never recognise. Ascending
/// rather than by recency for the same reason the task deck sorts by workspace:
/// it is the order `Super+N` already taught, so the row is a map rather than a
/// history.
fn desk_order(tasks: &[hypr::StageTask]) -> Vec<i64> {
    let mut out: Vec<i64> = tasks
        .iter()
        // The park workspace is machinery, not a desk: it only ever holds
        // windows the mode itself moved out of the way, and it is dissolved on
        // exit. (It cannot hold any in desk mode, but a mode switch mid-session
        // can leave one there for as long as it takes to bring it home.)
        .filter(|t| t.workspace != hypr::PARK_WS)
        .map(|t| t.workspace)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Breadcrumb path — runtime dir, so it cannot survive a reboot.
fn breadcrumb() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("waverunner");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("stage-active.json"))
}

fn write_breadcrumb(
    animations: &[hypr::AnimLeaf],
    shaped: &std::collections::BTreeMap<String, i64>,
    parked: &std::collections::BTreeMap<String, i64>,
    gaps: &hypr::GapSnapshot,
) {
    let Some(path) = breadcrumb() else {
        return;
    };
    // Only what a dead daemon could not otherwise recover. Focus needs no
    // record: leaving the mode keeps the user where they are. The other two do —
    // several windows may be holding the stage shape by now, and others may be
    // parked on another workspace, and in both cases where each belongs is not
    // visible from the outside. A parked window nobody remembers is a window the
    // user has simply lost.
    // The general gaps travel too: showing a desk moves them, and a dead daemon
    // would otherwise leave a 155px hole along the bottom of every workspace
    // with nothing on screen to explain it.
    let body = serde_json::json!({
        "animations": animations,
        "shaped": shaped,
        "parked": parked,
        "gaps": gaps,
    });
    if let Err(e) = std::fs::write(&path, body.to_string()) {
        warn!("stage: could not write breadcrumb: {e}");
    }
}

/// Hand every window back the fullscreen state it was found in, and drop the
/// stage tag from all of them.
///
/// **Windowed first, fullscreen second.** A workspace holds one fullscreen
/// window, so restoring a window that was found fullscreen can only land once
/// whatever is currently holding that workspace's slot — typically a task still
/// wearing the stage shape — has let go. Done in map order it is a coin toss
/// decided by where the two addresses happen to sort, and losing it means
/// handing a task back windowed when it was found fullscreen.
///
/// One dispatch per window rather than one eval: a window that closed since the
/// set was recorded would abort an eval and strand every window after it.
fn restore_shaped(shaped: &std::collections::BTreeMap<String, i64>) {
    for pass in [false, true] {
        for (addr, prev_fs) in shaped.iter().filter(|(_, fs)| (**fs != 0) == pass) {
            hypr::set_fullscreen_of(addr, *prev_fs);
        }
    }
    for addr in shaped.keys() {
        hypr::set_stage_tag(addr, false);
    }
}

fn clear_breadcrumb() {
    if let Some(path) = breadcrumb() {
        let _ = std::fs::remove_file(path);
    }
}

/// Undo a stage that outlived its daemon. Called once at startup: if the
/// breadcrumb is there, the previous process died with the gap rules re-pointed
/// and a window maximized, so put both back before anyone notices.
pub fn recover_if_stranded() {
    let Some(path) = breadcrumb() else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return; // the common case: no breadcrumb, nothing to do
    };
    warn!("stage: found a stranded stage from a previous daemon, restoring");

    // Hand back every window the dead daemon had shaped. Read before anything
    // else touches the compositor, and falling back to un-maximizing whatever is
    // focused if the record is unreadable — an older breadcrumb, or a truncated
    // write.
    let record: Option<serde_json::Value> = serde_json::from_str(&raw).ok();
    let shaped: std::collections::BTreeMap<String, i64> = record
        .as_ref()
        .and_then(|v| serde_json::from_value(v["shaped"].clone()).ok())
        .unwrap_or_default();
    if shaped.is_empty() {
        warn!("stage: breadcrumb has no shaped set; un-maximizing the focused window only");
        hypr::maximize_focused(false);
    } else {
        restore_shaped(&shaped);
    }
    // Parked windows are the urgent half of recovery: a shape left behind is
    // merely wrong-looking, but a window still sitting on the park workspace has
    // effectively vanished from the user's desktop.
    let parked: std::collections::BTreeMap<String, i64> = record
        .as_ref()
        .and_then(|v| serde_json::from_value(v["parked"].clone()).ok())
        .unwrap_or_default();
    for (addr, ws) in &parked {
        hypr::unpark_window(addr, *ws);
    }
    hypr::clear_stage_gaps();
    // The general gaps, which a desk on the stage had moved. Nothing on screen
    // says what they were, so the record is the only way back — and its default
    // is the config's own numbers, for a breadcrumb too old or too broken to
    // carry them.
    let gaps: hypr::GapSnapshot = record
        .as_ref()
        .and_then(|v| serde_json::from_value(v["gaps"].clone()).ok())
        .unwrap_or_default();
    hypr::restore_general_gaps(&gaps);
    // The tag sweep is wider than the shaped set on purpose: a breadcrumb
    // written before the last switch would name the wrong window, and a leftover
    // `golem-stage` tag keeps a border and rounding on it forever. Clearing it
    // from every window is cheap and cannot miss.
    for task in hypr::stage_tasks() {
        hypr::set_stage_tag(&task.address, false);
    }
    // Most important of the lot: a stranded stage leaves the keyboard in the
    // stage submap, where almost nothing is bound.
    hypr::set_stage_submap(false);
    hypr::set_plugin_stage(false);

    if let Some(v) = record {
        // The animation snapshot travels in the breadcrumb precisely for this:
        // silencing a leaf wipes its speed and curve, so without the record
        // there is nothing to put back and the desktop would stay un-animated.
        if let Ok(leaves) = serde_json::from_value::<Vec<hypr::AnimLeaf>>(v["animations"].clone()) {
            hypr::restore_animations(&leaves);
        } else {
            warn!("stage: breadcrumb has no animation snapshot; leaves stay as they are");
        }
    }
    // Focus is left alone: wherever the dead daemon left the user is where the
    // mode would have left them anyway.
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(addr: &str, ws: i64) -> hypr::StageTask {
        hypr::StageTask {
            address: addr.to_owned(),
            class: "foot".into(),
            title: addr.to_owned(),
            workspace: ws,
            history: 0,
            fullscreen: 0,
            floating: false,
            pid: 0,
        }
    }

    #[test]
    fn a_parked_task_keeps_its_slot() {
        // A parked floating window physically sits on the park workspace, which
        // sorts after everything. Ordering by where it *belongs* is what keeps
        // its tile from being flung to the end of the row the moment the stage
        // moves it aside.
        let tasks = vec![task("a", 1), task("floater", 2), task("c", 3)];
        let mut parked = std::collections::BTreeMap::new();
        parked.insert("floater".to_owned(), 2);
        let moved = vec![task("a", 1), task("floater", hypr::PARK_WS), task("c", 3)];
        assert_eq!(
            deck_order(&moved, &parked),
            deck_order(&tasks, &Default::default())
        );
        assert_eq!(deck_order(&moved, &parked), vec!["a", "floater", "c"]);
    }

    #[test]
    fn resync_keeps_every_surviving_tile_in_its_slot() {
        let mut stage = Stage {
            on: true,
            ..Default::default()
        };
        stage.resync(&[task("a", 1), task("b", 4), task("c", 10)]);
        assert_eq!(stage.deck(), ["a", "b", "c"]);

        // `b` closes: the others keep their order, they just close up.
        stage.resync(&[task("a", 1), task("c", 10)]);
        assert_eq!(stage.deck(), ["a", "c"]);

        // A window opens on ws5: it lands where its workspace puts it, and the
        // tiles either side keep their relative order.
        stage.resync(&[task("a", 1), task("c", 10), task("new", 5)]);
        assert_eq!(stage.deck(), ["a", "new", "c"]);
    }

    #[test]
    fn resync_does_nothing_while_the_mode_is_off() {
        // The deck is empty and inert when the stage is down; a window opening
        // must not quietly build one.
        let mut stage = Stage::default();
        stage.resync(&[task("a", 1)]);
        assert!(stage.deck().is_empty());
    }

    #[test]
    fn the_deck_holds_every_task_including_the_staged_one() {
        let tasks = vec![task("a", 3), task("b", 1), task("c", 2)];
        assert_eq!(deck_order(&tasks, &Default::default()).len(), 3);
    }

    #[test]
    fn tiles_are_ordered_by_workspace() {
        let tasks = vec![task("a", 10), task("b", 1), task("c", 4)];
        assert_eq!(deck_order(&tasks, &Default::default()), vec!["b", "c", "a"]);
    }

    #[test]
    fn order_is_stable_for_two_windows_on_one_workspace() {
        // Same workspace keeps the compositor's order, so the pair does not
        // swap places from one rebuild to the next.
        let tasks = vec![task("first", 1), task("second", 1), task("other", 2)];
        assert_eq!(
            deck_order(&tasks, &Default::default()),
            vec!["first", "second", "other"]
        );
    }

    #[test]
    fn switching_tasks_does_not_reorder_the_deck() {
        // The property Max asked for: a tile never moves. Whatever is staged,
        // the same task list yields the same order.
        let tasks = vec![task("a", 2), task("b", 1)];
        let before = deck_order(&tasks, &Default::default());
        let after = deck_order(&tasks, &Default::default());
        assert_eq!(before, after);
        assert_eq!(before, vec!["b", "a"]);
    }

    #[test]
    fn a_desk_deck_holds_one_tile_per_occupied_workspace() {
        // Three windows on ws1 are ONE desk — that is the whole difference
        // between the two decks.
        let tasks = vec![
            task("a", 1),
            task("b", 1),
            task("c", 1),
            task("d", 4),
            task("e", 2),
        ];
        assert_eq!(desk_order(&tasks), vec![1, 2, 4]);
    }

    #[test]
    fn the_desk_deck_is_ascending_whatever_the_focus_order() {
        // `stage_tasks` hands them over most-recently-focused first; the row is
        // a map, so it sorts by number regardless.
        let tasks = vec![task("a", 10), task("b", 1), task("c", 4)];
        assert_eq!(desk_order(&tasks), vec![1, 4, 10]);
    }

    #[test]
    fn the_park_workspace_is_not_a_desk() {
        // Windows the mode itself moved aside must not grow a tile of their
        // own — it would be a desk the user never made and cannot get back to.
        let tasks = vec![task("a", 1), task("floater", hypr::PARK_WS)];
        assert_eq!(desk_order(&tasks), vec![1]);
    }

    #[test]
    fn a_desk_stays_in_its_slot_when_another_empties() {
        // The task deck's promise, for desks: closing everything on ws4 must not
        // move ws10's tile relative to ws1's.
        let before = desk_order(&[task("a", 1), task("b", 4), task("c", 10)]);
        let after = desk_order(&[task("a", 1), task("c", 10)]);
        assert_eq!(before, vec![1, 4, 10]);
        assert_eq!(after, vec![1, 10]);
        assert_eq!(before.first(), after.first());
    }

    #[test]
    fn the_mode_flips_and_comes_back() {
        assert_eq!(Mode::Task.flipped(), Mode::Desk);
        assert_eq!(Mode::Desk.flipped().flipped(), Mode::Desk);
    }
}

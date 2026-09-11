//! The STAGE deck — the strip of task tiles under the staged window.
//!
//! One tile per open task — **including the one on stage** — floating on the
//! wallpaper near the bottom edge: no band, no glass, nothing behind them.
//!
//! ## Tiles never move
//!
//! The row is a fixed map. A tile's slot changes only when a window opens or
//! closes, never when you switch tasks; clicking one simply **raises** it and
//! lowers whatever was up. So a task stays where you last saw it and the deck
//! becomes something you can learn, rather than a list that reshuffles under
//! your hand. The raise is also what tells you which task is current.
//!
//! That is the whole animation. (An earlier design had the staged task leave
//! the deck, the row close over the gap and the displaced task rejoin at the
//! far left — Max moved away from it precisely because the tiles moved.)
//!
//! The deck's order lives in [`crate::stage`] (pure, unit-tested); this module
//! is only how it looks and moves.

use smithay_client_toolkit::shell::WaylandSurface;

use crate::content::{IconInst, Label, Rect, RectInst, Scene};
use crate::App;

/// Tile height in logical px; width follows the output's aspect so a tile reads
/// as a small screen.
pub const TILE_H: f32 = 138.0;
/// Gap between tiles.
pub const GAP: f32 = 14.0;
/// Corner radius — a touch tighter than a window's 12, so a tile reads as a
/// miniature rather than a small window.
const RADIUS: f32 = 10.0;

/// How close the tiles sit to the bottom edge of the screen. Small on purpose —
/// the row reads as a deck resting *on* the edge, not floating above it.
const BOTTOM_MARGIN: f32 = 4.0;
/// How far the current task's tile stands above the others — a nudge, not a
/// step: the peach frame below is what actually names it, so the raise only has
/// to hint that it is out of line with the rest.
const RAISE: f32 = 5.0;
/// The raise, in seconds — the deck's only motion now.
const RAISE_DUR: f32 = 0.20;

/// Inset of the speaker indicator from a sounding tile's top-left corner.
const BADGE_PAD: f32 = 8.0;
/// The indicator's label box.
const BADGE_W: f32 = 38.0;

/// sRGB hex → the **linear** values the pipeline wants.
///
/// The swapchain is an `…Srgb` format, so what a shader writes is treated as
/// linear and hardware-encoded on the way out. Authoring `0.05` for a near-black
/// tile therefore displays around `0.24` — four times too bright, which is
/// exactly how the first version of this deck came out. Converting here lets the
/// constants below stay the literal hex from the approved mockup.
fn srgb(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    fn lin(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    [lin(r), lin(g), lin(b), a]
}

/// `#0d0f14` — the tile body from the mockup.
fn tile_bg() -> [f32; 4] {
    srgb(0x0d, 0x0f, 0x14, 0.96)
}
/// `#08090d` — the identity strip along the tile's bottom edge.
fn scrim() -> [f32; 4] {
    srgb(0x08, 0x09, 0x0d, 0.94)
}
/// A white hairline rim at 10%.
const TILE_RIM: [f32; 4] = [1.0, 1.0, 1.0, 0.10];
/// Width of the staged tile's frame — the window's own `border_size`, not a
/// scaled-down version of it. The frame is a marking, not part of the
/// miniature, so it reads at the same weight in both places.
const STAGE_BORDER_W: f32 = 3.0;
const LABEL_W_PAD: f32 = 9.0;
/// Height of the identity strip along a tile's bottom edge, carrying the title.
/// Four `foot` windows are indistinguishable as pixels, so tiles are always
/// labelled.
const SCRIM_H: f32 = 34.0;
/// The title's size and line box. The title is the only thing telling several
/// windows of the same app apart, so it is read at a glance from across the
/// screen rather than merely present.
const TITLE_PX: f32 = 18.0;
const TITLE_LINE: f32 = 22.0;

/// One tile. Its slot `x` is fixed — tiles never trade places — so the only
/// thing that moves is `lift`: the current task's tile stands proud of the rest.
#[derive(Debug, Clone)]
pub struct Tile {
    pub addr: String,
    pub title: String,
    /// Owning process, for matching audio streams to this tile.
    pub pid: i64,
    /// Fixed slot position. Only recomputed when the deck's membership changes.
    x: f32,
    /// How far this tile currently stands above the row, easing toward
    /// `lift_to` (`RAISE` for the current task, `0` for everything else).
    lift: f32,
    lift_to: f32,
}

impl Tile {
    /// Whether this is the task on stage.
    ///
    /// Read from the *target* lift rather than the current one, so the answer
    /// flips the instant a switch begins instead of when the motion finishes —
    /// a click hands the cue over immediately, and the peach frame and the raise
    /// can then cross-fade at their own pace.
    fn is_current(&self) -> bool {
        self.lift_to > 0.0
    }
}

/// Deck view state. Empty and inert whenever stage mode is off.
///
/// There is no "am I animating" flag: [`Deck::tick`] returns whether anything
/// still moves, and the draw loop asks for the next frame on that — one source
/// of truth rather than a cached copy that can go stale.
#[derive(Default)]
pub struct Deck {
    pub tiles: Vec<Tile>,
    /// Tile under the pointer, for the hover lift.
    pub hover: Option<usize>,
}

/// Tile width for an output `screen_w` wide — a tile is a miniature of the
/// screen, so it carries the screen's aspect.
///
/// The row now holds *every* task, so it can outgrow the screen; past that
/// point the tiles shrink to fit rather than running off the edge.
pub fn tile_w(screen_w: f32, screen_h: f32, count: usize) -> f32 {
    let aspect = if screen_h > 0.0 {
        screen_w / screen_h
    } else {
        1.6
    };
    let natural = TILE_H * aspect;
    if count == 0 {
        return natural.round();
    }
    // Leave a margin so the row never touches the screen edges. The floor only
    // guards against a degenerate zero-width tile — a 60px floor here used to
    // quietly break the "tiles shrink rather than run off the edge" promise
    // past ~26 tasks, with slots computed off both sides of the screen.
    let budget = (screen_w - 2.0 * GAP - (count as f32 - 1.0) * GAP) / count as f32;
    natural.min(budget.max(8.0)).round()
}

/// Left edge of slot `n` in a centred row of `count` tiles. Fixed: a tile's
/// slot only changes when the deck's membership does, never on a switch.
pub fn slot_x(n: usize, count: usize, screen_w: f32, tw: f32) -> f32 {
    let row_w = count as f32 * (tw + GAP) - GAP;
    ((screen_w - row_w) / 2.0 + n as f32 * (tw + GAP)).round()
}

/// Top edge of a tile with the given lift. The row sits just off the bottom of
/// the band; the current task's tile stands [`RAISE`] above it.
pub fn tile_y(band_h: f32, lift: f32) -> f32 {
    band_h - BOTTOM_MARGIN - TILE_H - lift
}

/// Which tile a point falls in, if any. Accounts for the lift, so the raised
/// tile is clickable where it actually is.
pub fn hit(tiles: &[Tile], x: f32, y: f32, tw: f32, band_h: f32) -> Option<usize> {
    tiles.iter().position(|t| {
        if x < t.x || x > t.x + tw {
            return false;
        }
        let top = tile_y(band_h, t.lift);
        y >= top && y <= top + TILE_H
    })
}

impl Deck {
    /// Rebuild from a fresh task list, placing every tile at its fixed slot.
    /// `current` is the task on stage — its tile starts already raised, so
    /// opening the mode does not play an animation.
    pub fn reset(&mut self, tiles: Vec<Tile>, screen_w: f32, tw: f32, current: Option<&str>) {
        let count = tiles.len();
        self.tiles = tiles;
        for (n, tile) in self.tiles.iter_mut().enumerate() {
            tile.x = slot_x(n, count, screen_w, tw);
            let raised = current == Some(tile.addr.as_str());
            tile.lift = if raised { RAISE } else { 0.0 };
            tile.lift_to = tile.lift;
        }
        self.hover = None;
    }

    /// Raise the tile for `current` and lower everything else.
    ///
    /// This is the entire switch animation now. No tile changes place — the
    /// deck is a fixed row, and the only thing that moves is which one stands
    /// proud of it.
    pub fn set_current(&mut self, current: &str) {
        for tile in &mut self.tiles {
            tile.lift_to = if tile.addr == current { RAISE } else { 0.0 };
        }
    }

    /// Advance the lifts. Returns whether anything still moves.
    ///
    /// (There used to be a fade channel here for tiles arriving and leaving —
    /// it was dead code: `rebuild_deck` replaces the tile set wholesale on any
    /// membership change, so a fade could never actually play.)
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut busy = false;
        for tile in &mut self.tiles {
            if (tile.lift - tile.lift_to).abs() > 0.01 {
                let step = approach(dt, RAISE_DUR);
                tile.lift += (tile.lift_to - tile.lift) * step;
                if (tile.lift - tile.lift_to).abs() < 0.15 {
                    tile.lift = tile.lift_to;
                } else {
                    busy = true;
                }
            }
        }
        busy
    }
}

/// Fraction of the remaining distance to cover this frame for a move that
/// should take about `dur` seconds. Applied to what is *left* each tick, so the
/// motion is frame-rate independent — fast off the mark, soft on arrival, which
/// is the collapse curve's shape without needing to track progress per tile.
fn approach(dt: f32, dur: f32) -> f32 {
    (dt / dur * 2.2).clamp(0.0, 1.0)
}

impl App {
    /// Handle a `configure` for the deck strip: learn its size, build or resize
    /// its renderer, and draw. Mirrors the topbar's path.
    pub(crate) fn configure_deck(
        &mut self,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        self.deck_size = (width, height);
        // The deck spans the output, so its own width IS the screen width; the
        // height comes from the compositor so a tile can carry the screen's
        // aspect and read as a miniature of it.
        let screen_h = crate::hypr::focused_monitor()
            .map(|m| m.h as f32)
            .unwrap_or(width as f32 / 1.6);
        self.deck_screen = (width as f32, screen_h);
        let scale = self.config.options.render_scale.max(1);
        let (pw, ph) = (width * scale, height * scale);
        if let Some(renderer) = self.deck_renderer.as_mut() {
            renderer.resize(pw, ph);
        } else {
            let built = {
                let Some(layer) = self.deck_layer.as_ref() else {
                    return;
                };
                crate::renderer::Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
            };
            match built {
                Ok(mut renderer) => {
                    // Replay any thumbnails captured before this renderer
                    // existed. A failed first build returns early with the
                    // captures already filed, and without this they would
                    // reference texture layers the new renderer never received.
                    if self.deck_icon_capacity > 0 {
                        renderer.alloc_icon_array(self.deck_icon_capacity);
                        for (i, chain) in self.deck_thumb_chains.iter().enumerate() {
                            renderer.update_icon_layer(i as u32, chain);
                        }
                    }
                    self.deck_renderer = Some(renderer);
                }
                Err(e) => {
                    // Never fatal: the shell must survive a deck that cannot
                    // render (same rule as the dock — see `configure`).
                    tracing::error!("deck renderer init failed: {e:#}");
                    return;
                }
            }
        }
        self.sync_deck_input();
        self.draw_deck();
    }

    /// The deck takes pointer input only while it is up; the rest of the time
    /// its surface is click-through so the desktop underneath behaves normally.
    pub(crate) fn sync_deck_input(&mut self) {
        let Some(layer) = self.deck_layer.as_ref() else {
            return;
        };
        let (w, h) = self.deck_size;
        if self.stage.is_on() && !self.deck.tiles.is_empty() {
            crate::surface::set_input_rects(&self.compositor, layer, &[(0, 0, w as i32, h as i32)]);
        } else {
            crate::surface::set_input_rects(&self.compositor, layer, &[]);
        }
    }

    /// Rebuild the deck's tiles from the stage's ordering. Called when the mode
    /// opens, and whenever the task list changes underneath it.
    pub(crate) fn rebuild_deck(&mut self) {
        if !self.stage.is_on() {
            self.deck.tiles.clear();
            // The thumbnail cache is session-scoped: the mode re-photographs
            // the whole deck on open anyway, so pictures kept across sessions
            // could never be shown — holding them would only grow the chain
            // store (~350KB per window ever seen) for nothing.
            self.deck_thumb_layer.clear();
            self.deck_thumb_chains.clear();
            self.sync_deck_input();
            self.draw_deck();
            return;
        }
        // Re-derive the order from the live task list rather than trusting the
        // one recorded when the mode opened. A window opened or closed during
        // the session would otherwise leave the deck describing a desktop that
        // no longer exists — a tile for a window that is gone, which raises and
        // frames itself on click while the stage stays where it was, or no tile
        // at all for a window that has appeared.
        //
        // Re-deriving cannot reshuffle the row: `deck_order` sorts by workspace
        // and is stable, so the only slots that move are the ones a genuine
        // arrival or departure moves, which is the rule the deck promises.
        let tasks = crate::hypr::stage_tasks();
        self.stage.resync(&tasks);
        let order: Vec<String> = self.stage.deck().to_vec();
        let tiles: Vec<Tile> = order
            .iter()
            .filter_map(|addr| tasks.iter().find(|t| &t.address == addr))
            .map(|t| self.deck_tile(t))
            .collect();
        let (sw, sh) = self.deck_screen;
        let tw = tile_w(sw, sh, tiles.len());
        let current = self.stage.staged().map(str::to_owned);
        self.deck.reset(tiles, sw, tw, current.as_deref());
        self.sync_deck_input();
        // Size the icon array for this deck up front, so an arriving thumbnail
        // is only ever a single-layer write — never a reallocation of the whole
        // array mid-session. The headroom absorbs a few windows opening.
        let needed = self.deck.tiles.len() as u32;
        if needed > self.deck_icon_capacity {
            self.deck_icon_capacity = needed + 4;
            if let Some(r) = self.deck_renderer.as_mut() {
                r.alloc_icon_array(self.deck_icon_capacity);
                for (i, chain) in self.deck_thumb_chains.iter().enumerate() {
                    r.update_icon_layer(i as u32, chain);
                }
            }
        }
        // Photograph every tile that has no picture. The cache is cleared when
        // the mode closes, so at open this is the whole deck — the first of the
        // two moments a picture is taken (the other being a task leaving the
        // stage). Mid-session rebuilds — a window opened or closed — reach here
        // too, where "missing" is just the newcomer: the rest keep the picture
        // they have rather than being re-photographed for an identical image.
        let missing: Vec<String> = self
            .deck
            .tiles
            .iter()
            .filter(|t| !self.deck_thumb_layer.contains_key(&t.addr))
            .map(|t| t.addr.clone())
            .collect();
        self.deck_thumbs
            .request_many(missing, self.deck_tile_aspect());
        self.draw_deck();
    }

    /// Draw one deck frame, advancing the tile tweens and asking for another
    /// frame while anything still moves.
    pub(crate) fn draw_deck(&mut self) {
        let (w, h) = self.deck_size;
        if w == 0 || h == 0 {
            return;
        }
        let now = std::time::Instant::now();
        let dt = self
            .deck_last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        self.deck_last_frame = Some(now);
        let busy = self.deck.tick(dt);

        let scene = self.deck_scene();
        if let Some(renderer) = self.deck_renderer.as_mut() {
            // squircle 12 matches the system rounding_power, so a thumbnail quad
            // picks up the tile's corners; thumb_base MAX keeps that mask on
            // (layers at or above it would opt out of it).
            if let Err(e) = renderer.render(&scene, [1.0, 1.0, 1.0, 1.0], None, 12.0, u32::MAX) {
                tracing::warn!("deck render failed: {e:#}");
            }
        }
        if busy {
            self.schedule_deck_frame();
        } else {
            self.deck_last_frame = None;
        }
    }

    /// Ask the compositor for another deck frame (the tile tweens are dt-based,
    /// so motion is frame-rate independent).
    fn schedule_deck_frame(&mut self) {
        if self.deck_frame_pending {
            return;
        }
        if let Some(layer) = self.deck_layer.as_ref() {
            layer
                .wl_surface()
                .frame(&self.qh, layer.wl_surface().clone());
            layer.wl_surface().commit();
            self.deck_frame_pending = true;
        }
    }

    /// Pointer motion over the deck: track the hovered tile for the lift, and
    /// refresh a stale picture while you are looking at it.
    pub(crate) fn deck_motion(&mut self, x: f32, y: f32) {
        self.deck_ptr = Some((x, y));
        let (sw, sh) = self.deck_screen;
        let tw = tile_w(sw, sh, self.deck.tiles.len());
        let hit = hit(&self.deck.tiles, x, y, tw, self.deck_size.1 as f32);
        if hit != self.deck.hover {
            self.deck.hover = hit;
            self.draw_deck();
        }
    }

    /// A click on the deck: the speaker badge mutes; anywhere else on a tile
    /// puts that task on the stage.
    pub(crate) fn deck_click(&mut self, x: f32, y: f32) {
        let (sw, sh) = self.deck_screen;
        let tw = tile_w(sw, sh, self.deck.tiles.len());
        let Some(i) = hit(&self.deck.tiles, x, y, tw, self.deck_size.1 as f32) else {
            return;
        };
        self.stage_switch_to_index(i);
    }

    /// A fresh audio sample arrived: a tile sounds when its window pid appears
    /// in any sounding stream's process ancestry. Pure status — no
    /// attribution gymnastics, no click behaviour. (Windows sharing one
    /// process — Chrome webapps — light up together; that is the honest limit
    /// of what the pid can say, and an indicator can afford it where a button
    /// could not.)
    pub(crate) fn on_deck_audio(&mut self, streams: Vec<crate::deck_audio::Stream>) {
        let sounding: std::collections::HashSet<String> = self
            .deck
            .tiles
            .iter()
            .filter(|t| streams.iter().any(|s| s.ancestors.contains(&t.pid)))
            .map(|t| t.addr.clone())
            .collect();
        if sounding != self.deck_audio_map {
            self.deck_audio_map = sounding;
            self.draw_deck();
        }
    }

    /// Switch to the task in deck slot `i`. The single entry point for both a
    /// click and the `stage-show` verb — they used to take different paths, and
    /// the verb's snapped straight to the final layout, which is how a
    /// completely dead animation went unnoticed.
    ///
    /// Nothing is rebuilt and nothing changes place: the deck just raises a
    /// different tile.
    pub(crate) fn stage_switch_to_index(&mut self, i: usize) {
        let Some(addr) = self.deck.tiles.get(i).map(|t| t.addr.clone()) else {
            return;
        };
        let outgoing = self.stage.staged().map(str::to_owned);
        // The frame follows the stage, not the click. A tile whose window died
        // between the last deck rebuild and this click cannot be staged, and
        // raising it anyway would put the "this is the one you are looking at"
        // marking on a task that is not on screen — the deck lying about the
        // desktop, which is the one thing it must never do.
        if !self.stage.show(&addr) {
            self.rebuild_deck();
            return;
        }
        // Photograph the task that just LEFT — the single update a tile ever
        // gets. Its tile then shows what you were actually looking at when you
        // left it, which is the most a thumbnail can honestly promise: the task
        // stops painting the moment it is off screen, so this frame is also the
        // freshest one that will ever exist for it.
        //
        // The arriving task is deliberately not photographed. It used to be, 450ms
        // in, which caught some clients still relaying out after the resize.
        if let Some(prev) = outgoing.filter(|p| p != &addr) {
            self.deck_thumbs.request(prev, self.deck_tile_aspect());
        }
        self.deck.set_current(&addr);
        self.sync_deck_input();
        self.draw_deck();
    }

    /// A window mapped while the stage owns the screen.
    ///
    /// The ordinary open path fires a plain focus at a just-launched app
    /// ("show me what I launched") — which mid-stage yanked focus, kicked the
    /// staged window's fullscreen, and left the newcomer **tiled beside the
    /// stage** in its rect. Here the stage answers the same intent its own way:
    ///
    /// * a **tiled** newcomer takes the stage, through the full swap machinery
    ///   — evictions, parking, focus and maximize all handled;
    /// * a **floating** one is a dialog: it already renders above the staged
    ///   window, where it must be interactable, so it is left exactly where it
    ///   appeared and only gains its deck tile.
    pub(crate) fn on_window_opened_staged(&mut self, addr: &str) {
        // The stage satisfies the launch-focus intent; the 10s grace must not
        // fire a second, plain focus later.
        self.focus_launched = None;
        let floating = crate::hypr::window_states()
            .get(addr)
            .map(|s| s.floating)
            .unwrap_or(false);
        if floating {
            self.rebuild_deck();
        } else {
            self.stage_switch_to(addr);
        }
    }

    /// Switch to a task by address (the `stage-show` verb). Animated, exactly
    /// as a click is.
    pub(crate) fn stage_switch_to(&mut self, addr: &str) {
        if let Some(i) = self.deck.tiles.iter().position(|t| t.addr == addr) {
            self.stage_switch_to_index(i);
        } else {
            // Not on the deck (already staged, or gone): let the stage decide,
            // then resync the tiles rather than animating a swap that isn't one.
            // The result needs no checking — the rebuild reads whatever the
            // stage settled on either way.
            let _ = self.stage.show(addr);
            self.rebuild_deck();
        }
    }

    /// A captured thumbnail came back. Accept it for any task still on the deck.
    ///
    /// This used to insist the task was *still the one on stage*, because back
    /// when the picture was a screenshot of the stage rect, a swap during the
    /// settle would have filed the new window's pixels under the old window's
    /// name. The compositor photographs a **named window** now, so a capture
    /// cannot be about anything but the task it names, and that guard would
    /// throw away every result of a whole-deck fill.
    pub(crate) fn on_deck_thumb(&mut self, ev: crate::deck_thumbs::Event) {
        if !self.deck.tiles.iter().any(|t| t.addr == ev.addr) {
            tracing::debug!(
                "deck: dropping thumbnail for {}, no longer on the deck",
                ev.addr
            );
            return;
        }
        let layer = match self.deck_thumb_layer.get(&ev.addr).copied() {
            Some(layer) => layer,
            None => {
                let layer = self.deck_thumb_chains.len() as u32;
                // Overflow past the pre-sized array is possible only when more
                // windows than the headroom opened since the last rebuild —
                // grow once, replaying what the array already held.
                if layer >= self.deck_icon_capacity {
                    self.deck_icon_capacity = layer + 4;
                    if let Some(r) = self.deck_renderer.as_mut() {
                        r.alloc_icon_array(self.deck_icon_capacity);
                        for (i, chain) in self.deck_thumb_chains.iter().enumerate() {
                            r.update_icon_layer(i as u32, chain);
                        }
                    }
                }
                self.deck_thumb_chains.push(Vec::new());
                self.deck_thumb_layer.insert(ev.addr.clone(), layer);
                layer
            }
        };
        // Every path is a single-layer write; the CPU copy is kept only so a
        // renderer built later (configure after a failed init) can be replayed.
        if let Some(r) = self.deck_renderer.as_mut() {
            r.update_icon_layer(layer, &ev.pixels);
        }
        if let Some(slot) = self.deck_thumb_chains.get_mut(layer as usize) {
            *slot = ev.pixels;
        }
        self.draw_deck();
    }

    /// The shape a thumbnail will finally be drawn at — the tile's own aspect,
    /// which the compositor needs so its capture survives the atlas round trip
    /// undistorted. Read live rather than assumed constant, because `tile_w`
    /// narrows the tiles once the row stops fitting.
    pub(crate) fn deck_tile_aspect(&self) -> f32 {
        let (sw, sh) = self.deck_screen;
        let tw = tile_w(sw, sh, self.deck.tiles.len().max(1));
        (tw / TILE_H).max(0.1)
    }

    /// Build a tile for a task.
    pub(crate) fn deck_tile(&mut self, task: &crate::hypr::StageTask) -> Tile {
        Tile {
            addr: task.address.clone(),
            title: if task.title.is_empty() {
                task.class.clone()
            } else {
                task.title.clone()
            },
            pid: task.pid,
            x: 0.0,
            lift: 0.0,
            lift_to: 0.0,
        }
    }

    /// Compose the deck's scene: floating tiles, nothing behind them.
    pub(crate) fn deck_scene(&self) -> Scene {
        let (w, h) = self.deck_size;
        let (sw, sh) = self.deck_screen;
        let tw = tile_w(sw, sh, self.deck.tiles.len());
        // The staged tile's frame colour: whatever the staged WINDOW is
        // wearing right now. Both come from `App::border_stops`, so a tile's
        // frame is the same paint as the border around the window it stands
        // for and the two cannot drift (Max, 2026-09-11). It used to be a
        // fixed `#ffbe98`, matched by hand to what the desktop border used
        // to be. Read once for the whole deck.
        let stage_rim = self.border_tint();
        let mut scene = Scene {
            alpha: 1.0,
            ..Default::default()
        };
        if w == 0 {
            return scene;
        }

        for (i, tile) in self.deck.tiles.iter().enumerate() {
            // A small magnetic nudge under the pointer, per the design
            // language's hover displacement — but never on the staged tile. That
            // one is already out of line with the row, and lifting it further
            // would muddle the single thing its height is there to say.
            let hovered = self.deck.hover == Some(i);
            let hover = if hovered && !tile.is_current() {
                4.0
            } else {
                0.0
            };
            let y = tile_y(h as f32, tile.lift + hover);
            let body = Rect {
                x: tile.x,
                y,
                w: tw,
                h: TILE_H,
            };
            scene.rects.push(RectInst {
                rect: body,
                radius: RADIUS,
                color: {
                    let c = tile_bg();
                    [c[0], c[1], c[2], c[3]]
                },
                glass: 0.0,
                border: 0.0,
            });
            // The task's own picture, once its capture has come back (they are
            // requested for the whole deck at open). Stored square and
            // pre-squashed to the tile's aspect, so drawing it into the tile
            // rect stretches it back true.
            let thumb = self.deck_thumb_layer.get(&tile.addr).copied();
            if let Some(layer) = thumb {
                scene.icons.push(IconInst {
                    rect: body,
                    layer,
                    tint: [0.0, 0.0, 0.0, 0.0],
                    ring: -1.0,
                    plate: crate::content::NO_PLATE,
                });
            } else {
                // No picture yet: a plain strip so the title stays readable.
                // (With a thumbnail the equivalent gradient is baked into the
                // image — the scene draws every icon above every rect, so a
                // scrim rect here would end up underneath the photo.)
                scene.rects.push(RectInst {
                    rect: Rect {
                        x: tile.x,
                        y: y + TILE_H - SCRIM_H,
                        w: tw,
                        h: SCRIM_H,
                    },
                    radius: RADIUS,
                    color: {
                        let c = scrim();
                        [c[0], c[1], c[2], c[3]]
                    },
                    glass: 0.0,
                    border: 0.0,
                });
            }
            // v1 labels the tile with its title alone. App icons want the
            // dock's atlas indexing, which is a closure local to its scene
            // builder — threading it through a third surface is a bigger change
            // than the deck needs to be useful, and the titles are what
            // actually tell four `foot` windows apart.
            let text_x = tile.x + LABEL_W_PAD;
            scene.labels.push(Label {
                text: tile.title.clone(),
                // Centred in the scrim: the line box is TITLE_LINE tall in a
                // strip SCRIM_H tall, so half the difference sits above it.
                pos: (text_x, y + TILE_H - SCRIM_H + (SCRIM_H - TITLE_LINE) / 2.0),
                max_w: (tile.x + tw - LABEL_W_PAD - text_x).max(10.0),
                font_px: TITLE_PX,
                line_px: TITLE_LINE,
                centered: false,
                dim: false,
                cache: true,
                clip: None,
                family: None,
                color: None,
            });
            // Top-left speaker, only on a task making sound — white, the way a
            // browser tab marks an audible page. Pure status: no click
            // behaviour, and a silent task (paused, muted, whatever) carries
            // nothing. (The workspace number that used to sit here is gone —
            // Max dropped it 2026-09-06; the title is the tile's identity.)
            if self.deck_audio_map.contains(&tile.addr) {
                scene.labels.push(Label {
                    text: crate::options::GLYPH_VOL_LIVE.to_owned(),
                    // Hugging the corner a touch tighter vertically than
                    // horizontally — the glyph's own line box already carries
                    // air above the drawn speaker.
                    pos: (tile.x + BADGE_PAD, y + 4.0),
                    max_w: BADGE_W,
                    font_px: 22.0,
                    line_px: 24.0,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: None,
                    family: Some(crate::options::NERD),
                    color: Some([1.0, 1.0, 1.0, 0.95]),
                });
            }
            // A hairline rim, brighter under the pointer.
            scene.rects.push(RectInst {
                rect: body,
                radius: RADIUS,
                color: [
                    TILE_RIM[0],
                    TILE_RIM[1],
                    TILE_RIM[2],
                    // Keyed off the pointer, not the nudge: the staged tile no
                    // longer moves under the pointer but should still answer it.
                    TILE_RIM[3] * if hovered { 2.4 } else { 1.0 },
                ],
                glass: 0.0,
                border: 1.0,
            });
            // The stage frame. The task on screen wears a peach border, so its
            // tile wears the same one — that, not the small raise, is what says
            // "this is the one you're looking at".
            //
            // It rides *outside* the body, the way the compositor draws a
            // window's border outside its content, and for a second reason: the
            // renderer draws every icon above every rect, so a ring inside the
            // tile would disappear under the thumbnail. It fades in with the
            // raise rather than snapping, so a switch stays one movement.
            let raised = (tile.lift / RAISE).clamp(0.0, 1.0);
            if raised > 0.01 {
                let c = stage_rim;
                scene.rects.push(RectInst {
                    rect: Rect {
                        x: body.x - STAGE_BORDER_W,
                        y: body.y - STAGE_BORDER_W,
                        w: body.w + STAGE_BORDER_W * 2.0,
                        h: body.h + STAGE_BORDER_W * 2.0,
                    },
                    radius: RADIUS + STAGE_BORDER_W,
                    color: [c[0], c[1], c[2], c[3] * raised],
                    glass: 0.0,
                    border: STAGE_BORDER_W,
                });
            }
        }
        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(addr: &str) -> Tile {
        Tile {
            addr: addr.into(),
            title: addr.into(),
            pid: 0,
            x: 0.0,
            lift: 0.0,
            lift_to: 0.0,
        }
    }

    #[test]
    fn a_centred_row_puts_its_middle_on_the_screen_centre() {
        let (screen, tw, count) = (2000.0, 211.0, 6usize);
        let first = slot_x(0, count, screen, tw);
        let last = slot_x(count - 1, count, screen, tw);
        let mid = (first + last + tw) / 2.0;
        assert!((mid - screen / 2.0).abs() <= 1.0, "row centre was {mid}");
    }

    #[test]
    fn tiles_are_one_slot_apart() {
        let tw = 211.0;
        let a = slot_x(2, 6, 2000.0, tw);
        let b = slot_x(3, 6, 2000.0, tw);
        assert!((b - a - (tw + GAP)).abs() < 0.001);
    }

    #[test]
    fn tile_width_carries_the_screen_aspect() {
        // 2000x1250 is 1.6, so a tile is 1.6 * TILE_H wide — while the row fits.
        assert_eq!(tile_w(2000.0, 1250.0, 6), (TILE_H * 1.6).round());
    }

    #[test]
    fn tiles_shrink_rather_than_running_off_the_screen() {
        // The deck now holds EVERY task, so a big enough row must fit itself.
        let many = 20;
        let tw = tile_w(2000.0, 1250.0, many);
        let row = many as f32 * (tw + GAP) - GAP;
        assert!(row <= 2000.0, "row of {many} was {row}px wide");
        assert!(
            tw < tile_w(2000.0, 1250.0, 1),
            "should have shrunk below the natural width"
        );
    }

    #[test]
    fn the_row_sits_just_off_the_bottom_edge() {
        // An unraised tile's bottom edge sits BOTTOM_MARGIN off the band's
        // bottom — which is the screen's bottom, the band being bottom-anchored.
        let y = tile_y(200.0, 0.0);
        assert_eq!(y, 200.0 - BOTTOM_MARGIN - TILE_H);
        assert_eq!(200.0 - (y + TILE_H), BOTTOM_MARGIN);
        assert!(BOTTOM_MARGIN < 12.0, "the row should hug the bottom edge");
    }

    #[test]
    fn the_band_has_room_for_the_raised_tile_and_its_frame() {
        // The band is both the stage's bottom gap and the deck's surface, so
        // shrinking it walks the tiles up toward the staged window. Past this
        // point the raised tile's peach frame would touch it.
        let top = tile_y(crate::stage::BAND as f32, RAISE);
        assert!(
            top >= STAGE_BORDER_W,
            "band {} leaves the raised tile at {top}",
            crate::stage::BAND
        );
    }

    #[test]
    fn the_current_tile_sits_higher_than_the_rest() {
        assert!(tile_y(200.0, RAISE) < tile_y(200.0, 0.0));
        assert_eq!(tile_y(200.0, 0.0) - tile_y(200.0, RAISE), RAISE);
    }

    #[test]
    fn setting_current_raises_exactly_one_tile() {
        let mut deck = Deck::default();
        deck.reset(
            vec![tile("a"), tile("b"), tile("c")],
            2000.0,
            211.0,
            Some("a"),
        );
        deck.set_current("b");
        // Targets flip immediately; the motion is what eases.
        let targets: Vec<f32> = deck.tiles.iter().map(|t| t.lift_to).collect();
        assert_eq!(targets, vec![0.0, RAISE, 0.0]);
    }

    #[test]
    fn switching_never_moves_a_tile_sideways() {
        // The property Max asked for. Slots are assigned once at reset and the
        // raise must not disturb them.
        let mut deck = Deck::default();
        deck.reset(
            vec![tile("a"), tile("b"), tile("c")],
            2000.0,
            211.0,
            Some("a"),
        );
        let before: Vec<f32> = deck.tiles.iter().map(|t| t.x).collect();
        deck.set_current("c");
        for _ in 0..60 {
            deck.tick(1.0 / 60.0);
        }
        let after: Vec<f32> = deck.tiles.iter().map(|t| t.x).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn the_raise_settles() {
        let mut deck = Deck::default();
        deck.reset(vec![tile("a"), tile("b")], 2000.0, 211.0, Some("a"));
        deck.set_current("b");
        let mut ticks = 0;
        while deck.tick(1.0 / 60.0) && ticks < 600 {
            ticks += 1;
        }
        assert!(ticks < 600, "raise never settled");
        assert_eq!(deck.tiles[1].lift, RAISE);
        assert_eq!(deck.tiles[0].lift, 0.0);
    }

    #[test]
    fn hit_testing_follows_the_raised_tile() {
        let mut deck = Deck::default();
        deck.reset(vec![tile("a"), tile("b")], 2000.0, 211.0, Some("b"));
        let band = 200.0;
        let x = deck.tiles[1].x + 5.0;
        // The raised tile is clickable where it actually sits…
        let raised_top = tile_y(band, RAISE);
        assert_eq!(hit(&deck.tiles, x, raised_top + 5.0, 211.0, band), Some(1));
        // …and not above it.
        assert_eq!(hit(&deck.tiles, x, raised_top - 5.0, 211.0, band), None);
    }

    #[test]
    fn hit_testing_misses_below_the_row() {
        let mut deck = Deck::default();
        deck.reset(vec![tile("a")], 2000.0, 211.0, None);
        let band = 200.0;
        let x = deck.tiles[0].x + 5.0;
        assert_eq!(hit(&deck.tiles, x, band - 2.0, 211.0, band), None);
    }
}

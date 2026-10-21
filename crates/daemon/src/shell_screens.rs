//! THE SHELL ON THE RIGHT SCREEN (multi-monitor foundation, Max 2026-10-21:
//! "I open Golem Files on the TV, press Quick Look on a file and it showed on
//! the laptop").
//!
//! Every surface of the shell but the dock was made once, at start, on the
//! screen the compositor had focused then, and stayed there for good: the
//! bar, the desktop and its veil, the stage's deck, the desktop's menus (and
//! Quick Look, drawn there), the card. A layer surface cannot change
//! screens, so each one is MADE AGAIN where it must be, as the dock is
//! (`dock_screen.rs`): a new surface on that screen, and at its first
//! configure its renderer moves onto it (`Renderer::retarget`: pipelines,
//! pictures and labels kept) and the old one goes.
//!
//! Where each one belongs:
//! - the bar, the desktop, its veil and the deck: the MAIN screen (the
//!   built-in, else the main one: `launcher_prefs::main_screen`) — at start
//!   whatever had the focus, and again when the main screen changes;
//! - Quick Look, on the menus' surface: the screen of the Files window it is
//!   opened from, home again once it has closed;
//! - the card: its window's screen.
//!
//! A screen unplugged closes what was on it: each surface is made again on
//! the main screen, never the end of the shell (the bar's and the deck's
//! used to end it).

use std::collections::{HashMap, HashSet};

use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{info, warn};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;

use crate::fractional::{SurfaceKind, SurfaceScale};
use crate::renderer::Renderer;
use crate::visible::SurfaceVisible;
use crate::App;

/// A surface of the shell that can be made again on another screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Options,
    Deck,
    Desktop,
    DesktopTop,
    Card,
    Veil,
}

impl Part {
    const ALL: [Part; 6] = [Part::Options, Part::Deck, Part::Desktop, Part::DesktopTop, Part::Card, Part::Veil];

    fn name(self) -> &'static str {
        match self {
            Part::Options => "the bar",
            Part::Deck => "the deck",
            Part::Desktop => "the desktop",
            Part::DesktopTop => "the desktop's menus",
            Part::Card => "the card",
            Part::Veil => "the veil",
        }
    }

    fn kind(self) -> Option<SurfaceKind> {
        match self {
            Part::Options => Some(SurfaceKind::Options),
            Part::Deck => Some(SurfaceKind::Deck),
            Part::Desktop => Some(SurfaceKind::Desktop),
            Part::DesktopTop => Some(SurfaceKind::DesktopTop),
            Part::Card => Some(SurfaceKind::Card),
            Part::Veil => None,
        }
    }
}

/// What is run once a surface is up on its new screen.
pub(crate) type Then = Box<dyn FnOnce(&mut App)>;

/// The new surface itself: a layer with a renderer to move onto it, or the
/// veil (a stretched pixel, no renderer).
enum Made {
    Layer(LayerSurface),
    Veil(crate::veil::VeilLayer),
}

impl Made {
    fn surface(&self) -> &WlSurface {
        match self {
            Made::Layer(l) => l.wl_surface(),
            Made::Veil(v) => v.surface(),
        }
    }
}

/// A surface being made again on another screen, until it is up there.
struct Remake {
    part: Part,
    made: Made,
    fscale: Option<SurfaceScale>,
    visible: Option<SurfaceVisible>,
    output: Option<WlOutput>,
    then: Option<Then>,
    /// Nothing of it is drawn until it is up (the card: drawn where it was,
    /// it would show at the other screen's place on this one).
    hide: bool,
}

#[derive(Default)]
pub(crate) struct ShellScreens {
    remakes: Vec<Remake>,
    /// The screen each surface is on (its enter; set on a move).
    on: HashMap<Part, WlOutput>,
    /// Surfaces closed under us (their screen went): drawn on nothing until
    /// they are made again.
    lost: HashSet<Part>,
    /// Surfaces made on a screen we named (not left to the compositor).
    placed: HashSet<Part>,
}

/// Let a surface go: its scale and visible-region objects first, then the
/// surface (dropping the layer destroys it).
fn let_go(made: Made, fscale: Option<SurfaceScale>, visible: Option<SurfaceVisible>) {
    if let Some(f) = fscale {
        f.destroy();
    }
    if let Some(v) = visible {
        v.destroy();
    }
    match made {
        Made::Layer(l) => drop(l),
        Made::Veil(v) => v.destroy(),
    }
}

/// Put a made-again layer in place of the old one: the renderer moves onto
/// it (a new one is made by the configure when it cannot), the old surface
/// goes.
fn swap(
    conn: &wayland_client::Connection,
    part: Part,
    layer: &mut Option<LayerSurface>,
    renderer: &mut Option<Renderer>,
    fscale: &mut Option<SurfaceScale>,
    visible: &mut Option<SurfaceVisible>,
    new: (LayerSurface, Option<SurfaceScale>, Option<SurfaceVisible>),
) {
    if let Some(r) = renderer.as_mut() {
        if let Err(e) = r.retarget(conn, new.0.wl_surface()) {
            warn!("{}: cannot move its renderer ({e:#}); a new one", part.name());
            *renderer = None;
        }
    }
    let old_layer = layer.replace(new.0);
    let old_fscale = std::mem::replace(fscale, new.1);
    let old_visible = std::mem::replace(visible, new.2);
    if let Some(l) = old_layer {
        let_go(Made::Layer(l), old_fscale, old_visible);
    }
}

impl App {
    /// The surface `part` draws on now.
    fn part_surface(&self, part: Part) -> Option<&WlSurface> {
        match part {
            Part::Options => self.options_layer.as_ref().map(|l| l.wl_surface()),
            Part::Deck => self.deck_layer.as_ref().map(|l| l.wl_surface()),
            Part::Desktop => self.desktop_layer.as_ref().map(|l| l.wl_surface()),
            Part::DesktopTop => self.desktop_top.layer.as_ref().map(|l| l.wl_surface()),
            Part::Card => self.card_layer.as_ref().map(|l| l.wl_surface()),
            Part::Veil => self.veil_layer.as_ref().map(|v| v.surface()),
        }
    }

    fn part_of(&self, surface: &WlSurface) -> Option<Part> {
        Part::ALL.into_iter().find(|&p| self.part_surface(p) == Some(surface))
    }

    /// Nothing may be drawn on `part` now: its screen went, or it is being
    /// made again elsewhere and must not show here meanwhile.
    pub(crate) fn part_lost(&self, part: Part) -> bool {
        self.shell_screens.lost.contains(&part)
            || self.shell_screens.remakes.iter().any(|r| r.part == part && r.hide)
    }

    /// THE SCREENS MOVED (the Monitors page's arrangement): the compositor
    /// moves a screen when its rule says so but leaves the layer surfaces on
    /// it where they were (Hyprland 0.55: `hyprctl layers` still had them at
    /// the old corner, and the shell drew a screen's width off). A layer
    /// commit that names its margins has it lay out every layer of that
    /// screen again — ours and the wallpaper's — so every one of ours says
    /// its (unchanged, zero) margins once.
    pub(crate) fn nudge_layers(&self) {
        let mut layers: Vec<&LayerSurface> = [
            self.options_layer.as_ref(),
            self.deck_layer.as_ref(),
            self.desktop_layer.as_ref(),
            self.desktop_top.layer.as_ref(),
            self.card_layer.as_ref(),
            self.veil_layer.as_ref().map(|v| v.layer()),
        ]
        .into_iter()
        .flatten()
        .collect();
        layers.extend(self.dock_layers());
        for layer in layers {
            layer.set_margin(0, 0, 0, 0);
            layer.commit();
        }
    }

    /// The bar's screen, by its connector name.
    pub(crate) fn bar_screen_name(&self) -> Option<String> {
        let output = self.shell_output.as_ref().or_else(|| self.part_output(Part::Options))?;
        self.output_state.info(output).and_then(|i| i.name)
    }

    /// Whether window `addr` is on another screen than the bar's (two
    /// screens or more; one: never).
    pub(crate) fn on_another_screen(&self, addr: Option<&str>) -> bool {
        if self.output_state.outputs().nth(1).is_none() {
            return false;
        }
        match (addr.and_then(crate::hypr::window_monitor), self.bar_screen_name()) {
            (Some(its), Some(bar)) => its != bar,
            _ => false,
        }
    }

    /// The screen `part` is on, by its connector name.
    pub(crate) fn part_screen_name(&self, part: Part) -> Option<String> {
        self.output_state.info(self.part_output(part)?).and_then(|i| i.name)
    }

    /// The screen `part` is on, when known.
    pub(crate) fn part_output(&self, part: Part) -> Option<&WlOutput> {
        self.shell_screens.on.get(&part)
    }

    /// The fractional state of a surface being made again (dock or shell),
    /// for a scale it is told before it takes over.
    pub(crate) fn incoming_fscale(&mut self, proxy: &WpFractionalScaleV1) -> Option<&mut SurfaceScale> {
        if self.dock_next_fscale(proxy).is_some() {
            return self.dock_next_fscale(proxy);
        }
        self.shell_screens
            .remakes
            .iter_mut()
            .find_map(|r| r.fscale.as_mut().filter(|f| f.is(proxy)))
    }

    /// Make `part` again on `output` (`None`: wherever the compositor puts
    /// it); `then` runs once it is up there. A move of it already on is
    /// replaced (its `then` given way to this one's). False when it cannot
    /// be (no kit): it stays where it is and `then` runs now.
    pub(crate) fn remake_part(&mut self, part: Part, output: Option<WlOutput>, then: Option<Then>, hide: bool) -> bool {
        if let Some(i) = self.shell_screens.remakes.iter().position(|r| r.part == part) {
            let old = self.shell_screens.remakes.remove(i);
            let_go(old.made, old.fscale, old.visible);
        }
        let Some(kit) = self.screen_kit.as_ref() else {
            if let Some(then) = then {
                then(self);
            }
            return false;
        };
        let (compositor, qh, out) = (&self.compositor, &self.qh, output.as_ref());
        let made = match part {
            Part::Options => Made::Layer(crate::surface::create_top_surface(
                compositor,
                &kit.shell,
                qh,
                self.config.options.height,
                self.config.options.height + crate::OPTIONS_OVERHANG + crate::OPTIONS_DROPDOWN_H,
                self.config.options.render_scale.max(1),
                out,
            )),
            Part::Deck => Made::Layer(crate::surface::create_deck_surface(
                compositor,
                &kit.shell,
                qh,
                crate::stage::BAND as u32,
                self.config.options.render_scale.max(1),
                out,
            )),
            Part::Desktop => Made::Layer(crate::surface::create_desktop_surface(
                compositor,
                &kit.shell,
                qh,
                self.config.desktop.render_scale.max(1),
                out,
            )),
            Part::DesktopTop => Made::Layer(crate::surface::create_desktop_top_surface(
                compositor,
                &kit.shell,
                qh,
                self.config.desktop.render_scale.max(1),
                out,
            )),
            Part::Card => Made::Layer(crate::surface::create_card_surface(
                compositor,
                &kit.shell,
                qh,
                self.config.options.render_scale.max(1),
                out,
            )),
            Part::Veil => {
                let Some(viewporter) = kit.viewporter.as_ref() else {
                    return false;
                };
                Made::Veil(crate::veil::create(compositor, &kit.shell, qh, viewporter, out))
            }
        };
        let fscale = part
            .kind()
            .zip(kit.fractional.as_ref())
            .map(|(kind, f)| f.attach(made.surface(), kind, qh));
        let visible = (part != Part::Veil)
            .then(|| kit.visible.as_ref().map(|v| v.attach(made.surface(), qh)))
            .flatten();
        self.shell_screens.remakes.push(Remake { part, made, fscale, visible, output, then, hide });
        true
    }

    /// A configure for a surface being made again: it takes its part's
    /// place (the configure then goes on as that part's), and what it was
    /// made for is handed back to run after it.
    pub(crate) fn part_arrived(&mut self, surface: &WlSurface) -> Option<Then> {
        let i = self.shell_screens.remakes.iter().position(|r| r.made.surface() == surface)?;
        let Remake { part, made, fscale, visible, output, then, .. } = self.shell_screens.remakes.remove(i);
        let conn = self.conn.clone();
        match made {
            Made::Veil(v) => {
                if let Some(old) = self.veil_layer.replace(v) {
                    old.destroy();
                }
            }
            Made::Layer(layer) => {
                let new = (layer, fscale, visible);
                match part {
                    Part::Options => {
                        swap(&conn, part, &mut self.options_layer, &mut self.options_renderer, &mut self.options_fscale, &mut self.options_visible, new);
                        // (Its zone re-committed at the live scale; frames
                        // asked of the old surface never come.)
                        self.options_zone = None;
                        self.options_frame_pending = false;
                        self.options_frame_asked = None;
                    }
                    Part::Deck => {
                        swap(&conn, part, &mut self.deck_layer, &mut self.deck_renderer, &mut self.deck_fscale, &mut self.deck_visible, new);
                        self.deck_frame_pending = false;
                    }
                    Part::Desktop => {
                        swap(&conn, part, &mut self.desktop_layer, &mut self.desktop_renderer, &mut self.desktop_fscale, &mut self.desktop_visible, new);
                        self.desktop_frame_pending = false;
                    }
                    Part::DesktopTop => {
                        let top = &mut self.desktop_top;
                        swap(&conn, part, &mut top.layer, &mut top.renderer, &mut top.fscale, &mut top.visible, new);
                        top.frame_pending = false;
                        top.input_on = false;
                    }
                    Part::Card => {
                        swap(&conn, part, &mut self.card_layer, &mut self.card_renderer, &mut self.card_fscale, &mut self.card_visible, new);
                        self.card_frame_pending = false;
                        self.card_input = Vec::new();
                    }
                    Part::Veil => {}
                }
            }
        }
        self.shell_screens.lost.remove(&part);
        if let Some(output) = output {
            self.shell_screens.on.insert(part, output);
            self.shell_screens.placed.insert(part);
        }
        info!("{}: made again on its screen", part.name());
        then
    }

    /// A surface of the shell closed by the compositor (its screen went):
    /// made again on the main screen. True when it was one of them and it
    /// could be; false leaves it to the old ways (let go, or the end).
    pub(crate) fn part_closed(&mut self, surface: &WlSurface) -> bool {
        if let Some(i) = self.shell_screens.remakes.iter().position(|r| r.made.surface() == surface) {
            // The screen it was being made on went before it was up.
            let Remake { part, made, fscale, visible, then, .. } = self.shell_screens.remakes.remove(i);
            let_go(made, fscale, visible);
            if self.shell_screens.lost.contains(&part) {
                let home = self.main_output();
                return self.remake_part(part, home, then, false);
            }
            if let Some(then) = then {
                then(self);
            }
            return true;
        }
        let Some(part) = self.part_of(surface) else {
            return false;
        };
        warn!("{}: its screen went; made again on the main one", part.name());
        self.shell_screens.on.remove(&part);
        self.shell_screens.lost.insert(part);
        // What was up on it went with it.
        match part {
            Part::DesktopTop => self.peek.up = None,
            Part::Desktop => self.desktop.rename = None,
            _ => {}
        }
        let home = self.main_output();
        if self.remake_part(part, home, None, false) {
            return true;
        }
        self.shell_screens.lost.remove(&part);
        false
    }

    /// A surface of the shell entered a screen (its first map, or the
    /// compositor moved it): remember where it is.
    pub(crate) fn part_entered(&mut self, surface: &WlSurface, output: &WlOutput) {
        let Some(part) = self.part_of(surface) else {
            return;
        };
        if self.shell_screens.on.get(&part) == Some(output) {
            return;
        }
        self.shell_screens.on.insert(part, output.clone());
        self.sync_parts_home();
    }

    /// Keep the bar, the desktop, its veil and the deck on the main screen,
    /// and the desktop's menus there too while nothing of them is up
    /// elsewhere (Quick Look). Run when the screens change and when a
    /// surface enters one.
    pub(crate) fn sync_parts_home(&mut self) {
        if self.output_state.outputs().nth(1).is_none() {
            return;
        }
        let Some(home) = self.main_output() else {
            return;
        };
        for part in Part::ALL {
            let at_home = match part {
                Part::Options | Part::Deck | Part::Desktop | Part::Veil => true,
                Part::DesktopTop => !self.peek.is_up() && self.desktop.menu.is_none() && self.desktop.props.is_none(),
                // (The card goes where its window is: `card_follow_screen`.)
                Part::Card => false,
            };
            if !at_home
                || self.part_surface(part).is_none()
                || self.shell_screens.remakes.iter().any(|r| r.part == part)
            {
                continue;
            }
            // Where it is not known yet — nothing shown on it so far (the
            // veil while the light needs none) — and it was left to the
            // compositor: made where it belongs, to be sure.
            let Some(on) = self.shell_screens.on.get(&part) else {
                if part == Part::Veil && !self.shell_screens.placed.contains(&part) {
                    self.remake_part(part, Some(home.clone()), None, false);
                }
                continue;
            };
            if *on != home {
                info!("{}: not on the main screen; made again there", part.name());
                self.remake_part(part, Some(home.clone()), None, false);
            }
        }
    }

    /// QUICK LOOK comes out on the screen it is opened from: a Files
    /// window's (it has the keyboard: the focused screen), else the
    /// desktop's. When the menus' surface is on another, it is made again
    /// there and Quick Look opens once it is (true: the caller leaves it).
    pub(crate) fn peek_follow(&mut self, win: u32, thing: &crate::quicklook::Thing) -> bool {
        if self.output_state.outputs().nth(1).is_none() {
            return false;
        }
        let target = if win == crate::quicklook::DESKTOP {
            self.part_output(Part::Desktop).cloned()
        } else {
            crate::hypr::focused_monitor().ok().and_then(|m| self.output_named(&m.name))
        };
        let Some(target) = target else {
            return false;
        };
        if self.part_output(Part::DesktopTop) == Some(&target) {
            return false;
        }
        let thing = thing.clone();
        self.remake_part(
            Part::DesktopTop,
            Some(target),
            Some(Box::new(move |app: &mut App| app.peek_show(win, thing))),
            false,
        )
    }

    /// THE CARD rides its window onto that window's screen: when its surface
    /// is on another, it is made again there (shown nowhere meanwhile) and
    /// laid out once it is (true: the caller leaves it).
    pub(crate) fn card_follow_screen(&mut self, monitor: Option<&str>) -> bool {
        if self.output_state.outputs().nth(1).is_none() {
            return false;
        }
        let Some(target) = monitor.and_then(|m| self.output_named(m)) else {
            return false;
        };
        // (Not known yet — nothing drawn on it so far: made where it is
        // wanted, to be sure.)
        if self.part_output(Part::Card) == Some(&target) {
            return false;
        }
        if self
            .shell_screens
            .remakes
            .iter()
            .any(|r| r.part == Part::Card && r.output.as_ref() == Some(&target))
        {
            return true;
        }
        self.remake_part(Part::Card, Some(target), Some(Box::new(|app: &mut App| app.sync_card())), true)
    }
}

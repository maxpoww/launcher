//! THE POWER PILLS: what the power key's short press brings to the banner —
//! Max, 2026-11-03: *"el botón corto haga aparecer tres opciones, suspender,
//! hibernar y reiniciar, y el botón largo apague (los botones son OPTIONS:
//! cuando aprieto el botón corto una píldora redonda, un solo botón aparece a
//! la derecha del engranaje en el banner, ese botón es para hibernar, en hover
//! revela dos botones a su derecha -> sleep -> reboot)"*.
//!
//! The key is the compositor's: Golem's hyprland.lua sends `waverunner-ctl
//! power` when it is let go, and logind leaves the short press alone and powers
//! the machine off on a long one. Where the kernel's tiny power button driver
//! owns the key instead, Golem sends the same verb from kbrequest.target
//! (desktop/idle.nix).
//!
//! AT REST: one round pill right of the gear, HIBERNATE (the machine off,
//! everything kept). What stands right of the gear moves over for it.
//!
//! THE POINTER ON IT: sleep and restart come out from behind it to its right,
//! as the minimize comes out of the close; what stands right of them steps
//! aside with them.
//!
//! A CLICK on hibernate or sleep does it at once: the key was the question,
//! and the apps stay open through either. The pills are gone before the
//! machine goes, so they do not greet the owner on the way back.
//!
//! RESTART closes the apps first, the way their own X does, so each can ask
//! to keep what is not saved (Max, 2026-11-03, after reading Hyprland's
//! hyprshutdown: "hacelo como tenga sentido, y el aviso tendría que estar en
//! una píldora, como mensaje"). The round pill grows into a MESSAGE — "Closing
//! apps…", then "Waiting for Seam" — with "Restart anyway" and "Cancel" as the
//! task pill's chips; once no window is left the machine restarts. Unlike
//! hyprshutdown the session is never left (Golem has no log-out) and the
//! banner stays to say what is happening. A dialog an app opens to ask is a
//! window too: the restart waits for the owner to answer it. The app named is
//! brought forward once (macOS's way: its question may be on another
//! workspace, or under other windows), and a click on the words does it again
//! — an app whose question was already open when the restart came took the
//! second close as "never mind" (the Text Editor on the test VM, 2026-11-03),
//! and the owner closes it by its own X.
//!
//! THEY GO with a second press of the key (a restart waiting: cancelled), when
//! the hand leaves after coming, or by themselves when it never comes
//! (`UNVISITED`).
//!
//! A machine that cannot hibernate (no disk swap): the round pill is sleep and
//! only restart comes out.
//!
//! THE ROUND PILL BREATHES RED while it stands there (Max, 2026-11-03: "agregá
//! un breathing rojo al botón"): its ground swells toward the palette's red
//! (Golem's where the palette's does not read red) and back, on the dictating card pill's breath (`BREATH_PERIOD`), deeper.
//! The words of a restart's message do not breathe. Reduced motion: a steady
//! half-way red.

use std::future::Future;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use tracing::{info, warn};
use zbus::Connection;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::i18n::tr;
use crate::options::{
    hover_grow, push_neumorph, PillId, BOND_GAP, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP, TEXT_FONT,
};
use crate::App;

/// fa-power-off: off, everything kept (hibernate).
const GLYPH_HIBERNATE: &str = "\u{f011}";
/// fa-moon: sleep (the gear page's own).
const GLYPH_SLEEP: &str = "\u{f186}";
/// fa-refresh: restart (the gear page's own).
const GLYPH_RESTART: &str = "\u{f021}";

/// How long the pills wait for a hand that never comes.
const UNVISITED: Duration = Duration::from_secs(8);
/// Asks closer than this are one press told twice (a machine whose key two
/// kernel drivers report: the compositor's bind and Golem's kbrequest path).
const ONE_PRESS: Duration = Duration::from_millis(400);
/// What logind is given to say whether this machine can hibernate.
const PATIENCE: Duration = Duration::from_secs(4);
/// How often a restart looks at what is still open.
const POLL: Duration = Duration::from_millis(300);
/// How long "Closing apps…" stands before an app still open is named (most
/// close in less; naming each on its way out would flicker).
const NAME_AFTER: Duration = Duration::from_secs(1);
/// A restart told to the machine that has not happened by then failed (an
/// inhibitor, a refusal): the pills go and the desktop is the owner's again.
const GIVE_UP: Duration = Duration::from_secs(20);
/// Between the message's words and its chips, and between the chips: the
/// task pill's.
const WORDS_GAP: f32 = 10.0;
const CHIP_GAP: f32 = 8.0;

/// The round pill's breath: one in this many seconds (the card pill's while
/// it listens), this far toward the red, the red this solid at the top.
const BREATH_PERIOD: f32 = 1.8;
const BREATH_DEPTH: f32 = 0.8;
const BREATH_RED: f32 = 0.85;
/// Golem's own red (its palette's, the shell's red before palettes, #e05252):
/// the breath's where the palette's is no red to the eye (Iris's dark red is a
/// pastel pink, #fba0ad).
const GOLEM_RED: [f32; 4] = [0.878, 0.322, 0.322, 1.0];

const CLOSING: &str = "Closing apps…";
const RESTARTING: &str = "Restarting…";
const ANYWAY: &str = "Restart anyway";
const CANCEL: &str = "Cancel";

/// A restart under way.
#[derive(Debug)]
struct Closing {
    since: Instant,
    /// The message: closing, waiting for an app, restarting.
    says: String,
    /// The machine has been told: nothing is left to choose.
    going: bool,
    /// The window of the app named, and the apps already brought forward.
    named: Option<String>,
    shown: Vec<String>,
    /// Counts restarts, so the poll of a cancelled one stops.
    run: u32,
}

/// The pills' state.
#[derive(Debug, Default)]
pub(crate) struct PowerPills {
    /// Asked onto the banner by the key.
    on: bool,
    /// 0 (gone) → 1 (there).
    t: f32,
    /// The hand on them: the two behind come out.
    open: bool,
    /// 0 (behind the round pill) → 1 (out).
    out: f32,
    /// The hand has come since the key: leaving now sends them away.
    visited: bool,
    /// Gone in a moment unless the hand comes back.
    leaving: bool,
    /// Counts the presses, so a wait armed for one is not acted on for another.
    asked: u32,
    /// When the last press came.
    asked_at: Option<Instant>,
    /// When the round pill came: its breath counts from there.
    breath_from: Option<Instant>,
    /// What logind said last (`None`: not asked yet — taken as yes).
    can_hibernate: Option<bool>,
    /// The banner was hidden (a fullscreen) and the key called it.
    called_bar: bool,
    /// A restart waiting for the apps to close.
    closing: Option<Closing>,
    runs: u32,
    /// 0 (the round pill) → 1 (the message out of it).
    say: f32,
    /// The message's words and the chips' words, measured as they last read.
    says_w: f32,
    anyway_w: f32,
    cancel_w: f32,
    measured: String,
    last: Option<Instant>,
    frame_pending: bool,
}

/// Whether logind says this machine can hibernate now (swap big enough, a
/// place to resume from, no lockdown). `false` when it does not answer.
fn can_hibernate() -> bool {
    answer(async {
        let conn = Connection::system().await?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "CanHibernate",
                &(),
            )
            .await?;
        let said: String = reply.body().deserialize()?;
        Ok(said == "yes")
    })
    .unwrap_or(false)
}

fn answer<T>(question: impl Future<Output = zbus::Result<T>>) -> Option<T> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    runtime.block_on(async { tokio::time::timeout(PATIENCE, question).await.ok()?.ok() })
}

/// `systemctl <verb>`, on a thread of its own (reaped; the loop never waits).
fn machine(verb: &'static str) {
    info!("power: {verb}");
    let spawned = std::thread::Builder::new().name("power".into()).spawn(move || {
        match std::process::Command::new("systemctl").arg(verb).output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => warn!("power: systemctl {verb}: {}", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => warn!("power: cannot run systemctl {verb}: {e}"),
        }
    });
    if let Err(e) = spawned {
        warn!("power: no thread for {verb}: {e}");
    }
}

/// The breath's red: the palette's own when it reads red (the menus' test,
/// `desktop_menu::reads_red`), else Golem's — LINEAR, as a fill is drawn: an
/// sRGB red given as it is came out salmon on the screen.
fn breath_red() -> [f32; 4] {
    let linear = |c: [f32; 4]| -> [f32; 4] {
        std::array::from_fn(|i| if i == 3 { 1.0 } else { crate::options::srgb_to_linear(c[i]) })
    };
    let own = linear(crate::palette::danger_fill_srgb());
    if crate::desktop_menu::reads_red(own) {
        own
    } else {
        linear(GOLEM_RED)
    }
}

/// What a restart waiting on `apps` (each named once, the first the one most
/// in the way) says.
fn waiting_words(apps: &[String]) -> String {
    match apps {
        [] => tr(CLOSING).to_owned(),
        [one] => crate::i18n::fill(tr("Waiting for {app}"), &[("app", one)]),
        [one, rest @ ..] => crate::i18n::fill(
            tr("Waiting for {app} and {n} more"),
            &[("app", one), ("n", &rest.len().to_string())],
        ),
    }
}

impl App {
    /// The power key's short press: the pills come, or go if they are there
    /// (a restart waiting is cancelled).
    pub(crate) fn power_pills_ask(&mut self) {
        if self.power.asked_at.is_some_and(|t| t.elapsed() < ONE_PRESS) {
            return;
        }
        self.power.asked_at = Some(Instant::now());
        if self.power.closing.as_ref().is_some_and(|c| !c.going) {
            self.power_restart_cancel();
            return;
        }
        if self.power.on {
            self.power_pills_away();
            return;
        }
        let p = &mut self.power;
        p.on = true;
        p.open = false;
        p.visited = false;
        p.leaving = false;
        p.asked = p.asked.wrapping_add(1);
        p.breath_from = Some(Instant::now());
        self.power_call_bar();
        // (Asked each time: swap can come and go.)
        self.desktop_off_loop(can_hibernate, |app, can| {
            if app.power.can_hibernate != Some(can) {
                app.power.can_hibernate = Some(can);
                app.options_update_hover();
                app.draw_options();
            }
        });
        let asked = self.power.asked;
        let _ = self.loop_handle.insert_source(Timer::from_duration(UNVISITED), move |_, _, app: &mut App| {
            if app.power.on && app.power.asked == asked && !app.power.visited {
                app.power_pills_away();
            }
            TimeoutAction::Drop
        });
        self.schedule_power_frame();
    }

    /// Over a fullscreen the banner is away: the key calls it, and it goes
    /// back as it came when the pills do.
    fn power_call_bar(&mut self) {
        if self.options_hidden {
            self.power.called_bar = true;
            self.set_options_hidden(false);
        }
    }

    /// The pills go (eased). Not while a restart waits: only its Cancel, a
    /// press of the key or the restart itself end that.
    fn power_pills_away(&mut self) {
        if !self.power.on || self.power.closing.is_some() {
            return;
        }
        let p = &mut self.power;
        p.on = false;
        p.open = false;
        p.visited = false;
        p.leaving = false;
        self.schedule_power_frame();
        if std::mem::take(&mut self.power.called_bar) && self.options_fullscreen && self.options_ptr.is_none() {
            self.schedule_options_hide();
        }
    }

    /// Hibernate there, or sleep where it cannot.
    fn power_hibernates(&self) -> bool {
        self.power.can_hibernate != Some(false)
    }

    /// The pills behind the round one, in the order they come out.
    fn power_behind(&self) -> &'static [PillId] {
        if self.power_hibernates() {
            &[PillId::PowerSleep, PillId::PowerRestart]
        } else {
            &[PillId::PowerRestart]
        }
    }

    /// Where the round pill stands: where what stands right of the gear
    /// started before it came.
    fn power_x(&self) -> f32 {
        EDGE_PAD + self.options_pill_h() + OPTION_GAP
    }

    /// How far toward the red the round pill is now: 0 → `BREATH_DEPTH` and
    /// back, a cosine (no edge to see); held half-way under reduced motion.
    fn power_breath(&self) -> f32 {
        if animation::reduce_motion() {
            return BREATH_DEPTH / 2.0;
        }
        let along = self.power.breath_from.map_or(0.0, |t| t.elapsed().as_secs_f32()) / BREATH_PERIOD;
        BREATH_DEPTH * (0.5 - 0.5 * (along * std::f32::consts::TAU).cos())
    }

    /// A chip's height (the task pill's: one line of the bar's text).
    fn power_chip_h(&self) -> f32 {
        LINE_PX * self.options_scale()
    }

    /// The message pill's whole width: the round pill's room for the symbol,
    /// the words, and the chips while there is a choice.
    fn power_say_w(&self) -> f32 {
        let ph = self.options_pill_h();
        let edge = (ph - self.power_chip_h()) / 2.0;
        let p = &self.power;
        let chips = if p.closing.as_ref().is_some_and(|c| !c.going) {
            WORDS_GAP + p.anyway_w + CHIP_GAP + p.cancel_w
        } else {
            0.0
        };
        ph + p.says_w + chips + edge.max(crate::options::PILL_PAD_X / 2.0)
    }

    /// How much of the banner right of the gear the pills take now (what
    /// stands there moves over by it).
    pub(crate) fn power_room(&self) -> f32 {
        let ph = self.options_pill_h();
        let p = &self.power;
        p.t.clamp(0.0, 1.0) * (ph + OPTION_GAP)
            + self.power_behind().len() as f32 * p.out.clamp(0.0, 1.0) * (ph + BOND_GAP)
            + p.say.clamp(0.0, 1.0) * (self.power_say_w() - ph).max(0.0)
    }

    /// The pills on the banner now: the round one first (as long as its
    /// message makes it), then the ones behind it, each as far out as they are.
    pub(crate) fn power_pill_rects(&self, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        if self.power.t <= 0.01 {
            return Vec::new();
        }
        let x = self.power_x();
        let w = ph + self.power.say.clamp(0.0, 1.0) * (self.power_say_w() - ph).max(0.0);
        let mut rects = vec![(PillId::Power, Rect::new(x, y, w, ph))];
        for (k, id) in self.power_behind().iter().enumerate() {
            let to = x + (k as f32 + 1.0) * (ph + BOND_GAP);
            rects.push((*id, Rect::new(lerp(x, to, self.power.out), y, ph, ph)));
        }
        rects
    }

    /// Whether a pill behind is out far enough to be pointed at.
    pub(crate) fn power_pill_reachable(&self, id: PillId) -> bool {
        id == PillId::Power || self.power.out > 0.5
    }

    /// Whether the pointer is on the pills — the gaps between them included,
    /// so crossing from one to the next is not leaving.
    fn power_ptr_on(&self) -> bool {
        let Some((x, y)) = self.options_ptr else {
            return false;
        };
        if !self.power.on || self.stats_covering() || y < 0.0 || y > self.options_bar_h() {
            return false;
        }
        let ph = self.options_pill_h();
        let rects = self.power_pill_rects(0.0, ph);
        let (Some(first), Some(last)) = (rects.first(), rects.iter().map(|(_, r)| r.x + r.w).reduce(f32::max)) else {
            return false;
        };
        x >= first.1.x && x <= last
    }

    /// The pointer moved on the banner (or left it): the pills open under it,
    /// and go a moment after it leaves them, once it has come. A restart's
    /// message stays whatever the hand does (its chips light under it).
    pub(crate) fn update_power_hover(&mut self) -> bool {
        if !self.power.on {
            return false;
        }
        let on = self.power_ptr_on();
        if self.power.closing.is_some() {
            return on;
        }
        if on {
            self.power.visited = true;
            self.power.leaving = false;
            if !self.power.open {
                self.power.open = true;
                self.schedule_power_frame();
            }
        } else {
            if self.power.open {
                self.power.open = false;
                self.schedule_power_frame();
            }
            if self.power.visited && !self.power.leaving {
                self.power.leaving = true;
                let asked = self.power.asked;
                let _ = self.loop_handle.insert_source(
                    Timer::from_duration(animation::LEAVE_HOLD),
                    move |_, _, app: &mut App| {
                        if app.power.leaving && app.power.asked == asked {
                            app.power_pills_away();
                        }
                        TimeoutAction::Drop
                    },
                );
            }
        }
        on
    }

    /// A click on one of the pills: hibernate and sleep are done at once, the
    /// pills gone first; restart closes the apps first. On a restart's
    /// message, its chips.
    pub(crate) fn power_pill_click(&mut self, id: PillId) {
        if self.power.closing.is_some() {
            if id == PillId::Power {
                self.power_chip_click();
            }
            return;
        }
        let verb = match id {
            PillId::Power if self.power_hibernates() => "hibernate",
            PillId::Power | PillId::PowerSleep => "suspend",
            PillId::PowerRestart => return self.power_restart(),
            _ => return,
        };
        let p = &mut self.power;
        p.on = false;
        p.open = false;
        p.visited = false;
        p.leaving = false;
        p.t = 0.0;
        p.out = 0.0;
        p.called_bar = false;
        self.options_hover = None;
        self.sync_options_input();
        self.draw_options();
        machine(verb);
    }

    /// Restart: every window is asked to close as its X would, and the round
    /// pill becomes the message while they go.
    fn power_restart(&mut self) {
        let Some(windows) = crate::hypr::open_windows() else {
            // (The compositor did not answer: nothing can be asked to close.)
            warn!("power: no window list, restarting as asked");
            return self.power_restart_now();
        };
        info!("power: restart, closing {} windows", windows.len());
        for (addr, _) in &windows {
            crate::hypr::close_window(addr);
        }
        self.power.runs = self.power.runs.wrapping_add(1);
        let run = self.power.runs;
        let p = &mut self.power;
        p.on = true;
        p.open = false;
        p.visited = true;
        p.leaving = false;
        p.closing = Some(Closing {
            since: Instant::now(),
            says: tr(CLOSING).to_owned(),
            going: false,
            named: None,
            shown: Vec::new(),
            run,
        });
        self.power_call_bar();
        if windows.is_empty() {
            return self.power_restart_now();
        }
        self.power_say_measure();
        self.power_poll_soon(run);
    }

    fn power_poll_soon(&mut self, run: u32) {
        let _ = self.loop_handle.insert_source(Timer::from_duration(POLL), move |_, _, app: &mut App| {
            app.power_poll(run);
            TimeoutAction::Drop
        });
    }

    /// One look at what is still open: nothing → the restart; else the
    /// message names who it is waiting for.
    fn power_poll(&mut self, run: u32) {
        let Some(c) = self.power.closing.as_ref().filter(|c| c.run == run && !c.going) else {
            return;
        };
        let since = c.since;
        let Some(windows) = crate::hypr::open_windows() else {
            return self.power_poll_soon(run);
        };
        if windows.is_empty() {
            return self.power_restart_now();
        }
        if since.elapsed() >= NAME_AFTER {
            let mut apps: Vec<String> = Vec::new();
            for (_, class) in &windows {
                let name = self.power_app_name(class);
                if !apps.contains(&name) {
                    apps.push(name);
                }
            }
            let says = waiting_words(&apps);
            let first = windows.first().map(|(addr, _)| addr.clone());
            let mut bring = None;
            if let Some(c) = self.power.closing.as_mut() {
                c.named.clone_from(&first);
                // The app named comes forward, once: its question is what the
                // owner has to see.
                if let (Some(app), Some(addr)) = (apps.first(), &first) {
                    if !c.shown.contains(app) {
                        c.shown.push(app.clone());
                        bring = Some(addr.clone());
                    }
                }
                if c.says != says {
                    c.says = says;
                    self.power_say_measure();
                }
            }
            if let Some(addr) = bring {
                crate::hypr::focus_window(&addr);
            }
        }
        self.power_poll_soon(run);
    }

    /// The app a window belongs to, by the name the dock knows it by.
    fn power_app_name(&self, class: &str) -> String {
        let key = class.to_lowercase();
        self.entries
            .iter()
            .zip(&self.kinds)
            .find(|(entry, kind)| Self::app_match_keys(entry, **kind).contains(&key))
            .map_or_else(|| crate::task_title::groom("", Some(class), None), |(entry, _)| entry.name.clone())
    }

    /// Now: whatever is still open is ended by the restart itself.
    fn power_restart_now(&mut self) {
        let run = self.power.runs;
        if let Some(c) = self.power.closing.as_mut() {
            c.going = true;
            c.says = tr(RESTARTING).to_owned();
        }
        self.power_say_measure();
        machine("reboot");
        let _ = self.loop_handle.insert_source(Timer::from_duration(GIVE_UP), move |_, _, app: &mut App| {
            if app.power.closing.as_ref().is_some_and(|c| c.run == run && c.going) {
                warn!("power: the restart did not happen");
                app.power.closing = None;
                app.power_pills_away();
            }
            TimeoutAction::Drop
        });
    }

    /// The restart is off: what was closed stays closed, the rest is as it was.
    fn power_restart_cancel(&mut self) {
        if self.power.closing.take().is_some() {
            info!("power: restart cancelled");
        }
        self.power_pills_away();
    }

    /// The message's chips, as laid out in the pill `rect`.
    fn power_chips(&self, rect: Rect) -> (Rect, Rect) {
        let ph = self.options_pill_h();
        let chip_h = self.power_chip_h();
        let edge = (ph - chip_h) / 2.0;
        let y = rect.y + (rect.h - chip_h) / 2.0;
        let (aw, cw) = (self.power.anyway_w, self.power.cancel_w);
        let cancel = Rect::new(rect.x + rect.w - edge - cw, y, cw, chip_h);
        let anyway = Rect::new(cancel.x - CHIP_GAP - aw, y, aw, chip_h);
        (anyway, cancel)
    }

    /// A click on the message: its chips.
    fn power_chip_click(&mut self) {
        if self.power.closing.as_ref().is_none_or(|c| c.going) {
            return;
        }
        let (Some(p), Some((_, rect))) = (self.options_ptr, self.power_pill_rects(0.0, self.options_pill_h()).first().copied()) else {
            return;
        };
        let rect = Rect::new(rect.x, rect.y, self.power_say_w(), rect.h);
        // (Across only: the pointer is on the pill, so on the bar.)
        let on = |chip: Rect| p.0 >= chip.x && p.0 <= chip.x + chip.w;
        let (anyway, cancel) = self.power_chips(rect);
        if on(anyway) {
            info!("power: restart anyway");
            self.power_restart_now();
        } else if on(cancel) {
            self.power_restart_cancel();
        } else if let Some(addr) = self.power.closing.as_ref().and_then(|c| c.named.clone()) {
            crate::hypr::focus_window(&addr);
        }
    }

    /// Measure the message and its chips as they read now.
    fn power_say_measure(&mut self) {
        let says = self.power.closing.as_ref().map(|c| c.says.clone()).unwrap_or_default();
        if says != self.power.measured {
            let font_px = FONT_PX * self.options_scale();
            if let Some(r) = self.options_renderer.as_mut() {
                self.power.says_w = r.measure_text(&says, font_px, TEXT_FONT);
                self.power.anyway_w = r.measure_text(tr(ANYWAY), font_px, TEXT_FONT) + 12.0;
                self.power.cancel_w = r.measure_text(tr(CANCEL), font_px, TEXT_FONT) + 12.0;
                self.power.measured = says;
            }
        }
        self.schedule_power_frame();
        self.draw_options();
    }

    fn schedule_power_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.power.frame_pending, &mut a.power.last), App::tick_power);
    }

    /// One frame of their coming, opening, growing into a message and going.
    fn tick_power(&mut self) {
        let now = Instant::now();
        let dt = self.power.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.power.last = Some(now);
        let ph = self.options_pill_h();
        let span = self.power_behind().len() as f32 * (ph + BOND_GAP);
        let say_span = (self.power_say_w() - ph).max(1.0);
        let p = &mut self.power;
        let (t, a) = ease_toward(p.t, f32::from(u8::from(p.on)), dt, animation::MORPH_RATE, animation::settle_t(ph + OPTION_GAP));
        let out_to = f32::from(u8::from(p.on && p.open && p.closing.is_none()));
        let (out, b) = ease_toward(p.out, out_to, dt, animation::MORPH_RATE, animation::settle_t(span.max(1.0)));
        let say_to = f32::from(u8::from(p.closing.is_some()));
        let (say, c) = ease_toward(p.say, say_to, dt, animation::MORPH_RATE, animation::settle_t(say_span));
        (p.t, p.out, p.say) = (t, out, say);
        // (Breathing is motion too: frames for as long as the round pill stands.)
        let breathing = p.on && p.closing.is_none() && !animation::reduce_motion();
        self.sync_options_input();
        self.draw_options();
        if a || b || c || breathing {
            self.schedule_power_frame();
        } else {
            self.power.last = None;
        }
    }

    /// `waverunner-ctl power [open|shut|away|say <app>|cancel|state]`: alone,
    /// the key's press; the rest show the pills without a pointer (never a
    /// click: it is real). `say` stands a restart's message up as if it waited
    /// for that app, closing and restarting nothing; `cancel` takes it down.
    pub(crate) fn power_command(&mut self, what: &str) -> String {
        let (verb, arg) = what.trim().split_once(' ').unwrap_or((what.trim(), ""));
        match verb {
            "" => self.power_pills_ask(),
            "open" => {
                if !self.power.on {
                    self.power_pills_ask();
                }
                self.power.visited = true;
                self.power.open = true;
                self.schedule_power_frame();
            }
            "shut" => {
                self.power.open = false;
                self.schedule_power_frame();
            }
            "away" => self.power_pills_away(),
            "say" => {
                self.power.runs = self.power.runs.wrapping_add(1);
                let says = if arg.is_empty() { tr(CLOSING).to_owned() } else { waiting_words(&[arg.to_owned()]) };
                let p = &mut self.power;
                p.on = true;
                p.open = false;
                p.visited = true;
                // (Its run is never polled: nothing is waited for.)
                p.closing = Some(Closing {
                    since: Instant::now(),
                    says,
                    going: false,
                    named: None,
                    shown: Vec::new(),
                    run: p.runs,
                });
                self.power_call_bar();
                self.power_say_measure();
            }
            "cancel" => self.power_restart_cancel(),
            _ => {}
        }
        let p = &self.power;
        format!(
            "on={} t={:.2} open={} out={:.2} say={:.2} hibernate={:?} closing={:?}",
            p.on,
            p.t,
            p.open,
            p.out,
            p.say,
            p.can_hibernate,
            p.closing.as_ref().map(|c| (&c.says, c.going))
        )
    }

    /// Draw one of the pills: every pill's ground, lifted under the pointer,
    /// its symbol on it; the round one, grown, its message and chips.
    pub(crate) fn push_power_pill(&self, scene: &mut Scene, rect: Rect, id: PillId) {
        let p = &self.power;
        let shown = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let a = if id == PillId::Power {
            shown
        } else {
            shown * ((p.out - 0.15) / 0.6).clamp(0.0, 1.0)
        };
        // (Sleep and restart are gone while the round pill is a message.)
        if a <= 0.004 || (id != PillId::Power && p.closing.is_some()) {
            return;
        }
        let saying = id == PillId::Power && p.say > 0.01;
        let glyph = match id {
            _ if saying => GLYPH_RESTART,
            PillId::Power if self.power_hibernates() => GLYPH_HIBERNATE,
            PillId::Power | PillId::PowerSleep => GLYPH_SLEEP,
            _ => GLYPH_RESTART,
        };
        let bright = self.options_bar_is_bright();
        // (A message is not a button: no lift; its chips light instead.)
        let hovered = self.options_hover == Some(id) && !saying;
        let r = if hovered { hover_grow(rect) } else { rect };
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        let mut wash = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        if id == PillId::Power && !saying {
            let red = breath_red();
            wash = lerp4(wash, [red[0], red[1], red[2], BREATH_RED], self.power_breath());
        }
        wash[3] *= a;
        scene.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: wash, glass: 0.0, border: 0.0 });
        // The symbols are drawn in one late pass, over every pill: one coming
        // out keeps its symbol off the pill it comes from behind.
        let ph = rect.h;
        let rects = self.power_pill_rects(rect.y, ph);
        let before = rects
            .iter()
            .position(|(i, _)| *i == id)
            .and_then(|k| k.checked_sub(1))
            .map(|k| rects[k].1.x + rects[k].1.w);
        let clip = match before {
            Some(edge) if edge > r.x => Rect::new(edge, r.y, (r.x + r.w - edge).max(0.0), r.h),
            _ => r,
        };
        let ink = self.options_text_color();
        let s = self.options_scale();
        // The symbol keeps the round pill's place as the message grows.
        let at = if saying { Rect::new(r.x, r.y, ph, r.h) } else { r };
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (at.x + at.w / 2.0, at.y + (at.h - LINE_PX * s) / 2.0),
            max_w: at.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(clip),
        });
        if saying {
            self.push_power_message(scene, r, ink, a);
        }
    }

    /// A restart's message in the grown pill: the words after the symbol, the
    /// chips at its end (the task pill's: the accent's word, its tint under
    /// the pointer).
    fn push_power_message(&self, scene: &mut Scene, r: Rect, ink: [f32; 4], a: f32) {
        let Some(c) = self.power.closing.as_ref() else {
            return;
        };
        let s = self.options_scale();
        let words_a = a * ((self.power.say - 0.4) / 0.5).clamp(0.0, 1.0);
        if words_a <= 0.004 {
            return;
        }
        // Laid out at the message's whole width and uncovered as the pill
        // grows (squeezed into the growing pill, the chips ran over the
        // symbol on the way out).
        let ph = self.options_pill_h();
        let whole = Rect::new(r.x, r.y, self.power_say_w(), r.h);
        let seen = |q: Rect| {
            let right = (q.x + q.w).min(r.x + r.w);
            Rect::new(q.x, q.y, (right - q.x).max(0.0), q.h)
        };
        let (anyway, cancel) = self.power_chips(whole);
        let end = if c.going { whole.x + whole.w } else { anyway.x - WORDS_GAP };
        let words = seen(Rect::new(r.x + ph, r.y, (end - r.x - ph).max(0.0), r.h));
        scene.labels.push(Label {
            text: c.says.clone(),
            pos: (words.x, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: 4096.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: false,
            dim: false,
            cache: true,
            family: TEXT_FONT,
            color: Some([ink[0], ink[1], ink[2], ink[3] * words_a]),
            clip: Some(words),
        });
        if c.going {
            return;
        }
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Power));
        let amber = crate::notif::amber();
        for (chip, text) in [(anyway, tr(ANYWAY)), (cancel, tr(CANCEL))] {
            let lit = ptr.is_some_and(|q| q.0 >= chip.x && q.0 <= chip.x + chip.w);
            let shows = seen(chip);
            if shows.w <= 0.5 {
                continue;
            }
            if lit {
                scene.rects.push(RectInst {
                    rect: shows,
                    radius: chip.h / 2.0,
                    color: [amber[0], amber[1], amber[2], 0.3 * words_a],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let tone = if lit { ink } else { amber };
            scene.labels.push(Label {
                text: text.to_owned(),
                pos: (chip.x + chip.w / 2.0, chip.y + (chip.h - LINE_PX * s) / 2.0),
                max_w: chip.w + 4.0,
                font_px: FONT_PX * s,
                line_px: LINE_PX * s,
                centered: true,
                dim: false,
                cache: true,
                family: TEXT_FONT,
                color: Some([tone[0], tone[1], tone[2], words_a]),
                clip: Some(shows),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_names_who_it_waits_for() {
        assert_eq!(waiting_words(&[]), CLOSING);
        assert_eq!(waiting_words(&["Seam".into()]), "Waiting for Seam");
        assert_eq!(waiting_words(&["Seam".into(), "Foot".into(), "GIMP".into()]), "Waiting for Seam and 2 more");
    }
}

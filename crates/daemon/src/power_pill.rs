//! THE POWER PILLS: what the power key's short press brings to the banner —
//! Max, 2026-11-03: *"el botón corto haga aparecer tres opciones, suspender,
//! hibernar y reiniciar, y el botón largo apague (los botones son OPTIONS:
//! cuando aprieto el botón corto una píldora redonda, un solo botón aparece a
//! la derecha del engranaje en el banner, ese botón es para hibernar, en hover
//! revela dos botones a su derecha -> sleep -> reboot)"*.
//!
//! The key is the compositor's: Golem's hyprland.lua sends `waverunner-ctl
//! power` when it is let go, and logind leaves the short press alone and powers
//! the machine off on a long one.
//!
//! AT REST: one round pill right of the gear, HIBERNATE (the machine off,
//! everything kept). What stands right of the gear moves over for it.
//!
//! THE POINTER ON IT: sleep and restart come out from behind it to its right,
//! as the minimize comes out of the close; what stands right of them steps
//! aside with them.
//!
//! A CLICK does it at once: the key was the question. The pills are gone
//! before the machine goes, so they do not greet the owner on the way back.
//!
//! THEY GO with a second press of the key, when the hand leaves after coming,
//! or by themselves when it never comes (`UNVISITED`).
//!
//! A machine that cannot hibernate (no disk swap): the round pill is sleep and
//! only restart comes out.

use std::future::Future;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use tracing::{info, warn};
use zbus::Connection;

use crate::animation::{self, ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, BOND_GAP, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP};
use crate::App;

/// fa-power-off: off, everything kept (hibernate).
const GLYPH_HIBERNATE: &str = "\u{f011}";
/// fa-moon: sleep (the gear page's own).
const GLYPH_SLEEP: &str = "\u{f186}";
/// fa-refresh: restart (the gear page's own).
const GLYPH_RESTART: &str = "\u{f021}";

/// How long the pills wait for a hand that never comes.
const UNVISITED: Duration = Duration::from_secs(8);
/// What logind is given to say whether this machine can hibernate.
const PATIENCE: Duration = Duration::from_secs(4);

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
    /// What logind said last (`None`: not asked yet — taken as yes).
    can_hibernate: Option<bool>,
    /// The banner was hidden (a fullscreen) and the key called it.
    called_bar: bool,
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

impl App {
    /// The power key's short press: the pills come, or go if they are there.
    pub(crate) fn power_pills_ask(&mut self) {
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
        // Over a fullscreen the banner is away: the key calls it, and it goes
        // back as it came when the pills do.
        if self.options_hidden {
            self.power.called_bar = true;
            self.set_options_hidden(false);
        }
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

    /// The pills go (eased).
    fn power_pills_away(&mut self) {
        if !self.power.on {
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

    /// How much of the banner right of the gear the pills take now (what
    /// stands there moves over by it).
    pub(crate) fn power_room(&self) -> f32 {
        let ph = self.options_pill_h();
        let p = &self.power;
        p.t.clamp(0.0, 1.0) * (ph + OPTION_GAP)
            + self.power_behind().len() as f32 * p.out.clamp(0.0, 1.0) * (ph + BOND_GAP)
    }

    /// The pills on the banner now: the round one first, then the ones behind
    /// it, each as far out as they are.
    pub(crate) fn power_pill_rects(&self, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        if self.power.t <= 0.01 {
            return Vec::new();
        }
        let x = self.power_x();
        let mut rects = vec![(PillId::Power, Rect::new(x, y, ph, ph))];
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
    /// and go a moment after it leaves them, once it has come.
    pub(crate) fn update_power_hover(&mut self) -> bool {
        if !self.power.on {
            return false;
        }
        let on = self.power_ptr_on();
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

    /// A click on one of the pills: it is done, the pills gone first.
    pub(crate) fn power_pill_click(&mut self, id: PillId) {
        let verb = match id {
            PillId::Power if self.power_hibernates() => "hibernate",
            PillId::Power | PillId::PowerSleep => "suspend",
            PillId::PowerRestart => "reboot",
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

    fn schedule_power_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.power.frame_pending, &mut a.power.last), App::tick_power);
    }

    /// One frame of their coming, opening and going.
    fn tick_power(&mut self) {
        let now = Instant::now();
        let dt = self.power.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.power.last = Some(now);
        let ph = self.options_pill_h();
        let span = self.power_behind().len() as f32 * (ph + BOND_GAP);
        let p = &mut self.power;
        let (t, a) = ease_toward(p.t, f32::from(u8::from(p.on)), dt, animation::MORPH_RATE, animation::settle_t(ph + OPTION_GAP));
        let out_to = f32::from(u8::from(p.on && p.open));
        let (out, b) = ease_toward(p.out, out_to, dt, animation::MORPH_RATE, animation::settle_t(span.max(1.0)));
        (p.t, p.out) = (t, out);
        self.sync_options_input();
        self.draw_options();
        if a || b {
            self.schedule_power_frame();
        } else {
            self.power.last = None;
        }
    }

    /// `waverunner-ctl power [open|shut|away|state]`: alone, the key's press;
    /// the rest show the pills without a pointer (never a click: it is real).
    pub(crate) fn power_command(&mut self, what: &str) -> String {
        match what.trim() {
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
            _ => {}
        }
        let p = &self.power;
        format!(
            "on={} t={:.2} open={} out={:.2} hibernate={:?}",
            p.on, p.t, p.open, p.out, p.can_hibernate
        )
    }

    /// Draw one of the pills: every pill's ground, lifted under the pointer,
    /// its symbol on it.
    pub(crate) fn push_power_pill(&self, scene: &mut Scene, rect: Rect, id: PillId) {
        let p = &self.power;
        let shown = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let a = if id == PillId::Power {
            shown
        } else {
            shown * ((p.out - 0.15) / 0.6).clamp(0.0, 1.0)
        };
        if a <= 0.004 {
            return;
        }
        let glyph = match id {
            PillId::Power if self.power_hibernates() => GLYPH_HIBERNATE,
            PillId::Power | PillId::PowerSleep => GLYPH_SLEEP,
            _ => GLYPH_RESTART,
        };
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(id);
        let r = if hovered { hover_grow(rect) } else { rect };
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        let mut wash = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
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
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (r.x + r.w / 2.0, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: r.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(clip),
        });
    }
}

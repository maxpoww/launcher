//! THE LONE APP'S ACTIONS (Max, 2026-10-22: "cuando el resultado sea una
//! sola app quiero que se muestren más opciones — ventana de incógnito,
//! nueva ventana, cada app trae sus acciones — … y desinstalar, con
//! confirmación"). A search that leaves ONE app in Apps puts that app's own
//! desktop actions (its desktop file's `Actions=`: Chrome's New Incognito
//! Window, Seam's New Private Window…) beside it as pills in the System
//! row's look, the app and its pills centred together under the title; and,
//! for an app the dock can take away (a package it installed, an installed
//! webapp — never the system's own), "Uninstall <app>" last, a little apart.
//!
//! The app stays the one selected: Enter opens it as ever. → or Tab walk
//! onto the pills (← or Shift+Tab back to the app), Enter runs the one the
//! keys are on; a click runs it too. An action starts the way its app does
//! (`launch::launch`: its graphics card, its terminal) and the box goes.
//! UNINSTALL ASKS ONCE: the first press turns it red, "Uninstall <app>?
//! Again to confirm" (just "Again to uninstall" where that is too long), the
//! app's other pills stepping aside meanwhile; a second press uninstalls
//! (`uninstall_app`, the drag-to-Install road). Escape, the keys or the
//! pointer leaving it, or `ARM_FOR` with nothing done put it back.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{error, info, warn};

use crate::i18n::{fill, tr};
use crate::{content, launch, options, panel, App};

/// How long Uninstall waits for its second press.
pub(crate) const ARM_FOR: Duration = Duration::from_secs(4);
/// Uninstall stands this many of the pills' gaps from the one before it.
const APART: f32 = 2.5;

/// What a pill does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Does {
    /// One of the app's desktop actions: its command.
    Run(String),
    /// Take the app away; `asked` is what it says while asking to be sure,
    /// and `short` where that does not fit — each with its words' width.
    Uninstall { asked: (String, f32), short: (String, f32) },
}

/// One pill beside the lone app.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pill {
    pub label: String,
    /// Its words' shaped width at the pill font.
    pub w: f32,
    pub does: Does,
}

impl Pill {
    /// Its words and their width as drawn now (`armed`: Uninstall asking —
    /// its whole sentence where the pill, `pad` round its words, has `room`,
    /// else the short one).
    pub(crate) fn shown(&self, armed: bool, room: f32, pad: f32) -> (&str, f32) {
        match &self.does {
            Does::Uninstall { asked, short } if armed => {
                let (text, w) = if asked.1 + pad <= room { asked } else { short };
                (text, *w)
            }
            _ => (&self.label, self.w),
        }
    }

    pub(crate) fn uninstalls(&self) -> bool {
        matches!(self.does, Does::Uninstall { .. })
    }
}

/// The lone app's pills and where the keys and the asking stand.
#[derive(Debug, Default)]
pub(crate) struct Actions {
    /// The app they are of (its desktop id); `None` with no pills.
    pub app: Option<String>,
    pub pills: Vec<Pill>,
    /// Each pill's eased hover (the System row's `control_lift`).
    pub lift: Vec<f32>,
    /// The pill the keys are on; `None` = the app itself.
    pub key: Option<usize>,
    /// Since when Uninstall has been asking to be sure.
    pub armed: Option<Instant>,
}

impl Actions {
    /// The Uninstall pill's index, when there is one.
    pub(crate) fn uninstall(&self) -> Option<usize> {
        self.pills.iter().position(Pill::uninstalls)
    }

    /// Whether pill `i` is Uninstall asking to be sure.
    pub(crate) fn asking(&self, i: usize) -> bool {
        self.armed.is_some() && self.uninstall() == Some(i)
    }
}

/// The gap before a pill: Uninstall's (`apart`) is wider when a pill
/// stands before it (`first`: none does).
fn gap_before(apart: bool, first: bool, gap: f32) -> f32 {
    if apart && !first {
        APART * gap
    } else {
        gap
    }
}

/// Which pills of `widths` (Uninstall's flagged in `apart`) a `room` this
/// wide holds, beside their app: the app's own actions give way from the
/// last, Uninstall last of all. Returns the kept indices and their span
/// (each with the gap before it).
pub(crate) fn fitting(widths: &[f32], apart: &[bool], gap: f32, room: f32) -> (Vec<usize>, f32) {
    let span = |kept: &[usize]| -> f32 {
        kept.iter()
            .enumerate()
            .map(|(k, &i)| widths[i] + gap_before(apart[i], k == 0, gap))
            .sum()
    };
    let mut kept: Vec<usize> = (0..widths.len()).collect();
    while !kept.is_empty() && span(&kept) > room {
        let k = kept.iter().rposition(|&i| !apart[i]).unwrap_or(kept.len() - 1);
        kept.remove(k);
    }
    let total = span(&kept);
    (kept, total)
}

impl App {
    /// The one app a search leaves, while the box searches (not the resting
    /// grid, not the control panel).
    fn lone_app(&self) -> Option<usize> {
        if self.control_panel || self.search.query.is_empty() || self.grid_resting() {
            return None;
        }
        match self.search.visible[content::SECTION_APPS].as_slice() {
            &[only] => Some(only),
            _ => None,
        }
    }

    /// Whether the dock can uninstall `id`: a package it installed, or an
    /// installed webapp — as `uninstall_app` does them; nothing busy.
    pub(crate) fn can_uninstall(&self, id: &str) -> bool {
        !self.busy_ids.contains(id)
            && ((self.removable_ids.contains(id) && self.managed.attr_for_app(id).is_some())
                || self.installed_webapp_slug(id).is_some())
    }

    /// Make the lone app's pills again (every `refilter`). The same app keeps
    /// where the keys were and its asking; another app starts afresh.
    pub(crate) fn sync_app_actions(&mut self) {
        let Some(entry) = self.lone_app().and_then(|i| self.entries.get(i)) else {
            self.search.actions = Actions::default();
            return;
        };
        let (id, name) = (entry.id.clone(), entry.name.clone());
        let mut made: Vec<(String, Does)> = entry
            .path
            .as_deref()
            .map(waverunner_core::index::desktop_actions)
            .unwrap_or_default()
            .into_iter()
            .map(|a| (a.name, Does::Run(a.exec)))
            .collect();
        if self.can_uninstall(&id) {
            let asked = (fill(tr("Uninstall {app}? Again to confirm"), &[("app", &name)]), 0.0);
            let short = (tr("Again to uninstall").to_string(), 0.0);
            made.push((fill(tr("Uninstall {app}"), &[("app", &name)]), Does::Uninstall { asked, short }));
        }
        // The System row's type: the control panel's near layer.
        let font = options::FONT_PX * self.options_scale() * panel::LAYER_SCALE[0];
        let mut measure = |text: &str| match self.renderer.as_mut() {
            Some(r) => r.measure_text(text, font, options::TEXT_FONT),
            None => options::est_text_w(text, font),
        };
        let pills: Vec<Pill> = made
            .into_iter()
            .map(|(label, does)| {
                let does = match does {
                    Does::Uninstall { asked: (a, _), short: (b, _) } => {
                        let (wa, wb) = (measure(&a), measure(&b));
                        Does::Uninstall { asked: (a, wa), short: (b, wb) }
                    }
                    run => run,
                };
                Pill { w: measure(&label), label, does }
            })
            .collect();
        let old = std::mem::take(&mut self.search.actions);
        let same = old.app.as_deref() == Some(id.as_str()) && old.pills.len() == pills.len();
        self.search.actions = Actions {
            app: (!pills.is_empty()).then_some(id),
            lift: if same { old.lift } else { vec![0.0; pills.len()] },
            key: old.key.filter(|_| same),
            armed: old.armed.filter(|_| same),
            pills,
        };
    }

    /// Place the lone app's pills (`layout_stretched`): the app steps left
    /// by half of what its pills take, so the two stand centred together
    /// under the title, and the pills sit beside its icon, centred on it.
    /// While Uninstall asks it stands there alone. Not beside a side dock
    /// (its Apps are one column).
    pub(crate) fn place_app_actions(&self, layout: &mut content::Layout, gap: f32) {
        let a = &self.search.actions;
        let sec = &mut layout.sections[content::SECTION_APPS];
        if a.pills.is_empty() || sec.cells != 1 || sec.vertical || !sec.centre_rows {
            return;
        }
        let scale = self.icon_scale();
        let h = self.controls_pill_h();
        let pad = 2.0 * options::PILL_PAD_X * self.options_scale() * panel::LAYER_SCALE[0];
        let cell_w = sec.viewport.w / sec.cols.max(1) as f32;
        // What the app leaves beside it in the grid's width.
        let room = sec.viewport.w - cell_w;
        let width = |i: usize| (a.pills[i].shown(a.asking(i), room - gap, pad).1 + pad).max(h);
        let widths: Vec<f32> = (0..a.pills.len()).map(width).collect();
        let apart: Vec<bool> = a.pills.iter().map(Pill::uninstalls).collect();
        let (kept, span) = match a.uninstall().filter(|_| a.armed.is_some()) {
            Some(u) => (vec![u], gap + widths[u].min(room - gap)),
            None => fitting(&widths, &apart, gap, room),
        };
        if kept.is_empty() {
            return;
        }
        sec.lead = span / 2.0;
        let mut x = sec.viewport.x + sec.row_shift(0) + cell_w;
        let y = sec.viewport.y + (content::GRID_ICON_TOP + content::GRID_ICON / 2.0) * scale - h / 2.0;
        let mut rects = vec![content::Rect::new(0.0, 0.0, 0.0, 0.0); a.pills.len()];
        for (k, &i) in kept.iter().enumerate() {
            x += gap_before(apart[i], k == 0, gap);
            // (Never past the grid's edge: a sentence too long is cut there.)
            let w = widths[i].min(sec.viewport.x + sec.viewport.w - x);
            rects[i] = content::Rect::new(x, y, w, h);
            x += w;
        }
        layout.app_actions = rects;
    }

    /// Whether the keys can step onto the lone app's pills from where they
    /// are: on the app itself, with pills placed beside it.
    pub(crate) fn actions_reachable(&self) -> bool {
        self.search.selected == Some(0)
            && self.lone_app().is_some()
            && self.current_layout().app_actions.iter().any(|r| r.w > 0.0)
    }

    /// The keys step `dir` (+1 right / Tab, -1 left / Shift+Tab) along the
    /// app and its placed pills; `wrap` goes round (Tab). Returns whether
    /// the step was theirs.
    pub(crate) fn actions_step(&mut self, dir: i32, wrap: bool) -> bool {
        if !self.actions_reachable() {
            return false;
        }
        let placed: Vec<usize> = self
            .current_layout()
            .app_actions
            .iter()
            .enumerate()
            .filter(|(_, r)| r.w > 0.0)
            .map(|(i, _)| i)
            .collect();
        // 0 = the app, k = the k-th placed pill.
        let n = placed.len() as i32 + 1;
        let at = self.search.actions.key.and_then(|k| placed.iter().position(|&p| p == k)).map_or(0, |p| p as i32 + 1);
        let to = if wrap { (at + dir).rem_euclid(n) } else { (at + dir).clamp(0, n - 1) };
        if to == at && !wrap {
            // ← on the app is the grid's own (its selection stays put).
            return to != 0 || dir > 0;
        }
        self.search.actions.key = (to > 0).then(|| placed[to as usize - 1]);
        self.actions_settle();
        self.schedule_frame();
        true
    }

    /// The dock box's keys, first, while the lone app has pills: → / ← and
    /// Tab / Shift+Tab walk the app and its pills, Enter runs the pill the
    /// keys are on, Escape puts an asking Uninstall back (the next Escape
    /// is the box's), ↑ / ↓ leave the pills for the grid's walk. Returns
    /// whether the key was theirs.
    pub(crate) fn app_actions_key(&mut self, keysym: Keysym) -> bool {
        if self.search.actions.pills.is_empty() {
            return false;
        }
        match keysym {
            Keysym::Escape if self.search.actions.armed.is_some() => {
                self.actions_disarm();
                true
            }
            Keysym::Right => self.actions_step(1, false),
            Keysym::Left => self.actions_step(-1, false),
            Keysym::Tab => self.actions_step(if self.modifiers.shift { -1 } else { 1 }, true),
            Keysym::ISO_Left_Tab => self.actions_step(-1, true),
            Keysym::Return | Keysym::KP_Enter => match self.search.actions.key {
                Some(i) if self.actions_reachable() => {
                    self.run_app_action(i);
                    true
                }
                _ => false,
            },
            Keysym::Up | Keysym::Down => {
                self.search.actions.key = None;
                self.actions_settle();
                false
            }
            _ => false,
        }
    }

    /// Put Uninstall back once neither the keys nor the pointer are on it.
    pub(crate) fn actions_settle(&mut self) {
        let a = &self.search.actions;
        let Some(u) = a.uninstall().filter(|_| a.armed.is_some()) else {
            return;
        };
        if a.key != Some(u) && self.hover != Some(content::Hit::AppAction(u)) {
            self.actions_disarm();
        }
    }

    /// Uninstall stops asking.
    pub(crate) fn actions_disarm(&mut self) {
        if self.search.actions.armed.take().is_some() {
            self.schedule_frame();
        }
    }

    /// Run pill `i`: an action starts and the box goes; Uninstall asks, and
    /// uninstalls when asked again.
    pub(crate) fn run_app_action(&mut self, i: usize) {
        let Some(pill) = self.search.actions.pills.get(i).cloned() else {
            return;
        };
        let Some(index) = self.lone_app() else {
            return;
        };
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let (id, needs_terminal) = (entry.id.clone(), entry.needs_terminal);
        match pill.does {
            Does::Run(exec) => {
                info!("app action {:?} of {id}", pill.label);
                let exec = self.gpu_exec(&id, exec);
                if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
                    error!("app action failed for {id}: {e:#}");
                }
                self.launched(index);
            }
            Does::Uninstall { .. } if self.search.actions.armed.is_none() => {
                self.search.actions.armed = Some(Instant::now());
                let timer = Timer::from_duration(ARM_FOR);
                if let Err(e) = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                    if app.search.actions.armed.is_some_and(|t| t.elapsed() >= ARM_FOR) {
                        app.actions_disarm();
                    }
                    TimeoutAction::Drop
                }) {
                    warn!("failed to arm the uninstall's wait: {e}");
                }
                self.schedule_frame();
            }
            Does::Uninstall { .. } => {
                self.search.actions.armed = None;
                self.search.actions.key = None;
                if self.uninstall_app(&id) {
                    info!("uninstalling {id} from its search");
                }
                self.refilter();
                self.schedule_frame();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_own_actions_give_way_before_uninstall() {
        let widths = [100.0, 160.0, 140.0];
        let apart = [false, false, true];
        // Room for all: each with its gap, Uninstall's wider.
        let (kept, span) = fitting(&widths, &apart, 10.0, 1000.0);
        assert_eq!(kept, vec![0, 1, 2]);
        assert!((span - (100.0 + 160.0 + 140.0 + 10.0 + 10.0 + 25.0)).abs() < 0.01);
        // Too narrow for all three: the last action goes, Uninstall stays.
        let (kept, _) = fitting(&widths, &apart, 10.0, 300.0);
        assert_eq!(kept, vec![0, 2]);
        // Too narrow for any action: Uninstall alone, then nothing.
        assert_eq!(fitting(&widths, &apart, 10.0, 170.0).0, vec![2]);
        assert!(fitting(&widths, &apart, 10.0, 100.0).0.is_empty());
        // Alone (an app with no actions of its own), it is not set apart.
        assert!((fitting(&[140.0], &[true], 10.0, 1000.0).1 - 150.0).abs() < 0.01);
    }

    #[test]
    fn uninstall_asks_in_a_sentence_where_it_fits_else_in_short() {
        let pill = Pill {
            label: "Uninstall Zeta".into(),
            w: 100.0,
            does: Does::Uninstall { asked: ("Uninstall Zeta? Again to confirm".into(), 250.0), short: ("Again to uninstall".into(), 120.0) },
        };
        assert_eq!(pill.shown(false, 50.0, 20.0).0, "Uninstall Zeta");
        assert_eq!(pill.shown(true, 270.0, 20.0).0, "Uninstall Zeta? Again to confirm");
        assert_eq!(pill.shown(true, 269.0, 20.0), ("Again to uninstall", 120.0));
    }
}

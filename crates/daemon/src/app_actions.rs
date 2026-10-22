//! THE LONE APP'S ACTIONS (Max, 2026-10-22: "cuando el resultado sea una
//! sola app quiero que se muestren más opciones — ventana de incógnito,
//! nueva ventana, cada app trae sus acciones — … y desinstalar, con
//! confirmación"). A search that leaves ONE app in Apps puts that app's own
//! desktop actions (its desktop file's `Actions=`: Chrome's New Incognito
//! Window, Seam's New Private Window…) after it in the row, as cells of the
//! grid — and, for an app the dock can take away (a package it installed,
//! an installed webapp — never the system's own), "Uninstall <app>" last.
//!
//! THE LOOK (Max, 2026-10-22, the mockup's "3a — con su color, oscurecido";
//! canvas https://claude.ai/artifact/9WByNs3AHU4wZNYCR9tUgi): each action
//! is its app's own tile — the plate, the app's icon at half its light, its
//! colours kept — with the action's SYMBOL big and white in the middle, a
//! soft shadow under it, and its name under the tile on up to two lines.
//! The symbols are the apps' own where Golem has them (Chrome's hat and
//! glasses and its window, from its resources; Seam's mask and window, from
//! Firefox's `omni.ja`), else one by what the action says (`symbol_for`:
//! a window, home, the computer, a terminal, the bin), else an open arrow.
//!
//! They are cells like any other (temporary entries, id `act:<n>`, the
//! minimized windows' way): the app stays the one selected — Enter opens it
//! as ever; the arrows walk onto the actions, Enter or a click runs one; it
//! starts the way its app does (`launch::launch`: its graphics card, its
//! terminal) and the box goes. UNINSTALL ASKS ONCE: the first press turns
//! its plate red and its name "Again to uninstall"; a second press
//! uninstalls (`uninstall_app`, the drag-to-Install road). Escape, the keys
//! or the pointer leaving it, or `ARM_FOR` with nothing done put it back.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{error, info, warn};
use waverunner_core::index::AppEntry;

use crate::i18n::{fill, tr};
use crate::{apps, content, launch, App};

/// The id prefix of an action's cell (`act:<n>`, n its place in the list).
pub(crate) const PREFIX: &str = "act:";
/// How long Uninstall waits for its second press.
pub(crate) const ARM_FOR: Duration = Duration::from_secs(4);
/// How big the symbol's square is, of its tile (the symbol itself takes the
/// middle 16 of its 20 units, clear of the squircle's corners).
pub(crate) const SYMBOL: f32 = 0.675;
/// How much of its light the app's icon keeps under the symbol.
pub(crate) const DIM: f32 = 0.5;

/// Chrome's own symbols (its resources' New Window and Incognito, BSD-3 like
/// Chromium) and Seam's (Firefox's `window.svg` and `privateBrowsing.svg`,
/// MPL 2.0), each in a 16-unit box, drawn white.
const CHROME_WINDOW: &str = r#"<path fill-rule="evenodd" clip-rule="evenodd" d="M1 3.5A1.5 1.5 0 0 1 2.5 2H13a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 13 14H2.5A1.5 1.5 0 0 1 1 12.5ZM2.5 5v7.5H13V5Z"/>"#;
const INCOGNITO: &str = r#"<path d="M10.75 9c-.83 0-1.55.46-1.94 1.13-.26-.08-.53-.13-.81-.13s-.55.05-.81.13A2.249 2.249 0 0 0 5.25 9C4.01 9 3 10.01 3 11.25s1.01 2.25 2.25 2.25c1.12 0 2.04-.82 2.21-1.89.17-.07.35-.11.54-.11.19 0 .37.05.54.11.17 1.07 1.1 1.89 2.21 1.89 1.24 0 2.25-1.01 2.25-2.25S11.99 9 10.75 9Zm-5.5 3c-.41 0-.75-.34-.75-.75s.34-.75.75-.75.75.34.75.75-.34.75-.75.75Zm5.5 0c-.41 0-.75-.34-.75-.75s.34-.75.75-.75.75.34.75.75-.34.75-.75.75Zm.87-5.5-1.41-4a.763.763 0 0 0-.43-.45.787.787 0 0 0-.62.02L8 2.65l-1.16-.58a.764.764 0 0 0-1.05.43l-1.41 4H2V8h12V6.5h-2.38Zm-5.65 0 .95-2.7.74.37c.21.11.46.11.67 0l.74-.37.95 2.7H5.97Z"/>"#;
const SEAM_WINDOW: &str = r#"<path d="M13 1 3 1a2 2 0 0 0-2 2l0 9a2 2 0 0 0 2 2l10 0a2 2 0 0 0 2-2l0-9a2 2 0 0 0-2-2zM2.85 2.25l10.3 0 .6.6 0 2.15-11.5 0 0-2.15.6-.6zm10.3 10.5-10.3 0-.6-.6 0-5.9 11.5 0 0 5.9-.6.6z"/>"#;
const MASK: &str = r#"<path d="M15.931 5.479c-1.525-2.001-4.517-2.257-6.381-.545l-1.043.958-1.02 0-1.042-.958C4.581 3.223 1.589 3.478.064 5.479c-.197.78.108 3.66.28 4.113.347 1.83 1.842 3.206 3.635 3.206.895 0 1.706-.357 2.347-.929l.391-.334a1.936 1.936 0 0 1 2.511-.021l.586.486 0-.003c.617.497 1.375.801 2.203.801 1.792 0 3.288-1.376 3.635-3.206.172-.453.488-3.321.279-4.113zM6.232 8.452c-.427.482-1.088.792-1.832.792-.744 0-1.405-.31-1.832-.792a.693.693 0 0 1 0-.905c.427-.482 1.088-.792 1.832-.792.744 0 1.405.31 1.832.792a.693.693 0 0 1 0 .905zm7.2 0c-.427.482-1.088.792-1.832.792-.744 0-1.405-.31-1.832-.792a.693.693 0 0 1 0-.905c.427-.482 1.088-.792 1.832-.792.744 0 1.405.31 1.832.792a.693.693 0 0 1 0 .905z"/>"#;

/// Every symbol an action can wear, in the order of their texture layers:
/// a filled path of the apps', or one of Golem's own strokes by its name in
/// `glyphs::GLYPHS`.
const SYMBOLS: [(&str, Symbol); 9] = [
    ("window", Symbol::Filled(CHROME_WINDOW)),
    ("incognito", Symbol::Filled(INCOGNITO)),
    ("seam-window", Symbol::Filled(SEAM_WINDOW)),
    ("mask", Symbol::Filled(MASK)),
    ("trash", Symbol::Stroke("trash")),
    ("home", Symbol::Stroke("home")),
    ("computer", Symbol::Stroke("pc")),
    ("terminal", Symbol::Stroke("term")),
    ("open", Symbol::Stroke("open")),
];

#[derive(Clone, Copy)]
enum Symbol {
    Filled(&'static str),
    Stroke(&'static str),
}

impl Symbol {
    fn svg(self) -> Option<String> {
        let body = match self {
            Symbol::Filled(body) => {
                return Some(format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="-2 -2 20 20" fill="#fff">{body}</svg>"##
                ))
            }
            Symbol::Stroke(name) => crate::glyphs::GLYPHS.iter().find(|(n, _)| *n == name)?.1,
        };
        Some(format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="-2 -2 20 20" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##
        ))
    }
}

/// The symbols' tiles, one texture layer each in `SYMBOLS`' order (put
/// after the entries' icons as the dock's other figures are).
pub(crate) fn figures() -> Option<Vec<Vec<u8>>> {
    SYMBOLS.iter().map(|(_, s)| apps::svg_figure(&s.svg()?)).collect()
}

/// Which symbol an action of `app` wears, by its id and name in its desktop
/// file: the browsers' own for a new window and a private one, then by what
/// it says, else the open arrow.
fn symbol_for(app: &str, id: &str, name: &str) -> usize {
    let (app, said) = (app.to_lowercase(), format!("{} {}", id.to_lowercase(), name.to_lowercase()));
    let seam = app.contains("seam") || app.contains("firefox");
    let named = |what: &str| SYMBOLS.iter().position(|(n, _)| *n == what).unwrap_or(SYMBOLS.len() - 1);
    let says = |words: &[&str]| words.iter().any(|w| said.contains(w));
    if says(&["private", "incognito", "privad", "incógnito"]) {
        return named(if seam { "mask" } else { "incognito" });
    }
    if says(&["window", "ventana"]) {
        return named(if seam { "seam-window" } else { "window" });
    }
    let by_word = [
        ("home", "home"),
        ("trash", "trash"),
        ("papelera", "trash"),
        ("computer", "computer"),
        ("equipo", "computer"),
        ("terminal", "terminal"),
    ];
    by_word
        .iter()
        .find(|(w, _)| said.contains(w))
        .map_or(named("open"), |(_, s)| named(s))
}

/// A name on two lines under its tile, broken between WORDS: as many as
/// fit on the first, the rest on the second (cut with "…" if still too
/// long); a first word too long for the line is broken as Files breaks a
/// name (`filer::two_lines`).
fn by_words(text: &str, room: f32, measure: &mut dyn FnMut(&str) -> f32) -> (String, String) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let fits = |n: usize, measure: &mut dyn FnMut(&str) -> f32| measure(&words[..n].join(" ")) <= room - crate::desktop::FIT_SLACK;
    let mut n = 0;
    while n < words.len() && fits(n + 1, measure) {
        n += 1;
    }
    if n == 0 || n == words.len() {
        return crate::filer::two_lines(text, room, measure);
    }
    (words[..n].join(" "), crate::desktop::fit_label(&words[n..].join(" "), room, measure))
}

/// What an action does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Does {
    /// One of the app's desktop actions: its command.
    Run(String),
    /// Take the app away.
    Uninstall,
}

/// One of the lone app's actions.
#[derive(Debug, Clone)]
pub(crate) struct Action {
    /// Its name, as its cell says it.
    pub name: String,
    pub does: Does,
    /// Its symbol (an index of `SYMBOLS`).
    pub symbol: usize,
}

/// The lone app's actions, the cells they stand in, and the asking.
#[derive(Debug, Default)]
pub(crate) struct Actions {
    /// The app they are of (its desktop id).
    pub app: Option<String>,
    pub list: Vec<Action>,
    /// The entry each action's cell is, this refilter (`list` order).
    pub cells: Vec<usize>,
    /// Since when Uninstall has been asking to be sure.
    pub armed: Option<Instant>,
}

impl Actions {
    /// The Uninstall action's place in `list`.
    fn uninstall(&self) -> Option<usize> {
        self.list.iter().position(|a| a.does == Does::Uninstall)
    }
}

impl App {
    /// Whether the dock can uninstall `id`: a package it installed, or an
    /// installed webapp — as `uninstall_app` does them; nothing busy.
    pub(crate) fn can_uninstall(&self, id: &str) -> bool {
        !self.busy_ids.contains(id)
            && ((self.removable_ids.contains(id) && self.managed.attr_for_app(id).is_some())
                || self.installed_webapp_slug(id).is_some())
    }

    /// The action entry `index` is, as its place in the list.
    pub(crate) fn action_of(&self, index: usize) -> Option<usize> {
        self.entries.get(index)?.id.strip_prefix(PREFIX)?.parse().ok()
    }

    /// After a search's Apps are ranked (`refilter`): when they are ONE app,
    /// its actions follow it in the row as cells of their own. The same app
    /// keeps its asking; another starts afresh.
    pub(crate) fn add_action_cells(&mut self, apps: &mut Vec<usize>, resting: bool) {
        let lone = match apps.as_slice() {
            &[only] if !resting && !self.control_panel && !self.search.query.is_empty() => Some(only),
            _ => None,
        };
        let Some((index, entry)) = lone.and_then(|i| Some((i, self.entries.get(i)?.clone()))) else {
            self.search.actions = Actions::default();
            return;
        };
        let mut list: Vec<Action> = entry
            .path
            .as_deref()
            .map(waverunner_core::index::desktop_actions)
            .unwrap_or_default()
            .into_iter()
            .map(|a| Action { symbol: symbol_for(&entry.id, &a.id, &a.name), name: a.name, does: Does::Run(a.exec) })
            .collect();
        if self.can_uninstall(&entry.id) {
            list.push(Action {
                name: fill(tr("Uninstall {app}"), &[("app", &entry.name)]),
                does: Does::Uninstall,
                symbol: SYMBOLS.iter().position(|(n, _)| *n == "trash").unwrap_or(0),
            });
        }
        let same = self.search.actions.app.as_deref() == Some(entry.id.as_str());
        let armed = self.search.actions.armed.filter(|_| same);
        self.search.actions = Actions { app: Some(entry.id.clone()), list, cells: Vec::new(), armed };
        let (placeholder, layer) = (
            self.placeholders.get(index).copied().unwrap_or(false),
            self.icon_layers.get(index).copied().unwrap_or(index as u32),
        );
        for n in 0..self.search.actions.list.len() {
            let name = self.action_label(n);
            let exec = match &self.search.actions.list[n].does {
                Does::Run(exec) => exec.clone(),
                Does::Uninstall => String::new(),
            };
            let cell = AppEntry {
                id: format!("{PREFIX}{n}"),
                name,
                description: None,
                exec,
                icon: entry.icon.clone(),
                startup_wm_class: None,
                needs_terminal: entry.needs_terminal,
                path: None,
            };
            let at = self.push_transient(cell, apps::EntryKind::App, placeholder, layer);
            self.search.actions.cells.push(at);
            apps.push(at);
        }
    }

    /// What action `n`'s cell says: its name — Uninstall asking, "Again to
    /// uninstall" — on up to two lines (a `\n` between them), broken where
    /// the grid's label has room.
    fn action_label(&mut self, n: usize) -> String {
        let a = &self.search.actions;
        let Some(action) = a.list.get(n) else {
            return String::new();
        };
        let text = if action.does == Does::Uninstall && a.armed.is_some() {
            tr("Again to uninstall").to_string()
        } else {
            action.name.clone()
        };
        let layout = self.layout_at(self.ui.extent_of(crate::state::Target::Open));
        let sec = &layout.sections[content::SECTION_APPS];
        let room = sec.viewport.w / sec.cols.max(1) as f32 - 12.0;
        let mut measure = |s: &str| match self.renderer.as_mut() {
            Some(r) => r.measure_text(s, content::LABEL_FONT_PX, None),
            None => crate::options::est_text_w(s, content::LABEL_FONT_PX),
        };
        let (first, second) = by_words(&text, room, &mut measure);
        if second.is_empty() {
            first
        } else {
            format!("{first}\n{second}")
        }
    }

    /// Write the actions' names into their cells again (the asking came or
    /// went).
    fn relabel_actions(&mut self) {
        for n in 0..self.search.actions.cells.len() {
            let name = self.action_label(n);
            if let Some(entry) = self.search.actions.cells.get(n).and_then(|&i| self.entries.get_mut(i)) {
                entry.name = name;
            }
        }
        self.schedule_frame();
    }

    /// What the frame draws of them: each action cell's entry with its
    /// symbol's texture layer, and the cell asking to be sure.
    pub(crate) fn action_draw(&self) -> (Vec<(usize, u32)>, Option<usize>) {
        let a = &self.search.actions;
        let Some(base) = self.act_symbol_layer else {
            return (Vec::new(), None);
        };
        let cells = a.cells.iter().zip(&a.list).map(|(&e, act)| (e, base + act.symbol as u32)).collect();
        let asking = a.uninstall().filter(|_| a.armed.is_some()).and_then(|u| a.cells.get(u).copied());
        (cells, asking)
    }

    /// Escape puts an asking Uninstall back (the next Escape is the box's).
    pub(crate) fn app_actions_key(&mut self, keysym: Keysym) -> bool {
        if keysym == Keysym::Escape && self.search.actions.armed.is_some() {
            self.actions_disarm();
            return true;
        }
        false
    }

    /// Put Uninstall back once neither the keys nor the pointer are on its
    /// cell (every frame of the box).
    pub(crate) fn actions_settle(&mut self) {
        let a = &self.search.actions;
        let Some(u) = a.uninstall().filter(|_| a.armed.is_some()) else {
            return;
        };
        let Some(&entry) = a.cells.get(u) else {
            return;
        };
        let at = self.search.visible[content::SECTION_APPS].iter().position(|&e| e == entry);
        let keys_on = at.is_some() && self.search.selected == at;
        let pointer_on = at.is_some_and(|i| self.hover == Some(content::Hit::GridCell(content::SECTION_APPS, i)));
        if !keys_on && !pointer_on {
            self.actions_disarm();
        }
    }

    /// Uninstall stops asking.
    fn actions_disarm(&mut self) {
        if self.search.actions.armed.take().is_some() {
            self.relabel_actions();
        }
    }

    /// Run action `n`: a command starts and the box goes; Uninstall asks,
    /// and uninstalls when asked again.
    pub(crate) fn run_app_action(&mut self, n: usize) {
        let Some(action) = self.search.actions.list.get(n).cloned() else {
            return;
        };
        let Some(app) = self.search.actions.app.clone() else {
            return;
        };
        let Some(index) = self.entries.iter().position(|e| e.id == app) else {
            return;
        };
        let needs_terminal = self.entries[index].needs_terminal;
        match action.does {
            Does::Run(exec) => {
                info!("app action {:?} of {app}", action.name);
                let exec = self.gpu_exec(&app, exec);
                if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
                    error!("app action failed for {app}: {e:#}");
                }
                self.launched(index);
            }
            Does::Uninstall if self.search.actions.armed.is_none() => {
                self.search.actions.armed = Some(Instant::now());
                self.relabel_actions();
                let timer = Timer::from_duration(ARM_FOR);
                if let Err(e) = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                    if app.search.actions.armed.is_some_and(|t| t.elapsed() >= ARM_FOR) {
                        app.actions_disarm();
                    }
                    TimeoutAction::Drop
                }) {
                    warn!("failed to arm the uninstall's wait: {e}");
                }
            }
            Does::Uninstall => {
                self.search.actions.armed = None;
                if self.uninstall_app(&app) {
                    info!("uninstalling {app} from its search");
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
    fn each_action_wears_its_apps_own_symbol_or_one_by_what_it_says() {
        let of = |app: &str, id: &str, name: &str| SYMBOLS[symbol_for(app, id, name)].0;
        assert_eq!(of("google-chrome", "new-private-window", "New Incognito Window"), "incognito");
        assert_eq!(of("google-chrome", "new-window", "New Window"), "window");
        assert_eq!(of("seam", "new-private-window", "New Private Window"), "mask");
        assert_eq!(of("seam", "new-window", "Nueva ventana"), "seam-window");
        assert_eq!(of("code", "new-empty-window", "New Empty Window"), "window");
        assert_eq!(of("thunar", "open-home", "Home"), "home");
        assert_eq!(of("thunar", "open-computer", "Computer"), "computer");
        assert_eq!(of("thunar", "open-trash", "Trash"), "trash");
        assert_eq!(of("Alacritty", "New", "New Terminal"), "terminal");
        assert_eq!(of("org.inkscape.Inkscape", "new-drawing", "New Drawing"), "open");
    }

    #[test]
    fn a_name_breaks_between_words() {
        // Ten pixels a letter, spaces too.
        let mut w = |s: &str| s.chars().count() as f32 * 10.0;
        let room = 140.0 + crate::desktop::FIT_SLACK;
        assert_eq!(by_words("New Window", room, &mut w), ("New Window".into(), String::new()));
        assert_eq!(by_words("New Incognito Window", room, &mut w), ("New Incognito".into(), "Window".into()));
        assert_eq!(by_words("Uninstall Google Chrome", room, &mut w), ("Uninstall".into(), "Google Chrome".into()));
    }

    #[test]
    fn every_symbol_is_drawn() {
        for (name, s) in SYMBOLS {
            let svg = s.svg().unwrap_or_else(|| panic!("{name} has no symbol"));
            assert!(apps::svg_figure(&svg).is_some(), "{name} does not draw");
        }
    }
}

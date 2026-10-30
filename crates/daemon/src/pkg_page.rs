//! THE PAGE OF AN APP TO INSTALL (Max, 2026-10-29: "if click or shift on an
//! app, instead of drag it to install, i want the app to take the whole box,
//! on that whole box i want the option install the app, website (or more
//! info) and i want some info about the app"). A click, Enter or Shift on a
//! cell of the Install section — a package or a catalog webapp — gives the
//! box to it: the results go up and out as they do for the control panel
//! and the page rises in their place (it is drawn as a `PanelDraw` and rides
//! the panel's push). Its icon, name, what it is and what is known of it, and
//! its buttons: Install (Installing… / Installed, a webapp's Open), Website
//! (or More info: the package on search.nixos.org) and Try. What a package's
//! page says beyond its name comes from the nix threads (`nix::Request::
//! Details`: the details file beside the index, else nix asked once).
//! Escape, Backspace, Shift again, its ‹, or typing go back to the results;
//! the drag to the grid still installs, as before.

use crate::content::{self, Label, Rect, RectInst};
use crate::{apps, nix, panel::PanelDraw, App};
use tracing::info;

/// What the page is of.
#[derive(Clone)]
pub(crate) enum Of {
    Package(nix::PkgEntry),
    /// A webapp of the catalog: its entry's id, its slug, its address.
    Webapp { id: String, slug: String, name: String, url: String },
}

impl Of {
    fn id(&self) -> &str {
        match self {
            Of::Package(p) => &p.attr,
            Of::Webapp { id, .. } => id,
        }
    }
}

/// A button of the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Button {
    Back,
    Install,
    Open,
    Website,
    Try,
}

pub(crate) struct Page {
    pub of: Of,
    /// What a package's page says beyond its name (`None`: asked, not come).
    pub details: Option<nix::PkgDetails>,
    /// Where its buttons were drawn last, and the one under the pointer.
    pub buttons: Vec<(Button, Rect)>,
    pub hover: Option<Button>,
    /// The button the keys are on (Enter presses it).
    pub keyed: Option<Button>,
    /// Going: it is drawn while the results come back down, then dropped.
    pub leaving: bool,
}

/// The page's sizes, at a scale of 1.
const PAD: f32 = 30.0;
const ICON: f32 = 96.0;
const NAME_PX: f32 = 25.0;
const FACTS_PX: f32 = 14.0;
const TEXT_PX: f32 = 15.0;
const LONG_PX: f32 = 14.0;
const BUTTON_H: f32 = 34.0;
const BUTTON_PX: f32 = 15.0;
const BUTTON_PAD: f32 = 20.0;
const BUTTON_GAP: f32 = 10.0;
/// The column's widest (a page on a wide card keeps its lines readable), and the
/// long description's most lines.
const COLUMN: f32 = 560.0;
const LONG_MAX: usize = 6;

impl App {
    /// The page is up (not on its way out, and not under the control panel:
    /// the panel's field takes the card's clicks).
    pub(crate) fn page_up(&self) -> bool {
        !self.control_panel && self.search.page.as_ref().is_some_and(|p| !p.leaving)
    }

    /// The package whose page is up (or going): it keeps an entry for its
    /// icon when it leaves the list — installed from the page.
    pub(crate) fn page_package(&self) -> Option<&nix::PkgEntry> {
        match self.search.page.as_ref().map(|p| &p.of) {
            Some(Of::Package(p)) => Some(p),
            _ => None,
        }
    }

    /// Install-section entry `e` takes the box: its page.
    pub(crate) fn page_open(&mut self, e: usize) -> bool {
        let Some(entry) = self.entries.get(e) else {
            return false;
        };
        let of = match self.kinds.get(e) {
            Some(apps::EntryKind::Package) => match self.pkg_hits.iter().find(|p| p.attr == entry.id) {
                Some(p) => Of::Package(p.clone()),
                None => return false,
            },
            Some(apps::EntryKind::App) if self.is_catalog_webapp(e) => {
                let Some(slug) = crate::webapps::slug_of_id(&entry.id) else {
                    return false;
                };
                let url = crate::webapps::exec_app_url(&entry.exec).unwrap_or_default().to_owned();
                Of::Webapp { id: entry.id.clone(), slug: slug.to_owned(), name: entry.name.clone(), url }
            }
            _ => return false,
        };
        info!("page of {}", of.id());
        if let Of::Package(p) = &of {
            self.nix.request(nix::Request::Details { attr: p.attr.clone() });
        }
        let details = matches!(of, Of::Webapp { .. }).then(nix::PkgDetails::default);
        self.search.page = Some(Page { of, details, buttons: Vec::new(), hover: None, keyed: None, leaving: false });
        self.hover = None;
        self.gesture.pressed = None;
        self.schedule_frame();
        true
    }

    /// Back to the results (the page goes down with them coming back).
    pub(crate) fn page_close(&mut self) {
        if let Some(p) = self.search.page.as_mut() {
            p.leaving = true;
            p.hover = None;
            self.schedule_frame();
        }
    }

    /// What a package's page says came (`nix::Event::Details`).
    pub(crate) fn page_details(&mut self, attr: &str, details: nix::PkgDetails) {
        if let Some(p) = self.search.page.as_mut().filter(|p| p.of.id() == attr) {
            p.details = Some(details);
            self.schedule_frame();
        }
    }

    /// Where the page has its parts: the card's own room between the dock's
    /// band (riding the card's top, or down its side with the dock on the left
    /// or the right) and the search.
    fn page_area(layout: &content::Layout, sc: f32) -> Rect {
        let band = layout.dock_band;
        let top = if !layout.sideways() && band.y >= layout.card_top - 1.0 && band.y < layout.card_top + layout.card_h / 2.0 {
            band.y + band.h
        } else {
            layout.card_top
        };
        let bottom = if layout.search_box.y > top { layout.search_box.y } else { layout.card_top + layout.card_h };
        let (mut x0, mut x1) = (layout.card_x, layout.card_x + layout.card_w);
        if layout.sideways() && band.w > 0.0 && band.y < layout.card_top + layout.card_h && band.y + band.h > layout.card_top {
            let mid = layout.card_x + layout.card_w / 2.0;
            if band.x + band.w <= mid && band.x + band.w > x0 {
                x0 = band.x + band.w; // the band down the card's left
            } else if band.x >= mid && band.x < x1 {
                x1 = band.x; // down its right
            }
        }
        let pad = PAD * sc;
        Rect::new(x0 + pad, top + pad * 0.6, (x1 - x0 - 2.0 * pad).max(0.0), (bottom - top - pad).max(0.0))
    }

    /// The state of the page's package or webapp: what Install says.
    fn page_install_state(&self) -> Button {
        let Some(page) = self.search.page.as_ref() else {
            return Button::Install;
        };
        // (Being installed: on the list already, not on the machine yet.)
        if self.page_busy() {
            return Button::Install;
        }
        match &page.of {
            Of::Package(p) => {
                if self.pkg_installed(p) {
                    Button::Open
                } else {
                    Button::Install
                }
            }
            Of::Webapp { slug, .. } => {
                if self.managed_webapps.contains(slug) {
                    Button::Open
                } else {
                    Button::Install
                }
            }
        }
    }

    fn page_busy(&self) -> bool {
        self.search.page.as_ref().is_some_and(|p| {
            let id = match &p.of {
                Of::Package(pkg) => pkg.attr.as_str(),
                Of::Webapp { id, .. } => id.as_str(),
            };
            self.busy_ids.contains(id) || self.pending_installs.iter().any(|i| i.attr == id) || self.pending_webapps.iter().any(|w| w.id == id)
        })
    }

    /// A package tried is being built to run (`start_launch`).
    fn page_trying(&self) -> bool {
        matches!(self.search.page.as_ref().map(|p| &p.of), Some(Of::Package(p)) if self.launching.contains(&p.attr))
    }

    /// The buttons the page has now, in order.
    fn page_buttons(&self) -> Vec<Button> {
        let mut out = vec![self.page_install_state(), Button::Website];
        // (Tried without installing: a package built and run, a webapp opened
        // — not while it is being installed.)
        if self.page_install_state() == Button::Install && !self.page_busy() {
            out.push(Button::Try);
        }
        out
    }

    fn page_words(&self, b: Button) -> String {
        let installed_pkg = matches!(self.search.page.as_ref().map(|p| &p.of), Some(Of::Package(_)));
        match b {
            Button::Back => crate::i18n::tr("‹ Back").to_string(),
            Button::Install if self.page_busy() => crate::i18n::tr("Installing…").to_string(),
            Button::Install => crate::i18n::tr("Install").to_string(),
            // (An installed package is in the grid; what to open is its app.)
            Button::Open if installed_pkg => crate::i18n::tr("Installed").to_string(),
            Button::Open => crate::i18n::tr("Open").to_string(),
            Button::Website if self.page_site().1 => crate::i18n::tr("Website").to_string(),
            Button::Website => crate::i18n::tr("More info").to_string(),
            Button::Try if self.page_trying() => crate::i18n::tr("Starting…").to_string(),
            Button::Try => crate::i18n::tr("Try").to_string(),
        }
    }

    /// Where Website goes, and whether it is the app's own site (else the
    /// package on search.nixos.org: its site, licence and more are there).
    fn page_site(&self) -> (String, bool) {
        match self.search.page.as_ref().map(|p| (&p.of, &p.details)) {
            Some((Of::Webapp { url, .. }, _)) => (url.clone(), true),
            Some((Of::Package(p), details)) => match details.as_ref().and_then(|d| d.homepage.clone()) {
                Some(site) if site.starts_with("https://") || site.starts_with("http://") => (site, true),
                _ => (format!("https://search.nixos.org/packages?channel=unstable&show={0}&query={0}", p.attr), false),
            },
            None => (String::new(), false),
        }
    }

    /// The page as the card draws it this frame (`content` puts it where the
    /// results were and pushes it as the control panel's field).
    pub(crate) fn page_draw(&mut self, layout: &content::Layout, ink: [f32; 4], bright: bool) -> PanelDraw {
        let mut draw = PanelDraw::default();
        let Some(page) = self.search.page.as_ref() else {
            return draw;
        };
        let sc = self.options_scale();
        let area = Self::page_area(layout, sc);
        let dim = [ink[0], ink[1], ink[2], ink[3] * 0.62];
        let faint = [ink[0], ink[1], ink[2], ink[3] * 0.5];
        let measure = |app: &App, text: &str, px: f32, family: Option<&'static str>| -> f32 {
            app.renderer.as_ref().map_or(text.chars().count() as f32 * px * 0.55, |r| r.measure_text(text, px, family))
        };
        let label = |text: String, x: f32, y: f32, w: f32, px: f32, line: f32, family: Option<&'static str>, color: [f32; 4]| Label {
            text,
            pos: (x, y),
            max_w: w,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: true,
            clip: None,
            family,
            color: Some(color),
        };
        let (name, facts, text, long) = match &page.of {
            Of::Package(p) => {
                let d = page.details.as_ref();
                let mut facts = vec![p.version.clone()];
                if let Some(l) = d.and_then(|d| d.license.clone()) {
                    facts.push(l);
                }
                facts.push(crate::i18n::tr("App to install").to_string());
                let text = match d.map(|d| d.description.trim()) {
                    Some(t) if !t.is_empty() => t.to_owned(),
                    Some(_) => String::new(),
                    None => "…".to_owned(),
                };
                let long = d.and_then(|d| d.long.clone()).unwrap_or_default();
                (p.name.clone(), facts.into_iter().filter(|f| !f.is_empty()).collect::<Vec<_>>().join(" · "), text, long)
            }
            Of::Webapp { name, url, .. } => {
                let host = crate::web_actions::origin(url).map(|(_, h)| h).unwrap_or_default();
                let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
                (
                    name.clone(),
                    format!("{host} · {}", crate::i18n::tr("Web app")),
                    crate::i18n::tr("It opens in its own window of Seam, signed in as you are there.").to_string(),
                    String::new(),
                )
            }
        };
        // ‹ Back, its own quiet button at the top left.
        let mut buttons: Vec<(Button, Rect)> = Vec::new();
        let back = self.page_words(Button::Back);
        let back_w = measure(self, &back, FACTS_PX * sc, None) + 18.0 * sc;
        let back_rect = Rect::new(area.x - 9.0 * sc, area.y, back_w, 26.0 * sc);
        if page.hover == Some(Button::Back) || page.keyed == Some(Button::Back) {
            draw.rects.push(RectInst {
                rect: back_rect,
                radius: back_rect.h / 2.0,
                color: crate::options::wash(!bright, 0.10),
                glass: 0.0,
                border: 0.0,
            });
        }
        draw.labels.push(label(back, back_rect.x + 9.0 * sc, back_rect.y + (back_rect.h - FACTS_PX * sc * 1.3) / 2.0, back_w, FACTS_PX * sc, FACTS_PX * sc * 1.3, None, dim));
        buttons.push((Button::Back, back_rect));
        // ONE COLUMN, CENTRED (Max, 2026-10-30: "i want it centered"): the icon, the
        // name, what it is, what it does, the buttons, and as much of the long
        // description as fits (`LONG_MAX` lines at most) — the whole of it in the
        // middle of the room under ‹ Back, across and down.
        let cw = area.w.min(COLUMN * sc);
        let cx = area.x + area.w / 2.0;
        let x0 = cx - cw / 2.0;
        let room_top = back_rect.y + back_rect.h + 8.0 * sc;
        let room_h = (area.y + area.h - room_top).max(0.0);
        let side = (ICON * sc).min(room_h * 0.3);
        let name_line = NAME_PX * sc * 1.3;
        let facts_line = FACTS_PX * sc * 1.35;
        let text_line = TEXT_PX * sc * 1.4;
        let long_line = LONG_PX * sc * 1.5;
        let texts = crate::peek_page::wrap(&text, cw, 2, &mut |s: &str| measure(self, s, TEXT_PX * sc, None));
        let head = side + 16.0 * sc + name_line + 2.0 * sc + facts_line + 10.0 * sc + texts.len() as f32 * text_line + 20.0 * sc + BUTTON_H * sc;
        let long = long.split_whitespace().collect::<Vec<_>>().join(" ");
        let long_gap = 24.0 * sc;
        let long_room = (((room_h - head - long_gap) / long_line).floor().max(0.0) as usize).min(LONG_MAX);
        let longs = if long.is_empty() || long_room == 0 {
            Vec::new()
        } else {
            crate::peek_page::wrap(&long, cw, long_room, &mut |s: &str| measure(self, s, LONG_PX * sc, None))
        };
        let total = head + if longs.is_empty() { 0.0 } else { long_gap + longs.len() as f32 * long_line };
        let mut y = room_top + ((room_h - total) / 2.0).max(0.0);
        let centred = |text: String, y: f32, px: f32, line: f32, family: Option<&'static str>, color: [f32; 4]| Label {
            centered: true,
            ..label(text, cx, y, cw, px, line, family, color)
        };
        let icon = Rect::new(cx - side / 2.0, y, side, side);
        if let Some(e) = self.entries.iter().position(|e| e.id == page.of.id()) {
            draw.icons.push((e, icon));
        }
        y += side + 16.0 * sc;
        let name = crate::desktop::fit_label(&name, cw, &mut |s: &str| measure(self, s, NAME_PX * sc, Some(content::FONT_MEDIUM)));
        draw.labels.push(centred(name, y, NAME_PX * sc, name_line, Some(content::FONT_MEDIUM), ink));
        y += name_line + 2.0 * sc;
        let facts = crate::desktop::fit_label(&facts, cw, &mut |s: &str| measure(self, s, FACTS_PX * sc, None));
        draw.labels.push(centred(facts, y, FACTS_PX * sc, facts_line, None, dim));
        y += facts_line + 10.0 * sc;
        for line in texts {
            draw.labels.push(centred(line, y, TEXT_PX * sc, text_line, None, ink));
            y += text_line;
        }
        // The buttons, a row in the middle.
        y += 20.0 * sc;
        let order = self.page_buttons();
        let widths: Vec<f32> = order.iter().map(|&b| measure(self, &self.page_words(b), BUTTON_PX * sc, None) + 2.0 * BUTTON_PAD * sc).collect();
        let row = widths.iter().sum::<f32>() + BUTTON_GAP * sc * (widths.len().saturating_sub(1)) as f32;
        let mut bx = (cx - row / 2.0).max(x0.min(area.x));
        let lit = self.page_install_state() == Button::Install && !self.page_busy();
        for (&b, &w) in order.iter().zip(&widths) {
            let words = self.page_words(b);
            let r = Rect::new(bx, y, w, BUTTON_H * sc);
            let first = b == Button::Install && lit;
            let on = page.hover == Some(b) || page.keyed == Some(b);
            let rest = if first { 0.20 } else { 0.10 };
            draw.rects.push(RectInst {
                rect: r,
                radius: r.h / 2.0,
                color: crate::options::wash(!bright, rest + if on { 0.10 } else { 0.0 }),
                glass: 0.0,
                border: 0.0,
            });
            let quiet = (b == Button::Open && matches!(page.of, Of::Package(_)))
                || (b == Button::Install && self.page_busy())
                || (b == Button::Try && self.page_trying());
            draw.labels.push(Label {
                centered: true,
                ..label(words, r.x + r.w / 2.0, r.y + (r.h - BUTTON_PX * sc * 1.3) / 2.0, w, BUTTON_PX * sc, BUTTON_PX * sc * 1.3, None, if quiet { dim } else { ink })
            });
            buttons.push((b, r));
            bx += w + BUTTON_GAP * sc;
        }
        y += BUTTON_H * sc + long_gap;
        for line in longs {
            draw.labels.push(centred(line, y, LONG_PX * sc, long_line, None, faint));
            y += long_line;
        }
        if let Some(p) = self.search.page.as_mut() {
            p.buttons = buttons;
        }
        draw
    }

    /// The pointer moved over the page: which button it is on.
    pub(crate) fn page_hover(&mut self, pos: (f32, f32)) {
        let Some(p) = self.search.page.as_mut().filter(|p| !p.leaving) else {
            return;
        };
        let on = p.buttons.iter().find(|(_, r)| r.contains(pos)).map(|&(b, _)| b);
        if on != p.hover {
            p.hover = on;
            if on.is_some() {
                p.keyed = None;
            }
            self.schedule_frame();
        }
    }

    /// A click on the page.
    pub(crate) fn page_click(&mut self, pos: (f32, f32)) {
        let b = self.search.page.as_ref().and_then(|p| p.buttons.iter().find(|(_, r)| r.contains(pos)).map(|&(b, _)| b));
        if let Some(b) = b {
            self.page_press(b);
        }
    }

    /// What a button does.
    fn page_press(&mut self, b: Button) {
        let Some(of) = self.search.page.as_ref().map(|p| p.of.clone()) else {
            return;
        };
        let entry = self.entries.iter().position(|e| e.id == of.id());
        match b {
            Button::Back => self.page_close(),
            Button::Install if self.page_busy() => {}
            Button::Install => match &of {
                Of::Package(p) => {
                    info!("page: installing {}", p.attr);
                    self.start_pending_install(&p.attr, p.name.clone(), p.version.clone(), None, None);
                }
                Of::Webapp { slug, .. } => {
                    if let Some(e) = entry {
                        self.install_webapp(slug, e);
                    }
                }
            },
            Button::Open => {
                if let (Of::Webapp { id, .. }, Some(e)) = (&of, self.entries.iter().position(|en| en.id == of.id())) {
                    info!("page: opening {id}");
                    self.page_close();
                    self.activate(e, crate::LaunchFrom::Box);
                }
            }
            Button::Website => {
                let (site, _) = self.page_site();
                if !site.is_empty() {
                    info!("page: {site}");
                    let _ = crate::launch::launch(&format!("xdg-open {}", crate::launch::shell_quote(&site)), false, "");
                    self.dismiss();
                }
            }
            Button::Try if self.page_trying() => {}
            Button::Try => match &of {
                // (It is built first: the page stays and says so.)
                Of::Package(p) => self.start_launch(&p.attr),
                Of::Webapp { .. } => {
                    if let Some(e) = entry {
                        self.launch_webapp(e);
                        self.dismiss();
                    }
                }
            },
        }
        self.schedule_frame();
    }

    /// A key while the page is up: true when it was the page's.
    pub(crate) fn page_key(&mut self, keysym: smithay_client_toolkit::seat::keyboard::Keysym) -> bool {
        use smithay_client_toolkit::seat::keyboard::Keysym as K;
        if !self.page_up() {
            return false;
        }
        let order = self.page_buttons();
        let at = self.search.page.as_ref().and_then(|p| p.keyed).and_then(|k| order.iter().position(|&b| b == k));
        match keysym {
            K::Escape | K::BackSpace => self.page_close(),
            K::Return | K::KP_Enter | K::space => {
                let b = self.search.page.as_ref().and_then(|p| p.keyed).unwrap_or(order[0]);
                self.page_press(b);
            }
            K::Left | K::Right | K::Tab | K::ISO_Left_Tab => {
                let n = order.len() as i64;
                let step = if matches!(keysym, K::Left | K::ISO_Left_Tab) { -1 } else { 1 };
                let next = at.map_or(if step > 0 { 0 } else { n - 1 }, |i| (i as i64 + step).rem_euclid(n));
                if let Some(p) = self.search.page.as_mut() {
                    p.keyed = Some(order[next as usize]);
                    p.hover = None;
                }
                self.schedule_frame();
            }
            _ => return false,
        }
        true
    }
}

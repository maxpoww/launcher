//! THE DEEP FILE SEARCH — "/" in the box (Max, 2026-10-27: "the FILE: will
//! be /. … typing / opens the searcher, which should be a deep file search";
//! built from a study of Windows' new search, Spotlight, KRunner, Raycast,
//! Alfred and Everything). Typed into an empty search, "/" says "I am only
//! looking for files": the box is files and nothing else.
//!
//! - THE CHIP in the search pill ("Files", "Files › Facturas" when kept to a
//!   folder): Backspace in an empty field takes the folder off, then the kind,
//!   then the deep search; Escape leaves it (KRunner's one-runner mode,
//!   Spotlight's `/PDF`).
//! - THE TABS under the field (Max, 2026-10-27: "letters as we use on the
//!   boxes… under the searchbox… 9, and two, three pages more… the ones the
//!   user uses start being closer… if i search '.html' the tag should be
//!   there"): words, not pills (`Tag`: All, the kinds, file types as ".html",
//!   places as "Downloads/"), NINE a page (`TABS_PAGE`) and pages after them
//!   (`first_tabs`: the kinds and Downloads first, then types and places);
//!   ORDERED BY USE as fast launch is (`SearchState::tab_uses`, a use = a
//!   click on one, or a file opened while it is lit; All stays first; the
//!   order is made as "/" opens, never under the pointer). A click, Tab /
//!   Shift+Tab (the page follows), the wheel over them, or a kind word or an
//!   ending typed (".html" — a tab made for it if there is none) lights one.
//! - BEFORE A WORD: three SHELVES — what was opened lately (the system's
//!   record and what the box opened), what came lately (the watched folders'
//!   tops, newest first) and the places (the home's folders).
//! - WITH WORDS: the files found, a shelf A KIND, the kind with the best
//!   match first, titled "PDF · 4" (KRunner's categories) — and "Inside
//!   files": what SAYS the words (`file_inside.rs`). A shelf with nothing on it
//!   folds away, its room the first's (`content::fold_shelves`). Under each
//!   name, when it was used (Windows' "25 minutes ago").
//! - Ctrl+↑/↓ jump between shelves; Tab on a FOLDER chosen keeps the search
//!   to it (Spotlight's Tab to a place); Ctrl+Space or Ctrl+click PICKS files
//!   to act on together, and Shift opens the row of what is picked (Alfred's
//!   file buffer); Space shows the chosen one big; Ctrl+Enter shows it in
//!   Files.
//! - The three sections ARE the shelves: retitled, every cell, picture, key,
//!   page and drag works as it always did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::info;
use waverunner_core::index::AppEntry;

use crate::file_search::{self, Kind, Narrow};
use crate::i18n::{fill, tr};
use crate::{apps, content, launch, App};

/// A TAB of the deep search: what a search keeps to.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Tag {
    #[default]
    All,
    Kind(Kind),
    /// A file type, by its ending (no dot).
    Ext(String),
    /// One of the owner's folders.
    Place(PathBuf),
}

impl Tag {
    /// Its key in the tabs' use.
    fn key(&self) -> String {
        match self {
            Tag::All => "all".to_owned(),
            Tag::Kind(k) => format!("kind:{}", kind_title(Some(*k))),
            Tag::Ext(e) => format!("ext:{e}"),
            Tag::Place(p) => format!("place:{}", p.display()),
        }
    }

    /// Its word on the tab: "PDF", ".html", "Downloads/".
    pub(crate) fn label(&self) -> String {
        match self {
            Tag::All => tr("All").to_owned(),
            Tag::Kind(k) => tr(kind_title(Some(*k))).to_owned(),
            Tag::Ext(e) => format!(".{e}"),
            Tag::Place(p) => format!("{}/", p.file_name().map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy())),
        }
    }
}

/// Tabs a page.
pub(crate) const TABS_PAGE: usize = 9;

/// The tabs as they first stand (`places`: the owner's folders by key): the
/// kinds and the downloads, then file types and the other places — what is
/// used moves up from there.
fn first_tabs(places: &[(&'static str, PathBuf)]) -> Vec<Tag> {
    let place = |key: &str| places.iter().find(|(k, _)| *k == key).map(|(_, p)| Tag::Place(p.clone()));
    let ext = |e: &str| Some(Tag::Ext(e.to_owned()));
    [
        Some(Tag::All),
        Some(Tag::Kind(Kind::Document)),
        Some(Tag::Kind(Kind::Pdf)),
        Some(Tag::Kind(Kind::Picture)),
        Some(Tag::Kind(Kind::Video)),
        Some(Tag::Kind(Kind::Music)),
        Some(Tag::Kind(Kind::Folder)),
        place("DOWNLOAD"),
        ext("html"),
        place("DESKTOP"),
        Some(Tag::Kind(Kind::Archive)),
        ext("txt"),
        ext("md"),
        ext("docx"),
        ext("xlsx"),
        ext("pptx"),
        ext("odt"),
        ext("csv"),
        ext("png"),
        ext("jpg"),
        ext("gif"),
        ext("svg"),
        ext("mp4"),
        ext("mp3"),
        ext("zip"),
        ext("json"),
        place("SCREENSHOTS"),
        place("DOCUMENTS"),
        place("PICTURES"),
        place("MUSIC"),
        place("VIDEOS"),
        ext("py"),
        ext("rs"),
        ext("sh"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// `tabs` in the order of use (`score` by key): All first, then the more
/// used, the rest as they stood.
fn by_use(mut tabs: Vec<Tag>, score: &HashMap<String, (f64, f64)>) -> Vec<Tag> {
    let worth = |t: &Tag| if *t == Tag::All { f64::INFINITY } else { score.get(&t.key()).map_or(0.0, |&(s, _)| s) };
    // (A stable sort: equal worth keeps the first order.)
    tabs.sort_by(|a, b| worth(b).total_cmp(&worth(a)));
    tabs
}

/// The chip after the kinds, and the last cell of the box's Files while it
/// searches: every file found, in Golem Files.
pub(crate) const SHOW_ALL: &str = "Show all in Files";
/// That cell's entry id.
pub(crate) const SHOW_ALL_ID: &str = "golem:show-all-files";

/// How many files a deep search ranks.
const DEEP_MAX: usize = 400;
/// How many go on one shelf (three pages at six across; the pictures of
/// every cell shown fit the thumbnailer's ring, `thumbs::THUMB_CAP`).
const SHELF_MAX: usize = 36;
/// How many recent and new files the shelves hold before a word.
const BEFORE_MAX: usize = 24;
/// How many files the text inside can bring.
const INSIDE_MAX: usize = 48;
/// How long after the last letter the text inside is read.
const INSIDE_WAIT: Duration = Duration::from_millis(280);
/// How old the papers' read text may be before it is brought up to date.
const INSIDE_STALE: Duration = Duration::from_secs(30 * 60);

/// One shelf: its title and whether it is folded away.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shelf {
    pub title: String,
    pub hidden: bool,
}

/// The deep search, while it is up.
#[derive(Debug, Default)]
pub(crate) struct Deep {
    /// The tab chosen (All: none) — a kind or a type; a place is `scope`.
    pub tag: Tag,
    /// The folder the search keeps to (a place's tab, or Tab on a folder),
    /// and whether a tab set it (Tab walking the tabs lets go of it).
    pub scope: Option<PathBuf>,
    pub scope_tab: bool,
    /// The tabs in their order (made as "/" opens), their words' widths at
    /// the tabs' size, the page shown, and its wheel.
    pub tags: Vec<Tag>,
    pub tab_w: Vec<f32>,
    pub page: usize,
    pub pager: crate::pager::Pager,
    /// The tab what is typed lit last (its page shown once, as it comes).
    pub typed_was: Option<usize>,
    /// The files picked to act on together (their paths), and their cells.
    pub marked: Vec<String>,
    pub marked_cells: Vec<usize>,
    /// What each section shows.
    pub shelves: [Shelf; content::N_SECTIONS],
    /// When each file shown was used, under its name (by entry).
    pub subs: HashMap<usize, String>,
    /// The text inside: the words (and the folder) the found are for, what
    /// they found.
    pub inside: Inside,
}

/// The reading of the text inside (`file_inside::find`).
#[derive(Debug, Default)]
pub(crate) struct Inside {
    /// What was read for: the words, after the folder kept to (`inside_key`).
    pub words: String,
    pub found: Vec<String>,
    /// A reading on its way, and what to read when it is done.
    pub asking: bool,
    pub next: Option<String>,
    /// The wait before reading: armed, and until when (each letter moves it).
    pub timer: bool,
    pub due: Option<Instant>,
}

/// What a reading of the text inside is for: the folder kept to and the
/// words (the same words in another folder are read again).
fn inside_key(scope: Option<&Path>, words: &str) -> String {
    format!("{}\u{1}{words}", scope.map(|s| s.to_string_lossy()).unwrap_or_default())
}

/// The words of a reading's key.
fn key_words(key: &str) -> &str {
    key.split_once('\u{1}').map_or(key, |(_, w)| w)
}

/// A file a shelf shows.
struct Found {
    path: String,
    name: String,
    is_dir: bool,
    when: f64,
    size: u64,
}

/// A size, short: "8.4 MB", "1.2 GB" (from a megabyte; less says nothing).
pub(crate) fn big(size: u64) -> Option<String> {
    let mb = size as f64 / (1024.0 * 1024.0);
    if mb < 1.0 {
        return None;
    }
    Some(if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else if mb >= 100.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") })
}

/// What a kind is called on its shelf and chip.
fn kind_title(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::Pdf) => "PDF",
        Some(Kind::Picture) => "Pictures",
        Some(Kind::Video) => "Videos",
        Some(Kind::Music) => "Music",
        Some(Kind::Document) => "Documents",
        Some(Kind::Archive) => "Archives",
        Some(Kind::Folder) => "Folders",
        None => "Other",
    }
}

/// A shelf's title (in the shell's words) with how many it holds: "PDF · 4".
fn counted(title: &str, n: usize) -> String {
    format!("{title} · {n}")
}

/// When something was used, short: "now", "5 min", "3 h", "yesterday",
/// "4 d", then the day ("12/10" in Spanish, "10/12" in English).
pub(crate) fn ago(when: f64, now: f64) -> String {
    if when <= 0.0 {
        return String::new();
    }
    let secs = (now - when).max(0.0);
    let today = file_search::local_midnight(now);
    if secs < 60.0 {
        return tr("now").to_string();
    }
    if secs < 3600.0 {
        return fill(tr("{n} min"), &[("n", &((secs / 60.0) as u64).to_string())]);
    }
    if when >= today {
        return fill(tr("{n} h"), &[("n", &((secs / 3600.0) as u64).to_string())]);
    }
    if when >= today - 86_400.0 {
        return tr("yesterday").to_string();
    }
    let days = ((today - when) / 86_400.0).ceil() as u64;
    if days < 7 {
        return fill(tr("{n} d"), &[("n", &days.to_string())]);
    }
    // SAFETY: `localtime_r` writes only into the `tm` we give it.
    let (day, month, year) = unsafe {
        let t = when as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_mday, tm.tm_mon + 1, tm.tm_year + 1900)
    };
    let this_year = {
        // SAFETY: as above.
        unsafe {
            let t = now as libc::time_t;
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            tm.tm_year + 1900
        }
    };
    let spanish = crate::i18n::lang().is_some_and(|l| l.starts_with("es"));
    let (a, b) = if spanish { (day, month) } else { (month, day) };
    if year == this_year { format!("{a}/{b}") } else { format!("{a}/{b}/{}", year % 100) }
}

impl App {
    /// "/" typed into an empty search: the deep search comes up.
    pub(crate) fn deep_enter(&mut self) {
        info!("deep search: up");
        // (The tabs in the order of use, made now — never under the pointer;
        // a type typed and used once is one of them from then on.)
        let uses = self.search.tab_uses.snapshot();
        let mut tags = first_tabs(&file_search::user_places());
        let mut typed: Vec<&String> = uses.keys().filter(|k| k.starts_with("ext:")).collect();
        typed.sort();
        for key in typed {
            let tag = Tag::Ext(key["ext:".len()..].to_owned());
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        let tags = by_use(tags, &uses);
        self.search.deep = Some(Deep { tags, ..Default::default() });
        self.search.open = true;
        self.search.query.clear();
        self.search.focus = None;
        self.search.focus_file = None;
        self.search.selected = None;
        self.search.chosen = false;
        self.inside_refresh();
        self.refilter();
        self.schedule_frame();
    }

    /// The deep search goes: the box as it was, nothing typed.
    pub(crate) fn deep_leave(&mut self) {
        if self.search.deep.take().is_none() {
            return;
        }
        info!("deep search: down");
        self.search.query.clear();
        self.search.focus_file = None;
        self.search.selected = None;
        self.search.open = false;
        self.refilter();
        // (The card at the box's own height at once: the folded shelves'
        // room it gave back would show the sections over each other easing.)
        let (trim, _) = self.apps_trim_target();
        self.ui.set_open_trim(trim);
        self.schedule_frame();
    }

    /// The deep search's words for the pill's chip: "Files", "Files ›
    /// Facturas".
    pub(crate) fn deep_chip_text(&self) -> Option<String> {
        let d = self.search.deep.as_ref()?;
        Some(match &d.scope {
            Some(dir) => format!("{} › {}", tr("Files"), dir.file_name().map_or_else(|| dir.to_string_lossy(), |n| n.to_string_lossy())),
            None => tr("Files").to_string(),
        })
    }

    /// The tabs' words measured (as many as there are), and — what is typed
    /// asking for a type there is no tab of — a tab made for it; the page
    /// turned to the tab lit as what is typed lights another.
    pub(crate) fn deep_tabs(&mut self) {
        let q = file_search::parse(&self.search.query);
        // (A type some file has: ".g" on the way to ".gif" makes none.)
        let real = q.ext.filter(|e| self.file_index.iter().any(|f| !f.is_dir && file_search::has_ext(&f.name, e)));
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        if let Some(ext) = real.filter(|e| !d.tags.contains(&Tag::Ext(e.clone()))) {
            info!("deep search: a tab for .{ext}");
            d.tags.push(Tag::Ext(ext));
        }
        if d.tab_w.len() != d.tags.len() {
            let labels: Vec<String> = d.tags.iter().map(Tag::label).collect();
            let widths: Vec<f32> = labels
                .iter()
                .map(|l| match self.renderer.as_ref() {
                    Some(r) => r.measure_text(l, content::TITLE_FONT_PX, Some(content::FONT_BOLD)),
                    None => crate::options::est_text_w(l, content::TITLE_FONT_PX),
                })
                .collect();
            if let Some(d) = self.search.deep.as_mut() {
                d.tab_w = widths;
            }
        }
        // (Typing lit a tab on another page: that page — once, as it comes.)
        let typed = self.deep_typed_tab();
        if let Some(d) = self.search.deep.as_mut() {
            if typed != d.typed_was {
                if let Some(i) = typed {
                    d.page = i / TABS_PAGE;
                }
                d.typed_was = typed;
            }
        }
    }

    /// The tab what is typed asks for, while no tab is chosen: a type's
    /// ending, else a kind word.
    fn deep_typed_tab(&self) -> Option<usize> {
        let d = self.search.deep.as_ref().filter(|d| d.tag == Tag::All)?;
        let q = file_search::parse(&self.search.query);
        let want = q.ext.map(Tag::Ext).or(q.kind.map(Tag::Kind))?;
        d.tags.iter().position(|t| *t == want)
    }

    /// The tabs lit: the one chosen (else the one typed), the place kept to;
    /// All when neither.
    pub(crate) fn deep_lit(&self) -> Vec<usize> {
        let Some(d) = self.search.deep.as_ref() else {
            return Vec::new();
        };
        let mut lit: Vec<usize> = Vec::new();
        let chosen = (d.tag != Tag::All).then(|| d.tags.iter().position(|t| *t == d.tag)).flatten();
        lit.extend(chosen.or_else(|| self.deep_typed_tab()));
        if let Some(s) = &d.scope {
            lit.extend(d.tags.iter().position(|t| *t == Tag::Place(s.clone())));
        }
        if lit.is_empty() {
            lit.push(0);
        }
        lit
    }

    /// A tab clicked: on, or off again (a place: kept to, or let go); it is
    /// used — it moves up the next time.
    pub(crate) fn deep_tab(&mut self, i: usize) {
        let Some(tag) = self.search.deep.as_ref().and_then(|d| d.tags.get(i).cloned()) else {
            return;
        };
        if let Some(d) = self.search.deep.as_mut() {
            match &tag {
                Tag::Place(p) if d.scope.as_ref() == Some(p) => {
                    d.scope = None;
                    d.scope_tab = false;
                }
                Tag::Place(p) => {
                    d.scope = Some(p.clone());
                    d.scope_tab = true;
                }
                Tag::All => {
                    d.tag = Tag::All;
                    if d.scope_tab {
                        d.scope = None;
                        d.scope_tab = false;
                    }
                }
                t if d.tag == *t => d.tag = Tag::All,
                t => d.tag = t.clone(),
            }
            info!("deep search: tab {:?} (scope {:?})", d.tag, d.scope);
        }
        if tag != Tag::All {
            self.search.tab_uses.increment(&tag.key());
        }
        self.search.selected = None;
        self.search.chosen = false;
        self.refilter();
        self.schedule_frame();
    }

    /// Tab walking the tabs: the next one (or the one before) chosen alone,
    /// its page shown.
    fn deep_tab_walk(&mut self, back: bool) {
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        let n = d.tags.len().max(1);
        let at = self.deep_lit().first().copied().unwrap_or(0);
        let next = if back { (at + n - 1) % n } else { (at + 1) % n };
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        match d.tags.get(next).cloned().unwrap_or_default() {
            Tag::Place(p) => {
                d.tag = Tag::All;
                d.scope = Some(p);
                d.scope_tab = true;
            }
            t => {
                d.tag = t;
                if d.scope_tab {
                    d.scope = None;
                    d.scope_tab = false;
                }
            }
        }
        d.page = next / TABS_PAGE;
        self.search.selected = None;
        self.search.chosen = false;
        self.refilter();
        self.schedule_frame();
    }

    /// The tabs' page turned (the wheel over them): `dir` +1 the next.
    pub(crate) fn deep_tab_page(&mut self, dir: i64) {
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        let pages = d.tags.len().div_ceil(TABS_PAGE).max(1);
        d.page = (d.page as i64 + dir).rem_euclid(pages as i64) as usize;
        self.schedule_frame();
    }

    /// The wheel over the tabs: their pages. Whether it was over them.
    pub(crate) fn deep_tabs_wheel(&mut self, value: f64) -> bool {
        let over = self.search.deep.is_some()
            && self.pointer_pos.is_some_and(|p| self.current_layout().tabs_row.is_some_and(|r| r.contains(p)));
        if !over {
            return false;
        }
        let dir = self.search.deep.as_mut().and_then(|d| d.pager.wheel(value));
        if let Some(dir) = dir {
            self.deep_tab_page(dir);
        }
        true
    }

    /// A file opened from the deep search: the tabs lit were used.
    pub(crate) fn deep_used(&mut self) {
        let keys: Vec<String> = match self.search.deep.as_ref() {
            Some(d) => self.deep_lit().into_iter().filter_map(|i| d.tags.get(i)).filter(|t| **t != Tag::All).map(Tag::key).collect(),
            None => return,
        };
        for k in keys {
            self.search.tab_uses.increment(&k);
        }
    }

    /// The shelves (the three sections' lists), what each is called, and
    /// when each file was used.
    pub(crate) fn deep_results(&mut self) -> [Vec<usize>; content::N_SECTIONS] {
        let mut out: [Vec<usize>; content::N_SECTIONS] = Default::default();
        let Some(d) = self.search.deep.as_ref() else {
            return out;
        };
        let (tag, scope) = (d.tag.clone(), d.scope.clone());
        let mut shelves: [Shelf; content::N_SECTIONS] = Default::default();
        // A file Shift took to its row: that row, on the first shelf.
        if self.search.focus_file.is_some() {
            shelves[0].title = tr("Files").to_string();
            shelves[1].hidden = true;
            shelves[2].hidden = true;
            if let Some(d) = self.search.deep.as_mut() {
                d.shelves = shelves;
                d.subs.clear();
                d.marked_cells.clear();
            }
            return out;
        }
        let q = file_search::parse(&self.search.query);
        let now = crate::usage::now_secs();
        let mut lists: Vec<(String, Vec<Found>)> = Vec::new();
        let mut places = false;
        if !q.asks() && tag == Tag::All && scope.is_none() {
            // BEFORE A WORD: used lately, come lately, the places.
            let recent = self.deep_recent();
            let fresh = self.deep_fresh(&recent);
            // (None used or come lately: the next shelf moves up.)
            for (title, list) in [(tr("Recent"), recent), (tr("New files"), fresh)] {
                if !list.is_empty() {
                    lists.push((title.to_string(), list));
                }
            }
            places = true;
        } else {
            let narrow = Narrow {
                kind: if let Tag::Kind(k) = tag { Some(k) } else { None },
                ext: if let Tag::Ext(e) = &tag { Some(e.clone()) } else { None },
                scope: scope.clone(),
            };
            let ranked = self.file_ranker.rank_narrow(&self.file_index, &q, &narrow, &self.file_opened, now, DEEP_MAX);
            let found: Vec<Found> = ranked
                .into_iter()
                .filter_map(|i| self.file_index.get(i))
                .map(|f| Found {
                    path: f.path.clone(),
                    name: f.name.clone(),
                    is_dir: f.is_dir,
                    when: f.mtime.max(self.file_opened.get(&f.path).copied().unwrap_or(0.0)),
                    size: f.size,
                })
                .collect();
            let inside = self.deep_inside_shelf(&q, &narrow, &found);
            if tag != Tag::All || q.ext.is_some() || q.words.is_empty() && q.kind.is_none() {
                // One kind or type (or a folder, a time, a size): one shelf.
                let title = match (&tag, &q.ext, &scope) {
                    (Tag::All, Some(e), _) => Tag::Ext(e.clone()).label(),
                    (Tag::All, None, Some(dir)) => dir.file_name().map_or_else(|| tr("Files").to_owned(), |n| n.to_string_lossy().into_owned()),
                    (Tag::All, None, None) => tr("Files").to_owned(),
                    (t, _, _) => t.label(),
                };
                let n = found.len();
                lists.push((counted(&title, n), found));
            } else {
                // A shelf a kind, the kind with the best match first.
                let mut groups: Vec<(Option<Kind>, Vec<Found>)> = Vec::new();
                for f in found {
                    let k = file_search::kind_of(&f.name, f.is_dir);
                    match groups.iter_mut().find(|(g, _)| *g == k) {
                        Some((_, list)) => list.push(f),
                        None => groups.push((k, vec![f])),
                    }
                }
                for (k, list) in groups {
                    let n = list.len();
                    lists.push((counted(tr(kind_title(k)), n), list));
                }
            }
            if !inside.is_empty() {
                let n = inside.len();
                let at = lists.len().min(2);
                lists.insert(at, (counted(tr("Inside files"), n), inside));
            }
            if lists.is_empty() {
                lists.push((tr("Files").to_string(), Vec::new()));
            }
        }
        // Onto the shelves: each file once, at most a shelf's worth.
        let mut subs = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        let filled = lists.len().min(content::N_SECTIONS);
        for (s, (title, list)) in lists.into_iter().take(content::N_SECTIONS).enumerate() {
            shelves[s].title = title;
            for f in list.into_iter().filter(|f| seen.insert(f.path.clone())).take(SHELF_MAX) {
                let exec = format!("xdg-open {}", launch::shell_quote(&f.path));
                let entry = self.push_transient_file(&f.path, &f.name, exec, f.is_dir);
                // When it was used, and how big when that says something.
                let sub = match (ago(f.when, now), big(f.size).filter(|_| !f.is_dir)) {
                    (w, Some(b)) if !w.is_empty() => format!("{w} · {b}"),
                    (w, Some(b)) if w.is_empty() => b,
                    (w, _) => w,
                };
                if !sub.is_empty() {
                    subs.insert(entry, sub);
                }
                out[s].push(entry);
            }
        }
        if places {
            let at = filled.min(content::N_SECTIONS - 1);
            shelves[at].title = tr("Places").to_string();
            out[at] = self.home_listing();
        } else if q.asks() && !out[0].is_empty() {
            // (Then every file found, in Golem Files.)
            out[0].push(self.show_all_cell());
        }
        for (s, shelf) in shelves.iter_mut().enumerate() {
            if s > 0 && out[s].is_empty() {
                shelf.hidden = true;
            }
        }
        if shelves[0].title.is_empty() {
            shelves[0].title = tr("Files").to_string();
        }
        let marked: Vec<usize> = match self.search.deep.as_ref() {
            Some(d) => out.iter().flatten().copied().filter(|&e| self.entries.get(e).is_some_and(|x| d.marked.contains(&x.id))).collect(),
            None => Vec::new(),
        };
        if let Some(d) = self.search.deep.as_mut() {
            d.shelves = shelves;
            d.subs = subs;
            d.marked_cells = marked;
        }
        out
    }

    /// What was opened lately — the system's record and what the box opened
    /// — still there, the latest first.
    fn deep_recent(&self) -> Vec<Found> {
        let mut used: Vec<(&String, f64)> = self.file_opened.iter().map(|(p, &t)| (p, t)).collect();
        used.sort_by(|a, b| b.1.total_cmp(&a.1));
        used.into_iter()
            .filter_map(|(path, when)| {
                let p = Path::new(path);
                let meta = std::fs::metadata(p).ok()?;
                let name = p.file_name()?.to_string_lossy().into_owned();
                (!name.starts_with('.')).then(|| Found { path: path.clone(), name, is_dir: meta.is_dir(), when, size: meta.len() })
            })
            .take(BEFORE_MAX)
            .collect()
    }

    /// What came lately: the watched folders' tops (Downloads, Desktop,
    /// Screenshots…), the newest first, but what is recent already.
    fn deep_fresh(&self, recent: &[Found]) -> Vec<Found> {
        let mut new: Vec<&apps::FileEntry> = self
            .file_index
            .iter()
            .filter(|f| !f.is_dir && Path::new(&f.path).parent().is_some_and(|p| self.file_watched.iter().any(|d| d.as_path() == p)))
            .filter(|f| !recent.iter().any(|r| r.path == f.path))
            .collect();
        new.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        new.into_iter()
            .take(BEFORE_MAX)
            .map(|f| Found { path: f.path.clone(), name: f.name.clone(), is_dir: false, when: f.mtime, size: f.size })
            .collect()
    }

    /// The text inside, for these words: what was read for them (asked if
    /// not yet), kept to the kind and the folder, without what the names
    /// found already.
    fn deep_inside_shelf(&mut self, q: &file_search::Query, narrow: &Narrow, named: &[Found]) -> Vec<Found> {
        let words = q.words.join(" ");
        if words.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
            return Vec::new();
        }
        let key = inside_key(narrow.scope.as_deref(), &words);
        let have = self.search.deep.as_ref().map(|d| (d.inside.words.clone(), d.inside.found.clone()));
        let Some((read_for, found)) = have else {
            return Vec::new();
        };
        if read_for != key {
            self.deep_inside_soon(key);
            return Vec::new();
        }
        found
            .into_iter()
            .filter(|p| !named.iter().any(|f| f.path == *p))
            .filter(|p| narrow.scope.as_ref().is_none_or(|s| Path::new(p).starts_with(s)))
            .filter_map(|path| {
                let p = Path::new(&path);
                let meta = std::fs::metadata(p).ok()?;
                let name = p.file_name()?.to_string_lossy().into_owned();
                let kind = file_search::kind_of(&name, meta.is_dir());
                let keep = narrow.kind.is_none_or(|k| kind == Some(k) || (k == Kind::Document && kind == Some(Kind::Pdf)));
                let when = file_search::mtime_of(&meta).max(self.file_opened.get(&path).copied().unwrap_or(0.0));
                keep.then_some(Found { path, name, is_dir: meta.is_dir(), when, size: meta.len() })
            })
            .collect()
    }

    /// Read the text inside for `key` (`inside_key`), once the letters stop
    /// for `INSIDE_WAIT` (one reading at a time: what is asked meanwhile is
    /// read after it).
    fn deep_inside_soon(&mut self, key: String) {
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        d.inside.next = Some(key);
        d.inside.due = Some(Instant::now() + INSIDE_WAIT);
        if d.inside.timer || d.inside.asking {
            return;
        }
        d.inside.timer = true;
        let armed = self.loop_handle.insert_source(Timer::from_duration(INSIDE_WAIT), |_, _, app: &mut App| {
            let Some(d) = app.search.deep.as_mut() else {
                return TimeoutAction::Drop;
            };
            // (A letter since: wait on from it.)
            if let Some(due) = d.inside.due.filter(|&due| due > Instant::now()) {
                return TimeoutAction::ToInstant(due);
            }
            d.inside.timer = false;
            app.deep_inside_read();
            TimeoutAction::Drop
        });
        if armed.is_err() {
            if let Some(d) = self.search.deep.as_mut() {
                d.inside.timer = false;
            }
        }
    }

    /// The reading itself, off the loop (`file_inside::find`); what it found
    /// shown when the words are still those.
    fn deep_inside_read(&mut self) {
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        let Some(key) = d.inside.next.take() else {
            return;
        };
        d.inside.asking = true;
        let roots: Vec<PathBuf> = match &d.scope {
            Some(s) => vec![s.clone()],
            None => self.file_watched.clone(),
        };
        let list: Vec<String> = key_words(&key).split_whitespace().map(str::to_owned).collect();
        let words = key;
        let started = Instant::now();
        self.desktop_off_loop(
            move || crate::file_inside::find(&list, &roots, &crate::file_inside::cache_dir(), INSIDE_MAX),
            move |app, found| {
                let Some(d) = app.search.deep.as_mut() else {
                    return;
                };
                info!("deep search: {} inside for {words:?} in {} ms", found.len(), started.elapsed().as_millis());
                d.inside.asking = false;
                d.inside.words = words.clone();
                d.inside.found = found;
                let again = d.inside.next.as_ref().is_some_and(|n| *n != words);
                if again {
                    app.deep_inside_read();
                    return;
                }
                d.inside.next = None;
                // (Not while the owner walks what is shown, or looks at one.)
                let scope = app.search.deep.as_ref().and_then(|d| d.scope.clone());
                let now_key = inside_key(scope.as_deref(), &file_search::parse(&app.search.query).words.join(" "));
                if now_key == words && !app.search.chosen && !app.box_peeking() {
                    app.refilter();
                    app.schedule_frame();
                }
            },
        );
    }

    /// The papers' text brought up to date (`file_inside::refresh`), off the
    /// loop, when it is older than `INSIDE_STALE` or was never read.
    pub(crate) fn inside_refresh(&mut self) {
        if self.inside_read.is_some_and(|t| t.elapsed() < INSIDE_STALE) || self.file_index.is_empty() {
            return;
        }
        self.inside_read = Some(Instant::now());
        let papers: Vec<(String, f64, u64)> = self
            .file_index
            .iter()
            .filter(|f| !f.is_dir && crate::file_inside::PAPER_ENDINGS.contains(&crate::file_inside::ending(&f.path).as_str()))
            .map(|f| (f.path.clone(), f.mtime, f.size))
            .collect();
        info!("inside files: {} papers to keep read", papers.len());
        self.desktop_off_loop(move || crate::file_inside::refresh(papers), |_, _| {});
    }

    /// The keys the deep search takes (after the lone app's and Quick
    /// Look's): whether it took this one.
    pub(crate) fn deep_key(&mut self, keysym: Keysym) -> bool {
        if self.search.deep.is_none() || self.control_panel {
            return false;
        }
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        match keysym {
            Keysym::Escape => {
                let picked = self.search.deep.as_ref().is_some_and(|d| !d.marked.is_empty());
                if picked {
                    if let Some(d) = self.search.deep.as_mut() {
                        d.marked.clear();
                        d.marked_cells.clear();
                    }
                    self.schedule_frame();
                } else {
                    self.deep_leave();
                }
                true
            }
            Keysym::BackSpace if self.search.query.is_empty() => {
                let Some(d) = self.search.deep.as_mut() else {
                    return false;
                };
                if d.scope.take().is_none() && std::mem::take(&mut d.tag) == Tag::All {
                    self.deep_leave();
                    return true;
                }
                d.scope_tab = false;
                self.refilter();
                self.schedule_frame();
                true
            }
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                // A FOLDER chosen: the search keeps to it (what was typed goes).
                if !shift && keysym == Keysym::Tab && self.search.chosen {
                    if let Some(dir) = self.chosen_file().and_then(|e| self.entries.get(e)).map(|e| PathBuf::from(&e.id)).filter(|p| p.is_dir()) {
                        info!("deep search: kept to {}", dir.display());
                        if let Some(d) = self.search.deep.as_mut() {
                            d.scope = Some(dir);
                        }
                        self.search.query.clear();
                        self.search.selected = None;
                        self.search.chosen = false;
                        self.refilter();
                        self.schedule_frame();
                        return true;
                    }
                }
                // Else the next tab (Shift: the one before), round.
                self.deep_tab_walk(shift || keysym == Keysym::ISO_Left_Tab);
                true
            }
            Keysym::Up | Keysym::Down if ctrl => {
                self.deep_jump(keysym == Keysym::Down);
                true
            }
            Keysym::space if ctrl => {
                if let Some(e) = self.chosen_file().or_else(|| self.selected_file()) {
                    self.deep_mark(e);
                }
                true
            }
            _ => false,
        }
    }

    /// The file the selection is on, chosen or not.
    fn selected_file(&self) -> Option<usize> {
        let (s, cell) = self.search.selected.and_then(|f| self.flat_to_pos(f))?;
        let e = *self.search.visible[s].get(cell)?;
        (self.kinds.get(e) == Some(&apps::EntryKind::File) && self.entries.get(e).is_some_and(|x| x.id.starts_with('/'))).then_some(e)
    }

    /// The selection to the first of the next shelf down (or up).
    fn deep_jump(&mut self, down: bool) {
        let lens: Vec<usize> = self.search.visible.iter().map(Vec::len).collect();
        let starts: Vec<usize> = lens.iter().scan(0, |acc, &n| {
            let at = *acc;
            *acc += n;
            Some(at)
        }).collect();
        let cur = self.search.selected.and_then(|f| self.flat_to_pos(f)).map_or(0, |(s, _)| s);
        let order: Vec<usize> = if down { (cur + 1..lens.len()).collect() } else { (0..cur).rev().collect() };
        if let Some(s) = order.into_iter().find(|&s| lens[s] > 0) {
            self.search.selected = Some(starts[s]);
            self.search.keyed = true;
            self.search.chosen = true;
            let layout = self.current_layout();
            self.scroll.per[s].target = content::scroll_to_reveal(&layout.sections[s], 0);
            self.box_peek_follow();
            self.schedule_frame();
        }
    }

    /// File `entry` picked to act on together, or let go.
    pub(crate) fn deep_mark(&mut self, entry: usize) {
        let Some(path) = self.entries.get(entry).map(|e| e.id.clone()).filter(|p| p.starts_with('/')) else {
            return;
        };
        let Some(d) = self.search.deep.as_mut() else {
            return;
        };
        match d.marked.iter().position(|p| *p == path) {
            Some(k) => {
                d.marked.remove(k);
                d.marked_cells.retain(|&e| e != entry);
            }
            None => {
                d.marked.push(path);
                d.marked_cells.push(entry);
            }
        }
        info!("deep search: {} picked", d.marked.len());
        self.schedule_frame();
    }

    /// The files picked, while the deep search is up.
    pub(crate) fn deep_picked(&self) -> Vec<String> {
        self.search.deep.as_ref().map(|d| d.marked.clone()).unwrap_or_default()
    }

    /// Every file found, in Golem Files: its search, in the folder kept to.
    pub(crate) fn deep_show_all(&mut self) {
        let scope = self.search.deep.as_ref().and_then(|d| d.scope.clone());
        let words = file_search::parse(&self.search.query).all_or_words();
        self.files_show_all(&words, scope);
    }

    /// Golem Files on `dir` (home) searching `words` — and below, as its
    /// search goes on below.
    pub(crate) fn files_show_all(&mut self, words: &str, dir: Option<PathBuf>) {
        let dir = dir.unwrap_or_else(crate::filer::home);
        info!("show all in Files: {words:?} in {}", dir.display());
        self.search.query.clear();
        self.search.open = false;
        self.search.deep = None;
        self.refilter();
        self.dismiss();
        self.filer_open(dir);
        if let Some(id) = self.filer.newest() {
            self.filer_find(id, Some(words.to_owned()));
        }
    }

    /// The last cell of the box's Files while it searches (and the deep
    /// search's last chip): every file found, in Golem Files — with Files'
    /// own picture.
    pub(crate) fn show_all_cell(&mut self) -> usize {
        let files = self.entries[..self.base_len.min(self.entries.len())].iter().position(|e| e.id == crate::filer::APP_ID);
        let (placeholder, layer) = files.map_or((true, 0), |i| (self.placeholders.get(i).copied().unwrap_or(true), self.icon_layers.get(i).copied().unwrap_or(0)));
        let cell = AppEntry {
            id: SHOW_ALL_ID.to_owned(),
            name: tr(SHOW_ALL).to_string(),
            description: None,
            exec: String::new(),
            icon: None,
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        };
        self.push_transient(cell, apps::EntryKind::App, placeholder, layer)
    }
}

impl file_search::Query {
    /// The words as typed, the kind's too.
    pub(crate) fn all_or_words(&self) -> String {
        if self.all.is_empty() { self.words.join(" ") } else { self.all.join(" ") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_reads_short_and_near() {
        let now = 1_800_000_000.0;
        assert_eq!(ago(now - 30.0, now), "now");
        assert_eq!(ago(now - 600.0, now), "10 min");
        assert_eq!(ago(0.0, now), "");
        let midnight = file_search::local_midnight(now);
        assert_eq!(ago(midnight - 3600.0, now), "yesterday");
        assert_eq!(ago(midnight - 3.0 * 86_400.0 + 10.0, now), "3 d");
        assert!(ago(now - 40.0 * 86_400.0, now).contains('/'));
    }

    #[test]
    fn a_size_is_said_from_a_megabyte() {
        assert_eq!(big(500 << 10), None);
        assert_eq!(big(8_800_000).as_deref(), Some("8.4 MB"));
        assert_eq!(big(300 << 20).as_deref(), Some("300 MB"));
        assert_eq!(big(3 << 30).as_deref(), Some("3.0 GB"));
    }

    #[test]
    fn the_tabs_stand_kinds_first_and_move_up_with_use() {
        let places = vec![("DOWNLOAD", PathBuf::from("/h/Descargas")), ("DESKTOP", PathBuf::from("/h/Escritorio"))];
        let tabs = first_tabs(&places);
        assert_eq!(tabs[0], Tag::All);
        assert_eq!(tabs[7], Tag::Place(PathBuf::from("/h/Descargas")));
        assert_eq!(tabs[8], Tag::Ext("html".into()));
        assert!(tabs.len() > 3 * TABS_PAGE, "pages after the first");
        assert_eq!(Tag::Place(PathBuf::from("/h/Descargas")).label(), "Descargas/");
        assert_eq!(Tag::Ext("md".into()).label(), ".md");
        // Used: up the order, All still first; the rest as they stood.
        let score: HashMap<String, (f64, f64)> = [(Tag::Ext("gif".into()).key(), (3.0, 0.0)), (Tag::Kind(Kind::Music).key(), (1.0, 0.0))].into();
        let ordered = by_use(tabs.clone(), &score);
        assert_eq!(ordered[0], Tag::All);
        assert_eq!(ordered[1], Tag::Ext("gif".into()));
        assert_eq!(ordered[2], Tag::Kind(Kind::Music));
        assert_eq!(ordered[3], Tag::Kind(Kind::Document));
    }

    #[test]
    fn every_shelf_has_its_words() {
        assert_eq!(kind_title(Some(Kind::Pdf)), "PDF");
        assert_eq!(counted("PDF", 4), "PDF · 4");
        for k in [None, Some(Kind::Pdf), Some(Kind::Picture), Some(Kind::Video), Some(Kind::Music), Some(Kind::Document), Some(Kind::Archive), Some(Kind::Folder)] {
            assert!(!kind_title(k).is_empty());
        }
    }
}

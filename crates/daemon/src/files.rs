//! The Files section: the home-strip listing, directory navigation, and
//! the transient file entries search results ride on.

use std::time::Instant;

use calloop::timer::{TimeoutAction, Timer};
use tracing::{error, info};
use waverunner_core::index::AppEntry;

use crate::apps;
use crate::launch;
use crate::App;
use crate::BOUNCE_DURATION;

/// Cap on file-search results shown in the Files section.
pub(crate) const FILE_RESULTS_MAX: usize = 24;

/// Cap on entries listed when navigated into a directory.
pub(crate) const FILES_LIST_MAX: usize = 300;

/// Cap on a pinned directory's dock-stack listing (five 3×3 pages).
pub(crate) const DIR_STACK_MAX: usize = 45;

/// Entry id of the ".." tile leading a navigated listing — clicking it
/// goes up one level (back to the home strip from the top).
pub(crate) const FILES_UP_ID: &str = "files-up";

/// The icon-carrier asset for a file, by extension: media, documents,
/// archives and code each get their own themed icon; everything else
/// falls back to the generic file. (Thumbnails, where available,
/// override these — see `thumbs`.)
pub(crate) fn file_asset_name(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" | "mid" => "asset-audio",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => {
            "asset-video"
        }
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "tif" | "tiff" | "avif"
        | "heic" | "ico" | "xcf" | "kra" => "asset-image",
        "pdf" | "epub" | "djvu" => "asset-pdf",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "iso" => "asset-archive",
        "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx"
        | "odp" => "asset-doc",
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "hpp" | "go" | "sh" | "zsh" | "bash"
        | "lua" | "nix" | "toml" | "yaml" | "yml" | "json" | "html" | "css" | "scss" | "sql"
        | "vim" | "el" => "asset-code",
        _ => "asset-file",
    }
}

impl App {
    /// Append one transient Files-section entry (kind File, an icon by
    /// type — or its thumbnail once one is ready) and return its index.
    /// `id` is the path.
    pub(crate) fn push_transient_file(
        &mut self,
        id: &str,
        name: &str,
        exec: String,
        is_dir: bool,
    ) -> usize {
        let asset = if is_dir {
            "asset-folder"
        } else {
            file_asset_name(name)
        };
        // Without an asset icon, fall back to a letter-tile placeholder.
        let (mut layer, mut placeholder) = self.asset(asset).unwrap_or((0, true));
        // A finished thumbnail overrides the type icon; a thumbable file
        // without one queues a job (the worker answers via `on_thumb`).
        if !is_dir {
            if let Some(&(slot, _)) = self.thumb_map.get(id) {
                layer = self.thumb_layer_base() + slot as u32;
                placeholder = false;
            } else if crate::thumbs::thumbable(name) && self.thumb_pending.insert(id.to_owned()) {
                self.thumbs.request(id);
            }
        }
        let entry = AppEntry {
            id: id.to_owned(),
            name: name.to_owned(),
            description: Some(id.to_owned()),
            exec,
            icon: None,
            needs_terminal: false,
            path: None,
        };
        self.push_transient(entry, apps::EntryKind::File, placeholder, layer)
    }

    /// Rank the home-tree file index against the query and append the
    /// top matches as transient entries, returning their indices for
    /// the Files section.
    pub(crate) fn file_results(&mut self) -> Vec<usize> {
        let names: Vec<&str> = self.file_index.iter().map(|f| f.name.as_str()).collect();
        let ranked = self.search.matcher.rank(&self.search.query, &names);
        let mut out = Vec::new();
        for fi in ranked.into_iter().take(FILE_RESULTS_MAX) {
            let f = self.file_index[fi].clone();
            let exec = format!("xdg-open {}", launch::shell_quote(&f.path));
            out.push(self.push_transient_file(&f.path, &f.name, exec, f.is_dir));
        }
        out
    }

    /// Transient listing of the navigated directory's visible children —
    /// a ".." tile leading *every page* (the same entry interleaved at
    /// each page's first slot, so it stays put while paging: browsing
    /// exits through it), then folders, then files, each alphabetical.
    pub(crate) fn dir_listing(&mut self) -> Vec<usize> {
        let Some(dir) = self.files_dir.clone() else {
            return Vec::new();
        };
        let up = self.push_transient_file(FILES_UP_ID, "..", "true".to_owned(), true);
        let mut out = vec![up];
        let mut children: Vec<(bool, String, String)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let is_dir = e.file_type().ok()?.is_dir();
                Some((is_dir, name, e.path().to_string_lossy().into_owned()))
            })
            .collect();
        // Folders first, then files, each alphabetical. The ".." tile is
        // cell 0; the layout pins it as a static lead cell (the rest of
        // the listing pages beside it — see `SectionLayout::lead`).
        children.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (is_dir, name, path) in children.into_iter().take(FILES_LIST_MAX) {
            let exec = format!("xdg-open {}", launch::shell_quote(&path));
            out.push(self.push_transient_file(&path, &name, exec, is_dir));
        }
        out
    }

    /// Base texture layer of the reserved thumbnail block (past the app
    /// icons and both package blocks).
    pub(crate) fn thumb_layer_base(&self) -> u32 {
        self.pkg_layer_base + (crate::nix::RANK_HITS_MAX + crate::nix::PENDING_INSTALL_CAP) as u32
    }

    /// A finished thumbnail: park it in a reserved slot (round-robin,
    /// recycling the oldest), upload it, and repoint every entry for
    /// that path at it.
    pub(crate) fn on_thumb(&mut self, ev: crate::thumbs::Event) {
        self.thumb_pending.remove(&ev.path);
        let slot = match self.thumb_map.get(&ev.path) {
            Some(&(slot, _)) => slot,
            None => {
                let slot = self.thumb_next % crate::thumbs::THUMB_CAP;
                self.thumb_next += 1;
                // Recycled: the evicted path falls back to its type icon
                // (and re-queues if it comes on screen again).
                self.thumb_map.retain(|_, v| v.0 != slot);
                slot
            }
        };
        let layer = self.thumb_layer_base() + slot as u32;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.update_icon_layer(layer, &ev.pixels);
        }
        for (i, e) in self.entries.iter().enumerate() {
            if e.id == ev.path {
                self.icon_layers[i] = layer;
                self.placeholders[i] = false;
            }
        }
        self.thumb_map.insert(ev.path, (slot, ev.pixels));
        self.schedule_frame();
    }

    /// Re-upload every retained thumbnail after a rescan rebuilt the
    /// icon texture array.
    pub(crate) fn reupload_thumb_icons(&mut self) {
        let base = self.thumb_layer_base();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (slot, pixels) in self.thumb_map.values() {
            renderer.update_icon_layer(base + *slot as u32, pixels);
        }
    }

    /// Synthesize dock entries for pinned filesystem paths (dirs or files
    /// dragged from the Files section onto the dock): a path pin has no
    /// scanned entry behind it — listings are transient — so each
    /// refilter recreates one (folder/file icon, xdg-open exec) for the
    /// dock to render. Ids already covered by a live entry are skipped.
    pub(crate) fn pinned_path_entries(&mut self) {
        let paths: Vec<String> = self
            .pins
            .pins()
            .iter()
            .filter(|p| p.starts_with('/') && !self.entries.iter().any(|e| &e.id == *p))
            .cloned()
            .collect();
        for path in paths {
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let exec = format!("xdg-open {}", launch::shell_quote(&path));
            let is_dir = std::fs::metadata(&path)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            self.push_transient_file(&path, &name, exec, is_dir);
        }
    }

    /// Rebuild an open directory stack's listing: transient entries for
    /// the box overlay to render, and the paged member list (folders
    /// first, alphabetical — the same order the Files section uses).
    pub(crate) fn rebuild_dir_stack(&mut self) {
        let Some(path) = self.dir_stack.as_ref().map(|ds| ds.path.clone()) else {
            return;
        };
        let mut children: Vec<(bool, String, String)> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let is_dir = e.file_type().ok()?.is_dir();
                Some((is_dir, name, e.path().to_string_lossy().into_owned()))
            })
            .collect();
        children.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut ids = Vec::with_capacity(children.len().min(DIR_STACK_MAX));
        for (is_dir, name, cpath) in children.into_iter().take(DIR_STACK_MAX) {
            let exec = format!("xdg-open {}", launch::shell_quote(&cpath));
            self.push_transient_file(&cpath, &name, exec, is_dir);
            ids.push(cpath);
        }
        let mut members = crate::pages::PagedList::from_flat(ids);
        members.normalize(crate::groups::PAGE_CAP, |_| true);
        if let Some(ds) = &mut self.dir_stack {
            ds.members = members;
        }
    }

    /// Files-section navigation: returns true when the hit was a folder
    /// and the strip navigated into it (plain files fall through to the
    /// launch path). Clicking a folder in search results jumps there
    /// too, clearing the query.
    pub(crate) fn try_navigate(&mut self, entry_idx: usize) -> bool {
        if self.kinds.get(entry_idx) != Some(&apps::EntryKind::File) {
            return false;
        }
        // The ".." tile leads every navigated listing: go up a level.
        if self
            .entries
            .get(entry_idx)
            .is_some_and(|e| e.id == FILES_UP_ID)
        {
            self.files_nav_up();
            return true;
        }
        let Some(path) = self
            .entries
            .get(entry_idx)
            .and_then(|e| e.description.clone())
        else {
            return false;
        };
        if !std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return false;
        }
        self.files_dir = Some(std::path::PathBuf::from(path));
        // A folder clicked in search results jumps navigation there;
        // the query has done its job.
        self.search.query.clear();
        self.search.open = false;
        self.refilter();
        true
    }

    /// Go up one level from the navigated directory; reaching (or
    /// escaping) home lands back on the home-folder strip.
    pub(crate) fn files_nav_up(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        self.files_dir = self
            .files_dir
            .as_ref()
            .and_then(|d| d.parent().map(std::path::Path::to_path_buf))
            .filter(|p| {
                !home.is_empty() && p.starts_with(&home) && *p != std::path::Path::new(&home)
            });
        self.refilter();
    }

    /// The directory a Files-section entry stands for: the folder itself,
    /// or the containing folder of a plain file. `None` for non-File
    /// entries. (The path lives in `description`; the id is a path only
    /// for transient entries, home-strip folders use "folder-<name>".)
    pub(crate) fn entry_dir_path(&self, entry_idx: usize) -> Option<std::path::PathBuf> {
        if self.kinds.get(entry_idx) != Some(&apps::EntryKind::File) {
            return None;
        }
        let path = self.entries.get(entry_idx)?.description.as_deref()?;
        let path = std::path::Path::new(path);
        if std::fs::metadata(path).ok()?.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(std::path::Path::to_path_buf)
        }
    }

    /// Right-click on a Files cell: open a terminal in that directory
    /// (the folder itself, or a file's containing folder), with launch
    /// feedback and dismissal like any other activation.
    pub(crate) fn open_terminal_at(&mut self, entry_idx: usize) {
        let Some(dir) = self.entry_dir_path(entry_idx) else {
            return;
        };
        let dir = dir.to_string_lossy().into_owned();
        let exec = format!(
            "cd {} && exec {}",
            launch::shell_quote(&dir),
            self.config.launch.terminal
        );
        info!("terminal at {dir}");
        if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
            error!("terminal launch failed: {e:#}");
            return;
        }
        self.bounce = Some((entry_idx, Instant::now()));
        self.schedule_frame();
        let timer = Timer::from_duration(BOUNCE_DURATION);
        if self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.dismiss();
                TimeoutAction::Drop
            })
            .is_err()
        {
            self.dismiss();
        }
    }

    /// Display string of the navigated directory ("~/Documents/x"),
    /// empty at the top level.
    pub(crate) fn files_path_display(&self) -> String {
        let Some(dir) = &self.files_dir else {
            return String::new();
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let s = dir.to_string_lossy();
        match s.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => s.into_owned(),
        }
    }
}

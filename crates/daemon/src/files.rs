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

impl App {
    /// Append one transient Files-section entry (kind File, generic
    /// folder/file icon layer) and return its index. `id` is the path.
    pub(crate) fn push_transient_file(
        &mut self,
        id: &str,
        name: &str,
        exec: String,
        is_dir: bool,
    ) -> usize {
        let asset = if is_dir {
            self.asset_folder
        } else {
            self.asset_file
        };
        // Without an asset icon, fall back to a letter-tile placeholder.
        let (layer, placeholder) = asset.unwrap_or((0, true));
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
    /// folders first, alphabetical. (Going up is the "‹ Back" button by
    /// the section title; a terminal there is a right-click away.)
    pub(crate) fn dir_listing(&mut self) -> Vec<usize> {
        let Some(dir) = self.files_dir.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
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
        // Folders first, then files, each alphabetical.
        children.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (is_dir, name, path) in children.into_iter().take(FILES_LIST_MAX) {
            let exec = format!("xdg-open {}", launch::shell_quote(&path));
            out.push(self.push_transient_file(&path, &name, exec, is_dir));
        }
        out
    }

    /// Files-section navigation: returns true when the hit was a folder
    /// and the strip navigated into it (plain files fall through to the
    /// launch path). Clicking a folder in search results jumps there
    /// too, clearing the query.
    pub(crate) fn try_navigate(&mut self, entry_idx: usize) -> bool {
        if self.kinds.get(entry_idx) != Some(&apps::EntryKind::File) {
            return false;
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

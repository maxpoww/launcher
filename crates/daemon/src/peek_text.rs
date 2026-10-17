//! THE PENCIL on TEXT (Max, 2026-10-16, the lab: "también en todos los
//! archivos que se puedan editar como texto"): Quick Look's text becomes the
//! file itself, written straight in the frame — a caret, the arrows, Home and
//! End, the page keys; Ctrl+S writes it over (its original kept, as a
//! picture's is), Ctrl+Shift+S beside it; Escape lets it go (asking once
//! when something was changed).
//!
//! Only a file read WHOLE is offered (small enough, no line cut short):
//! what is written is exactly what was read and changed — its tabs, its
//! line ends (`\r\n` stays `\r\n`) and whether it ended with one.

use std::path::{Path, PathBuf};
use std::time::Instant;

use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::warn;

use crate::content::{GridContent, Label, Rect, RectInst, FONT_BOLD};
use crate::i18n::{fill, tr};
use crate::peek_edit::SaveAs;
use crate::quicklook::{text_label, HEAD};
use crate::App;

/// How text is set here (Quick Look's own text: the bar's mono font).
pub(crate) const PX: f32 = 13.0;
pub(crate) const LINE: f32 = 19.0;
pub(crate) const PAD: f32 = 18.0;
/// A tab is drawn this many spaces wide.
const TAB: usize = 4;

pub(crate) struct TextEdit {
    tag: u64,
    pub(crate) lines: Vec<String>,
    /// Where the caret is: a line, and a place among its characters.
    line: usize,
    col: usize,
    pub(crate) scroll: f32,
    crlf: bool,
    ends_with_newline: bool,
    changed: bool,
    armed: bool,
    saving: bool,
    /// The caret's place measured in the font as drawn: (line, col, x).
    pub(crate) caret_x: Option<(usize, usize, f32)>,
    /// Code's colours (`peek_syntax`), of the lines as they were when
    /// `coloured` (a hash of them) was taken; the file's language.
    pub(crate) syntax: Vec<Vec<crate::peek_syntax::Span>>,
    coloured: u64,
    lang: Option<&'static crate::peek_syntax::Lang>,
}

impl TextEdit {
    /// Where the caret is: (line, col).
    pub(crate) fn at(&self) -> (usize, usize) {
        (self.line, self.col)
    }

    pub(crate) fn line_text(&self, line: usize) -> &str {
        self.lines.get(line).map_or("", String::as_str)
    }

    /// Colour the lines again if they changed since (a keystroke's whole
    /// text, at most `TEXT_BYTES`: a millisecond).
    pub(crate) fn recolor(&mut self) {
        let Some(lang) = self.lang else {
            return;
        };
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.lines.hash(&mut h);
        let now = h.finish();
        if now != self.coloured || self.syntax.is_empty() {
            let shown: Vec<String> = self.lines.iter().map(|l| shown(l)).collect();
            self.syntax = crate::peek_syntax::spans(&shown, lang);
            self.coloured = now;
        }
    }
}

/// The file as lines, how its lines end, whether it ended with one.
pub(crate) fn read(path: &Path) -> Option<(Vec<String>, bool, bool)> {
    let text = std::fs::read_to_string(path).ok()?;
    let crlf = text.contains("\r\n");
    let ends = text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect();
    if ends {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    Some((lines, crlf, ends))
}

/// The lines as a file again.
pub(crate) fn join(lines: &[String], crlf: bool, ends: bool) -> String {
    let sep = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(sep);
    if ends {
        text.push_str(sep);
    }
    text
}

/// Where a character of a line is drawn, in columns (a tab is `TAB` wide).
fn cols_to(line: &str, col: usize) -> usize {
    line.chars()
        .take(col)
        .map(|c| if c == '\t' { TAB } else { 1 })
        .sum()
}

/// What is drawn of a line before its character `col`.
pub(crate) fn before(line: &str, col: usize) -> String {
    shown(&line.chars().take(col).collect::<String>())
}

/// A line as it is drawn.
pub(crate) fn shown(line: &str) -> String {
    line.replace('\t', &" ".repeat(TAB))
}

fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

/// Save: write `text` over `path` (its original kept) or beside it.
fn save(path: &Path, text: &str, how: SaveAs, word: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let to = match how {
        SaveAs::Same => {
            let dir = crate::peek_edit::versions_dir();
            std::fs::create_dir_all(&dir)?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            std::fs::copy(path, dir.join(format!("{stamp}-{name}")))
                .context("keeping the original")?;
            path.to_path_buf()
        }
        SaveAs::Copy => copy_path(path, word),
    };
    // (Whole or not at all: a temporary file renamed over it.)
    let tmp = to.with_file_name(format!(
        ".{}.{}.golem-tmp",
        crate::peek_edit::nonce(),
        to.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, text).context("writing")?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &to).context("writing")?;
    Ok(to)
}

/// "name (edited).ext" beside it, numbered if taken (any ending kept).
fn copy_path(path: &Path, word: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    (1..)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{stem} ({word}){ext}")
            } else {
                format!("{stem} ({word} {n}){ext}")
            })
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem} ({word}){ext}")))
}

impl App {
    /// Take the pencil on the text shown (only one read whole).
    pub(crate) fn text_begin(&mut self) {
        self.peek.tag += 1;
        let tag = self.peek.tag;
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let Some((lines, crlf, ends)) = read(&up.path) else {
            return;
        };
        up.text_edit = Some(TextEdit {
            tag,
            lines,
            line: 0,
            col: 0,
            scroll: 0.0,
            crlf,
            ends_with_newline: ends,
            changed: false,
            armed: false,
            saving: false,
            caret_x: None,
            syntax: Vec::new(),
            coloured: 0,
            lang: crate::peek_syntax::lang_of(&up.path),
        });
        self.request_desktop_top_draw();
    }

    fn text_end(&mut self) {
        if let Some(up) = self.peek.up.as_mut() {
            up.text_edit = None;
        }
        self.request_desktop_top_draw();
    }

    /// Keep the caret on screen.
    fn text_reveal(&mut self) {
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let body_h = up.rect.h - HEAD - 2.0 * PAD;
        let Some(t) = up.text_edit.as_mut() else {
            return;
        };
        let y = t.line as f32 * LINE;
        if y < t.scroll {
            t.scroll = y;
        } else if y + LINE > t.scroll + body_h {
            t.scroll = y + LINE - body_h;
        }
    }

    /// A key while the text is being written: always the editor's.
    pub(crate) fn text_key(&mut self, key: Keysym, utf8: Option<&str>) {
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        let page = self
            .peek
            .up
            .as_ref()
            .map_or(10, |u| ((u.rect.h - HEAD) / LINE) as usize)
            .max(1);
        let Some(t) = self.peek.up.as_mut().and_then(|u| u.text_edit.as_mut()) else {
            return;
        };
        if t.saving {
            return;
        }
        let len = |t: &TextEdit| t.lines[t.line].chars().count();
        let mut edited = false;
        match key {
            Keysym::s | Keysym::S if ctrl => {
                return self.text_save(if shift { SaveAs::Copy } else { SaveAs::Same })
            }
            Keysym::Escape => return self.text_cancel(),
            Keysym::Left => {
                if t.col > 0 {
                    t.col -= 1;
                } else if t.line > 0 {
                    t.line -= 1;
                    t.col = len(t);
                }
            }
            Keysym::Right => {
                if t.col < len(t) {
                    t.col += 1;
                } else if t.line + 1 < t.lines.len() {
                    t.line += 1;
                    t.col = 0;
                }
            }
            Keysym::Up | Keysym::Down | Keysym::Page_Up | Keysym::Page_Down => {
                let by = match key {
                    Keysym::Up => -1,
                    Keysym::Down => 1,
                    Keysym::Page_Up => -(page as isize),
                    _ => page as isize,
                };
                t.line = (t.line as isize + by).clamp(0, t.lines.len() as isize - 1) as usize;
                t.col = t.col.min(len(t));
            }
            Keysym::Home => t.col = 0,
            Keysym::End => t.col = len(t),
            Keysym::Return | Keysym::KP_Enter => {
                let at = byte_at(&t.lines[t.line], t.col);
                let rest = t.lines[t.line].split_off(at);
                // (The new line starts where the one before did.)
                let indent: String = t.lines[t.line]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                t.col = indent.chars().count();
                t.lines.insert(t.line + 1, indent + &rest);
                t.line += 1;
                edited = true;
            }
            Keysym::BackSpace => {
                if t.col > 0 {
                    let at = byte_at(&t.lines[t.line], t.col - 1);
                    t.lines[t.line].remove(at);
                    t.col -= 1;
                    edited = true;
                } else if t.line > 0 {
                    let line = t.lines.remove(t.line);
                    t.line -= 1;
                    t.col = len(t);
                    t.lines[t.line].push_str(&line);
                    edited = true;
                }
            }
            Keysym::Delete => {
                if t.col < len(t) {
                    let at = byte_at(&t.lines[t.line], t.col);
                    t.lines[t.line].remove(at);
                    edited = true;
                } else if t.line + 1 < t.lines.len() {
                    let next = t.lines.remove(t.line + 1);
                    t.lines[t.line].push_str(&next);
                    edited = true;
                }
            }
            Keysym::Tab => {
                let at = byte_at(&t.lines[t.line], t.col);
                t.lines[t.line].insert(at, '\t');
                t.col += 1;
                edited = true;
            }
            _ if !ctrl => {
                if let Some(text) =
                    utf8.filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
                {
                    let at = byte_at(&t.lines[t.line], t.col);
                    t.lines[t.line].insert_str(at, text);
                    t.col += text.chars().count();
                    edited = true;
                }
            }
            _ => {}
        }
        if edited {
            t.changed = true;
            t.armed = false;
        }
        self.text_reveal();
        self.request_desktop_top_draw();
    }

    /// Save from the head's buttons.
    pub(crate) fn text_key_save(&mut self, copy: bool) {
        self.text_save(if copy { SaveAs::Copy } else { SaveAs::Same });
    }

    fn text_cancel(&mut self) {
        let Some(t) = self.peek.up.as_mut().and_then(|u| u.text_edit.as_mut()) else {
            return;
        };
        if t.changed && !t.armed {
            t.armed = true;
            self.request_desktop_top_draw();
            return;
        }
        self.text_end();
    }

    /// Write it (over, its original kept; or beside), off the loop.
    fn text_save(&mut self, how: SaveAs) {
        let Some(tx) = self.peek.edits.clone().or_else(|| self.peek_edit_channel()) else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let path = up.path.clone();
        let Some(t) = up.text_edit.as_mut() else {
            return;
        };
        if !t.changed {
            return self.text_end();
        }
        t.saving = true;
        let (text, tag) = (join(&t.lines, t.crlf, t.ends_with_newline), t.tag);
        let word = tr("edited").to_owned();
        let spawned = std::thread::Builder::new()
            .name("quick-look-save".into())
            .spawn(move || {
                let result = save(&path, &text, how, &word).map_err(|e| format!("{e:#}"));
                let _ = tx.send(crate::peek_editor::Msg::Saved(tag, result));
            });
        if let Err(e) = spawned {
            warn!("quick look: no save thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    /// A save of the text came back (the editor's channel carries it).
    pub(crate) fn text_saved(&mut self, tag: u64, result: Result<PathBuf, String>) -> bool {
        let ours = self
            .peek
            .up
            .as_ref()
            .and_then(|u| u.text_edit.as_ref())
            .is_some_and(|t| t.tag == tag);
        if !ours {
            return false;
        }
        match result {
            Ok(to) => {
                let same = self.peek.up.as_ref().is_some_and(|u| u.path == to);
                let word = if same {
                    tr("Saved. The original is kept.").to_owned()
                } else {
                    fill(
                        tr("Copy saved: {name}"),
                        &[(
                            "name",
                            &to.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        )],
                    )
                };
                self.text_end();
                self.peek_forget(&to);
                self.thumb_stale(&to.to_string_lossy());
                if let Some(up) = self.peek.up.as_mut() {
                    if !same {
                        up.name = to
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        up.path = to.clone();
                    }
                    up.notice = Some((word, Instant::now()));
                }
                self.peek_want_now((to, 0));
            }
            Err(e) => {
                warn!("quick look: saving failed: {e}");
                if let Some(up) = self.peek.up.as_mut() {
                    if let Some(t) = up.text_edit.as_mut() {
                        t.saving = false;
                    }
                    up.notice = Some((
                        fill(tr("Could not save: {why}"), &[("why", &e)]),
                        Instant::now(),
                    ));
                }
            }
        }
        self.request_desktop_top_draw();
        true
    }

    /// A click in the text: the caret there.
    pub(crate) fn text_click(&mut self, at: (f32, f32), char_w: f32) {
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let body = Rect::new(up.rect.x, up.rect.y + HEAD, up.rect.w, up.rect.h - HEAD);
        let Some(t) = up.text_edit.as_mut() else {
            return;
        };
        if !body.contains(at) {
            return;
        }
        let line = (((at.1 - body.y - PAD + t.scroll) / LINE).floor().max(0.0) as usize)
            .min(t.lines.len() - 1);
        let want = ((at.0 - body.x - PAD) / char_w).round().max(0.0) as usize;
        // (The character whose drawn column is nearest.)
        let l = &t.lines[line];
        let n = l.chars().count();
        let col = (0..=n)
            .min_by_key(|&c| cols_to(l, c).abs_diff(want))
            .unwrap_or(0);
        t.line = line;
        t.col = col;
        self.request_desktop_top_draw();
    }

    /// The text being written into the frame `r`: its lines, the caret, the
    /// head's Cancel · Save copy · Save.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn text_push(
        &self,
        g: &mut GridContent,
        r: Rect,
        open: f32,
        ink: [f32; 4],
        ground: [f32; 4],
        char_w: f32,
    ) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let Some(t) = up.text_edit.as_ref() else {
            return;
        };
        let body = Rect::new(r.x, r.y + HEAD, r.w, (r.h - HEAD).max(0.0));
        let clip = Rect::new(body.x, body.y + 4.0, body.w, body.h - 10.0);
        let first = (t.scroll / LINE).floor().max(0.0) as usize;
        let count = (body.h / LINE).ceil() as usize + 2;
        for (n, line) in t.lines.iter().enumerate().skip(first).take(count) {
            let y = body.y + PAD + n as f32 * LINE - t.scroll;
            // Code, in its colours (once measured; the line plain meanwhile).
            if let Some(spans) = t.syntax.get(n).filter(|s| !s.is_empty()) {
                let line = shown(line);
                if let Some(xs) = self.code_xs(&line, spans) {
                    crate::quicklook::push_code_line(
                        g,
                        &line,
                        spans,
                        xs,
                        (body.x + PAD, y),
                        body.w - 2.0 * PAD,
                        clip,
                        ink,
                    );
                    continue;
                }
            }
            g.labels.push(Label {
                clip: Some(clip),
                family: Some(crate::options::NERD),
                ..text_label(
                    &shown(line),
                    (body.x + PAD, y),
                    body.w - 2.0 * PAD,
                    PX,
                    None,
                    ink,
                )
            });
        }
        // The caret.
        // (Where the font put the characters before it, once measured;
        // meanwhile, columns.)
        let measured = t
            .caret_x
            .filter(|&(l, c, _)| l == t.line && c == t.col)
            .map(|(_, _, x)| x);
        let cx =
            body.x + PAD + measured.unwrap_or(cols_to(&t.lines[t.line], t.col) as f32 * char_w);
        let cy = body.y + PAD + t.line as f32 * LINE - t.scroll;
        if cy >= clip.y && cy + LINE <= clip.y + clip.h {
            g.rects.push(RectInst {
                rect: Rect::new(cx - 1.0, cy + 1.0, 2.0, LINE - 2.0),
                radius: 1.0,
                color: [1.0, 0.75, 0.6, open],
                glass: 0.0,
                border: 0.0,
            });
        }
        // The head.
        let parts = crate::peek_editor::edit_parts(r, false);
        let mut dim = ink;
        dim[3] *= 0.6;
        let text_w = (parts.cancel.x - (r.x + 46.0) - 12.0).max(20.0);
        g.labels.push(text_label(
            &up.name,
            (r.x + 46.0, r.y + 7.0),
            text_w,
            13.5,
            Some(FONT_BOLD),
            ink,
        ));
        let doing = if t.saving {
            tr("Saving…").to_owned()
        } else {
            fill(tr("Editing · line {n}"), &[("n", &(t.line + 1))])
        };
        g.labels.push(text_label(
            &doing,
            (r.x + 46.0, r.y + 25.0),
            text_w,
            11.5,
            None,
            dim,
        ));
        let hot = |rect: Rect| up.ptr.is_some_and(|p| rect.contains(p));
        let soft = |rect: Rect| {
            [
                ink[0],
                ink[1],
                ink[2],
                (if hot(rect) { 0.26 } else { 0.14 }) * open,
            ]
        };
        let pill =
            |g: &mut GridContent, rect: Rect, color: [f32; 4], word: &str, word_ink: [f32; 4]| {
                g.rects.push(RectInst {
                    rect,
                    radius: rect.h / 2.0,
                    color,
                    glass: 0.0,
                    border: 0.0,
                });
                g.labels.push(Label {
                    centered: true,
                    ..text_label(
                        word,
                        (rect.x + rect.w / 2.0, rect.y + 5.0),
                        rect.w - 6.0,
                        12.0,
                        Some(FONT_BOLD),
                        word_ink,
                    )
                });
            };
        let (cancel_word, cancel_fill, cancel_ink) = if t.armed {
            (
                tr("Discard changes?"),
                [0.88, 0.32, 0.32, open],
                [1.0, 1.0, 1.0, open],
            )
        } else {
            (tr("Cancel"), soft(parts.cancel), ink)
        };
        pill(g, parts.cancel, cancel_fill, cancel_word, cancel_ink);
        pill(g, parts.copy, soft(parts.copy), tr("Save copy"), ink);
        pill(
            g,
            parts.save,
            [1.0, 0.75, 0.6, open],
            tr("Save"),
            [ground[0], ground[1], ground[2], open],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_comes_back_as_it_was_read() {
        let d = std::env::temp_dir().join(format!("pt-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for text in ["a\n\tb\n", "a\r\nb", "only", "", "x\n\n"] {
            let f = d.join("t.txt");
            std::fs::write(&f, text).unwrap();
            let (lines, crlf, ends) = read(&f).unwrap();
            assert_eq!(join(&lines, crlf, ends), text, "{text:?}");
        }
        assert_eq!(
            copy_path(&d.join("t.txt"), "edited").file_name().unwrap(),
            "t (edited).txt"
        );
        assert_eq!(
            copy_path(&d.join("Makefile"), "edited")
                .file_name()
                .unwrap(),
            "Makefile (edited)"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tabs_are_drawn_four_wide() {
        assert_eq!(cols_to("\tab", 1), 4);
        assert_eq!(cols_to("\tab", 3), 6);
        assert_eq!(shown("\tx"), "    x");
    }
}

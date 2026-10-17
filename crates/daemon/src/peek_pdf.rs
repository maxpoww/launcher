//! THE PENCIL on a PDF (Max, 2026-10-16, the lab: highlight, draw, write,
//! SIGN): what was done on a page is written into the PDF as its own
//! annotations — ink, a highlight, free text — by MuPDF (`mutool run` with a
//! small script), so the PDF stays a PDF: its text is still its text, the
//! marks can be read, moved or taken away by any PDF reader. Nothing of the
//! page itself is changed (that cannot be done well, and is not promised).
//!
//! The signature is drawn once in a box and kept for next time
//! (`signature.json` in Golem's data folder).

use std::path::{Path, PathBuf};

use crate::peek_edit::{Op, SaveAs};

/// The script that writes `ops` (shares of page `page`) into the PDF.
pub(crate) fn script(page: u32, ops: &[Op]) -> String {
    let mut js = String::from("var doc = Document.openDocument(scriptArgs[0]);\n");
    js.push_str(&format!("var page = doc.loadPage({page});\nvar b = page.getBounds();\nvar W = b[2] - b[0], H = b[3] - b[1];\n"));
    js.push_str("function X(s) { return b[0] + s * W; }\nfunction Y(s) { return b[1] + s * H; }\n");
    let colour = |ink: [u8; 3]| {
        format!(
            "[{:.3},{:.3},{:.3}]",
            f32::from(ink[0]) / 255.0,
            f32::from(ink[1]) / 255.0,
            f32::from(ink[2]) / 255.0
        )
    };
    let line = |points: &[(f32, f32)]| {
        let pts: Vec<String> = points
            .iter()
            .map(|(x, y)| format!("[X({x:.5}),Y({y:.5})]"))
            .collect();
        format!("[{}]", pts.join(","))
    };
    for op in ops {
        match op {
            Op::Stroke { points, width, ink } => {
                js.push_str(&format!(
                    "(function() {{ var a = page.createAnnotation(\"Ink\"); a.setInkList([{}]); a.setColor({}); a.setBorderWidth({width:.5} * W); a.update(); }})();\n",
                    line(points),
                    colour(*ink)
                ));
            }
            Op::Sign { rect, strokes } => {
                let lines: Vec<String> = strokes
                    .iter()
                    .map(|l| {
                        line(
                            &l.iter()
                                .map(|&(sx, sy)| (rect[0] + sx * rect[2], rect[1] + sy * rect[3]))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect();
                js.push_str(&format!(
                    "(function() {{ var a = page.createAnnotation(\"Ink\"); a.setInkList([{}]); a.setColor({}); a.setBorderWidth({:.5} * W); a.update(); }})();\n",
                    lines.join(","),
                    colour(crate::peek_edit::SIGN_INK),
                    rect[2] * 0.012
                ));
            }
            Op::Mark([x, y, w, h]) => {
                js.push_str(&format!(
                    "(function() {{ var a = page.createAnnotation(\"Highlight\"); a.setQuadPoints([[X({x:.5}),Y({y:.5}), X({x2:.5}),Y({y:.5}), X({x:.5}),Y({y2:.5}), X({x2:.5}),Y({y2:.5})]]); a.setColor({}); a.update(); }})();\n",
                    colour(crate::peek_edit::MARK_INK),
                    x2 = x + w,
                    y2 = y + h
                ));
            }
            Op::Text {
                at,
                text,
                size,
                ink,
            } => {
                let words = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned());
                let wide = text.chars().count() as f32 * size * 0.62;
                js.push_str(&format!(
                    "(function() {{ var a = page.createAnnotation(\"FreeText\"); a.setRect([X({x:.5}), Y({y:.5}), X({x:.5}) + {wide:.5} * H, Y({y:.5}) + {tall:.5} * H]); a.setContents({words}); a.setDefaultAppearance(\"Helv\", {size:.5} * H, {}); a.update(); }})();\n",
                    colour(*ink),
                    x = at.0,
                    y = at.1,
                    tall = size * 1.4
                ));
            }
            // (Not offered on a PDF.)
            Op::Turn | Op::Crop(_) | Op::Blur(_) => {}
        }
    }
    js.push_str("doc.save(scriptArgs[1], \"garbage=yes\");\n");
    js
}

/// Write the marks of page `page` into the PDF: over it (its original kept)
/// or beside it.
pub(crate) fn save(
    path: &Path,
    page: u32,
    ops: &[Op],
    how: SaveAs,
    word: &str,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    // (Names of this save's own: two saves at once must not share them.)
    let nonce = crate::peek_edit::nonce();
    let js_file =
        std::env::temp_dir().join(format!("golem-peek-{}-{nonce}.js", std::process::id()));
    std::fs::write(&js_file, script(page, ops)).context("writing the script")?;
    let tmp = dir.join(format!(".{name}.{nonce}.golem-tmp.pdf"));
    let out = crate::peek_edit::dies_with_us(std::process::Command::new("mutool"))
        .arg("run")
        .arg(&js_file)
        .arg(path)
        .arg(&tmp)
        .stdin(std::process::Stdio::null())
        .output()
        .context("running mutool")?;
    let _ = std::fs::remove_file(&js_file);
    if !out.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "mutool: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
        );
    }
    let to = match how {
        SaveAs::Same => {
            let versions = crate::peek_edit::versions_dir();
            std::fs::create_dir_all(&versions)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            std::fs::copy(path, versions.join(format!("{stamp}-{name}")))
                .context("keeping the original")?;
            path.to_path_buf()
        }
        // (A PDF stays a PDF: the picture's copy path made it ".jpg" — found
        // on the laptops, 2026-10-17.)
        SaveAs::Copy => crate::peek_text::copy_path(path, word),
    };
    std::fs::rename(&tmp, &to).context("writing")?;
    Ok(to)
}

/// The signature kept from last time (its lines in shares of its box).
pub(crate) fn kept_signature() -> Option<Vec<Vec<(f32, f32)>>> {
    let text = std::fs::read_to_string(signature_file()).ok()?;
    serde_json::from_str(&text)
        .ok()
        .filter(|s: &Vec<Vec<(f32, f32)>>| !s.is_empty())
}

pub(crate) fn keep_signature(strokes: &[Vec<(f32, f32)>]) {
    let file = signature_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(strokes) {
        let tmp = file.with_extension("tmp");
        if std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &file))
            .is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn signature_file() -> PathBuf {
    crate::peek_edit::versions_dir().with_file_name("signature.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_names_every_mark() {
        let js = script(
            2,
            &[
                Op::Mark([0.1, 0.1, 0.5, 0.05]),
                Op::Text {
                    at: (0.2, 0.3),
                    text: "Hola \"Golem\"".into(),
                    size: 0.03,
                    ink: [10, 20, 30],
                },
                Op::Sign {
                    rect: [0.5, 0.8, 0.3, 0.1],
                    strokes: vec![vec![(0.0, 0.5), (1.0, 0.5)]],
                },
            ],
        );
        assert!(js.contains("loadPage(2)"));
        assert!(
            js.contains("\"Highlight\"") && js.contains("\"FreeText\"") && js.contains("\"Ink\"")
        );
        assert!(js.contains(r#""Hola \"Golem\"""#), "words quoted safely");
    }
}

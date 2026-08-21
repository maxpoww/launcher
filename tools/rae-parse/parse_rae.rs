// Build-time tool: parse the RAE (Real Academia Española) source dump into the
// compact `{ "word": … }` JSON the waverunner clipboard dictionary panel loads.
//
// Input is the plain-text dump from eneko98/RAE-Corpus
// (`RealAcademiaEspanola-DiccionarioLlenguaEspanola.txt`), one headword per line:
//
//     ->headword[digit][, forms]. [(etymology).] 1. sense. 2. sense. ~ locution…
//
// For each line we recover **all** homonyms (word1, word2, … — RAE marks them
// with a trailing digit, glued or as " (1)") merged under the bare headword,
// keep the numbered senses up to the first " ~ " (RAE locutions are dropped),
// and pull out the parenthesised etymology. Output value is a plain definition
// string, or `{"e": etymology, "d": definition}` when an etymology was found —
// both forms are accepted by the daemon's loader (`crate::dict`).
//
// Pure std, no dependencies: compile with `rustc -O parse_rae.rs`.
// Usage: `parse_rae [input.txt] [output.json]`
//        (defaults: `rae.txt`, `dictionary-es.json`).

use std::collections::BTreeMap;
use std::io::{Read, Write};

/// Replace glued homonym markers (`base` immediately followed by a digit, e.g.
/// "gato2." / "perro2." / a "gato1" cross-reference) with a " · " separator, so
/// the merged homonyms read cleanly. `base` only ever precedes a digit at such a
/// marker — never in ordinary Spanish text — so this is safe.
fn strip_markers(body: &str, base: &str) -> String {
    let rb = body.as_bytes();
    let bb = base.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < rb.len() {
        if !bb.is_empty()
            && i + bb.len() < rb.len()
            && &rb[i..i + bb.len()] == bb
            && rb[i + bb.len()].is_ascii_digit()
        {
            out.push_str(" · ");
            i += bb.len() + 1;
            // Swallow the marker's trailing punctuation/space so no stray ". ,"
            // is left behind.
            while i < rb.len() && matches!(rb[i], b'.' | b',' | b' ') {
                i += 1;
            }
        } else {
            // Advance one whole UTF-8 char (indices land on char boundaries).
            let ch = body[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// The first parenthesised group in `head` whose content has a letter — the RAE
/// etymology ("(Del lat. cor, cordis…)"), skipping homonym-number parens like
/// "(1)". Trailing period dropped.
fn etymology(head: &str) -> Option<String> {
    let mut search = head;
    loop {
        let open = search.find('(')?;
        let after = &search[open + 1..];
        let close = after.find(')')?;
        let inner = &after[..close];
        if inner.chars().any(|c| c.is_alphabetic()) {
            let t = inner.split_whitespace().collect::<Vec<_>>().join(" ");
            let t = t.trim().trim_end_matches('.').trim();
            if !t.is_empty() {
                return Some(t.to_owned());
            }
        }
        search = &after[close + 1..];
    }
}

/// The `(etymology, definition)` for one headword line. Definition = everything
/// from the first numbered sense ("1. ") up to the first " ~ " (where RAE
/// locutions begin), with glued homonym markers turned into " · " separators and
/// whitespace collapsed. Etymology = the parenthesised origin before the senses.
fn definition(rest: &str, base: &str) -> Option<(Option<String>, String)> {
    // Strip homonym markers first, so a glued "mano1." at the head doesn't leave
    // its "1. " to be mistaken for the first sense (which would pull in the
    // parenthesised etymology that follows).
    let cleaned = strip_markers(rest, base);
    let start = cleaned.find("1. ")?;
    let etym = etymology(&cleaned[..start]);
    let mut body = &cleaned[start..];
    if let Some(loc) = body.find(" ~ ") {
        body = &body[..loc];
    }
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().trim_matches(['·', '.', ' ']).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some((etym, trimmed.to_owned()))
    }
}

fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().unwrap_or_else(|| "rae.txt".to_owned());
    let output = args.next().unwrap_or_else(|| "dictionary-es.json".to_owned());

    let mut txt = String::new();
    std::fs::File::open(&input)
        .unwrap_or_else(|e| panic!("open {input}: {e}"))
        .read_to_string(&mut txt)
        .expect("read");

    let mut map: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
    for line in txt.lines() {
        let Some(rest) = line.strip_prefix("->") else {
            continue;
        };
        let base: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
        if base.is_empty() {
            continue;
        }
        let key = base.to_lowercase();
        let Some(entry) = definition(rest, &base) else {
            continue;
        };
        // Keep the first parse for a given headword (homonyms already merged).
        map.entry(key).or_insert(entry);
    }

    // Value is a plain "definition" string, or a `{"e":etym,"d":def}` object when
    // an etymology was found (the daemon's loader accepts either).
    let mut out = String::from("{");
    let mut first = true;
    for (k, (etym, def)) in &map {
        if !first {
            out.push(',');
        }
        first = false;
        json_escape(k, &mut out);
        out.push(':');
        match etym {
            Some(e) => {
                out.push_str("{\"e\":");
                json_escape(e, &mut out);
                out.push_str(",\"d\":");
                json_escape(def, &mut out);
                out.push('}');
            }
            None => json_escape(def, &mut out),
        }
    }
    out.push('}');

    std::fs::File::create(&output)
        .unwrap_or_else(|e| panic!("create {output}: {e}"))
        .write_all(out.as_bytes())
        .expect("write");
    eprintln!("wrote {} entries to {output}", map.len());
}

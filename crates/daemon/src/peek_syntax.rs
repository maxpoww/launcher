//! QUICK LOOK's COLOURS FOR CODE (the approved design: "text/code = ours +
//! syntax colours"): a small tokenizer of our own — comments, strings,
//! numbers, keywords and capitalised names — for the languages one meets in
//! a home folder, chosen by the file's ending. Run on the worker with the
//! reading (`Prepared::syntax`); drawn in the palette's own hues, so a code
//! file wears the same colours as the terminal and the editor.
//!
//! Not a parser: it is wrong where a language is clever (a Rust lifetime
//! looks like a char, a shell `#` in a word is not a comment) and that is
//! accepted — it reads well, at no cost, on any machine.

use std::path::Path;

/// What a piece of a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Comment,
    Str,
    Num,
    Key,
    Type,
}

/// A coloured piece of a line: from which character, how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub from: u16,
    pub len: u16,
    pub class: Class,
}

/// How a language is read.
pub(crate) struct Lang {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    keys: &'static [&'static str],
    /// Names that start with a capital are types (Rust, Go, Java…).
    types: bool,
    /// Markdown: a `#` line is a heading, `` ` `` a piece of code.
    markdown: bool,
}

#[rustfmt::skip]
const C_KEYS: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
    "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
    "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
    "unsigned", "void", "volatile", "while", "bool", "true", "false", "NULL", "nullptr",
    "class", "namespace", "template", "typename", "public", "private", "protected", "virtual",
    "override", "new", "delete", "this", "using", "try", "catch", "throw", "#include",
    "#define", "#ifdef", "#ifndef", "#endif", "#if", "#else", "#pragma",
];
#[rustfmt::skip]
const RUST_KEYS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while",
];
#[rustfmt::skip]
const PY_KEYS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try",
    "while", "with", "yield", "self",
];
#[rustfmt::skip]
const JS_KEYS: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
    "delete", "do", "else", "export", "extends", "false", "finally", "for", "from", "function",
    "if", "import", "in", "instanceof", "interface", "let", "new", "null", "of", "return",
    "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined",
    "var", "void", "while", "yield", "enum", "implements", "private", "public", "readonly",
];
#[rustfmt::skip]
const GO_KEYS: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
    "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range",
    "return", "select", "struct", "switch", "type", "var", "nil", "true", "false",
];
#[rustfmt::skip]
const JAVA_KEYS: &[&str] = &[
    "abstract", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally",
    "float", "for", "if", "implements", "import", "instanceof", "int", "interface", "long",
    "new", "package", "private", "protected", "public", "return", "short", "static", "super",
    "switch", "this", "throw", "throws", "try", "void", "while", "true", "false", "null", "val",
    "var", "fun", "object", "when", "data",
];
#[rustfmt::skip]
const SH_KEYS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "return", "local", "export", "readonly", "set", "unset", "shift", "exit",
    "source", "echo", "cd", "true", "false",
];
#[rustfmt::skip]
const NIX_KEYS: &[&str] = &[
    "let", "in", "with", "rec", "inherit", "if", "then", "else", "assert", "import", "true",
    "false", "null", "or",
];
#[rustfmt::skip]
const LUA_KEYS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];
#[rustfmt::skip]
const SQL_KEYS: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "drop", "alter", "join", "left", "right", "inner", "outer", "on", "group", "by",
    "order", "having", "limit", "and", "or", "not", "null", "as", "distinct", "union", "SELECT",
    "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE",
    "DROP", "ALTER", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP", "BY", "ORDER",
    "HAVING", "LIMIT", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "UNION",
];
#[rustfmt::skip]
const DATA_KEYS: &[&str] = &[
    "true", "false", "null", "yes", "no", "on", "off", "True", "False",
];

const fn lang(
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    keys: &'static [&'static str],
    types: bool,
) -> Lang {
    Lang {
        line,
        block,
        quotes,
        keys,
        types,
        markdown: false,
    }
}

static C: Lang = lang(&["//"], Some(("/*", "*/")), &['"', '\''], C_KEYS, false);
static RUST: Lang = lang(&["//"], Some(("/*", "*/")), &['"'], RUST_KEYS, true);
static PY: Lang = lang(&["#"], None, &['"', '\''], PY_KEYS, true);
static JS: Lang = lang(
    &["//"],
    Some(("/*", "*/")),
    &['"', '\'', '`'],
    JS_KEYS,
    true,
);
static GO: Lang = lang(&["//"], Some(("/*", "*/")), &['"', '`'], GO_KEYS, true);
static JAVA: Lang = lang(&["//"], Some(("/*", "*/")), &['"', '\''], JAVA_KEYS, true);
static SH: Lang = lang(&["#"], None, &['"', '\''], SH_KEYS, false);
static NIX: Lang = lang(&["#"], Some(("/*", "*/")), &['"'], NIX_KEYS, false);
static LUA: Lang = lang(&["--"], Some(("--[[", "]]")), &['"', '\''], LUA_KEYS, false);
static SQL: Lang = lang(&["--"], Some(("/*", "*/")), &['\''], SQL_KEYS, false);
static CSS: Lang = lang(&[], Some(("/*", "*/")), &['"', '\''], &[], false);
static HTML: Lang = lang(&[], Some(("<!--", "-->")), &['"', '\''], &[], false);
static DATA: Lang = lang(&["#"], None, &['"', '\''], DATA_KEYS, false);
static JSON: Lang = lang(&[], None, &['"'], DATA_KEYS, false);
static INI: Lang = lang(&["#", ";"], None, &['"'], DATA_KEYS, false);
static MD: Lang = Lang {
    line: &[],
    block: None,
    quotes: &['`'],
    keys: &[],
    types: false,
    markdown: true,
};

/// The language of `path`, by its ending (or its name).
pub(crate) fn lang_of(path: &Path) -> Option<&'static Lang> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    match name.as_str() {
        "makefile" | "dockerfile" | "pkgbuild" | ".bashrc" | ".zshrc" | ".profile" => {
            return Some(&SH)
        }
        _ => {}
    }
    let ext = name.rsplit_once('.')?.1;
    Some(match ext {
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "glsl" | "wgsl" | "frag"
        | "vert" => &C,
        "rs" => &RUST,
        "py" | "pyw" => &PY,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => &JS,
        "go" => &GO,
        "java" | "kt" | "kts" | "scala" | "cs" | "swift" | "dart" => &JAVA,
        "sh" | "bash" | "zsh" | "fish" => &SH,
        "nix" => &NIX,
        "lua" => &LUA,
        "sql" => &SQL,
        "css" | "scss" | "less" => &CSS,
        "html" | "htm" | "xml" | "svg" | "xhtml" => &HTML,
        "yaml" | "yml" | "toml" | "conf" | "cfg" | "env" => &DATA,
        "json" | "jsonc" => &JSON,
        "ini" | "desktop" | "service" | "timer" | "socket" | "mount" => &INI,
        "md" | "markdown" => &MD,
        _ => return None,
    })
}

/// The coloured pieces of every line (a block comment carried across lines).
pub(crate) fn spans(lines: &[String], lang: &Lang) -> Vec<Vec<Span>> {
    let mut in_block = false;
    lines
        .iter()
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let mut out = Vec::new();
            line_spans(&chars, lang, &mut in_block, &mut out);
            out
        })
        .collect()
}

fn starts(chars: &[char], at: usize, word: &str) -> bool {
    (at..)
        .zip(word.chars())
        .all(|(i, w)| chars.get(i) == Some(&w))
}

fn push(out: &mut Vec<Span>, from: usize, to: usize, class: Class) {
    if to > from {
        out.push(Span {
            from: from.min(u16::MAX as usize) as u16,
            len: (to - from).min(u16::MAX as usize) as u16,
            class,
        });
    }
}

fn line_spans(chars: &[char], lang: &Lang, in_block: &mut bool, out: &mut Vec<Span>) {
    let n = chars.len();
    if lang.markdown {
        let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
        if chars.get(lead) == Some(&'#') {
            return push(out, lead, n, Class::Key);
        }
        if chars.get(lead) == Some(&'>') {
            return push(out, lead, n, Class::Comment);
        }
    }
    let mut i = 0;
    while i < n {
        if *in_block {
            let (_, end) = lang.block.unwrap_or(("", ""));
            let from = i;
            while i < n && !starts(chars, i, end) {
                i += 1;
            }
            if i < n {
                i += end.chars().count();
                *in_block = false;
            }
            push(out, from, i, Class::Comment);
            continue;
        }
        if let Some((start, _)) = lang.block {
            if starts(chars, i, start) {
                *in_block = true;
                let from = i;
                i += start.chars().count();
                // (Back round the loop: the rest is the comment's.)
                push(out, from, i, Class::Comment);
                continue;
            }
        }
        if lang.line.iter().any(|p| starts(chars, i, p))
            // (A `#` inside a word is not a comment: `a#b`, `#include`.)
            && !(chars[i] == '#' && i > 0 && !chars[i - 1].is_whitespace())
            && !(chars[i] == '#' && lang.keys.iter().any(|k| k.starts_with('#') && starts(chars, i, k)))
        {
            return push(out, i, n, Class::Comment);
        }
        let c = chars[i];
        if lang.quotes.contains(&c) {
            let from = i;
            i += 1;
            while i < n && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            push(out, from, i, Class::Str);
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_word(chars[i - 1])) {
            let from = i;
            while i < n && (is_word(chars[i]) || chars[i] == '.') {
                i += 1;
            }
            push(out, from, i, Class::Num);
            continue;
        }
        if is_word(c) || c == '#' {
            let from = i;
            i += 1;
            while i < n && is_word(chars[i]) {
                i += 1;
            }
            let word: String = chars[from..i].iter().collect();
            if lang.keys.contains(&word.as_str()) {
                push(out, from, i, Class::Key);
            } else if lang.types && c.is_uppercase() && word.len() > 1 {
                push(out, from, i, Class::Type);
            }
            continue;
        }
        i += 1;
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(text: &str, lang: &Lang) -> Vec<(String, Class)> {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        spans(&lines, lang)
            .into_iter()
            .zip(&lines)
            .flat_map(|(s, l)| {
                let chars: Vec<char> = l.chars().collect();
                s.into_iter()
                    .map(move |sp| {
                        (
                            chars[sp.from as usize..(sp.from + sp.len) as usize]
                                .iter()
                                .collect(),
                            sp.class,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn rust_reads_as_rust() {
        let got = classes("pub fn x(a: Vec<u8>) -> u32 { 42 } // done", &RUST);
        assert_eq!(
            got,
            vec![
                ("pub".into(), Class::Key),
                ("fn".into(), Class::Key),
                ("Vec".into(), Class::Type),
                ("42".into(), Class::Num),
                ("// done".into(), Class::Comment),
            ]
        );
    }

    #[test]
    fn a_block_comment_crosses_lines_and_strings_keep_escapes() {
        let got = classes("a /* one\ntwo */ \"x\\\"y\" 3", &C);
        assert_eq!(
            got,
            vec![
                ("/*".into(), Class::Comment),
                (" one".into(), Class::Comment),
                ("two */".into(), Class::Comment),
                ("\"x\\\"y\"".into(), Class::Str),
                ("3".into(), Class::Num),
            ]
        );
    }

    #[test]
    fn a_hash_in_a_word_is_not_a_comment_and_markdown_has_headings() {
        assert_eq!(
            classes("echo a#b # c", &SH),
            vec![("echo".into(), Class::Key), ("# c".into(), Class::Comment)]
        );
        assert_eq!(
            classes("## Title", &MD),
            vec![("## Title".into(), Class::Key)]
        );
        assert_eq!(
            classes("#include <x.h>", &C),
            vec![("#include".into(), Class::Key)]
        );
    }

    #[test]
    fn languages_are_known_by_their_endings() {
        assert!(lang_of(Path::new("/a/b.rs")).is_some());
        assert!(lang_of(Path::new("/a/Makefile")).is_some());
        assert!(lang_of(Path::new("/a/notes.txt")).is_none());
    }
}

//! What the `[current task]` pill actually SAYS.
//!
//! The OPTION is named *current task*, but a window title is not a task — it is
//! a task wearing the app's chrome. Firefox signs every title with
//! `" — Mozilla Firefox"`, VS Code with `" - Visual Studio Code"`, a shell
//! opens with `max@nixos:/home/max/launcher`, and a working agent parks a
//! spinner in front of the sentence. None of that is what you are doing; all of
//! it costs the pill width that the task itself should have.
//!
//! So the raw title is **groomed** into the task before the pill ever sees it.
//! This module is that grooming: one pure function, so the rules are readable,
//! reversible and unit-tested against real titles off this machine rather than
//! tuned live against whatever happens to be focused.
//!
//! It is also the fix for a flicker. The pill crossfades whenever the title
//! STRING changes ([`crate::options::TitleMeta`]), and an agent's spinner
//! rewrites that string about once a second while it works — so the pill blinked
//! every second for a task that had not changed at all. Grooming removes the
//! only thing that was changing, so the comparison upstream finds the text
//! identical and nothing moves. (The backstop for titles that churn in ways
//! grooming can't see is in `options.rs` — see `App::title_churn`.)
//!
//! The bias throughout is **conservative**: every rule that cannot prove the
//! text it wants to remove is chrome leaves it alone, and a rule that would
//! empty the pill is abandoned. A title we fail to improve is a title that still
//! reads correctly; a title we over-groom is a lie about what you are doing.

/// Separators an app signs its title with. Spaced on purpose — an unspaced
/// hyphen belongs to the content (`foo-bar.rs`), a spaced one is punctuation
/// between a title and its signature.
const SEPARATORS: [&str; 6] = [" — ", " – ", " - ", " | ", " · ", " :: "];

/// App names that are chrome no matter what class the window reports. The class
/// is the better witness and is tried first (see [`is_app_name`]); this list
/// exists for the windows whose class cannot testify — a Chrome PWA reports
/// `chrome-open.spotify.com__-Default` and still signs its title
/// `" - Google Chrome"`, and nothing in that class string says so.
const WELL_KNOWN_APPS: [&str; 12] = [
    "mozilla firefox",
    "firefox",
    "google chrome",
    "chromium",
    "brave",
    "microsoft edge",
    "visual studio code",
    "vscodium",
    "code - oss",
    "thunderbird",
    "libreoffice",
    "nautilus",
];

/// Marks apps put in front of a title to say "something is happening" — spinner
/// frames, progress bullets, hourglasses. They carry state, never the task, and
/// they are the volatile part: the frame changes on the app's own timer while
/// the sentence behind it stands still.
///
/// Deliberately NOT here: `✓`, `✗`, `⚠`, `*`, `●` as an editor's unsaved dot —
/// those mark a *result* or a *condition*, they do not cycle, and removing them
/// would remove information rather than noise.
fn is_status_mark(c: char) -> bool {
    matches!(c,
        // Braille Patterns — the classic `⠂⠄⠈⠐` spinner (and `⣾⣽⣻⢿`).
        '\u{2800}'..='\u{28FF}'
        // Geometric Shapes — `◐◓◑◒`, `◜◝◞◟`, bullets, block runners.
        | '\u{25A0}'..='\u{25FF}'
        // Asterisk/star dingbats — Claude Code's own `✳`, plus `✻✽✢✶✷✹✺❋✱✲✴`.
        | '\u{2217}' | '\u{2722}' | '\u{2731}'..='\u{273D}' | '\u{274B}'
        // Waiting: hourglasses and clocks.
        | '\u{231A}'..='\u{231B}' | '\u{23F0}'..='\u{23F3}'
        // Rotating arrows — `↺↻⟲⟳`.
        | '\u{21BA}'..='\u{21BB}' | '\u{27F2}'..='\u{27F3}'
        // Bullets used as separators-in-front: `•‣`.
        | '\u{2022}'..='\u{2023}'
    )
}

/// The user's home directory, read once. Grooming runs on every title change,
/// and `$HOME` does not change under a running session.
pub(crate) fn home() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| std::env::var("HOME").ok().filter(|h| h.len() > 1))
        .as_deref()
}

/// Groom a raw window title into the task it names.
///
/// `class` is the focused window's app class (the witness for its own
/// signature) and `home` the user's home directory (so shell paths can wear the
/// `~` everyone reads them in). Both are optional — an overview thumbnail is
/// titled without either, and grooming still does everything that does not need
/// them.
pub(crate) fn groom(title: &str, class: Option<&str>, home: Option<&str>) -> String {
    let s = title.trim();
    // Strip the volatile marks first: they hide the shell prompt and the app
    // signature behind them, and every later rule wants to see the real start.
    let s = strip_marks(s);
    let s = strip_shell_prompt(s);
    let s = strip_app_suffix(s, class).trim();
    // Marks can sit on the far side of the signature too, and removing the
    // signature is what exposes them.
    let s = strip_marks(s);
    let s = collapse_home(s, home);
    if s.trim().is_empty() {
        // Nothing survived, or there was nothing to begin with (a window that
        // has not titled itself yet). The app's own name is a true statement
        // about what you are doing; an empty pill is not.
        return class
            .map(prettify_class)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| title.trim().to_owned());
    }
    s
}

/// Peel status marks, unread counters and the whitespace between them off both
/// ends, until the text starts and ends with itself.
fn strip_marks(s: &str) -> &str {
    let mut s = s.trim();
    loop {
        let before = s;
        s = s.trim_start_matches(is_status_mark).trim_start();
        s = s.trim_end_matches(is_status_mark).trim_end();
        s = strip_counter(s);
        if s == before || s.is_empty() {
            return s;
        }
    }
}

/// Drop a leading unread counter — `(3) Messages`, `[12] Inbox`, `(99+) Chat`.
/// It is the notification OPTION's news, not the task's, and it re-flows the
/// pill every time it ticks.
fn strip_counter(s: &str) -> &str {
    let Some((open, close)) = s.chars().next().and_then(|c| match c {
        '(' => Some(('(', ')')),
        '[' => Some(('[', ']')),
        _ => None,
    }) else {
        return s;
    };
    let Some(end) = s.find(close) else { return s };
    // A count, not a year or a note: at most three digits, plus the `99+` form
    // browsers cap at. Four digits is `(2026) in review`, which is content.
    let inner = s[open.len_utf8()..end].trim_end_matches('+');
    if inner.is_empty() || inner.len() > 3 || !inner.chars().all(|c| c.is_ascii_digit()) {
        return s;
    }
    s[end + close.len_utf8()..].trim_start()
}

/// Drop a shell's `user@host:` prompt prefix, which says who you are rather than
/// what you are doing. Only fires on the exact shape — a single segment holding
/// an `@`, no whitespace, no slash — so a URL or a timestamped title is safe.
fn strip_shell_prompt(s: &str) -> &str {
    let Some(i) = s.find(':') else { return s };
    let head = &s[..i];
    if head.contains('@')
        && !head.contains(char::is_whitespace)
        && !head.contains('/')
        && i + 1 < s.len()
    {
        return s[i + 1..].trim_start();
    }
    s
}

/// Write paths the way they are read: `/home/max/launcher` → `~/launcher`.
fn collapse_home(s: &str, home: Option<&str>) -> String {
    match home {
        Some(h) if h.len() > 1 && s.contains(h) => s.replace(h, "~"),
        _ => s.to_owned(),
    }
}

/// Remove the app's signature from the end of the title — twice at most, since
/// an editor signs with both the project and the app (`file - project - Code`)
/// and only the app is chrome.
fn strip_app_suffix<'a>(mut s: &'a str, class: Option<&str>) -> &'a str {
    for _ in 0..2 {
        let Some((head, tail)) = split_last(s) else {
            break;
        };
        // A title that is ONLY its app name keeps it: "Mozilla Firefox" with a
        // blank tab still has to say something.
        if head.trim().is_empty() || !is_app_name(tail, class) {
            break;
        }
        s = head.trim_end();
    }
    s
}

/// Split at the LAST separator in the title — the signature is at the end, and
/// the content in front of it may well use the same punctuation.
fn split_last(s: &str) -> Option<(&str, &str)> {
    SEPARATORS
        .iter()
        .filter_map(|sep| s.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)
        .map(|(i, len)| (&s[..i], &s[i + len..]))
}

/// Whether a trailing segment is the app naming itself.
///
/// The window's own class is the witness, compared on alphanumerics alone so
/// `google-chrome` recognises `Google Chrome`. Where the two differ by more
/// than punctuation the match is made on the tail's **last word** — a signature
/// ends with the app's name (`Mozilla Firefox`, `Visual Studio Code`) — and only
/// ever with the word ANCHORED in the class, never merely contained by it:
/// `barefoot` ends with the class `foot` and is not it.
fn is_app_name(tail: &str, class: Option<&str>) -> bool {
    let t = norm(tail);
    // An app name is a name, not a sentence — a long tail is content.
    if t.is_empty() || t.len() > 40 {
        return false;
    }
    if WELL_KNOWN_APPS.iter().any(|w| norm(w) == t) {
        return true;
    }
    let Some(c) = class.map(norm).filter(|c| !c.is_empty()) else {
        return false;
    };
    // The whole tail is the class, give or take punctuation and packaging
    // (`Code` against `code-url-handler`).
    if t == c || (t.len() >= 4 && (c.starts_with(&t) || c.ends_with(&t))) {
        return true;
    }
    let words: Vec<String> = tail.split_whitespace().map(norm).collect();
    // Signatures are short. Four words in and this is a sentence.
    if words.len() > 4 {
        return false;
    }
    words
        .last()
        .is_some_and(|w| w.len() >= 4 && (*w == c || c.starts_with(w) || c.ends_with(w)))
}

/// Lowercase alphanumerics only — the comparable core of a name, so
/// `google-chrome`, `Google Chrome` and `googlechrome` are one thing.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A human-readable app name out of a window class, for a window that has not
/// titled itself: `org.gnome.Nautilus` → `Nautilus`, `foot` → `Foot`.
fn prettify_class(class: &str) -> String {
    let base = class.rsplit('.').next().unwrap_or(class);
    let base = base.split("__").next().unwrap_or(base);
    let base = base.trim_end_matches("-Default");
    let cleaned = base.replace(['-', '_'], " ");
    let cleaned = cleaned.trim();
    let mut chars = cleaned.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        Some(_) => cleaned.to_owned(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Option<&str> = Some("/home/max");

    #[test]
    fn strips_an_agents_spinner() {
        assert_eq!(
            groom("✳ Work on OPTIONS", Some("foot"), HOME),
            "Work on OPTIONS"
        );
        assert_eq!(
            groom("⠂ Work on stage", Some("foot"), HOME),
            "Work on stage"
        );
    }

    /// The flicker, stated as a test: every spinner frame must groom to the SAME
    /// text, or the pill crossfades once a second for a task that never changed.
    #[test]
    fn spinner_frames_groom_to_one_stable_title() {
        let frames = ["✳", "⠂", "⠐", "⠄", "⣾", "◐", "•", "⏳"];
        let groomed: Vec<String> = frames
            .iter()
            .map(|f| {
                groom(
                    &format!("{f} Improve the current task option"),
                    Some("foot"),
                    HOME,
                )
            })
            .collect();
        assert!(
            groomed.windows(2).all(|w| w[0] == w[1]),
            "spinner frames disagreed: {groomed:?}"
        );
        assert_eq!(groomed[0], "Improve the current task option");
    }

    #[test]
    fn strips_the_browsers_signature() {
        assert_eq!(
            groom(
                "pro slider bars - Buscar con Google — Mozilla Firefox",
                Some("firefox"),
                HOME
            ),
            "pro slider bars - Buscar con Google"
        );
    }

    /// A PWA's class names the site, not the browser, so only the well-known
    /// list can testify here.
    #[test]
    fn strips_a_pwa_hosts_signature() {
        assert_eq!(
            groom(
                "Inbox - Google Chrome",
                Some("chrome-open.spotify.com__-Default"),
                HOME
            ),
            "Inbox"
        );
    }

    #[test]
    fn strips_the_editors_app_name_but_keeps_the_project() {
        assert_eq!(
            groom(
                "options.rs - launcher - Visual Studio Code",
                Some("Code"),
                HOME
            ),
            "options.rs - launcher"
        );
    }

    #[test]
    fn a_title_that_is_only_the_app_name_survives() {
        assert_eq!(
            groom("Mozilla Firefox", Some("firefox"), HOME),
            "Mozilla Firefox"
        );
    }

    #[test]
    fn shell_prompt_becomes_the_path_you_are_in() {
        assert_eq!(
            groom("max@nixos:/home/max/launcher/crates", Some("foot"), HOME),
            "~/launcher/crates"
        );
        assert_eq!(
            groom("max@nixos:~/launcher", Some("foot"), HOME),
            "~/launcher"
        );
    }

    #[test]
    fn a_url_is_not_a_shell_prompt() {
        assert_eq!(
            groom("https://example.com/docs", Some("firefox"), HOME),
            "https://example.com/docs"
        );
    }

    #[test]
    fn drops_an_unread_counter() {
        assert_eq!(
            groom("(3) Messages — Google Chrome", Some("firefox"), HOME),
            "Messages"
        );
        // A number that is not a count stays: this is content.
        assert_eq!(
            groom("(2026) in review", Some("firefox"), HOME),
            "(2026) in review"
        );
    }

    #[test]
    fn content_punctuation_is_left_alone() {
        assert_eq!(
            groom(
                "Here We Go Again • Oliver Tree, David Guetta",
                Some("foot"),
                HOME
            ),
            "Here We Go Again • Oliver Tree, David Guetta"
        );
    }

    #[test]
    fn an_untitled_window_says_its_app() {
        assert_eq!(groom("", Some("foot"), HOME), "Foot");
        assert_eq!(groom("   ", Some("org.gnome.Nautilus"), HOME), "Nautilus");
    }

    /// Overview thumbnails are groomed without a class or a home; whatever needs
    /// neither must still happen, and nothing may panic.
    #[test]
    fn grooms_without_a_class_or_home() {
        assert_eq!(groom("⠂ Work on stage", None, None), "Work on stage");
        assert_eq!(groom("Inbox - Google Chrome", None, None), "Inbox");
        assert_eq!(groom("", None, None), "");
    }

    /// Grooming is idempotent — the pill can groom an already-groomed title (a
    /// re-measure on a scale change) without eating into it.
    #[test]
    fn grooming_twice_changes_nothing() {
        for raw in [
            "✳ Work on OPTIONS",
            "pro slider bars - Buscar con Google — Mozilla Firefox",
            "max@nixos:/home/max/launcher",
            "Mozilla Firefox",
        ] {
            let once = groom(raw, Some("firefox"), HOME);
            assert_eq!(groom(&once, Some("firefox"), HOME), once, "raw: {raw}");
        }
    }

    #[test]
    fn a_short_class_cannot_claim_a_word_that_contains_it() {
        // class `foot` must not eat " - barefoot"; only a real signature goes.
        assert_eq!(
            groom("notes - barefoot", Some("foot"), HOME),
            "notes - barefoot"
        );
    }
}

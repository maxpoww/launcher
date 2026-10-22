//! THE OTHER APPS' THEMES (Max, 2026-10-17: "on WS 2 i have a bunch of apps
//! that wont take our theme… cover as many approaches as possible").
//!
//! GTK and Qt apps take the palette from the files `palette.rs` writes. The
//! rest each read colours in a place of their own; each route is one writer
//! here, all from the APPS' theme (`palette::for_apps`: the Apps slider and
//! its Dark/Light):
//!
//! * KDE / Kirigami (Easy Effects, KDE Connect, every KDE app): a KDE colour
//!   scheme in `kdeglobals` (+ `color-schemes/Golem.colors`), and the D-Bus
//!   signal KDE apps reload on.
//! * VS Code and its kin (Code, VSCodium, Code - OSS): `workbench.color
//!   Customizations` in their `settings.json` (they reload it live).
//! * Inkscape: it pins a GTK theme of its own in `preferences.xml` — set to
//!   the system's (Golem's) and its dark flag (on its next start).
//! * JetBrains IDEs and Android Studio (Swing): a theme plugin of our own
//!   (`golem-theme.jar`: plugin.xml + a `.theme.json` + an editor scheme),
//!   chosen in `laf.xml` — on the IDE's next start.
//!
//!
//! THE RULE (Max, 2026-10-17: "more is just ambition that can kill us"): a
//! route is added only for an app Golem ships or many people use, and every
//! route fails quietly — an app it cannot reach just keeps its own colours.
//! `theme-reach` (golem-deep) lists what is not reached; that is fine.
//!
//! Browsers other than Seam are never coloured (Max, 2026-10-17: "i dont
//! want to color browsers, only seam").
//!
//! Every writer leaves an app alone when it is not installed (its config
//! folder is not there), and never writes a file it cannot read whole.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::palette::{hex, Theme};

type Rgb = [u8; 3];

/// Dress every app that has a route of its own.
pub(crate) fn write(t: &Theme) {
    kde(t);
    vscode(t);
    inkscape(t);
    jetbrains(t);
    easyeffects(t);
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn config() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".config"))
}

fn data() -> PathBuf {
    std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share"))
}

/// Written whole or not at all (a tmp file renamed over it), keeping the
/// file's permissions. A LINK is never written through or over: it is
/// someone else's (home-manager links settings into the store, and a plain
/// file in its place breaks the next activation).
fn put(path: &Path, text: &str) {
    put_bytes(path, text.as_bytes());
}

fn mix(a: Rgb, b: Rgb, k: f32) -> Rgb {
    std::array::from_fn(|i| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * k).round() as u8)
}

// ------------------------------------------------------------------ KDE

fn rgb(c: Rgb) -> String {
    format!("{},{},{}", c[0], c[1], c[2])
}

/// The KDE colour scheme's sections.
pub(crate) fn kde_scheme(t: &Theme) -> Vec<(String, Vec<(String, String)>)> {
    let c = &t.c;
    let set = |bg: Rgb, alt: Rgb, fg: Rgb| {
        vec![
            ("BackgroundNormal".to_owned(), rgb(bg)),
            ("BackgroundAlternate".to_owned(), rgb(alt)),
            ("ForegroundNormal".to_owned(), rgb(fg)),
            ("ForegroundInactive".to_owned(), rgb(t.ink_dim())),
            // (Text, all of these: the text-safe colour — Theme::as_text.)
            ("ForegroundActive".to_owned(), rgb(t.accent_text())),
            ("ForegroundLink".to_owned(), rgb(t.as_text(c[0xD]))),
            ("ForegroundVisited".to_owned(), rgb(t.as_text(c[0xE]))),
            ("ForegroundNegative".to_owned(), rgb(t.as_text(c[0x8]))),
            ("ForegroundNeutral".to_owned(), rgb(t.as_text(c[0xA]))),
            ("ForegroundPositive".to_owned(), rgb(t.as_text(c[0xB]))),
            ("DecorationFocus".to_owned(), rgb(t.accent())),
            ("DecorationHover".to_owned(), rgb(t.accent())),
        ]
    };
    let ink = t.ink();
    // What sits ON a selection reads on the accent: every ink there is the
    // accent's own (the faint one a step toward it), never the window's — a
    // light accent wore ink_dim, the links and the accent itself at 1–2:1
    // (India Ink, 2026-10-20).
    let on = t.accent_ink();
    let mut selection = set(t.accent(), mix(t.accent(), c[0], 0.3), on);
    for (k, v) in &mut selection {
        if k.starts_with("Foreground") {
            *v = rgb(if k == "ForegroundInactive" { mix(on, t.accent(), 0.2) } else { on });
        }
    }
    vec![
        ("Colors:Window".into(), set(c[1], c[2], ink)),
        ("Colors:View".into(), set(c[0], c[1], ink)),
        ("Colors:Button".into(), set(c[2], c[3], ink)),
        ("Colors:Header".into(), set(c[1], c[2], ink)),
        ("Colors:Tooltip".into(), set(c[2], c[3], ink)),
        ("Colors:Complementary".into(), set(c[0], c[1], ink)),
        ("Colors:Selection".into(), selection),
    ]
}

/// An INI file with these sections put in place of their old selves (the
/// rest of the file kept as it was).
fn ini_with(old: &str, ours: &[(String, Vec<(String, String)>)]) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in old.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            skip = ours.iter().any(|(name, _)| l[1..l.len() - 1] == *name);
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    // (The blank line before a section is the section's: kept with what came
    // before, it grew by one at every change — the Acer's kdeglobals.)
    out.truncate(out.trim_end().len());
    for (name, keys) in ours {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("[{name}]\n"));
        for (k, v) in keys {
            out.push_str(&format!("{k}={v}\n"));
        }
        out.truncate(out.trim_end().len());
    }
    out.push('\n');
    out
}

fn kde(t: &Theme) {
    // Only where a KDE app is (its settings there, or one of them on the
    // path): a colour scheme for nobody is clutter in the owner's config.
    let kdeglobals = config().join("kdeglobals");
    let any_kde = kdeglobals.exists()
        || ["kdeconnect-app", "easyeffects", "dolphin", "kate", "konsole", "okular", "gwenview", "krita", "kdenlive", "haruna"]
            .iter()
            .any(|p| crate::launch::on_path(p));
    if !any_kde {
        return;
    }
    let mut sections = kde_scheme(t);
    // The scheme on its own, by name (what KDE's settings would list).
    let mut file = vec![("General".to_owned(), vec![("Name".to_owned(), "Golem".to_owned()), ("ColorScheme".to_owned(), "Golem".to_owned())])];
    file.extend(sections.iter().cloned());
    put(&data().join("color-schemes/Golem.colors"), &ini_with("", &file));
    // …and in use: kdeglobals (its other settings kept).
    sections.push(("General".into(), vec![("ColorScheme".to_owned(), "Golem".to_owned())]));
    let path = kdeglobals;
    // (One that is there but cannot be read whole is never replaced.)
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            warn!("apps: kdeglobals not read ({e}); left alone");
            return;
        }
    };
    // (General's other keys kept: only ColorScheme is ours there.)
    let mut general: Vec<(String, String)> = Vec::new();
    let mut in_general = false;
    for line in old.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_general = l == "[General]";
        } else if in_general {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() != "ColorScheme" {
                    general.push((k.to_owned(), v.to_owned()));
                }
            }
        }
    }
    if let Some((_, keys)) = sections.iter_mut().find(|(n, _)| n == "General") {
        keys.extend(general);
    }
    put(&path, &ini_with(&old, &sections));
    // A KDE app with a colour scheme of ITS OWN picked (its `[UiSettings]
    // ColorScheme`, KColorSchemeManager's — Easy Effects showed Breeze until
    // Golem was picked there): Golem. It takes it as it starts; one that
    // follows the system's (no pick) changes live.
    if let Ok(list) = std::fs::read_dir(config()) {
        for e in list.flatten() {
            let p = e.path();
            let is_rc = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("rc") && n != "kdeglobals");
            let Ok(text) = std::fs::read_to_string(&p).map_err(|_| ()).and_then(|t| if is_rc { Ok(t) } else { Err(()) }) else { continue };
            if !text.contains("[UiSettings]") {
                continue;
            }
            // Line by line, the file's own layout kept: only the pick of
            // [UiSettings] turns to Golem. (An empty pick is "the system's" —
            // the live way: left alone.)
            let mut in_ui = false;
            let mut changed = false;
            let lines: Vec<String> = text
                .split_inclusive('\n')
                .map(|line| {
                    let l = line.trim();
                    if l.starts_with('[') && l.ends_with(']') {
                        in_ui = l == "[UiSettings]";
                    } else if in_ui {
                        if let Some(v) = l.strip_prefix("ColorScheme=") {
                            if !v.is_empty() && v != "Golem" {
                                changed = true;
                                let end = &line[line.trim_end().len()..];
                                return format!("ColorScheme=Golem{end}");
                            }
                        }
                    }
                    line.to_owned()
                })
                .collect();
            if changed {
                put(&p, &lines.concat());
            }
        }
    }
    // KDE apps reload their colours on these: KConfig's own notice that
    // kdeglobals changed, naming the groups (what Kirigami's desktop style
    // watches — KDE Connect did not change live without it), and the older
    // KGlobalSettings palette notice.
    let groups: Vec<String> = sections
        .iter()
        .map(|(name, keys)| {
            let keys: Vec<String> = keys.iter().map(|(k, _)| format!("b'{k}'")).collect();
            format!("'{name}': [{}]", keys.join(", "))
        })
        .collect();
    let _ = std::process::Command::new("gdbus")
        .args(["emit", "--session", "--object-path", "/kdeglobals", "--signal", "org.kde.kconfig.notify.ConfigChanged"])
        .arg(format!("{{{}}}", groups.join(", ")))
        .status();
    let _ = std::process::Command::new("dbus-send")
        .args(["--session", "--type=signal", "/KGlobalSettings", "org.kde.KGlobalSettings.notifyChange", "int32:0", "int32:0"])
        .status();
}

// -------------------------------------------------------------- VS Code

fn vscode(t: &Theme) {
    let c = &t.c;
    let h = |x: Rgb| format!("#{}", hex(x));
    let ink = h(t.ink());
    let pairs: Vec<(&str, String)> = vec![
        ("foreground", ink.clone()),
        ("focusBorder", h(t.accent())),
        ("editor.background", h(c[0])),
        ("editor.foreground", ink.clone()),
        ("editorGutter.background", h(c[0])),
        ("editor.lineHighlightBackground", h(c[1])),
        ("editor.selectionBackground", h(t.selection())),
        ("editorCursor.foreground", h(t.accent())),
        ("editorGroupHeader.tabsBackground", h(c[1])),
        ("tab.activeBackground", h(c[0])),
        ("tab.inactiveBackground", h(c[1])),
        ("tab.activeBorderTop", h(t.accent())),
        ("sideBar.background", h(c[1])),
        ("sideBarSectionHeader.background", h(c[1])),
        ("activityBar.background", h(c[1])),
        ("activityBar.foreground", ink.clone()),
        ("activityBarBadge.background", h(t.accent())),
        ("activityBarBadge.foreground", h(t.accent_ink())),
        ("titleBar.activeBackground", h(c[1])),
        ("titleBar.inactiveBackground", h(c[1])),
        ("statusBar.background", h(c[1])),
        ("statusBar.foreground", ink.clone()),
        ("panel.background", h(c[0])),
        ("terminal.background", h(c[0])),
        ("terminal.foreground", ink.clone()),
        ("input.background", h(c[2])),
        ("dropdown.background", h(c[2])),
        ("button.background", h(t.accent())),
        ("button.foreground", h(t.accent_ink())),
        ("list.activeSelectionBackground", h(c[2])),
        ("list.hoverBackground", h(c[1])),
        ("menu.background", h(c[1])),
        ("progressBar.background", h(t.accent())),
        ("textLink.foreground", h(t.accent_text())),
        ("badge.background", h(t.accent())),
        ("badge.foreground", h(t.accent_ink())),
    ];
    for app in ["Code", "VSCodium", "Code - OSS"] {
        let dir = config().join(app).join("User");
        if !dir.is_dir() {
            continue;
        }
        let path = dir.join("settings.json");
        // (Only a file that is not there starts from nothing: one that
        // cannot be read is left alone, never replaced by our two keys.)
        let old = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(_) => continue,
        };
        // (A settings file with comments is not JSON: left as it is.)
        let Ok(serde_json::Value::Object(mut map)) = serde_json::from_str::<serde_json::Value>(if old.trim().is_empty() { "{}" } else { &old }) else {
            warn!("apps: {} is not plain JSON — its colours left alone", path.display());
            continue;
        };
        let colours: serde_json::Map<String, serde_json::Value> =
            pairs.iter().map(|(k, v)| ((*k).to_owned(), serde_json::Value::String(v.clone()))).collect();
        map.insert("workbench.colorCustomizations".into(), serde_json::Value::Object(colours));
        // (The base theme only where none is picked, or ours is: a theme the
        // owner picked is theirs.)
        let base = if t.dark { "Default Dark Modern" } else { "Default Light Modern" };
        let ours = |v: &serde_json::Value| v.as_str().is_some_and(|s| s.starts_with("Default ") && s.ends_with(" Modern"));
        if map.get("workbench.colorTheme").is_none_or(ours) {
            map.insert("workbench.colorTheme".into(), serde_json::Value::String(base.into()));
        }
        if let Ok(text) = serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
            put(&path, &(text + "\n"));
        }
    }
}

// -------------------------------------------------------------- Inkscape

fn inkscape(t: &Theme) {
    let path = config().join("inkscape/preferences.xml");
    if !path.is_file() {
        return;
    }
    // A theme name that never changes: `Golem`, a link to whichever of
    // Golem-A|B is the system's now (the system's own flips name so GTK apps
    // reload; Inkscape keeps the name it read, and would be left on old
    // colours).
    let gtk = std::process::Command::new("dconf")
        .args(["read", "/org/gnome/desktop/interface/gtk-theme"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_owned())
        .filter(|s| s.starts_with("Golem-") && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if let Some(gtk) = gtk {
        // A REAL folder (Inkscape lists the themes folders and passes over a
        // link — it fell back to Adwaita): its sheet imports the one in use.
        let dir = data().join("themes/Golem");
        if dir.is_symlink() {
            let _ = std::fs::remove_file(&dir);
        }
        let now = data().join("themes").join(&gtk).join("gtk-3.0/gtk.css");
        // (A path a CSS url cannot hold as it is: left as it was.)
        if now.to_string_lossy().contains(['"', '\\', '\n']) {
            return;
        }
        put(&dir.join("gtk-3.0/gtk.css"), &format!("@import url(\"file://{}\");\n", now.display()));
        put(&dir.join("index.theme"), "[Desktop Entry]\nType=X-GNOME-Metatheme\nName=Golem\n\n[X-GNOME-Metatheme]\nGtkTheme=Golem\n");
    }
    let dark = t.dark;
    // Inkscape writes its preferences back as it quits: written while it is
    // open, ours would be undone — so it is written once it has quit.
    when_closed("=(\\.inkscape-wrapp|inkscape)", move || {
        let Ok(old) = std::fs::read_to_string(&path) else { return };
        let text = set_attr(&set_attr(&old, "gtkTheme", "Golem"), "darkTheme", if dark { "1" } else { "0" });
        if text != old {
            put(&path, &text);
            info!("apps: Inkscape on Golem's theme");
        }
    });
}

/// Run `then` now if no process `name` names runs (`=name`: by its exact
/// process name; else a pattern of the command line), else once the last one
/// has quit (a thread waits; a newer wait for the same app replaces it).
fn when_closed(name: &'static str, then: impl FnOnce() + Send + 'static) {
    static TURNS: std::sync::Mutex<Vec<(&'static str, u64)>> = std::sync::Mutex::new(Vec::new());
    // (Each app its own turn: a newer wait replaces only its own app's. The
    // check of the turn and the writing happen under the one lock, so an
    // older waiter cannot write after a newer change has.)
    fn turn_of(t: &mut Vec<(&'static str, u64)>, name: &'static str, bump: bool) -> u64 {
        let i = t.iter().position(|(n, _)| *n == name).unwrap_or_else(|| {
            t.push((name, 0));
            t.len() - 1
        });
        if bump {
            t[i].1 += 1;
        }
        t[i].1
    }
    let turn = |name: &'static str, bump: bool| -> u64 {
        let mut t = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        turn_of(&mut t, name, bump)
    };
    let running = |name: &str| {
        // (`=name`: that process name exactly; else a pattern of its command.)
        let args: [&str; 2] = match name.strip_prefix('=') {
            Some(exact) => ["-x", exact],
            None => ["-f", name],
        };
        std::process::Command::new("pgrep")
            .args(args)
            .output()
            .is_ok_and(|o| !o.stdout.is_empty())
    };
    // (Every call takes a turn — the one done at once too: an older waiter
    // must not write its older colours after it.)
    let mine = turn(name, true);
    if !running(name) {
        let _hold = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        then();
        return;
    }
    let _ = std::thread::Builder::new().name("apps-wait".into()).spawn(move || {
        // (Half a day at most.)
        for _ in 0..(12 * 60 * 20) {
            std::thread::sleep(std::time::Duration::from_secs(3));
            if turn(name, false) != mine {
                return;
            }
            if !running(name) {
                let mut t = TURNS.lock().unwrap_or_else(|e| e.into_inner());
                if turn_of(&mut t, name, false) == mine {
                    then();
                }
                return;
            }
        }
    });
}

/// `name="…"` set to `value` wherever it is.
fn set_attr(xml: &str, name: &str, value: &str) -> String {
    let key = format!("{name}=\"");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(&key) {
        // (A whole attribute name: what comes before is a space.)
        let whole = i == 0 || rest.as_bytes()[i - 1].is_ascii_whitespace();
        out.push_str(&rest[..i + key.len()]);
        rest = &rest[i + key.len()..];
        if whole {
            if let Some(end) = rest.find('"') {
                out.push_str(value);
                rest = &rest[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------- Easy Effects

/// Easy Effects runs as a user SERVICE and reads its colour scheme once, as
/// that service starts (its own "Golem" pick; following the system's left it
/// white). So a change of the apps' colours restarts the service — but only
/// once nothing is playing (Max, 2026-10-17: "do the restart when there is
/// no sound happening"): it waits for 10 s of silence; a newer change takes
/// the place of a waiting one. It is not restarted for colours it already
/// wears (the stamp is kept across dock starts), nor for the light's small
/// steps through the day: only for a palette or a look (a restart is a window
/// gone and back — Max, 2026-10-21: "no jumps at all").
fn easyeffects(t: &Theme) {
    let picked = std::fs::read_to_string(config().join("easyeffectsrc")).is_ok_and(|s| s.contains("ColorScheme=Golem"));
    if !picked {
        return;
    }
    let stamp = format!("{} {} {}", t.name, t.dark, hex(t.accent()));
    let file = data().join("golem/easyeffects-stamp");
    if std::fs::read_to_string(&file).ok().as_deref() == Some(stamp.as_str()) {
        return;
    }
    let active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "easyeffects.service"])
        .status()
        .is_ok_and(|s| s.success());
    if !active {
        put(&file, &stamp);
        return;
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static TURN: AtomicU64 = AtomicU64::new(0);
    let mine = TURN.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = std::thread::Builder::new().name("easyeffects-wait".into()).spawn(move || {
        let mut quiet = 0;
        // (Six hours at most.)
        for _ in 0..(6 * 60 * 12) {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if TURN.load(Ordering::Relaxed) != mine {
                return;
            }
            quiet = if sound_playing() { 0 } else { quiet + 1 };
            if quiet >= 2 {
                // (One restart at a time, and only by the newest change.)
                static RESTART: std::sync::Mutex<()> = std::sync::Mutex::new(());
                let _one = RESTART.lock().unwrap_or_else(|e| e.into_inner());
                if TURN.load(Ordering::Relaxed) != mine {
                    return;
                }
                let ok = std::process::Command::new("systemctl")
                    .args(["--user", "restart", "easyeffects.service"])
                    .status()
                    .is_ok_and(|s| s.success());
                if ok {
                    put(&file, &stamp);
                    info!("apps: Easy Effects restarted in the new colours (nothing was playing)");
                } else {
                    warn!("apps: Easy Effects did not restart");
                }
                return;
            }
        }
    });
}

/// Whether any app is playing sound now: a PipeWire output stream running.
fn sound_playing() -> bool {
    let Ok(out) = std::process::Command::new("pw-dump").output() else { return false };
    let Ok(serde_json::Value::Array(objects)) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return false };
    objects.iter().any(|o| {
        let info = &o["info"];
        let props = &info["props"];
        props["media.class"].as_str().is_some_and(|c| c.starts_with("Stream/Output/Audio"))
            && info["state"] == "running"
            // (A stream of pure silence is not sound: Golem keeps one playing
            // so a Bluetooth speaker does not sleep — ffplay of `anullsrc`.)
            && !props["application.process.id"]
                .as_u64()
                .or_else(|| props["application.process.id"].as_str().and_then(|p| p.parse().ok()))
                .and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok())
                .is_some_and(|cmd| String::from_utf8_lossy(&cmd).contains("anullsrc"))
    })
}

// ------------------------------------------------------------ JetBrains

fn jetbrains(t: &Theme) {
    // Each IDE's settings folder, and where its plugins go.
    let mut ides: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (vendor, conf) in [("JetBrains", config().join("JetBrains")), ("Google", config().join("Google"))] {
        let Ok(list) = std::fs::read_dir(&conf) else { continue };
        for e in list.flatten() {
            let name = e.file_name();
            if e.path().join("options").is_dir() {
                ides.push((e.path(), data().join(vendor).join(&name)));
            }
        }
    }
    if ides.is_empty() {
        return;
    }
    let jar = theme_jar(t);
    for (_, plugins) in &ides {
        put_bytes(&plugins.join("golem-theme/lib/golem-theme.jar"), &jar);
    }
    // The IDE writes its settings back as it quits: they are chosen once it
    // has — the look (laf.xml) AND the editor's own scheme (colors.scheme.xml:
    // the editor keeps a scheme of its own apart from the look).
    when_closed("android-studio-unwrapped/bin/studio|JetBrains/Toolbox/apps/[^ ]+/bin/[a-z]+(\\.sh)?( |$)", move || {
        for (conf, _) in ides {
            put(
                &conf.join("options/laf.xml"),
                "<application>\n  <component name=\"LafManager\" autodetect=\"false\">\n    <laf class-name=\"com.intellij.ide.ui.laf.UIThemeBasedLookAndFeelInfo\" themeId=\"golem-theme\" />\n  </component>\n</application>\n",
            );
            put(
                &conf.join("options/colors.scheme.xml"),
                "<application>\n  <component name=\"EditorColorsManagerImpl\">\n    <global_color_scheme name=\"Golem\" />\n  </component>\n</application>\n",
            );
            info!("apps: Golem's theme for {}", conf.display());
        }
    });
}

fn put_bytes(path: &Path, bytes: &[u8]) {
    let old = std::fs::symlink_metadata(path);
    if old.as_ref().is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // (Appended, not substituted: `foo.json` and `foo.rgba` must not share
    // one temp name — `persist.rs` learnt the same.)
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".golem-tmp");
    let tmp = path.with_file_name(tmp_name);
    let written = std::fs::write(&tmp, bytes).and_then(|()| {
        if let Ok(m) = &old {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, path)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        warn!("apps: could not write {}", path.display());
    }
}

/// The plugin: its declaration, the UI theme, and an editor scheme.
fn theme_jar(t: &Theme) -> Vec<u8> {
    let c = &t.c;
    let h = |x: Rgb| format!("#{}", hex(x));
    let plugin = r#"<idea-plugin>
  <id>org.golem.theme</id>
  <name>Golem</name>
  <version>1</version>
  <vendor>Golem</vendor>
  <description>Golem's palette, written by the dock.</description>
  <depends>com.intellij.modules.platform</depends>
  <extensions defaultExtensionNs="com.intellij">
    <themeProvider id="golem-theme" path="/golem.theme.json"/>
    <bundledColorScheme path="/golem"/>
  </extensions>
</idea-plugin>
"#;
    let theme = serde_json::json!({
        "name": "Golem",
        "dark": t.dark,
        "author": "Golem",
        "parentTheme": if t.dark { "Darcula" } else { "IntelliJ" },
        "editorScheme": "/golem.xml",
        "ui": {
            "*": {
                "background": h(c[1]),
                "foreground": h(t.ink()),
                "infoForeground": h(t.ink_dim()),
                "selectionBackground": h(t.accent()),
                "selectionForeground": h(t.accent_ink()),
                "selectionInactiveBackground": h(c[2]),
                "borderColor": h(c[2]),
                "separatorColor": h(c[2]),
                "focusColor": h(t.accent()),
                "hoverBackground": h(c[2]),
            },
            "MainToolbar": { "background": h(c[1]) },
            "ToolWindow.Header.background": h(c[1]),
            "Editor.background": h(c[0]),
            "EditorTabs.background": h(c[1]),
            "EditorTabs.underlineColor": h(t.accent()),
            "Button.default.startBackground": h(t.accent()),
            "Button.default.endBackground": h(t.accent()),
            "Button.default.foreground": h(t.accent_ink()),
            "Link.activeForeground": h(t.accent_text()),
            "ProgressBar.progressColor": h(t.accent()),
        }
    });
    let hx = |x: Rgb| hex(x);
    let icls = format!(
        r#"<scheme name="Golem" version="142" parent_scheme="{parent}">
  <colors>
    <option name="CARET_COLOR" value="{acc}"/>
    <option name="CARET_ROW_COLOR" value="{row}"/>
    <option name="GUTTER_BACKGROUND" value="{bg}"/>
    <option name="SELECTION_BACKGROUND" value="{sel}"/>
  </colors>
  <attributes>
    <option name="TEXT">
      <value>
        <option name="FOREGROUND" value="{fg}"/>
        <option name="BACKGROUND" value="{bg}"/>
      </value>
    </option>
  </attributes>
</scheme>
"#,
        parent = if t.dark { "Darcula" } else { "Default" },
        acc = hx(t.accent()),
        row = hx(c[1]),
        bg = hx(c[0]),
        sel = hx(t.selection()),
        fg = hx(t.ink()),
    );
    zip_stored(&[
        ("META-INF/plugin.xml", plugin.as_bytes()),
        ("golem.theme.json", serde_json::to_string_pretty(&theme).unwrap_or_default().as_bytes()),
        ("golem.xml", icls.as_bytes()),
    ])
}

/// A zip (a jar) of these files, stored without compression.
fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let at = out.len() as u32;
        let (n, len) = (name.len() as u16, data.len() as u32);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]); // version, flags, method, time, date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&n.to_le_bytes());
        central.extend_from_slice(&[0; 12]); // extra, comment, disk, attrs
        central.extend_from_slice(&at.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (start, size) = (out.len() as u32, central.len() as u32);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kde_scheme_replaces_only_its_own_sections() {
        let old = "[General]\nfont=x\n\n[Colors:View]\nBackgroundNormal=1,2,3\n\n[KDE]\nSingleClick=false\n";
        let ours = vec![("Colors:View".to_owned(), vec![("BackgroundNormal".to_owned(), "9,9,9".to_owned())])];
        let out = ini_with(old, &ours);
        assert!(out.contains("font=x") && out.contains("SingleClick=false"));
        assert!(out.contains("BackgroundNormal=9,9,9") && !out.contains("1,2,3"));
        // Written again, the same file: nothing grows at each change.
        assert_eq!(ini_with(&out, &ours), out);
        assert_eq!(ini_with(&ini_with("", &ours), &ours), "[Colors:View]\nBackgroundNormal=9,9,9\n");
        // (A file a reader left with blank lines at the top is tidied once.)
        assert_eq!(ini_with("\n\n\n", &ours), "[Colors:View]\nBackgroundNormal=9,9,9\n");
    }

    #[test]
    fn inkscape_takes_the_systems_theme() {
        let xml = r#"<group id="theme" gtkTheme="Adwaita" darkTheme="1" mygtkTheme="keep"/>"#;
        let out = set_attr(&set_attr(xml, "gtkTheme", "Golem-A"), "darkTheme", "0");
        assert_eq!(out, r#"<group id="theme" gtkTheme="Golem-A" darkTheme="0" mygtkTheme="keep"/>"#);
    }

    #[test]
    fn the_plugin_is_a_jar_with_its_files() {
        let z = zip_stored(&[("META-INF/plugin.xml", b"<idea-plugin/>"), ("a.json", b"{}")]);
        assert_eq!(&z[..4], b"PK\x03\x04");
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("META-INF/plugin.xml") && s.contains("<idea-plugin/>"));
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
    }
}

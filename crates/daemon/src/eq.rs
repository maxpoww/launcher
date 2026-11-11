//! THE EQUALIZER (Max, 2026-11-10: "quiero el estándar de la industria") —
//! one per output, two doors: the gear box's Equalizer row (presets and the
//! three tone controls, the phones' and TVs' standard) and Controls'
//! Equalizer page (the curve with its bands, the studio standard: FabFilter
//! Pro-Q, Logic, Ableton). Lab of the three directions each:
//! https://claude.ai/artifact/Pz4nbBHkVaEFSyg8X7hSMG.
//!
//! WHERE IT RUNS: inside the output's own PipeWire node, as its filter graph
//! 1 (`audioconvert.filter-graph.1`) — over graph 0, the model's speaker
//! tuning Golem loads (`system/Modular/desktop/speakers`). No second sink, no
//! process of ours in the sound path, builtin filters only.
//!
//! WHAT IS KEPT: WirePlumber's state file `golem-eq` (`[golem-eq]` then one
//! line per output, its node name = this module's [`Eq`] as JSON). Golem's
//! `equalizer/eq.lua` reads it and loads each output's graph when the
//! output appears; this module writes it and changes the live sound itself
//! (`pw-cli set-param` on the node) — the band's controls only while a hand
//! moves one, the whole graph when the shape changes.
//!
//! NEVER LOUDER: the bands may boost, so [`Eq::pre`] lowers everything by
//! the curve's highest point first; a boost changes the tone, never pushes a
//! peak past what came in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// The rate the curve is worked out at (the filters' own rate follows the
/// graph; the shape is the same at 44.1 kHz to a hair).
const FS: f64 = 48_000.0;
/// The curve's span: what a band may be moved to, and the drawing's edges.
pub(crate) const F_MIN: f32 = 20.0;
pub(crate) const F_MAX: f32 = 20_000.0;
/// A band's level reaches this far either way.
pub(crate) const G_MAX: f32 = 12.0;
/// A band's width (Q) stays between these.
pub(crate) const Q_MIN: f32 = 0.3;
pub(crate) const Q_MAX: f32 = 6.0;
/// The bands the curve starts with; the tone controls are the 1st, 3rd and
/// 5th of them.
pub(crate) const TONE_BANDS: [usize; 3] = [0, 2, 4];
/// A wheel notch over a band widens or narrows it by this factor.
pub(crate) const Q_STEP: f32 = 1.12;

/// A band's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// Low shelf: everything under the frequency up or down.
    Low,
    /// A bell (peak) around the frequency.
    Bell,
    /// High shelf: everything over the frequency up or down.
    High,
    /// High-pass: what is under the frequency goes (12 dB an octave).
    Cut,
}

impl Kind {
    pub(crate) const ALL: [Kind; 4] = [Kind::Low, Kind::Bell, Kind::High, Kind::Cut];

    /// Its name on the page (the shell's words, translated).
    pub(crate) fn word(self) -> &'static str {
        match self {
            Kind::Low => "Low shelf",
            Kind::Bell => "Bell",
            Kind::High => "High shelf",
            Kind::Cut => "Low cut",
        }
    }

    /// PipeWire's builtin filter for it.
    fn label(self) -> &'static str {
        match self {
            Kind::Low => "bq_lowshelf",
            Kind::Bell => "bq_peaking",
            Kind::High => "bq_highshelf",
            Kind::Cut => "bq_highpass",
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Kind::Low => "low",
            Kind::Bell => "bell",
            Kind::High => "high",
            Kind::Cut => "cut",
        }
    }

    pub(crate) fn from_key(k: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|t| t.key() == k)
    }
}

/// One band: its shape, frequency (Hz), level (dB) and width (Q).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Band {
    pub t: Kind,
    pub f: f32,
    pub g: f32,
    pub q: f32,
}

impl Band {
    /// Kept inside the curve's span.
    pub(crate) fn clamped(mut self) -> Band {
        self.f = self.f.clamp(F_MIN, F_MAX);
        self.g = if self.t == Kind::Cut { 0.0 } else { self.g.clamp(-G_MAX, G_MAX) };
        self.q = self.q.clamp(Q_MIN, Q_MAX);
        self
    }

    /// It does nothing (a shelf or bell left at 0 dB).
    pub(crate) fn flat(&self) -> bool {
        self.t != Kind::Cut && self.g.abs() < 0.05
    }

    /// Its level at `f` Hz, in dB.
    pub(crate) fn db_at(&self, f: f32) -> f64 {
        if self.flat() {
            return 0.0;
        }
        mag_db(&coeffs(self.t, self.f.into(), self.g.into(), self.q.into()), f.into())
    }
}

/// An output's equalizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Eq {
    /// On (the page's "Off" compares without it, keeping the bands).
    #[serde(default = "yes")]
    pub on: bool,
    /// dB in front of the bands: minus the curve's highest point (written
    /// for eq.lua, which does no arithmetic).
    #[serde(default)]
    pub pre: f32,
    pub bands: Vec<Band>,
}

fn yes() -> bool {
    true
}

/// The presets, by the gains of the five starting bands (low shelf 100 Hz,
/// bells at 400 Hz, 1.5 kHz, 4 kHz, high shelf 10 kHz). The names are the
/// shell's words (translated on the page).
pub(crate) const PRESETS: [(&str, [f32; 5]); 6] = [
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Voice", [-3.0, -1.0, 3.0, 2.0, -1.0]),
    ("Music", [2.0, -1.0, 0.0, 1.0, 2.0]),
    ("Films", [3.0, 0.0, 2.0, 0.0, 1.0]),
    ("Less bass", [-5.0, -2.0, 0.0, 0.0, 0.0]),
    ("More bass", [5.0, 1.0, 0.0, 0.0, 0.0]),
];

impl Default for Eq {
    fn default() -> Eq {
        Eq::flat()
    }
}

impl Eq {
    /// The five starting bands, all at 0 dB.
    pub(crate) fn flat() -> Eq {
        let b = |t, f, q| Band { t, f, g: 0.0, q };
        Eq {
            on: true,
            pre: 0.0,
            bands: vec![
                b(Kind::Low, 100.0, 0.7),
                b(Kind::Bell, 400.0, 1.0),
                b(Kind::Bell, 1500.0, 1.0),
                b(Kind::Bell, 4000.0, 1.0),
                b(Kind::High, 10_000.0, 0.7),
            ],
        }
    }

    /// Nothing to do: off, or every band flat.
    pub(crate) fn idle(&self) -> bool {
        !self.on || self.bands.iter().all(Band::flat)
    }

    /// The bands' level at `f` Hz (without `pre`), in dB.
    pub(crate) fn db_at(&self, f: f32) -> f64 {
        self.bands.iter().map(|b| b.db_at(f)).sum()
    }

    /// The curve's highest point over the span, in dB (0 when it only cuts).
    pub(crate) fn peak_db(&self) -> f32 {
        let mut top = 0.0_f64;
        for i in 0..=240 {
            top = top.max(self.db_at(f_at(i as f32 / 240.0)));
        }
        // (and at each band's own frequency, where a narrow bell peaks)
        for b in &self.bands {
            top = top.max(self.db_at(b.f));
        }
        top as f32
    }

    /// Settle `pre` from the bands (call after any change).
    pub(crate) fn settle(&mut self) {
        for b in &mut self.bands {
            *b = b.clamped();
        }
        self.pre = -self.peak_db();
    }

    /// The preset the bands are, if any (the five starting frequencies and
    /// shapes, and that preset's gains).
    pub(crate) fn preset(&self) -> Option<&'static str> {
        let base = Eq::flat();
        if self.bands.len() != base.bands.len()
            || self.bands.iter().zip(&base.bands).any(|(a, b)| a.t != b.t || (a.f - b.f).abs() > 0.5 || (a.q - b.q).abs() > 0.01)
        {
            return None;
        }
        PRESETS
            .iter()
            .find(|(_, gains)| self.bands.iter().zip(gains).all(|(b, g)| (b.g - g).abs() < 0.05))
            .map(|(name, _)| *name)
    }

    /// Set a preset (unknown names leave it be). "Flat" also brings the
    /// bands back to where they started.
    pub(crate) fn set_preset(&mut self, name: &str) {
        let Some((_, gains)) = PRESETS.iter().find(|(n, _)| *n == name) else { return };
        let mut fresh = Eq::flat();
        for (b, g) in fresh.bands.iter_mut().zip(gains) {
            b.g = *g;
        }
        fresh.on = true;
        *self = fresh;
        self.settle();
    }

    /// The three tone controls (bass, mids, treble): the levels of the
    /// first, third and fifth bands.
    pub(crate) fn tone(&self) -> [f32; 3] {
        TONE_BANDS.map(|i| self.bands.get(i).map_or(0.0, |b| b.g))
    }

    pub(crate) fn set_tone(&mut self, which: usize, g: f32) {
        if self.bands.len() < 5 {
            *self = Eq { on: self.on, ..Eq::flat() };
        }
        if let Some(b) = TONE_BANDS.get(which).and_then(|&i| self.bands.get_mut(i)) {
            b.g = g;
            if b.t == Kind::Cut {
                b.t = if which == 0 { Kind::Low } else if which == 2 { Kind::High } else { Kind::Bell };
            }
        }
        self.on = true;
        self.settle();
    }

    /// What the gear box's row says: the preset, Off, or "Custom".
    pub(crate) fn summary(&self) -> &'static str {
        if !self.on {
            return "Off";
        }
        self.preset().unwrap_or("Custom")
    }

    /// THE GRAPH THE DOCK LOADS: every band, flat ones too, named `b1…`, with
    /// `pre` in front — so a hand moving a band changes only its controls
    /// (`controls`), never the graph's shape. Mono: PipeWire runs one per
    /// channel.
    pub(crate) fn live_graph(&self) -> String {
        let mut nodes = vec![format!("{{ type = builtin name = pre label = linear control = {{ Mult = {} }} }}", num(db_mult(self.pre)))];
        let mut links = Vec::new();
        let mut prev = "pre".to_owned();
        for (i, b) in self.bands.iter().enumerate() {
            let name = format!("b{}", i + 1);
            nodes.push(format!(
                "{{ type = builtin name = {name} label = {} control = {{ Freq = {} Q = {}{} }} }}",
                b.t.label(),
                num(b.f.into()),
                num(b.q.into()),
                if b.t == Kind::Cut { String::new() } else { format!(" Gain = {}", num(b.g.into())) }
            ));
            links.push(format!("{{ output = \"{prev}:Out\" input = \"{name}:In\" }}"));
            prev = name;
        }
        format!("{{ nodes = [ {} ] links = [ {} ] }}", nodes.join(" "), links.join(" "))
    }

    /// The live graph's controls for the bands and `pre` as they are now
    /// (what a moved band sends; the shapes must be the loaded ones).
    pub(crate) fn controls(&self) -> Vec<(String, f64)> {
        let mut c = vec![("pre:Mult".to_owned(), db_mult(self.pre))];
        for (i, b) in self.bands.iter().enumerate() {
            let n = i + 1;
            c.push((format!("b{n}:Freq"), b.f.into()));
            c.push((format!("b{n}:Q"), b.q.into()));
            if b.t != Kind::Cut {
                c.push((format!("b{n}:Gain"), b.g.into()));
            }
        }
        c
    }

    /// The shape of the bands (what the loaded graph must match for
    /// `controls` to apply).
    pub(crate) fn shape(&self) -> Vec<Kind> {
        self.bands.iter().map(|b| b.t).collect()
    }
}

/// A frequency on the curve's span at `x` 0..1 (logarithmic).
pub(crate) fn f_at(x: f32) -> f32 {
    F_MIN * (F_MAX / F_MIN).powf(x.clamp(0.0, 1.0))
}

/// Where `f` sits on the span, 0..1.
pub(crate) fn x_of(f: f32) -> f32 {
    ((f.max(1.0) / F_MIN).ln() / (F_MAX / F_MIN).ln()).clamp(0.0, 1.0)
}

/// dB as a gain factor.
fn db_mult(db: f32) -> f64 {
    10f64.powf(f64::from(db) / 20.0)
}

/// A number as SPA-JSON reads it (Rust's own formatting: no locale).
fn num(x: f64) -> String {
    let s = format!("{x:.3}");
    if s == "-0.000" { "0.000".to_owned() } else { s }
}

/// RBJ's cookbook biquads — the same as PipeWire's builtin `bq_*`.
fn coeffs(t: Kind, f0: f64, g: f64, q: f64) -> [f64; 6] {
    let a = 10f64.powf(g / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * f0 / FS;
    let (s, c) = w0.sin_cos();
    let al = s / (2.0 * q);
    let sa = 2.0 * a.sqrt() * al;
    match t {
        Kind::Bell => [1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a],
        Kind::Low => [
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        ],
        Kind::High => [
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        ],
        Kind::Cut => [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al],
    }
}

/// A biquad's level at `f` Hz, in dB.
fn mag_db(co: &[f64; 6], f: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / FS;
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let (nr, ni) = (co[0] + co[1] * c1 + co[2] * c2, -(co[1] * s1 + co[2] * s2));
    let (dr, di) = (co[3] + co[4] * c1 + co[5] * c2, -(co[4] * s1 + co[5] * s2));
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).max(1e-30).log10()
}

// ----------------------------------------------------------- the drawing

/// THE CURVE ON THE PAGE (Controls → Equalizer, `eq_page.rs`): the output it
/// is of, its equalizer, the factory tuning drawn under it, the band picked.
/// The panel draws it and its hand moves the bands (`Panel::eq_press`…).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Curve {
    /// The output's node name (what is kept) and id (what is changed live).
    pub node: String,
    pub id: u32,
    pub eq: Eq,
    pub factory: Factory,
    pub sel: usize,
}

// ------------------------------------------------------------ the factory

/// THE MODEL'S OWN TUNING under an output (graph 0, Golem's
/// `speakers/tunings.nix`, listed for the shell in
/// `/etc/golem/speaker-tunings.json`): its level at `f`, drawn under the
/// owner's curve. Empty for an output with none.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Factory {
    /// (shape label, Freq, Q, Gain) per filter, and a fixed gain (dB).
    filters: Vec<(String, f64, f64, f64)>,
    gain_db: f64,
}

impl Factory {
    pub(crate) fn is_empty(&self) -> bool {
        self.filters.is_empty() && self.gain_db.abs() < 1e-6
    }

    pub(crate) fn db_at(&self, f: f32) -> f64 {
        let kind = |l: &str| match l {
            "bq_lowshelf" => Some(Kind::Low),
            "bq_peaking" => Some(Kind::Bell),
            "bq_highshelf" => Some(Kind::High),
            "bq_highpass" => Some(Kind::Cut),
            _ => None,
        };
        self.gain_db
            + self
                .filters
                .iter()
                .filter_map(|(l, fr, q, g)| kind(l).map(|k| mag_db(&coeffs(k, *fr, *g, *q), f.into())))
                .sum::<f64>()
    }

    /// From one tuning's graph (`{ nodes: [{ label, control }] }`).
    pub(crate) fn from_graph(graph: &serde_json::Value) -> Factory {
        let mut out = Factory::default();
        for n in graph["nodes"].as_array().into_iter().flatten() {
            let label = n["label"].as_str().unwrap_or_default();
            let c = &n["control"];
            let get = |k: &str, d: f64| c[k].as_f64().unwrap_or(d);
            if label == "linear" {
                let m = get("Mult", 1.0);
                if m > 0.0 {
                    out.gain_db += 20.0 * m.log10();
                }
            } else if label.starts_with("bq_") {
                out.filters.push((label.to_owned(), get("Freq", 1000.0), get("Q", 0.707), get("Gain", 0.0)));
            }
        }
        out
    }
}

/// The tuning Golem lays under the output named `node` whose card's
/// components are `components` (the speaker sink of a listed model).
pub(crate) fn factory_for(node: &str, components: &str) -> Factory {
    // (`GOLEM_SPEAKER_TUNINGS`: another list, for a test rig.)
    let path = std::env::var_os("GOLEM_SPEAKER_TUNINGS").map_or_else(|| PathBuf::from("/etc/golem/speaker-tunings.json"), PathBuf::from);
    let Ok(text) = std::fs::read_to_string(path) else { return Factory::default() };
    factory_in(&text, node, components)
}

pub(crate) fn factory_in(json: &str, node: &str, components: &str) -> Factory {
    let Ok(list) = serde_json::from_str::<serde_json::Value>(json) else { return Factory::default() };
    for t in list.as_array().into_iter().flatten() {
        let has = t["components"].as_str().unwrap_or("\u{0}");
        let suffix = t["node"].as_str().unwrap_or("HiFi__Speaker__sink");
        if components.contains(has) && node.ends_with(suffix) {
            return Factory::from_graph(&t["graph"]);
        }
    }
    Factory::default()
}

// ---------------------------------------------------------- what is kept

/// `$XDG_STATE_HOME/wireplumber/golem-eq` — WirePlumber's state file of
/// that name, read by Golem's eq.lua.
pub(crate) fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join(".local/state")
    });
    base.join("wireplumber/golem-eq")
}

/// Every output's equalizer in the file (lines that do not read are left
/// out; the file is a key file: `[golem-eq]`, then `name=json`).
pub(crate) fn parse_state(text: &str) -> BTreeMap<String, Eq> {
    let mut all = BTreeMap::new();
    let mut ours = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            ours = line == "[golem-eq]";
            continue;
        }
        if !ours || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if let Ok(eq) = serde_json::from_str::<Eq>(v) {
                all.insert(k.trim().to_owned(), eq);
            }
        }
    }
    all
}

pub(crate) fn render_state(all: &BTreeMap<String, Eq>) -> String {
    let mut out = String::from("[golem-eq]\n");
    for (name, eq) in all {
        // (A node name never holds '=' or a line break; one that did would
        // break the key file, so it is skipped.)
        if name.contains(['=', '\n', '[']) {
            continue;
        }
        if let Ok(j) = serde_json::to_string(eq) {
            out.push_str(name);
            out.push('=');
            out.push_str(&j);
            out.push('\n');
        }
    }
    out
}

/// The writes to the file, one at a time (the gear box's thread and the
/// control panel's worker both write).
static FILE: Mutex<()> = Mutex::new(());

pub(crate) fn load_all() -> BTreeMap<String, Eq> {
    let _held = FILE.lock();
    std::fs::read_to_string(state_path()).map(|t| parse_state(&t)).unwrap_or_default()
}

/// `node`'s equalizer (flat when it has none yet).
pub(crate) fn load(node: &str) -> Eq {
    load_all().remove(node).unwrap_or_default()
}

/// Keep `node`'s equalizer (an idle flat one is dropped from the file).
pub(crate) fn save(node: &str, eq: &Eq) {
    let _held = FILE.lock();
    let path = state_path();
    let mut all = std::fs::read_to_string(&path).map(|t| parse_state(&t)).unwrap_or_default();
    if eq.on && eq.bands.iter().all(Band::flat) && eq.preset().is_some() {
        all.remove(node);
    } else {
        all.insert(node.to_owned(), eq.clone());
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, render_state(&all)).and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
        warn!("eq: could not keep the equalizer in {}", path.display());
    }
}

// ------------------------------------------------------------- the sound

/// What the live thread is asked: this node's equalizer as it is now.
struct Want {
    id: u32,
    eq: Eq,
}

/// The shape loaded on each node by this process (`None`: the graph is
/// off). A node not listed may hold eq.lua's sparse graph, or none: it gets
/// the whole live graph first.
static LOADED: Mutex<BTreeMap<u32, Option<Vec<Kind>>>> = Mutex::new(BTreeMap::new());

static LIVE: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<Want>>> = std::sync::OnceLock::new();

/// Make node `id` sound like `eq` now: from a thread of its own that keeps
/// only the newest wish (a hand dragging a band sends what the hand says
/// now, not every step it passed).
pub(crate) fn live(id: u32, eq: &Eq) {
    let tx = LIVE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Want>();
        let _ = std::thread::Builder::new().name("eq-live".into()).spawn(move || {
            while let Ok(mut w) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    if newer.id != w.id {
                        apply_now(&w);
                    }
                    w = newer;
                }
                apply_now(&w);
            }
        });
        Mutex::new(tx)
    });
    if let Ok(tx) = tx.lock() {
        let _ = tx.send(Want { id, eq: eq.clone() });
    }
}

fn apply_now(w: &Want) {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let now = loaded.get(&w.id).cloned().flatten();
    let params = if w.eq.idle() {
        if loaded.get(&w.id) == Some(&None) {
            return;
        }
        loaded.insert(w.id, None);
        "{ params = [ \"audioconvert.filter-graph.1\" \"\" ] }".to_owned()
    } else if now.as_ref() == Some(&w.eq.shape()) {
        let c: Vec<String> = w.eq.controls().into_iter().map(|(k, v)| format!("\"{k}\" {}", num(v))).collect();
        format!("{{ params = [ {} ] }}", c.join(" "))
    } else {
        loaded.insert(w.id, Some(w.eq.shape()));
        let graph = serde_json::to_string(&w.eq.live_graph()).unwrap_or_default();
        format!("{{ params = [ \"audioconvert.filter-graph.1\" {graph} ] }}")
    };
    drop(loaded);
    debug!("eq: node {} ← {}", w.id, params.chars().take(160).collect::<String>());
    let ok = std::process::Command::new("pw-cli")
        .args(["set-param", &w.id.to_string(), "Props", &params])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        warn!("eq: pw-cli could not set node {}", w.id);
        LOADED.lock().unwrap_or_else(|e| e.into_inner()).remove(&w.id);
    }
}

// ----------------------------------------------------------------- words

/// "+3.0 dB", "−2.5 dB", "0 dB".
pub(crate) fn db_text(g: f32) -> String {
    if g.abs() < 0.05 {
        return "0 dB".to_owned();
    }
    format!("{}{:.1} dB", if g > 0.0 { "+" } else { "−" }, g.abs())
}

/// "100 Hz", "1.5 kHz", "10 kHz".
pub(crate) fn hz_text(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        let s = if k >= 10.0 { format!("{k:.0}") } else { format!("{k:.1}") };
        format!("{} kHz", s.trim_end_matches(".0"))
    } else {
        format!("{f:.0} Hz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn the_curve_is_the_cookbooks_and_matches_the_measured_graph() {
        // Measured on a file-backed ALSA sink through PipeWire (Golem's
        // eq.lua test, 2026-11-11): a low shelf +3 dB at 100 Hz, a bell
        // −2 dB at 1.5 kHz, a high shelf +1.5 dB at 10 kHz.
        let mut eq = Eq::flat();
        eq.bands[0].g = 3.0;
        eq.bands[2].g = -2.0;
        eq.bands[4].g = 1.5;
        assert!(near(eq.db_at(1500.0), -2.0, 0.05), "{}", eq.db_at(1500.0));
        assert!(near(eq.db_at(20.0), 3.0, 0.1));
        assert!(near(eq.db_at(20_000.0), 1.5, 0.15));
        // a flat band adds nothing, wherever it sits
        assert_eq!(Eq::flat().db_at(1234.0), 0.0);
    }

    #[test]
    fn a_boost_lowers_everything_first_so_nothing_gets_louder() {
        let mut eq = Eq::flat();
        eq.bands[2].g = 6.0;
        eq.settle();
        assert!(near(f64::from(eq.pre), -6.0, 0.1), "pre {}", eq.pre);
        for i in 0..=100 {
            let f = f_at(i as f32 / 100.0);
            assert!(eq.db_at(f) + f64::from(eq.pre) <= 0.05, "{f} Hz over");
        }
        let mut cut = Eq::flat();
        cut.bands[0].g = -6.0;
        cut.settle();
        assert_eq!(cut.pre, 0.0, "cutting needs no lowering");
    }

    #[test]
    fn presets_are_recognised_and_tone_is_bands_one_three_five() {
        let mut eq = Eq::flat();
        assert_eq!(eq.preset(), Some("Flat"));
        eq.set_preset("Voice");
        assert_eq!(eq.preset(), Some("Voice"));
        assert_eq!(eq.summary(), "Voice");
        assert_eq!(eq.tone(), [-3.0, 3.0, -1.0]);
        eq.set_tone(0, 4.0);
        assert_eq!(eq.preset(), None);
        assert_eq!(eq.summary(), "Custom");
        assert_eq!(eq.bands[0].g, 4.0);
        eq.on = false;
        assert_eq!(eq.summary(), "Off");
        eq.set_preset("Flat");
        assert_eq!(eq, Eq::flat());
    }

    #[test]
    fn the_live_graph_names_every_band_and_writes_no_comma() {
        let mut eq = Eq::flat();
        eq.bands[1].g = 2.5;
        eq.settle();
        let g = eq.live_graph();
        for n in ["name = pre", "name = b1", "name = b2", "name = b5", "\"b4:Out\" input = \"b5:In\""] {
            assert!(g.contains(n), "{n} in {g}");
        }
        assert!(!g.contains(','), "{g}");
        let c = eq.controls();
        assert!(c.iter().any(|(k, v)| k == "b2:Gain" && near(*v, 2.5, 1e-6)));
        assert!(c.iter().any(|(k, _)| k == "pre:Mult"));
    }

    #[test]
    fn the_state_file_round_trips_and_keeps_other_outputs() {
        let mut a = Eq::flat();
        a.set_preset("Music");
        let mut b = Eq::flat();
        b.on = false;
        b.bands[3].g = -4.0;
        let mut all = BTreeMap::new();
        all.insert("alsa_output.x.HiFi__Speaker__sink".to_owned(), a.clone());
        all.insert("bluez_output.20_18_5B_03_E5_47.1".to_owned(), b.clone());
        let text = render_state(&all);
        assert!(text.starts_with("[golem-eq]\n"));
        assert!(!text.contains('\\'), "a key file must not hold escapes: {text}");
        let back = parse_state(&format!("[other]\nx=1\n{text}"));
        assert_eq!(back.get("alsa_output.x.HiFi__Speaker__sink"), Some(&a));
        assert_eq!(back.get("bluez_output.20_18_5B_03_E5_47.1"), Some(&b));
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn the_factory_tuning_is_found_by_its_components_and_drawn() {
        let json = r#"[{"name":"lenovo","components":"HDA:10ec0287,17aa387e,","node":"HiFi__Speaker__sink",
            "graph":{"nodes":[{"label":"bq_highpass","control":{"Freq":60.0,"Q":0.707}},
                               {"label":"bq_lowshelf","control":{"Freq":180.0,"Q":0.707,"Gain":-6.0}},
                               {"label":"linear","control":{"Mult":0.891}}]}}]"#;
        let comp = "HDA:8086281f,80860101,00100000 HDA:10ec0287,17aa387e,00100002 cfg-dmics:4";
        let f = factory_in(json, "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink", comp);
        assert!(!f.is_empty());
        // the measured response of the 9i's tuning (lab, 2026-11-10)
        assert!(near(f.db_at(1000.0), -1.0, 0.1), "{}", f.db_at(1000.0));
        assert!(near(f.db_at(120.0), -6.2, 0.2), "{}", f.db_at(120.0));
        assert!(near(f.db_at(40.0), -14.8, 0.3), "{}", f.db_at(40.0));
        assert!(factory_in(json, "alsa_output.x.HiFi__Headphones__sink", comp).is_empty());
        assert!(factory_in(json, "alsa_output.x.HiFi__Speaker__sink", "HDA:10ec0287,17aa387d,0").is_empty());
    }

    #[test]
    fn words_for_levels_and_frequencies() {
        assert_eq!(db_text(0.0), "0 dB");
        assert_eq!(db_text(3.0), "+3.0 dB");
        assert_eq!(db_text(-2.5), "−2.5 dB");
        assert_eq!(hz_text(100.0), "100 Hz");
        assert_eq!(hz_text(1500.0), "1.5 kHz");
        assert_eq!(hz_text(10_000.0), "10 kHz");
        assert_eq!(hz_text(4000.0), "4 kHz");
    }
}

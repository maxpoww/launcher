//! Controls → EQUALIZER (2026-11-11; the studio standard: the curve with its
//! bands — `eq.rs` holds the model, the file and the live sound). The page:
//! which output (when there are several: each keeps its own), on or off (to
//! compare), THE CURVE (`Group::eq`, drawn and moved by the panel), the band
//! picked — its shape and width —, the presets, and what was lowered so
//! nothing saturates.
//!
//! The page is read on the controls worker; a pick runs here too. A band
//! DRAGGED on the curve changes the sound live from the UI thread
//! (`eq::live`) and, let go, is kept through `eq-set` here.

use std::sync::Mutex;

use crate::controls::{choice_of, group_of, line_of, out_of, ControlView, Group};
use crate::eq::{self, Curve, Eq, Kind};
use crate::i18n::{fill, tr};

/// The output the page is on (its node name; `None`: the default output).
static OUTPUT: Mutex<Option<String>> = Mutex::new(None);
/// The band picked on the curve.
static BAND: Mutex<usize> = Mutex::new(0);

/// A width step of the band's Wider / Narrower.
const WIDTH_STEP: f32 = 1.3;

/// An output the equalizer can be on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Output {
    pub id: u32,
    pub name: String,
    pub description: String,
    /// The card's `alsa.components` (how the factory tuning is found).
    pub components: String,
    pub default: bool,
}

/// The outputs PipeWire has now (pw-dump).
pub(crate) fn outputs() -> Vec<Output> {
    let dump = out_of("pw-dump", &[]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null);
    parse_outputs(&dump)
}

pub(crate) fn parse_outputs(dump: &serde_json::Value) -> Vec<Output> {
    let mut outs = Vec::new();
    let mut default = None;
    for o in dump.as_array().into_iter().flatten() {
        let ty = o["type"].as_str().unwrap_or_default();
        if ty.ends_with("Node") {
            let p = &o["info"]["props"];
            if p["media.class"].as_str() != Some("Audio/Sink") {
                continue;
            }
            let (Some(id), Some(name)) = (o["id"].as_u64(), p["node.name"].as_str()) else { continue };
            let description = p["node.description"].as_str().or_else(|| p["node.nick"].as_str()).unwrap_or(name).to_owned();
            outs.push(Output {
                id: u32::try_from(id).unwrap_or(u32::MAX),
                name: name.to_owned(),
                description,
                components: p["alsa.components"].as_str().unwrap_or_default().to_owned(),
                default: false,
            });
        } else if ty.ends_with("Metadata") && o["props"]["metadata.name"].as_str() == Some("default") {
            for m in o["metadata"].as_array().into_iter().flatten() {
                if m["key"].as_str() == Some("default.audio.sink") {
                    default = m["value"]["name"].as_str().map(str::to_owned);
                }
            }
        }
    }
    for o in &mut outs {
        o.default = Some(&o.name) == default.as_ref();
    }
    outs
}

/// The output the page is on: the one picked if it is still there, else the
/// default, else the first.
fn current(outs: &[Output]) -> Option<&Output> {
    let picked = OUTPUT.lock().ok().and_then(|g| g.clone());
    picked
        .and_then(|n| outs.iter().find(|o| o.name == n))
        .or_else(|| outs.iter().find(|o| o.default))
        .or_else(|| outs.first())
}

fn band() -> usize {
    BAND.lock().map(|b| *b).unwrap_or(0)
}

/// A name short enough for a word on the page.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

pub(crate) fn view(view: &mut ControlView) {
    let outs = outputs();
    let Some(out) = current(&outs).cloned() else {
        view.subtitle = tr("Nothing to play sound through.").to_owned();
        return;
    };
    let eq = eq::load(&out.name);
    let sel = band().min(eq.bands.len().saturating_sub(1));
    view.subtitle = format!("{} · {}", out.description, tr(eq.summary()));
    if outs.len() > 1 {
        let choices = outs.iter().map(|o| choice_of(short(&o.description, 22), o.name == out.name, o.name.clone())).collect();
        view.groups.push(group_of(None, "eq-output", choices));
    }
    view.groups.push(group_of(None, "eq-on", vec![choice_of(tr("Equalizer on"), eq.on, "on"), choice_of(tr("Off"), !eq.on, "off")]));
    let curve = Curve { node: out.name.clone(), id: out.id, eq: eq.clone(), factory: eq::factory_for(&out.name, &out.components), sel };
    let points = (0..eq.bands.len()).map(|i| choice_of(String::new(), i == sel, i.to_string())).collect();
    view.groups.push(Group { eq: Some(Box::new(curve)), ..group_of(None, "eq-curve", points) });
    if let Some(b) = eq.bands.get(sel) {
        let shapes = Kind::ALL.iter().map(|k| choice_of(tr(k.word()), b.t == *k, k.key())).collect();
        view.groups.push(group_of(None, "eq-type", shapes));
        let width = vec![choice_of(tr("Wider"), false, "wider"), choice_of(tr("Narrower"), false, "narrower")];
        view.groups.push(Group { buttons: true, ..group_of(None, "eq-width", width) });
    }
    let presets = eq::PRESETS.iter().map(|(n, _)| choice_of(tr(n), eq.preset() == Some(*n), *n)).collect();
    view.groups.push(group_of(None, "eq-preset", presets));
    let note = if !eq.on {
        tr("Off: the sound as it comes, to compare.").to_owned()
    } else if eq.pre < -0.05 {
        fill(tr("Everything is lowered {db} first, so nothing raised can saturate."), &[("db", &eq::db_text(-eq.pre.abs()))])
    } else {
        tr("Drag a point to move its band. The wheel over it makes it wider or narrower.").to_owned()
    };
    view.groups.push(line_of(note));
}

/// Change the equalizer of the output the page is on, keep it, and make the
/// sound so now.
fn change(f: impl FnOnce(&mut Eq)) {
    let outs = outputs();
    let Some(out) = current(&outs) else { return };
    let mut eq = eq::load(&out.name);
    f(&mut eq);
    eq.settle();
    eq::save(&out.name, &eq);
    eq::live(out.id, &eq);
}

/// A pick on the page (the controls worker). False: not this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "eq-output" => {
            if let Ok(mut o) = OUTPUT.lock() {
                *o = Some(value.to_owned());
            }
            if let Ok(mut b) = BAND.lock() {
                *b = 0;
            }
        }
        "eq-curve" => {
            if let (Ok(i), Ok(mut b)) = (value.parse::<usize>(), BAND.lock()) {
                *b = i;
            }
        }
        "eq-on" => change(|eq| eq.on = value == "on"),
        "eq-type" => {
            let sel = band();
            if let Some(k) = Kind::from_key(value) {
                change(|eq| {
                    if let Some(b) = eq.bands.get_mut(sel) {
                        b.t = k;
                        if k == Kind::Cut {
                            b.g = 0.0;
                            b.q = 0.707;
                        }
                    }
                    eq.on = true;
                });
            }
        }
        "eq-width" => {
            let sel = band();
            change(|eq| {
                if let Some(b) = eq.bands.get_mut(sel) {
                    b.q = if value == "wider" { b.q / WIDTH_STEP } else { b.q * WIDTH_STEP };
                }
            });
        }
        "eq-preset" => change(|eq| eq.set_preset(value)),
        // A band let go on the curve (or widened by the wheel): the whole
        // equalizer as the hand left it, for that output.
        "eq-set" => {
            let Some((node, json)) = value.split_once('\n') else { return true };
            let Ok(mut eq) = serde_json::from_str::<Eq>(json) else { return true };
            eq.settle();
            eq::save(node, &eq);
            if let Some(o) = outputs().into_iter().find(|o| o.name == node) {
                eq::live(o.id, &eq);
            }
        }
        // The gear box's switch, tone controls and presets on the default
        // output: `node|id|on|on`, `node|id|tone|1:-2.5`, `node|id|preset|Voice`.
        "eq-quick" => {
            let mut parts = value.splitn(4, '|');
            let (Some(node), Some(id), Some(what), Some(arg)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return true };
            let mut eq = eq::load(node);
            match what {
                "preset" => eq.set_preset(arg),
                "tone" => {
                    let Some((i, g)) = arg.split_once(':') else { return true };
                    let (Ok(i), Ok(g)) = (i.parse::<usize>(), g.parse::<f32>()) else { return true };
                    eq.set_tone(i, g);
                }
                "on" => eq.on = arg == "on",
                _ => return true,
            }
            eq.settle();
            eq::save(node, &eq);
            if let Ok(id) = id.parse::<u32>() {
                eq::live(id, &eq);
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_come_from_the_dump_with_the_default_marked() {
        let dump = serde_json::json!([
            { "id": 59, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Sink", "node.name": "alsa_output.x.HiFi__Speaker__sink",
                "node.description": "Speaker", "alsa.components": "HDA:10ec0287,17aa387e,00100002" } } },
            { "id": 37, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Sink", "node.name": "alsa_output.x.HiFi__HDMI1__sink", "node.description": "LG TV" } } },
            { "id": 80, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Source", "node.name": "mic" } } },
            { "id": 3, "type": "PipeWire:Interface:Metadata", "props": { "metadata.name": "default" },
              "metadata": [ { "key": "default.audio.sink", "value": { "name": "alsa_output.x.HiFi__HDMI1__sink" } } ] }
        ]);
        let outs = parse_outputs(&dump);
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].components, "HDA:10ec0287,17aa387e,00100002");
        assert!(!outs[0].default && outs[1].default);
        assert_eq!(current(&outs).map(|o| o.id), Some(37), "the default output first");
    }
}

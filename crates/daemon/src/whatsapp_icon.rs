//! WHATSAPP's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark, its
//! green bubble inside a white ring (the PLATE) and the white handset on it
//! (the PHONE), drawn apart so they move apart.
//!
//! A HOVER shakes the phone and turns the plate `HOVER_TURN` degrees, held
//! while the pointer stays and turned back when it leaves. A CLICK turns the
//! plate once round one way and the phone once round the other.
//!
//! As with Seam's compass (`compass_icon`) each part is drawn by resvg once a
//! look, on a low-priority thread: a frame a degree near rest, one every
//! `COARSE` degrees round the rest (only the spins go there, fast); a frame on
//! the dock is the phone laid over the plate.

use std::sync::{Arc, Mutex, OnceLock};

/// WhatsApp's entry on the dock (the webapp's `.desktop` id).
pub(crate) const WHATSAPP_ID: &str = "webapp-whatsapp";

/// Max's numbers from the lab: the plate's turn on hover (degrees, held),
/// how fast it gets there (s, an exponential approach), the shake (ms), the
/// click's turns (ms).
const HOVER_TURN: f32 = 15.0;
const TURN_TAU: f32 = 0.08;
const SHAKE_MS: f32 = 700.0;
const SPIN_MS: f32 = 900.0;
/// The shake: (share of `SHAKE_MS`, degrees), eased between.
const SHAKE: [(f32, f32); 8] = [
    (0.0, 0.0),
    (0.10, -24.0),
    (0.24, 22.0),
    (0.38, -18.0),
    (0.52, 13.0),
    (0.66, -8.0),
    (0.82, 3.0),
    (1.0, 0.0),
];

/// The logo's paths, on simple-icons' 24-unit square.
const INNER: &str = "M12.05 21.785h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884z";
const RING: &str = "M12.05 21.785h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884m8.413-18.297A11.815 11.815 0 0012.05 0C5.495 0 .16 5.335.157 11.892c0 2.096.547 4.142 1.588 5.945L.057 24l6.305-1.654a11.882 11.882 0 005.683 1.448h.005c6.554 0 11.89-5.335 11.893-11.893a11.821 11.821 0 00-3.48-8.413Z";
const PHONE: &str = "M17.472 14.382c-.297-.149-1.758-.867-2.03-.967-.273-.099-.471-.148-.67.15-.197.297-.767.966-.94 1.164-.173.199-.347.223-.644.075-.297-.15-1.255-.463-2.39-1.475-.883-.788-1.48-1.761-1.653-2.059-.173-.297-.018-.458.13-.606.134-.133.298-.347.446-.52.149-.174.198-.298.298-.497.099-.198.05-.371-.025-.52-.075-.149-.669-1.612-.916-2.207-.242-.579-.487-.5-.669-.51-.173-.008-.371-.01-.57-.01-.198 0-.52.074-.792.372-.272.297-1.04 1.016-1.04 2.479 0 1.462 1.065 2.875 1.213 3.074.149.198 2.096 3.2 5.077 4.487.709.306 1.262.489 1.694.625.712.227 1.36.195 1.871.118.571-.085 1.758-.719 2.006-1.413.248-.694.248-1.289.173-1.413-.074-.124-.272-.198-.57-.347";
/// Where each part turns, in the figure's 100 (the logo fitted to 80 of
/// them, as every theme icon is on the dock): the bubble's centre, the
/// handset's.
const PLATE_AT: (f32, f32) = (50.17, 49.67);
const PHONE_AT: (f32, f32) = (50.17, 50.0);

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Whole,
    Plate,
    Phone,
}

/// The figure with the plate turned `plate` and the phone `phone` degrees.
fn figure(plate: f32, phone: f32, part: Part) -> String {
    let fit = r#"transform="translate(10 10) scale(3.33333)""#;
    let plate_g = format!(
        r##"<g transform="rotate({plate:.2} {px} {py})"><g {fit}><path fill="url(#wa)" d="{INNER}"/><path fill="#fff" fill-rule="evenodd" d="{RING}"/></g></g>"##,
        px = PLATE_AT.0,
        py = PLATE_AT.1
    );
    let phone_g = format!(
        r##"<g transform="rotate({phone:.2} {px} {py})"><g {fit}><path fill="#fff" d="{PHONE}"/></g></g>"##,
        px = PHONE_AT.0,
        py = PHONE_AT.1
    );
    let body = match part {
        Part::Whole => format!("{plate_g}{phone_g}"),
        Part::Plate => plate_g,
        Part::Phone => phone_g,
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs><linearGradient id="wa" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#60d669"/><stop offset="1" stop-color="#1fae38"/></linearGradient></defs>{body}</svg>"##
    )
}

/// The figure at rest.
pub(crate) fn rest_svg() -> String {
    figure(0.0, 0.0, Part::Whole)
}

/// THE FRAMES, `FRAME` square.
pub(crate) const FRAME: u32 = 128;
/// A frame a degree this near rest (the plate's held turn, the shake), one
/// every `COARSE` degrees round the rest.
const FINE_PLATE: (i32, i32) = (-2, HOVER_TURN as i32 + 2);
const FINE_PHONE: i32 = 26;
const COARSE: i32 = 3;

type Px = Arc<Vec<u8>>;

struct Frames {
    look: u32,
    /// By degree (0..360; only the drawn ones).
    plate: Vec<Option<Px>>,
    phone: Vec<Option<Px>>,
}

fn cache() -> &'static Mutex<Frames> {
    static CACHE: OnceLock<Mutex<Frames>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Frames {
            look: u32::MAX,
            plate: Vec::new(),
            phone: Vec::new(),
        })
    })
}

/// The drawn degree nearest `angle` (0..360), a degree a frame within
/// `lo..=hi` of rest.
fn key(angle: f32, lo: i32, hi: i32) -> usize {
    let a = (angle.round() as i32).rem_euclid(360);
    let off = (a + 180).rem_euclid(360) - 180;
    if (lo..=hi).contains(&off) {
        return a as usize;
    }
    ((((a as f32) / COARSE as f32).round() as i32 * COARSE).rem_euclid(360)) as usize
}

fn plate_key(angle: f32) -> usize {
    key(angle, FINE_PLATE.0, FINE_PLATE.1)
}

fn phone_key(angle: f32) -> usize {
    key(angle, -FINE_PHONE, FINE_PHONE)
}

/// Draw the frames for the present look, unless they are (being) drawn.
pub(crate) fn warm() {
    let look = crate::look::generation();
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.look == look {
            return;
        }
        c.look = look;
        c.plate = vec![None; 360];
        c.phone = vec![None; 360];
    }
    let spawned = std::thread::Builder::new()
        .name("whatsapp-frames".into())
        .spawn(move || {
            // SAFETY: setpriority on the calling thread only.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 15) };
            // The hover's first (from rest outward), then the spins'.
            let mut work: Vec<(bool, f32)> = Vec::new();
            for d in 0..=FINE_PHONE {
                for s in [d, -d] {
                    if (FINE_PLATE.0..=FINE_PLATE.1).contains(&s) {
                        work.push((true, s as f32));
                    }
                    work.push((false, s as f32));
                }
            }
            for a in (0..360).step_by(COARSE as usize) {
                work.push((true, a as f32));
                work.push((false, a as f32));
            }
            for (is_plate, angle) in work {
                let k = if is_plate { plate_key(angle) } else { phone_key(angle) };
                {
                    let Ok(c) = cache().lock() else { return };
                    if c.look != look {
                        return;
                    }
                    let slot = if is_plate { &c.plate } else { &c.phone };
                    if slot.get(k).is_some_and(Option::is_some) {
                        continue;
                    }
                }
                let svg = if is_plate {
                    figure(angle, 0.0, Part::Plate)
                } else {
                    figure(0.0, angle, Part::Phone)
                };
                let Some(px) = draw(&svg) else { continue };
                let Ok(mut c) = cache().lock() else { return };
                if c.look != look {
                    return;
                }
                let slot = if is_plate { &mut c.plate } else { &mut c.phone };
                if let Some(s) = slot.get_mut(k) {
                    *s = Some(Arc::new(px));
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("whatsapp: no frame thread: {e}");
    }
}

fn draw(svg: &str) -> Option<Vec<u8>> {
    let tree =
        resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(FRAME, FRAME)?;
    let k = FRAME as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(k, k),
        &mut pixmap.as_mut(),
    );
    Some(pixmap.take())
}

/// The phone laid over the plate (premultiplied).
fn lay(phone: &[u8], plate: &[u8]) -> Vec<u8> {
    phone
        .chunks_exact(4)
        .zip(plate.chunks_exact(4))
        .flat_map(|(n, p)| {
            let keep = 255 - n[3] as u32;
            let px = |i: usize| (n[i] as u32 + (p[i] as u32 * keep + 127) / 255).min(255) as u8;
            [px(0), px(1), px(2), px(3)]
        })
        .collect()
}

/// The frame with the plate at `plate` and the phone at `phone`, if both are
/// drawn yet.
pub(crate) fn frame(plate: f32, phone: f32) -> Option<Vec<u8>> {
    let (p, n) = {
        let c = cache().lock().ok()?;
        (
            c.plate.get(plate_key(plate)).cloned().flatten()?,
            c.phone.get(phone_key(phone)).cloned().flatten()?,
        )
    };
    Some(lay(&n, &p))
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

/// The shake's angle `t` ms in.
fn shake(t: f32) -> f32 {
    let x = (t / SHAKE_MS).clamp(0.0, 1.0);
    for w in SHAKE.windows(2) {
        let ((x0, a0), (x1, a1)) = (w[0], w[1]);
        if x <= x1 {
            let s = (x - x0) / (x1 - x0);
            return a0 + (a1 - a0) * s * s * (3.0 - 2.0 * s);
        }
    }
    0.0
}

/// WhatsApp's motion: a hover shakes the phone and turns the plate a little,
/// a click turns both round, each its own way.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    /// The plate's held turn (toward `HOVER_TURN` while hovered, else 0).
    turn: f32,
    /// The shake, `t` ms in.
    shake: Option<f32>,
    /// The click's turns, `t` ms in.
    spin: Option<f32>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on WhatsApp's icon.
    pub(crate) fn click(&mut self) {
        self.spin = Some(0.0);
        self.shake = None;
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && self.spin.is_none() {
            self.shake = Some(0.0);
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        let target = if hover { HOVER_TURN } else { 0.0 };
        let turning = (self.turn - target).abs() > 0.05;
        self.turn = if turning {
            self.turn + (target - self.turn) * (1.0 - (-dt / TURN_TAU).exp())
        } else {
            target
        };
        self.shake = self.shake.map(|t| t + ms).filter(|t| *t < SHAKE_MS);
        self.spin = self.spin.map(|t| t + ms).filter(|t| *t < SPIN_MS);
        let now = turning || self.shake.is_some() || self.spin.is_some();
        let moved = now || self.moving;
        self.moving = now;
        moved
    }

    /// The plate's angle and the phone's now.
    pub(crate) fn pose(&self) -> (f32, f32) {
        let spin = self.spin.map(|t| 360.0 * ease(t / SPIN_MS)).unwrap_or(0.0);
        let phone = self.shake.map(shake).unwrap_or(0.0) + spin;
        (self.turn - spin, phone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_parses() {
        for part in [Part::Whole, Part::Plate, Part::Phone] {
            let svg = figure(10.0, -5.0, part);
            assert!(
                resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default())
                    .is_ok()
            );
        }
    }

    #[test]
    fn the_parts_laid_together_are_the_figure() {
        let (p, n) = (
            draw(&figure(7.0, 0.0, Part::Plate)).unwrap(),
            draw(&figure(0.0, -12.0, Part::Phone)).unwrap(),
        );
        let whole = draw(&figure(7.0, -12.0, Part::Whole)).unwrap();
        let worst = lay(&n, &p)
            .iter()
            .zip(&whole)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 6, "off by {worst}");
    }

    #[test]
    fn a_hover_shakes_the_phone_and_holds_the_plate_turned() {
        let mut m = Motion::default();
        let mut most = 0.0f32;
        for _ in 0..100 {
            m.step(true, 0.016);
            most = most.max(m.pose().1.abs());
        }
        assert!(most > 20.0, "it shook ({most})");
        assert_eq!(m.pose(), (HOVER_TURN, 0.0), "the plate held, the phone still");
        for _ in 0..100 {
            m.step(false, 0.016);
        }
        assert_eq!(m.pose(), (0.0, 0.0), "turned back on leaving");
        assert!(!m.step(false, 0.016));
    }

    #[test]
    fn a_click_turns_them_round_opposite_ways() {
        let mut m = Motion::default();
        m.click();
        let (mut plate, mut phone) = (0.0f32, 0.0f32);
        for _ in 0..100 {
            m.step(false, 0.016);
            let (p, n) = m.pose();
            plate = plate.min(p);
            phone = phone.max(n);
        }
        assert!(plate < -340.0 && phone > 340.0, "{plate} {phone}");
        assert_eq!(m.pose(), (0.0, 0.0));
    }

    #[test]
    fn every_pose_has_a_drawn_degree() {
        assert_eq!(plate_key(15.2), 15);
        assert_eq!(plate_key(-200.0) % COARSE as usize, 0);
        assert_eq!(phone_key(-24.0), 336);
        assert_eq!(phone_key(400.0), 39);
    }
}

//! Which part of a surface a frame changed, so the present can say so.
//!
//! Every frame of every surface used to be presented as "all of it changed".
//! The shell's surfaces are far larger than what they draw (the OPTIONS bar's
//! is 510 px tall for a 28 px bar, so its boxes have room to drop; the dock's
//! holds the whole launcher), and the compositor believed it: for each frame
//! of a hover or a box opening it redrew — and blurred again — everything
//! behind the whole surface. On the Acer that was two thirds of Hyprland's GPU
//! time during the shell's animations (round 3, 2026-10-04).
//!
//! The renderer redraws the whole image every frame, so nothing here decides
//! what is DRAWN. It only works out what came out different: the surface is
//! cut into [`TILE`]-px tiles and everything drawn folds a hash of itself
//! into each tile it can touch, in draw order. A tile whose hash equals the
//! last presented frame's shows the same pixels; the others are the damage.
//!
//! The rule that keeps it honest: a draw's hash must cover EVERYTHING its
//! pixels depend on (instance data, clip, the uniforms its shader reads, the
//! texture it samples), and its box must cover every pixel it can write.
//! Over-estimating either only costs area on the frame that draw changes;
//! under-estimating leaves stale pixels on screen. `WAVERUNNER_DAMAGE_CHECK=1`
//! reads every frame back and compares (see `Renderer::check_damage`).

/// Side of a tile, physical px.
pub(crate) const TILE: u32 = 32;

/// More rectangles than this are replaced by their bounding box: the
/// compositor scissors every element once per rectangle.
const MAX_RECTS: usize = 16;

/// `[x, y, width, height]`, physical px, origin top-left.
pub(crate) type Rect = [i32; 4];

const K: u64 = 0x9E37_79B9_7F4A_7C15;

/// Mix two words (a folded 128-bit multiply: every input bit reaches every
/// output bit).
#[inline]
pub(crate) fn mix(a: u64, b: u64) -> u64 {
    let m = u128::from(a ^ K).wrapping_mul(u128::from(b ^ 0xD6E8_FEB8_6659_FD93));
    (m as u64) ^ ((m >> 64) as u64)
}

/// Hash `bytes` onto `seed`.
pub(crate) fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = mix(seed, bytes.len() as u64);
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = mix(h, v);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        h = mix(h, u64::from_le_bytes(last));
    }
    h
}

/// One hash per tile of a surface: 0 where nothing was drawn.
#[derive(Clone, Default)]
pub(crate) struct TileMap {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    tiles: Vec<u64>,
}

impl TileMap {
    /// Empty the map and size it for a `width` x `height` px surface.
    pub(crate) fn reset(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.cols = width.div_ceil(TILE);
        self.rows = height.div_ceil(TILE);
        self.tiles.clear();
        self.tiles.resize((self.cols * self.rows) as usize, 0);
    }

    pub(crate) fn same_size(&self, other: &TileMap) -> bool {
        self.width == other.width && self.height == other.height
    }

    /// The tile range a pixel box touches (inclusive), or `None` when it
    /// misses the surface. The box is rounded outward; a NaN edge counts as
    /// the surface's edge, so a broken box damages more, never less.
    fn span(&self, b: [f32; 4]) -> Option<(u32, u32, u32, u32)> {
        if self.cols == 0 || self.rows == 0 {
            return None;
        }
        let x0 = b[0].floor().max(0.0);
        let y0 = b[1].floor().max(0.0);
        let x1 = b[2].ceil().min(self.width as f32);
        let y1 = b[3].ceil().min(self.height as f32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((
            x0 as u32 / TILE,
            y0 as u32 / TILE,
            (x1 as u32 - 1) / TILE,
            (y1 as u32 - 1) / TILE,
        ))
    }

    #[inline]
    fn fold(tile: &mut u64, hash: u64) {
        // Order matters (what is drawn later covers what was drawn before),
        // and a drawn tile is never 0.
        *tile = (tile.rotate_left(23) ^ hash).wrapping_mul(K) | 1;
    }

    /// Something drew inside `b` (`[x0, y0, x1, y1]`, physical px): fold its
    /// `hash` into every tile the box touches.
    pub(crate) fn mark(&mut self, b: [f32; 4], hash: u64) {
        let Some((c0, r0, c1, r1)) = self.span(b) else {
            return;
        };
        for row in r0..=r1 {
            let at = (row * self.cols) as usize;
            for tile in &mut self.tiles[at + c0 as usize..=at + c1 as usize] {
                Self::fold(tile, hash);
            }
        }
    }

    /// As [`TileMap::mark`], for a draw whose pixels inside `inner` depend on
    /// less than the rest of it: tiles wholly inside `inner` take `hash`, the
    /// others `edge_hash`.
    pub(crate) fn mark_split(&mut self, b: [f32; 4], inner: [f32; 4], hash: u64, edge_hash: u64) {
        let Some((c0, r0, c1, r1)) = self.span(b) else {
            return;
        };
        let tile = TILE as f32;
        for row in r0..=r1 {
            let (top, bottom) = (row as f32 * tile, (row + 1) as f32 * tile);
            let row_inside = top >= inner[1] && bottom <= inner[3];
            let at = (row * self.cols) as usize;
            for col in c0..=c1 {
                let (left, right) = (col as f32 * tile, (col + 1) as f32 * tile);
                let inside = row_inside && left >= inner[0] && right <= inner[2];
                Self::fold(
                    &mut self.tiles[at + col as usize],
                    if inside { hash } else { edge_hash },
                );
            }
        }
    }

    /// One hash of everything marked so far.
    pub(crate) fn digest(&self) -> u64 {
        self.tiles.iter().fold(K, |h, t| mix(h, *t))
    }

    /// The tiles that differ from `prev` (the same surface, one frame
    /// earlier), as few rectangles. Empty: the two frames show the same.
    pub(crate) fn changed(&self, prev: &TileMap) -> Vec<Rect> {
        self.rects(|i| self.tiles[i] != prev.tiles[i])
    }

    /// The tiles anything was drawn in, as few rectangles. Empty: the frame
    /// draws nothing.
    pub(crate) fn drawn(&self) -> Vec<Rect> {
        self.rects(|i| self.tiles[i] != 0)
    }

    /// The tiles `wanted` picks (by index), as few rectangles.
    fn rects(&self, wanted: impl Fn(usize) -> bool) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        // Runs of changed tiles still growing downward: (col0, col1, row0),
        // col1 exclusive.
        let mut open: Vec<(u32, u32, u32)> = Vec::new();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let cols = self.cols as usize;
        for row in 0..=self.rows {
            runs.clear();
            if row < self.rows {
                let at = row as usize * cols;
                let mut col = 0;
                while col < cols {
                    if !wanted(at + col) {
                        col += 1;
                        continue;
                    }
                    let start = col;
                    while col < cols && wanted(at + col) {
                        col += 1;
                    }
                    runs.push((start as u32, col as u32));
                }
            }
            // A run with the same columns as one above continues it; the
            // open ones this row does not continue are finished rectangles.
            open.retain(|&(c0, c1, r0)| {
                let continues = runs.contains(&(c0, c1));
                if !continues {
                    out.push(self.px(c0, r0, c1, row));
                }
                continues
            });
            for &(c0, c1) in &runs {
                if !open.iter().any(|o| o.0 == c0 && o.1 == c1) {
                    open.push((c0, c1, row));
                }
            }
        }
        if out.len() > MAX_RECTS {
            let x0 = out.iter().map(|r| r[0]).min().unwrap_or(0);
            let y0 = out.iter().map(|r| r[1]).min().unwrap_or(0);
            let x1 = out.iter().map(|r| r[0] + r[2]).max().unwrap_or(0);
            let y1 = out.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
            return vec![[x0, y0, x1 - x0, y1 - y0]];
        }
        out
    }

    /// Tile rectangle (columns `c0..c1`, rows `r0..r1`) in px, cut to the
    /// surface.
    fn px(&self, c0: u32, r0: u32, c1: u32, r1: u32) -> Rect {
        let x0 = c0 * TILE;
        let y0 = r0 * TILE;
        let x1 = (c1 * TILE).min(self.width);
        let y1 = (r1 * TILE).min(self.height);
        [x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32]
    }
}

/// Is the pixel inside any of `rects`?
#[inline]
pub(crate) fn covers(rects: &[Rect], x: i32, y: i32) -> bool {
    rects
        .iter()
        .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(w: u32, h: u32) -> TileMap {
        let mut m = TileMap::default();
        m.reset(w, h);
        m
    }

    #[test]
    fn identical_frames_have_no_damage() {
        let mut a = map(300, 200);
        let mut b = map(300, 200);
        for m in [&mut a, &mut b] {
            m.mark([10.0, 10.0, 120.0, 40.0], 7);
            m.mark([100.0, 30.0, 290.0, 190.0], 9);
        }
        assert!(a.changed(&b).is_empty());
    }

    #[test]
    fn a_change_damages_the_tiles_it_touches_and_only_those() {
        let mut a = map(320, 320);
        let mut b = map(320, 320);
        a.mark([0.0, 0.0, 320.0, 320.0], 1);
        b.mark([0.0, 0.0, 320.0, 320.0], 1);
        // A small thing appears in frame b, inside tile (2, 3).
        b.mark([70.0, 100.0, 90.0, 120.0], 5);
        assert_eq!(b.changed(&a), vec![[64, 96, 32, 32]]);
        // …and it is damaged again when it goes away.
        assert_eq!(a.changed(&b), vec![[64, 96, 32, 32]]);
    }

    #[test]
    fn draw_order_is_part_of_the_hash() {
        let mut a = map(64, 64);
        let mut b = map(64, 64);
        a.mark([0.0, 0.0, 30.0, 30.0], 1);
        a.mark([0.0, 0.0, 30.0, 30.0], 2);
        b.mark([0.0, 0.0, 30.0, 30.0], 2);
        b.mark([0.0, 0.0, 30.0, 30.0], 1);
        assert_eq!(a.changed(&b), vec![[0, 0, 32, 32]]);
    }

    #[test]
    fn boxes_round_outward_and_are_cut_to_the_surface() {
        let mut a = map(100, 50);
        let b = map(100, 50);
        // 31.5..32.5 straddles two columns; the surface's last column is 4 px.
        a.mark([31.5, 0.0, 32.5, 1.0], 3);
        a.mark([97.0, 40.0, 500.0, 500.0], 3);
        let mut got = a.changed(&b);
        got.sort();
        assert_eq!(got, vec![[0, 0, 64, 32], [96, 32, 4, 18]]);
        // Off the surface, inverted, or empty: nothing.
        let mut c = map(100, 50);
        c.mark([-50.0, -50.0, -1.0, -1.0], 3);
        c.mark([200.0, 0.0, 300.0, 10.0], 3);
        c.mark([10.0, 10.0, 10.0, 40.0], 3);
        assert!(c.changed(&b).is_empty());
    }

    #[test]
    fn a_broken_box_damages_more_never_less() {
        let mut a = map(100, 50);
        let b = map(100, 50);
        a.mark([f32::NAN, f32::NAN, f32::NAN, f32::NAN], 3);
        assert_eq!(a.changed(&b), vec![[0, 0, 100, 50]]);
    }

    #[test]
    fn a_ring_is_four_rectangles() {
        let mut a = map(320, 320);
        let mut b = map(320, 320);
        a.mark_split([0.0, 0.0, 320.0, 320.0], [40.0, 40.0, 280.0, 280.0], 1, 2);
        b.mark_split([0.0, 0.0, 320.0, 320.0], [40.0, 40.0, 280.0, 280.0], 1, 3);
        let mut got = b.changed(&a);
        got.sort();
        // Tiles wholly inside 40..280 are columns/rows 2..=7 (64..256).
        assert_eq!(
            got,
            vec![
                [0, 0, 320, 64],
                [0, 64, 64, 192],
                [0, 256, 320, 64],
                [256, 64, 64, 192]
            ]
        );
    }

    #[test]
    fn too_many_rectangles_become_their_bounding_box() {
        let mut a = map(32 * 40, 32 * 40);
        let b = map(32 * 40, 32 * 40);
        for i in 0..20 {
            let at = (i * 64) as f32;
            a.mark([at, at, at + 8.0, at + 8.0], 1);
        }
        assert_eq!(a.changed(&b), vec![[0, 0, 19 * 64 + 32, 19 * 64 + 32]]);
    }

    #[test]
    fn drawn_is_where_anything_was_marked() {
        let mut a = map(320, 96);
        assert!(a.drawn().is_empty());
        // A bar across the top and a box hanging from it.
        a.mark([0.0, 0.0, 320.0, 20.0], 1);
        a.mark([70.0, 20.0, 150.0, 90.0], 2);
        let mut got = a.drawn();
        got.sort();
        assert_eq!(got, vec![[0, 0, 320, 32], [64, 32, 96, 64]]);
    }

    #[test]
    fn hashing_sees_every_byte_and_the_length() {
        let a = hash_bytes(1, b"abcdefghij");
        assert_ne!(a, hash_bytes(1, b"abcdefghik"));
        assert_ne!(a, hash_bytes(1, b"abcdefghi"));
        assert_ne!(a, hash_bytes(2, b"abcdefghij"));
        assert_ne!(hash_bytes(1, &[0, 0]), hash_bytes(1, &[0, 0, 0]));
        assert_eq!(a, hash_bytes(1, b"abcdefghij"));
    }

    #[test]
    fn covers_is_half_open() {
        let r = [[10, 10, 5, 5]];
        assert!(covers(&r, 10, 10) && covers(&r, 14, 14));
        assert!(!covers(&r, 15, 10) && !covers(&r, 9, 10));
    }
}

//! Dev tool: turn a raw square RGBA capture from the deck's thumbnail path into
//! a PNG, so a capture can actually be looked at rather than guessed about.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (src, dst, n) = (&a[1], &a[2], a[3].parse::<u32>().unwrap());
    let raw = std::fs::read(src).expect("read raw");
    image::save_buffer(dst, &raw, n, n, image::ColorType::Rgba8).expect("write png");
    println!("wrote {dst}");
}

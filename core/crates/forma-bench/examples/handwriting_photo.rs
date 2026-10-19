//! A synthetic phone photo of a handwritten alphabet: Patrick Hand (OFL)
//! letters, each turned, scaled and lifted a little, in marker ink on
//! off-white paper, with uneven light, a darker corner, a slight tilt,
//! noise and JPEG compression.
//! `cargo run -p forma-bench --example handwriting_photo -- <out.jpg>`

use forma_bench::degrade::{add_noise, blur_rgb, encode_jpeg};
use forma_bench::render_font::{font_bytes, outline};
use forma_bench::scene::{Scene, SceneGlyph};
use kurbo::{Affine, Shape};

const ROWS: [&str; 5] = [
    "ABCDEFGHI",
    "JKLMNÑOPQ",
    "RSTUVWXYZ",
    "0123456789",
    ".,:;!?¡¿-",
];

/// A small deterministic pseudo-random sequence.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / f64::from(1u32 << 31) * 2.0 - 1.0
    }
}

fn main() {
    let out = std::env::args().nth(1).expect("out path");
    let bytes = font_bytes("patrick-hand").expect("font");
    let cap = 130.0;
    let scale = cap / outline(bytes, 'H').unwrap().path.bounding_box().height();
    let margin = 110.0;
    let mut rng = Lcg(0x5eed_f0a5);
    let mut glyphs = Vec::new();
    let mut width: f64 = 0.0;
    for (r, row) in ROWS.iter().enumerate() {
        let baseline = margin + cap * 1.25 + cap * 1.75 * r as f64;
        let mut pen = margin;
        for ch in row.chars() {
            let o = outline(bytes, ch).unwrap();
            let s = scale * (1.0 + 0.07 * rng.next());
            let turn = 0.07 * rng.next();
            let lift = 6.0 * rng.next();
            let b = o.path.bounding_box();
            let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            let place = Affine::translate((pen + cx * s, baseline + lift - cy * s))
                * Affine::rotate(turn)
                * Affine::new([s, 0.0, 0.0, -s, -cx * s, cy * s]);
            glyphs.push(SceneGlyph {
                key: ch.to_string(),
                shapes: vec![place * o.path],
                origin: None,
                advance: None,
            });
            pen += o.advance * s + cap * (0.35 + 0.08 * rng.next());
        }
        width = width.max(pen);
    }
    let height = margin * 2.0 + cap * 1.25 + cap * 1.75 * (ROWS.len() - 1) as f64 + 0.4 * cap;
    let layout = ROWS.iter().map(|r| (*r).to_owned()).collect();
    let mut scene = Scene::new((width + margin).ceil() as u32, height.ceil() as u32, layout);
    scene.glyphs = glyphs;
    scene.look.paper = [236, 230, 216];
    scene.look.ink = [28, 36, 92];
    scene.look.light = Some((0.80, 1.0));
    let scene = scene.rotated(1.8);
    let mut img = scene.render().unwrap();
    // A darker lower-left corner, as from the phone's shadow.
    let (w, h) = (img.width() as f64, img.height() as f64);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let d = ((x as f64 / w).powi(2) + (1.0 - y as f64 / h).powi(2)).sqrt();
        let shade = 1.0 - 0.18 * (1.0 - d.min(1.0));
        for c in px.0.iter_mut() {
            *c = (f64::from(*c) * shade).round() as u8;
        }
    }
    let mut img = blur_rgb(&img, 0.9);
    add_noise(&mut img, 0.05, 7);
    std::fs::write(&out, encode_jpeg(&img, 78).unwrap()).unwrap();
    println!("{} x {}", img.width(), img.height());
}

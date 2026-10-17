//! Background detection and the k-means palette.

use std::collections::BTreeMap;

use image::RgbImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most palette groups.
const MAX_K: usize = 8;
/// Lloyd iterations.
const ITERATIONS: usize = 10;
/// The most pixels sampled for k-means.
const MAX_SAMPLES: u64 = 50_000;
/// Fixed seed of the centroid initialisation.
const SEED: u64 = 0x464F_524D_4121_0001;

/// One color group of the image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PaletteEntry {
    /// The group's mean color.
    pub color: [u8; 3],
    /// The fraction of sampled pixels in the group, 0 to 1.
    pub share: f64,
}

pub(crate) fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

pub(crate) fn to_f(c: [u8; 3]) -> [f64; 3] {
    [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
}

fn to_u8(c: [f64; 3]) -> [u8; 3] {
    let r = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    [r(c[0]), r(c[1]), r(c[2])]
}

/// The most frequent 5-bit color on the 1 px border (ties: smallest key),
/// reported as the mean of the border pixels in that bin.
pub(crate) fn background(image: &RgbImage) -> [u8; 3] {
    let (w, h) = image.dimensions();
    let mut bins: BTreeMap<u16, (u64, [u64; 3])> = BTreeMap::new();
    let mut add = |x: u32, y: u32| {
        let p = image.get_pixel(x, y).0;
        let key = (u16::from(p[0] >> 3) << 10) | (u16::from(p[1] >> 3) << 5) | u16::from(p[2] >> 3);
        let e = bins.entry(key).or_insert((0, [0; 3]));
        e.0 += 1;
        for (acc, v) in e.1.iter_mut().zip(p) {
            *acc += u64::from(v);
        }
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 1..h.saturating_sub(1) {
        add(0, y);
        add(w - 1, y);
    }
    let mut best: Option<(u64, [u64; 3])> = None;
    for (count, sum) in bins.values() {
        if best.is_none_or(|(c, _)| *count > c) {
            best = Some((*count, *sum));
        }
    }
    match best {
        Some((n, s)) if n > 0 => {
            let m = |i: usize| ((s[i] as f64) / (n as f64)).round() as u8;
            [m(0), m(1), m(2)]
        }
        _ => [0, 0, 0],
    }
}

struct Lcg(u64);

impl Lcg {
    fn next_unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

fn sample(image: &RgbImage) -> Vec<[f64; 3]> {
    let (w, h) = image.dimensions();
    let n = u64::from(w) * u64::from(h);
    let mut step = 1u32;
    while n.div_ceil(u64::from(step) * u64::from(step)) > MAX_SAMPLES {
        step += 1;
    }
    let mut out = Vec::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            out.push(to_f(image.get_pixel(x, y).0));
            x += step;
        }
        y += step;
    }
    out
}

/// k-means palette (k <= 8, 10 iterations, fixed seed), sorted by share
/// descending then color.
pub(crate) fn palette(image: &RgbImage) -> Vec<PaletteEntry> {
    let pts = sample(image);
    if pts.is_empty() {
        return Vec::new();
    }
    // Deterministic k-means++ initialisation.
    let mut rng = Lcg(SEED);
    let first = (rng.next_unit() * pts.len() as f64) as usize;
    let mut centers = vec![pts[first.min(pts.len() - 1)]];
    while centers.len() < MAX_K {
        let d: Vec<f64> = pts
            .iter()
            .map(|p| {
                centers
                    .iter()
                    .map(|c| dist2(*p, *c))
                    .fold(f64::MAX, f64::min)
            })
            .collect();
        let total: f64 = d.iter().sum();
        if total <= 0.0 {
            break;
        }
        let target = rng.next_unit() * total;
        let mut acc = 0.0;
        let mut pick = d.len() - 1;
        for (i, v) in d.iter().enumerate() {
            acc += v;
            if acc > target && *v > 0.0 {
                pick = i;
                break;
            }
        }
        centers.push(pts[pick]);
    }
    let mut counts = vec![0u64; centers.len()];
    for _ in 0..ITERATIONS {
        let mut sums = vec![[0.0f64; 3]; centers.len()];
        counts.iter_mut().for_each(|c| *c = 0);
        for p in &pts {
            let k = nearest(&centers, *p);
            counts[k] += 1;
            for i in 0..3 {
                sums[k][i] += p[i];
            }
        }
        for k in 0..centers.len() {
            if counts[k] > 0 {
                let n = counts[k] as f64;
                centers[k] = [sums[k][0] / n, sums[k][1] / n, sums[k][2] / n];
            }
        }
    }
    counts.iter_mut().for_each(|c| *c = 0);
    for p in &pts {
        counts[nearest(&centers, *p)] += 1;
    }
    let total = pts.len() as f64;
    let mut out: Vec<PaletteEntry> = centers
        .iter()
        .zip(&counts)
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| PaletteEntry {
            color: to_u8(*c),
            share: *n as f64 / total,
        })
        .collect();
    out.sort_by(|a, b| {
        b.share
            .total_cmp(&a.share)
            .then_with(|| a.color.cmp(&b.color))
    });
    out
}

fn nearest(centers: &[[f64; 3]], p: [f64; 3]) -> usize {
    let mut best = 0;
    let mut bd = f64::MAX;
    for (i, c) in centers.iter().enumerate() {
        let d = dist2(*c, p);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// The ink: the explicit color, or the non-background group with most share.
pub(crate) fn pick_ink(
    palette: &[PaletteEntry],
    background: [u8; 3],
    tolerance: f64,
    explicit: Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    if let Some(c) = explicit {
        return Some(c);
    }
    let bg = to_f(background);
    palette
        .iter()
        .find(|e| dist2(to_f(e.color), bg).sqrt() > tolerance * 2.0)
        .map(|e| e.color)
}

//! Deterministic helpers: a fixed-seed PRNG and elementary functions made of
//! IEEE-exact operations only (`+ - * /`), so the corpus is the same, byte
//! for byte, on every platform. The platform `sin`, `cos` and `exp` may differ
//! in their last bit between operating systems; these do not.

use std::f64::consts::{PI, TAU};

/// SplitMix64: tiny, fast and fully specified.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator with a fixed seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / 9_007_199_254_740_992.0
    }

    /// A number in `[-1, 1)`.
    pub fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }

    /// An integer in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

fn reduce(x: f64) -> f64 {
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    x
}

/// Sine, by range reduction and a Taylor series.
#[must_use]
pub fn sin(x: f64) -> f64 {
    let x = reduce(x);
    let x2 = x * x;
    let (mut term, mut sum) = (x, x);
    for k in 1..=14 {
        let n = f64::from(2 * k * (2 * k + 1));
        term = -term * x2 / n;
        sum += term;
    }
    sum
}

/// Cosine, by range reduction and a Taylor series.
#[must_use]
pub fn cos(x: f64) -> f64 {
    let x = reduce(x);
    let x2 = x * x;
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..=14 {
        let n = f64::from((2 * k - 1) * (2 * k));
        term = -term * x2 / n;
        sum += term;
    }
    sum
}

/// `exp(-x)` for `x >= 0`: the Taylor series of `exp(-x / 16)` squared four
/// times.
#[must_use]
pub fn exp_neg(x: f64) -> f64 {
    let y = x / 16.0;
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..=18 {
        term = -term * y / f64::from(k);
        sum += term;
    }
    for _ in 0..4 {
        sum *= sum;
    }
    sum
}

/// One-dimensional value noise: random values on a lattice `wavelength`
/// apart, linearly interpolated. Always within `[-amplitude, amplitude]`.
#[derive(Clone, Debug)]
pub struct ValueNoise {
    lattice: Vec<f64>,
    wavelength: f64,
}

impl ValueNoise {
    /// Noise that covers `0..=length`.
    #[must_use]
    pub fn new(seed: u64, amplitude: f64, wavelength: f64, length: f64) -> Self {
        let mut rng = Rng::new(seed);
        let n = (length / wavelength).ceil().max(1.0) as usize + 2;
        Self {
            lattice: (0..n).map(|_| rng.signed() * amplitude).collect(),
            wavelength,
        }
    }

    /// The value at `s` (clamped to the covered range).
    #[must_use]
    pub fn at(&self, s: f64) -> f64 {
        let u = (s / self.wavelength).max(0.0);
        let i = (u.floor() as usize).min(self.lattice.len() - 2);
        let t = (u - i as f64).min(1.0);
        self.lattice[i] * (1.0 - t) + self.lattice[i + 1] * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elementary_functions_are_accurate() {
        for k in -40..=40 {
            let x = f64::from(k) * 0.37;
            assert!((sin(x) - x.sin()).abs() < 1e-9, "{x}");
            assert!((cos(x) - x.cos()).abs() < 1e-9, "{x}");
        }
        for k in 0..=40 {
            let x = f64::from(k) * 0.2;
            assert!((exp_neg(x) - (-x).exp()).abs() < 1e-9, "{x}");
        }
    }

    #[test]
    fn rng_is_fixed() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert_eq!(a.next_u64(), b.next_u64());
        assert!((0.0..1.0).contains(&a.unit()));
    }

    #[test]
    fn noise_stays_within_its_amplitude() {
        let n = ValueNoise::new(3, 2.0, 5.0, 100.0);
        for i in 0..=100 {
            assert!(n.at(f64::from(i)).abs() <= 2.0);
        }
    }
}

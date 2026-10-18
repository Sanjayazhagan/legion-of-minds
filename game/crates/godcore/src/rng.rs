//! Small seeded RNG (SplitMix64). Same seed, same numbers, on every platform.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B);
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// Random unit vector without trig, so it is bit-identical everywhere.
    pub fn unit_vec(&mut self) -> (f32, f32) {
        loop {
            let x = self.range(-1.0, 1.0);
            let y = self.range(-1.0, 1.0);
            let d2 = x * x + y * y;
            if d2 > 0.01 && d2 <= 1.0 {
                let d = d2.sqrt();
                return (x / d, y / d);
            }
        }
    }
}

/// exp() built only from +, *, and exponent bits, so results do not depend on
/// the platform's libm. Relative error is about 2e-7.
pub fn det_exp(x: f32) -> f32 {
    const LOG2E: f32 = 1.442_695;
    const LN2_HI: f32 = 0.693_145_75;
    const LN2_LO: f32 = 1.428_606_8e-6;
    let x = x.clamp(-87.0, 88.0);
    let k = (x * LOG2E + 0.5).floor();
    let r = (x - k * LN2_HI) - k * LN2_LO;
    let p = 1.0
        + r * (1.0
            + r * (0.5
                + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0 + r * (1.0 / 720.0))))));
    let scale = f32::from_bits(((k as i32 + 127) as u32) << 23);
    p * scale
}

/// tanh() from det_exp, deterministic across platforms.
pub fn det_tanh(x: f32) -> f32 {
    if x > 9.0 {
        return 1.0;
    }
    if x < -9.0 {
        return -1.0;
    }
    1.0 - 2.0 / (det_exp(2.0 * x) + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_and_tanh_are_accurate() {
        let mut x = -20.0f32;
        while x < 20.0 {
            let e = det_exp(x);
            assert!(((e - x.exp()) / x.exp()).abs() < 1e-5, "exp {x}");
            assert!((det_tanh(x) - x.tanh()).abs() < 1e-6, "tanh {x}");
            x += 0.013;
        }
    }

    #[test]
    fn rng_is_repeatable() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}

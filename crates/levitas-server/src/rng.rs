//! A small, fast pseudo-random generator for gameplay randomness (drops, item motion).
//! Not for anything security-related.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn from_time() -> Self {
        let nanos = std::time::UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_nanos() as u64;
        Self::new(nanos)
    }

    /// xorshift64*
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `0..n`; `n` must be positive.
    pub fn below(&mut self, n: u32) -> u32 {
        (self.f64() * f64::from(n)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_in_range() {
        let mut rng = Rng::new(42);
        for _ in 0..10_000 {
            let x = rng.f64();
            assert!((0.0..1.0).contains(&x));
            assert!(rng.below(7) < 7);
        }
    }
}

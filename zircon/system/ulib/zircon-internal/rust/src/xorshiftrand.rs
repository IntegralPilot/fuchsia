// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Xorshift32 and Xorshift64
//
// https://www.jstatsoft.org/article/view/v008i14
// https://en.wikipedia.org/wiki/Xorshift

pub struct Rand32 {
    pub n: u32,
}

pub struct Rand64 {
    pub n: u64,
}

impl Rand32 {
    pub fn new(seed: u32) -> Self {
        Self { n: seed }
    }

    pub fn next(&mut self) -> u32 {
        let mut n = self.n;
        n ^= n << 13;
        n ^= n >> 17;
        n ^= n << 5;
        self.n = n;
        n
    }

    pub fn seed_from_str(&mut self, s: &str) {
        self.n = crate::fnv1hash::fnv1a32(s.as_bytes());
    }
}

impl Rand64 {
    pub fn new(seed: u64) -> Self {
        Self { n: seed }
    }

    pub fn next(&mut self) -> u64 {
        let mut n = self.n;
        n ^= n << 13;
        n ^= n >> 7;
        n ^= n << 17;
        self.n = n;
        n
    }

    pub fn seed_from_str(&mut self, s: &str) {
        self.n = crate::fnv1hash::fnv1a64(s.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rand32() {
        let mut r = Rand32::new(12345);
        let n1 = r.next();
        let n2 = r.next();
        assert_ne!(n1, n2);
        assert_ne!(n1, 12345);
    }

    #[test]
    fn test_rand64() {
        let mut r = Rand64::new(1234567890);
        let n1 = r.next();
        let n2 = r.next();
        assert_ne!(n1, n2);
        assert_ne!(n1, 1234567890);
    }
}

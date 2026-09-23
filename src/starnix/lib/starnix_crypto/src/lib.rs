// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use rand::Rng as _;

/// Overwrite `buffer` with bytes drawn from a thread-local CSPRNG (backed by ChaCha12).
pub fn cprng_draw(buffer: &mut [u8]) {
    rand::rng().fill_bytes(buffer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cprng_draw() {
        let mut buf = [0u8; 32];
        cprng_draw(&mut buf);
        // Extremely low probability that 32 random bytes are all zero.
        assert_ne!(buf, [0u8; 32]);
    }
}

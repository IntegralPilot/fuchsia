// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use core::hint::cold_path;
use core::ops::Add;

#[derive(Debug, Copy, Clone)]
pub struct ModularIndex {
    index: usize,
    modulo: usize,
}

impl ModularIndex {
    pub const fn new(index: usize, modulo: usize) -> Self {
        Self { index: index % modulo, modulo }
    }

    pub const fn increment(mut self) -> Self {
        self.index += 1;
        if self.index >= self.modulo {
            // NOTE: We use >= here because it's easier to optimize for the compiler
            // but it's worth noting that index would never be > than modulo due to the invariants
            // of the type.
            cold_path();
            self.index = 0;
        }
        self
    }

    pub const fn get(&self) -> usize {
        self.index
    }
}

impl Add<usize> for ModularIndex {
    type Output = Self;

    fn add(mut self, rhs: usize) -> Self::Output {
        if rhs == 1 {
            self.increment()
        } else {
            self.index = (self.index + rhs) % self.modulo;
            self
        }
    }
}

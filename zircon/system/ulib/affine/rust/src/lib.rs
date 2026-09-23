// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

mod ratio;
mod transform;
mod utils;

pub use ratio::{Exact, Ratio, Round};
pub use transform::{Saturate, Transform};
pub use utils::{clamp_add, clamp_sub};

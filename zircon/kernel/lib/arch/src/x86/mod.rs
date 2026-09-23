// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod apic_id;
mod bug;
mod cache;
pub mod cpuid;
mod extension;
mod feature;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod intrin;
mod speculation;
mod system;

pub use apic_id::*;
pub use bug::*;
pub use cache::*;
pub use extension::*;
pub use feature::*;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub use intrin::*;
pub use speculation::*;
pub use system::*;

/// Enumeration of vendors.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Vendor {
    Intel,
    Amd,
    Unknown,
}

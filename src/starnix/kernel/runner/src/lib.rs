// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]
use starnix_types as _;

mod component_runner;
mod container;
mod features;
mod mounts;
mod serve_protocols;

pub use component_runner::*;
pub use container::*;
pub use features::*;
pub use mounts::*;
pub use serve_protocols::*;

/// Configure `starnix_core` with callbacks that are only available from this library's "higher
/// level" within the Starnix build graph.
pub fn initialize() {
    starnix_core::execution::initialize_syscall_loop(starnix_syscall_loop::enter);
}

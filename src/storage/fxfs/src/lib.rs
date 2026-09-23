// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs is a log-structured filesystem for [Fuchsia](https://fuchsia.dev/).
//!
//! For a high-level overview, please refer to the
//! [RFC](/docs/contribute/governance/rfcs/0136_fxfs.md).
//!
//! Where possible, Fxfs code tries to be target agnostic.
//! Fuchsia specific bindings are primarily found under [server].

// TODO(https://fxbug.dev/439053417): Investigate why the recursion limit was bumped
// to unblock the toolchain.
#![recursion_limit = "256"]

pub mod checksum;
pub mod drop_event;

#[macro_use]
mod debug_assert_not_too_long;

pub mod blob_metadata;
pub mod errors;
pub mod filesystem;
pub mod fsck;
pub mod future_with_guard;
pub mod hooks;
pub mod log;
pub mod lsm_tree;
pub mod metrics;
pub mod object_handle;
pub mod object_store;
pub mod range;
pub mod round;
pub mod serialized_types;
mod stable_hash;
#[cfg(any(test, feature = "benchmark", fuzz))]
pub mod testing;
pub mod virtual_device;
pub mod zerocopy_serialization;

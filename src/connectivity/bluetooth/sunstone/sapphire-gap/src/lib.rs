// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The `sapphire-gap` crate, providing Generic Access Profile (GAP) functionality.

#![no_std]

pub mod peer_cache;

pub use peer_cache::*;

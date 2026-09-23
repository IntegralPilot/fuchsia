// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//! This crate provides an API for running the Fuchsia assembly tool.
//! It allows programmatically invoking `product` and `create-system` commands.

#![deny(missing_docs)]

mod api;

/// Version information for release artifacts.
pub mod release_info;

pub use api::{assemble, create_system, product_assembly};

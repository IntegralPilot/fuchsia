// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! This crate provides utilities for the Fuchsia display-coordinator API.

/// Custom error definitions for `fuchsia.hardware.display` and sysmem API functions.
mod error;

/// The `types` module defines convenions wrappers for FIDL data types in the
/// `fuchsia.hardware.display` library.
mod types;

/// Stateless representation of a display configuration.
mod config;

/// Helper functions setting up shared image buffers that can be assigned to display layers.
mod image;

/// The `Controller` type is a client-side abstraction for the `fuchsia.hardware.display.Controller`
/// protocol.
mod controller;

/// Rust bindings bridging fuchsia sysmem PixelFormatType and images2 PixelFormat types.
mod pixel_format;

pub use config::*;
pub use controller::{Coordinator, VsyncEvent};
pub use error::*;
pub use image::*;
pub use pixel_format::{PixelFormat, get_bytes_per_pixel};
pub use types::*;

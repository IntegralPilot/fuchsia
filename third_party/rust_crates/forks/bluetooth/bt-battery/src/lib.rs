// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// An error type for the battery service crate.
mod error;
pub use error::Error;

/// Implements the Battery Service monitor (client) role.
pub mod monitor;

/// Common types used throughout the battery service crate.
pub mod types;

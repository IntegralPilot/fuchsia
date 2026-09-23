// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! This crate introduces generic ToolProvider and Tool traits that represent executable stages of
//! Assembly. It also introduces two implementations:
//!  - PlatformToolProvider provides tools from a platform artifacts directory
//!  - FakeToolProvider which no-ops execution for tests

mod platform;
mod serde_arc;
pub mod testing;
mod tool;

pub use platform::PlatformToolProvider;
pub use tool::{Tool, ToolCommand, ToolCommandLog, ToolProvider};

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Subtool implementation and support libraries for `ffx uart`.
//!
//! Provides CLI subcommands, driver daemon lifecycle management, connection metadata
//! persistence, and system process inspection for UART target devices.

/// Connection metadata resolution and persistence for active UART targets.
pub mod metadata;
/// Asynchronous stream abstractions and connection helpers for UART devices.
pub mod stream;
/// Host process inspection, peer PID discovery, and daemon liveness checking.
pub mod sys;

pub use metadata::*;
pub use stream::*;
pub use sys::*;

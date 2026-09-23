// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! The [Hanging Get pattern](https://fuchsia.dev/fuchsia-src/development/api/fidl#hanging-get)
//! can be used when pull-based flow control is needed on a protocol.

/// This module provides generalized rust implementations of the hanging get pattern for client side use.
pub mod client;

/// Server-side hanging get implementation.
///
/// See [crate::hanging_get::server::HangingGet] for usage documentation.
pub mod server;

/// This module provides error types that may be used by the async hanging-get server.
pub mod error;

#[allow(missing_docs)]
pub mod test_util;

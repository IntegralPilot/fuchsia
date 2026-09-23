// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod client;
pub mod server;
pub mod types;
pub use crate::client::error::Error as ClientError;
pub use crate::server::Error as ServerError;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

#[cfg(test)]
pub mod tests;

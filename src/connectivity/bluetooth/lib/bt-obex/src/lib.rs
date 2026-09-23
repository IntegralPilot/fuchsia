// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Contains the implementation of the Client role for OBEX. Services & Profiles that require
/// OBEX Client functionality should use this module.
pub mod client;

/// Contains the implementation of the Server role for OBEX. Services & Profiles that require
/// OBEX Server functionality should use this module.
pub mod server;

/// The error type used throughout this library.
mod error;
pub use error::Error as ObexError;

/// Definitions of the OBEX packet Header types.
pub mod header;

/// Types and interfaces associated with the supported OBEX operations.
pub mod operation;

/// Helpers for interacting with the `fuchsia.bluetooth.bredr.Profile` resource.
pub mod profile;

/// Defines the `ObexTransport` which is a wrapper around the underlying L2CAP or RFCOMM channel.
mod transport;

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Services are pretty tightly-coupled to our filesystem implementation, and so
// the APIs for them reflect some of that coupling. Unlike server and client
// handlers, service connectors and handlers accept `&self` and do not return a
// future. In the future, it would be nice to experiment with

/// A member connector for a FIDL service.
pub trait ServiceConnector<T> {
    /// The error type returned if the connector fails.
    type Error;

    /// Attempts to connect to the given service member.
    fn connect_to_member(&self, member: &str, server_end: T) -> Result<(), Self::Error>;
}

/// A type which handles incoming service connections for a server.
pub trait ServiceHandler<T> {
    /// Handles a received connection request.
    ///
    /// The service cannot handle more connection requests until `on_connection` completes. If
    /// `on_connection` should handle requests in parallel, it should spawn a new async task and
    /// return.
    fn on_connection(&self, member: &str, server_end: T);
}

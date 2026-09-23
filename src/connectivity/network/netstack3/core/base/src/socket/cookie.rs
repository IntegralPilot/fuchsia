// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_sync::rc::ResourceToken;
use static_assertions::const_assert_eq;

/// Socket cookie is a unique 64-bit value assigned to a socket.
///
/// Socket implementations set their cookie value based on the `ResourceId`.
#[derive(Debug, Clone)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq, Eq, PartialOrd, Ord))]
pub struct SocketCookie {
    token: ResourceToken<'static>,
}

const_assert_eq!(core::mem::size_of::<SocketCookie>(), 8);

// `SocketCookie` is always non-zero, so `Option<SocketCookie>` should fit in
// 8 bytes.
const_assert_eq!(core::mem::size_of::<Option<SocketCookie>>(), 8);

impl SocketCookie {
    /// Creates a new cookie from the socket's `ResourceToken`.
    pub fn new(token: ResourceToken<'_>) -> Self {
        // Extend the lifetime of the token since `SocketCookie` is allowed to
        // outlive the strong resource reference.
        let token = token.extend_lifetime();
        Self { token }
    }

    /// Returns the cookie value.
    pub fn export_value(self) -> u64 {
        self.token.export_value()
    }
}

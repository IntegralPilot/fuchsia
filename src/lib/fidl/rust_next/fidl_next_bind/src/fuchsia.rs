// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia-specific FIDL bindings.

use zx::Channel;

use crate::{ClientEnd, ServerEnd};
#[cfg(feature = "fasync")]
use crate::{HasExecutor, RunsTransport, fuchsia_async::FuchsiaAsync};

/// Creates a `ClientEnd` and `ServerEnd` for the given protocol over Zircon channels.
pub fn create_channel<P>() -> (ClientEnd<P, zx::Channel>, ServerEnd<P, zx::Channel>) {
    let (client_end, server_end) = Channel::create();
    (ClientEnd::from_untyped(client_end), ServerEnd::from_untyped(server_end))
}

#[cfg(feature = "fasync")]
impl RunsTransport<Channel> for FuchsiaAsync {}

#[cfg(feature = "fasync")]
impl HasExecutor for Channel {
    type Executor = FuchsiaAsync;

    fn executor(&self) -> Self::Executor {
        FuchsiaAsync
    }
}

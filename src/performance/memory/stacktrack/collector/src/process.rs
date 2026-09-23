// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_memory_stacktrack_client as fstacktrack_client;
use zx::Koid;

/// An instrumented process.
#[async_trait]
pub trait Process: Send + Sync {
    /// Returns the cached name of the process.
    fn get_name(&self) -> &str;

    /// Returns the koid of the process.
    fn get_koid(&self) -> Koid;

    /// Serves requests from the process and returns when the process disconnects.
    async fn serve_until_exit(&self) -> Result<(), anyhow::Error>;

    /// Gets the stack traces.
    fn get_stack_traces(&self) -> Result<Box<dyn Snapshot>, anyhow::Error>;
}

/// A snapshot of the stack traces in a given process.
#[async_trait]
pub trait Snapshot: Send + Sync {
    /// Serves this snapshot over an Iterator channel.
    async fn write_to(
        &self,
        receiver: &mut fstacktrack_client::SnapshotReceiverProxy,
    ) -> Result<(), anyhow::Error>;
}

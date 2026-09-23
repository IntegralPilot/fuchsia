// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_memory_heapdump_client as fheapdump_client;
use std::fmt::Debug;
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

    /// Takes a live snapshot.
    fn take_live_snapshot(&self, with_contents: bool) -> Result<Box<dyn Snapshot>, anyhow::Error>;
}

impl Debug for dyn Process {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Process")
            .field("name", &self.get_name())
            .field("koid", &self.get_koid())
            .finish()
    }
}

/// A snapshot of all the live allocations in a given process.
#[async_trait]
pub trait Snapshot: Send + Sync {
    /// Writes this snapshot into a SnapshotReceiver channel.
    async fn write_to(
        &self,
        dest: &mut fheapdump_client::SnapshotReceiverProxy,
    ) -> Result<(), anyhow::Error>;
}

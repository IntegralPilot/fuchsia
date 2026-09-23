// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This actor has actions to write/read logs to/from Archivist.

use anyhow::{Context, Result};
use diagnostics_reader::{ArchiveReader, Data, Logs, Subscription};
use fidl_fuchsia_diagnostics::ArchiveAccessorMarker;
use fuchsia_component::client::connect_to_protocol;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use log::info;
use rand::RngExt as _;
use rand::rngs::SmallRng;
use stress_test_actor::{Action, actor_loop};

/// Stores all data needed by this Worker. This singleton gets passed into every action.
struct WorkerData {
    /// The active log subscription created with Archivist.
    /// Used by the reader action to get logs.
    pub subscription: Subscription,
}

#[fuchsia::main]
async fn main() -> Result<()> {
    // Create a WorkerData singleton
    let accessor_proxy = connect_to_protocol::<ArchiveAccessorMarker>()
        .context("Could not connect to ArchiveAccessor protocol")?;
    let mut archive_reader = ArchiveReader::logs();
    archive_reader.with_archive(accessor_proxy);
    let subscription =
        archive_reader.snapshot_then_subscribe().context("Could not subscribe to logs")?;
    let data = WorkerData { subscription };

    actor_loop(
        data,
        vec![
            Action { name: "reader", run: reader },
            Action { name: "simple_writer", run: simple_writer },
            Action { name: "length_writer", run: length_writer },
        ],
    )
    .await
}

fn reader(data: &mut WorkerData, _: SmallRng) -> BoxFuture<'_, Result<()>> {
    async move {
        let log: Data<Logs> =
            data.subscription.next().await.context("No next log")?.context("Error getting log")?;
        log.msg().context("Log doesn't have message")?;

        // TODO(https://fxbug.dev/42162667): Add log verification

        Ok(())
    }
    .boxed()
}

fn simple_writer(_: &mut WorkerData, _: SmallRng) -> BoxFuture<'_, Result<()>> {
    async move {
        info!("This is a test log message");
        Ok(())
    }
    .boxed()
}

fn length_writer(_: &mut WorkerData, mut rng: SmallRng) -> BoxFuture<'_, Result<()>> {
    async move {
        let random_string_len = rng.random_range(1..1000);
        let mut random_string = String::new();

        for _ in 0..random_string_len {
            let c = rng.random::<char>();
            random_string.push(c)
        }

        info!("{}", random_string);
        Ok(())
    }
    .boxed()
}

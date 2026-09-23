// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/439053417): Investigate why the recursion limit was bumped to unblock the
// toolchain.
#![recursion_limit = "256"]

use anyhow::{Context, Error};
use fuchsia_runtime::HandleType;
use fxfs::log::*;
use fxfs::serialized_types::LATEST_VERSION;
use fxfs_platform::component::Component;

// TODO(https://fxbug.dev/402196421) improve scheduler integration
#[fuchsia::main(threads = num_worker_threads(), thread_role = "fuchsia.fs.fxfs")]
async fn main() -> Result<(), Error> {
    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::inspector(),
        inspect_runtime::PublishOptions::default().send_vmo_preference(
            inspect_runtime::TreeServerSendPreference::frozen_or(
                inspect_runtime::TreeServerSendPreference::DeepCopy,
            ),
        ),
    );

    fuchsia_trace_provider::trace_provider_create_with_fdio();

    info!(version:% = LATEST_VERSION; "Started");

    Component::new()
        .run(
            fuchsia_runtime::take_startup_handle(HandleType::DirectoryRequest.into())
                .context("Missing startup handle")?
                .into(),
            fuchsia_runtime::take_startup_handle(HandleType::Lifecycle.into()).map(|h| h.into()),
        )
        .await
}

// TODO(https://fxbug.dev/402196413) delegate thread count to product config
fn num_worker_threads() -> u8 {
    (std::cmp::max(zx::system_get_num_cpus(), 2) - 1).try_into().unwrap_or(u8::MAX)
}

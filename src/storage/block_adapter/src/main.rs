// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_storage_block as fstorage_block;
use fuchsia_runtime::HandleType;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut args = std::env::args();
    let name = args.next().ok_or_else(|| anyhow!("no arguments provided"))?;
    let binary =
        args.next().ok_or_else(|| anyhow!("{} invoked without path to child binary", name))?;
    let return_code = block_adapter::run(
        ClientEnd::<fstorage_block::BlockMarker>::from(zx::Channel::from(
            fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
                HandleType::User0,
                1,
            ))
            .ok_or_else(|| anyhow!("missing device handle"))?,
        ))
        .into_proxy(),
        &binary,
        args,
    )
    .await?
    .try_into()?;
    std::process::exit(return_code);
}

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::aggregator::{collect_data, provide_data, Aggregator};
use anyhow::Result;
use fidl_fuchsia_fuzzer as fuzz;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::warn;

mod aggregator;
mod options;

enum IncomingService {
    CoverageDataCollector(fuzz::CoverageDataCollectorRequestStream),
    CoverageDataProvider(fuzz::CoverageDataProviderRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<()> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::CoverageDataCollector);
    fs.dir("svc").add_fidl_service(IncomingService::CoverageDataProvider);
    fs.take_and_serve_directory_handle()?;
    let aggregator = &Aggregator::new();
    const MAX_CONCURRENT: usize = 10000;
    fs.for_each_concurrent(MAX_CONCURRENT, |incoming_service| async {
        match incoming_service {
            IncomingService::CoverageDataCollector(stream) => {
                collect_data(stream, aggregator).await.unwrap_or_else(|e| warn!("{:?}", e));
            }
            IncomingService::CoverageDataProvider(stream) => {
                provide_data(stream, aggregator).await.unwrap_or_else(|e| warn!("{:?}", e));
            }
        };
    })
    .await;
    Ok(())
}

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_diagnostics::ArchiveAccessorMarker;
use fuchsia_component::client;

#[fuchsia::test]
async fn verify_proxy_reuse() -> Result<(), Error> {
    let mut archive_reader = ArchiveReader::inspect();
    let proxy =
        client::connect_to_protocol::<ArchiveAccessorMarker>().expect("connect to accessor");
    archive_reader
        .with_archive(proxy)
        .add_selector("archivist:root/archive_accessor_stats/all:connections_opened".to_string());
    let results = archive_reader.snapshot().await?;

    assert_eq!(results.len(), 1);

    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        archive_accessor_stats: {
            all: {
                connections_opened: 1u64,
            }
        }
    });

    let results = archive_reader.snapshot().await?;

    assert_eq!(results.len(), 1);

    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        archive_accessor_stats: {
            all: {
                connections_opened: 1u64,
            }
        }
    });

    let results = ArchiveReader::inspect()
        .add_selector("archivist:root/archive_accessor_stats/all:connections_opened".to_string())
        .snapshot()
        .await?;

    assert_eq!(results.len(), 1);

    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        archive_accessor_stats: {
            all: {
                connections_opened: 2u64,
            }
        }
    });

    Ok(())
}

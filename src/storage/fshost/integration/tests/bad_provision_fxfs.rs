// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod config;
use crate::config::new_builder;
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fshost_test_fixture::disk_builder::{Disk, DiskBuilder};

#[cfg(feature = "fxblob")]
#[fuchsia::test]
async fn test_fshost_crashes_when_provision_fxfs_fails() {
    let mut builder = new_builder().force_fxfs_provisioner_failure();
    builder.fshost().set_config_value("provision_fxfs", true);
    builder.fshost().set_config_value("merge_super_and_userdata", true);
    let mut fixture = builder.build().await;

    let mut disk = DiskBuilder::new();
    disk.with_gpt()
        .with_unformatted_volume_manager()
        .with_system_partition_label("super")
        .with_extra_gpt_partition("userdata", 1)
        .with_extra_gpt_partition("other", 1);
    fixture.add_main_disk(Disk::Builder(disk)).await;

    let data_dir = fixture.dir("data", fio::PERM_READABLE);
    let status = data_dir
        .query_filesystem()
        .await
        .expect_err("Opening connection to data should have failed if fshost panicked");
    assert_matches!(
        status,
        fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }
    );

    fixture.tear_down().await;
}

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for some of the filesystem corruption handling paths. These naturally have a lot of error
//! messages since corruption is Bad, so they are in their own test package that allows error logs.

pub mod config;
use config::{blob_fs_type, data_fs_name, data_fs_spec, data_fs_type, new_builder, volumes_spec};

#[fuchsia::test]
async fn data_reformatted_when_corrupt() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec()).corrupt_data();
    let mut fixture = builder.build().await;

    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file_absent().await;

    // Ensure blobs are not reformatted.
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_test_blob().await;

    fixture
        .wait_for_crash_reports(
            1,
            data_fs_name(),
            &format!("fuchsia-{}-corruption", data_fs_name()),
        )
        .await;

    fixture.tear_down().await;
}

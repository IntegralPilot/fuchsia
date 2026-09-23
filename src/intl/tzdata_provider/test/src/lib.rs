// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use icu_data::Loader;
use rust_icu_ucal as ucal;

async fn load_icu_data(tzdata_dir: &str, revision_file_path: &str) {
    let _loader = Loader::new_with_tz_resources_and_validation(tzdata_dir, revision_file_path)
        .with_context(|| {
            format!(
                "while using:\n\tTZDATA_DIR={}\n\tREVISION_FILE_PATH={}",
                tzdata_dir, revision_file_path
            )
        })
        .expect("Failed to create Loader");

    let _version = ucal::get_tz_data_version().expect("Failed to get tzdata version");
}

#[fuchsia::test]
async fn test_tzdata_icu_44_le() {
    // These hard coded paths are derived from the test's CML file.
    const TZDATA_DIR: &str = "/config/tzdata/icu/44/le";
    const REVISION_FILE_PATH: &str = "/config/tzdata/icu/44/le/revision.txt";

    load_icu_data(TZDATA_DIR, REVISION_FILE_PATH).await;
}

#[fuchsia::test]
async fn test_tzdata_icu() {
    // These hard coded paths are derived from the test's CML file.
    const TZDATA_DIR: &str = "/config2/tzdata/icu/44/le";
    const REVISION_FILE_PATH: &str = "/config2/tzdata/icu/44/le/revision.txt";

    load_icu_data(TZDATA_DIR, REVISION_FILE_PATH).await;
}

#[fuchsia::test]
async fn test_tzdata_icu_flattened() {
    // These hard coded paths are derived from the test's CML file.
    const TZDATA_DIR: &str = "/config3/tzdata";
    const REVISION_FILE_PATH: &str = "/config3/tzdata/revision.txt";

    load_icu_data(TZDATA_DIR, REVISION_FILE_PATH).await;
}

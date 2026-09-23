// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Context, Error};
use fidl_fuchsia_io as fio;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;

const DATA_FOR_TEST_DIR_NAME: &str = "data_for_test";
const DATA_FOR_TESTS_PATH: &str = "/data/data_for_test";

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    if !std::path::Path::new("/data").exists() {
        return Err(anyhow!("Expected /data to be available"));
    }
    let mut fs = ServiceFs::new();
    // Delete data from previous run if leftover due to deletion failure.
    if std::path::Path::new(DATA_FOR_TESTS_PATH).exists() {
        std::fs::remove_dir_all(DATA_FOR_TESTS_PATH).context("Removing data for test directory")?;
    }

    // back /data provided to test using fx_fs
    std::fs::create_dir(DATA_FOR_TESTS_PATH).context("Create data for test directory")?;
    let data_directory = fuchsia_fs::directory::open_in_namespace(
        DATA_FOR_TESTS_PATH,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )?;
    fs.add_remote(DATA_FOR_TEST_DIR_NAME, data_directory);
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

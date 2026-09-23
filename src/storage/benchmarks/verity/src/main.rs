// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fxfs_test::TestFxfsAdminMarker;
use fidl_fuchsia_io as fio;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_path};
use fuchsiaperf::FuchsiaPerfBenchmarkResult;
use storage_verity_benchmarks_lib::{
    ENABLE_BENCHMARK_NAME, READ_BENCHMARK_NAME, results_file_name,
};

#[fuchsia::main]
async fn main() {
    // Connecting to the fuchsia_component::Binder protocol launches the setup_verity component.
    connect_to_protocol_at_path::<fidl_fuchsia_component::BinderMarker>("/svc/SetupVerityBinder")
        .unwrap();

    let root_dir = fuchsia_fs::directory::open_in_namespace("/data", fio::PERM_READABLE).unwrap();
    let mut results = wait_for_results(&root_dir, &results_file_name(ENABLE_BENCHMARK_NAME)).await;
    let fxfs_admin_proxy = connect_to_protocol::<TestFxfsAdminMarker>().unwrap();
    let new_root_dir = fxfs_admin_proxy.clear_cache().await.unwrap().unwrap().into_proxy();

    // Connecting to the fuchsia_component::Binder protocol launches the read_verified_file
    // component.
    connect_to_protocol_at_path::<fidl_fuchsia_component::BinderMarker>(
        "/svc/ReadVerifiedFileBinder",
    )
    .unwrap();

    let read_results =
        wait_for_results(&new_root_dir, &results_file_name(READ_BENCHMARK_NAME)).await;
    results.extend(read_results);
    std::fs::write(
        "/custom_artifacts/results.fuchsiaperf.json",
        serde_json::to_string_pretty(&results).unwrap(),
    )
    .unwrap();
    fxfs_admin_proxy.shutdown().await.unwrap();
}

async fn wait_for_results(
    dir: &fio::DirectoryProxy,
    path: &str,
) -> Vec<FuchsiaPerfBenchmarkResult> {
    device_watcher::recursive_wait(&dir, path).await.unwrap();
    let file = fuchsia_fs::directory::open_file(&dir, path, fio::PERM_READABLE).await.unwrap();
    serde_json::from_str::<Vec<FuchsiaPerfBenchmarkResult>>(
        &fuchsia_fs::file::read_to_string(&file).await.unwrap(),
    )
    .unwrap()
}

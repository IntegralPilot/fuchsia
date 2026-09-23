// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use delivery_blob::DeliveryBlobType;
use fuchsia_merkle::HASH_SIZE;
use scrutiny_collection::core::ComponentSource;
use scrutiny_collection::model::DataModel;
use scrutiny_collection::model_config::ModelConfig;
use std::sync::Arc;
use tempfile::tempdir;

/// Creates a simple fake model configuration that uses an in memory uri and
/// tempdata() directories for the required build locations.
pub fn fake_model_config() -> ModelConfig {
    let dir_path = tempdir().unwrap().keep();
    let update_package_path = dir_path.join("update.far");
    let blobs_directory = dir_path.join("blobs");
    ModelConfig {
        update_package_path,
        blobs_directory,
        delivery_blob_type: DeliveryBlobType::Reserved,
        component_tree_config_paths: Vec::new(),
        is_recovery: false,
    }
}

/// Constructs a simple fake data model with an in memory uri and tempdata()
/// build directory.
pub fn fake_data_model() -> Arc<DataModel> {
    Arc::new(DataModel::new(fake_model_config()).unwrap())
}

const FAKE_PKG_MERKLE: [u8; HASH_SIZE] = [0x42; HASH_SIZE];

pub fn fake_component_src_pkg() -> ComponentSource {
    ComponentSource::Package(FAKE_PKG_MERKLE.into())
}

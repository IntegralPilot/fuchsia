// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::scrutiny_artifacts::ScrutinyArtifacts;

use scrutiny_collector::unified_collector::UnifiedCollector;

use anyhow::Result;
use scrutiny_collection::model::DataModel;
use scrutiny_collection::model_config::ModelConfig;
use std::path::Path;
use std::sync::Arc;

pub struct Scrutiny {
    model_config: ModelConfig,
}

impl Scrutiny {
    pub fn from_product_bundle(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { model_config: ModelConfig::from_product_bundle(path)? })
    }

    pub fn from_product_bundle_recovery(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { model_config: ModelConfig::from_product_bundle_recovery(path)? })
    }

    pub fn set_component_tree_config_paths(&mut self, paths: &Vec<impl AsRef<Path>>) {
        let mut path_bufs = Vec::new();
        for p in paths {
            path_bufs.push(p.as_ref().to_path_buf());
        }
        self.model_config.component_tree_config_paths = path_bufs;
    }

    pub fn collect(self) -> Result<ScrutinyArtifacts> {
        let model = Arc::new(DataModel::new(self.model_config)?);
        let collector = UnifiedCollector::default();
        collector.collect(model.clone())?;
        Ok(ScrutinyArtifacts { model })
    }
}

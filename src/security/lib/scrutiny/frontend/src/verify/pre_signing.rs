// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use scrutiny_collection::additional_boot_args::AdditionalBootConfigCollection;
use scrutiny_collection::model::DataModel;
use scrutiny_collection::static_packages::StaticPkgsCollection;
use scrutiny_collection::zbi::Zbi;
use scrutiny_utils::artifact::{ArtifactReader, FileArtifactReader};
use scrutiny_utils::build_checks;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::PathBuf;
use std::sync::Arc;

/// The output of the PreSigningController is a set of errors found if any of the checks fail.
#[derive(Deserialize, Serialize)]
pub struct PreSigningResponse {
    pub errors: Vec<build_checks::ValidationError>,
}

pub struct PreSigningController;

impl PreSigningController {
    pub fn collect_errors(
        model: Arc<DataModel>,
        policy_path: String,
        golden_files_dir: String,
    ) -> Result<PreSigningResponse> {
        let policy_str = read_to_string(&policy_path)
            .context(format!("Failed to read policy file from {:?}", policy_path))?;
        let policy: build_checks::BuildCheckSpec = serde_json5::from_str(&policy_str)
            .context(format!("Failed to parse policy file from {:?}", policy_path))?;

        let boot_config_model = model
            .get::<AdditionalBootConfigCollection>()
            .context("Failed to get AdditionalBootConfigCollection")?;
        if boot_config_model.errors.len() > 0 {
            return Err(anyhow!(
                "Cannot validate additional boot args: AdditionalBootConfigCollector reported errors {:?}",
                boot_config_model.errors
            ));
        }

        let boot_args_data = match boot_config_model.additional_boot_args.clone() {
            Some(data) => data,
            None => HashMap::new(),
        };

        let static_pkgs =
            model.get::<StaticPkgsCollection>().context("Failed to get StaticPkgsCollection")?;
        if static_pkgs.errors.len() > 0 {
            return Err(anyhow!(
                "Cannot perform validations involving static packages: StaticPkgCollector reported errors {:?}",
                static_pkgs.errors
            ));
        }

        let static_pkgs_map = static_pkgs.static_pkgs.clone().unwrap_or_else(HashMap::new);

        // Remove variant from package name and convert hash to string.
        // Build checks validation expects a map of package names to merkle hash strings.
        let static_pkgs_map: HashMap<String, String> = static_pkgs_map
            .into_iter()
            .map(|((name, _variant), hash)| (name.as_ref().to_string(), hash.to_string()))
            .collect();

        let zbi_data = model.get::<Zbi>().context("Failed to get ZbiCollection")?;
        let bootfs_files = &zbi_data.bootfs_files.bootfs_files;

        let mut blobs_artifact_reader: Box<dyn ArtifactReader> = Box::new(FileArtifactReader::new(
            &PathBuf::new(),
            &model.config().blobs_directory(),
            model.config().delivery_blob_type,
        ));

        let validation_errors = build_checks::validate_build_checks(
            policy,
            boot_args_data,
            bootfs_files,
            static_pkgs_map,
            &mut blobs_artifact_reader,
            &golden_files_dir,
        )
        .context("Failed to run validation checks")?;

        Ok(PreSigningResponse { errors: validation_errors })
    }
}

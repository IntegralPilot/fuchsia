// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, anyhow};
use ffx_scrutiny_verify_args::route_sources::Command;
use scrutiny_frontend::Scrutiny;
use scrutiny_frontend::verify::route_sources::RouteSourceError;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

struct Query {
    product_bundle: PathBuf,
    config_path: String,
    tmp_dir_path: Option<PathBuf>,
    recovery: bool,
}

impl Query {
    fn with_temporary_directory(mut self, tmp_dir_path: Option<&PathBuf>) -> Self {
        self.tmp_dir_path = tmp_dir_path.map(PathBuf::clone);
        self
    }

    fn with_recovery_artifacts(mut self) -> Self {
        self.recovery = true;
        self
    }
}

impl TryFrom<&Command> for Query {
    type Error = Error;
    fn try_from(cmd: &Command) -> Result<Self, Self::Error> {
        let config_path = cmd.config.to_str().ok_or_else(|| {
            anyhow!(
                "Route sources configuration file path {:?} cannot be converted to string for passing to scrutiny",
                cmd.config
            )
        })?;
        let config_path = config_path.to_string();
        Ok(Query {
            product_bundle: cmd.product_bundle.clone(),
            config_path,
            tmp_dir_path: None,
            recovery: false,
        })
    }
}

fn verify_route_sources(query: Query) -> Result<HashSet<PathBuf>> {
    let artifacts = if query.recovery {
        Scrutiny::from_product_bundle_recovery(&query.product_bundle)
    } else {
        Scrutiny::from_product_bundle(&query.product_bundle)
    }?
    .collect()?;
    let route_sources_results = artifacts.get_route_sources(query.config_path)?;

    let mut errors = HashMap::new();
    for (path, results) in route_sources_results.results.iter() {
        let component_errors: Vec<&RouteSourceError> =
            results.iter().filter_map(|result| result.result.as_ref().err()).collect();
        if component_errors.len() > 0 {
            errors.insert(path.clone(), component_errors);
        }
    }
    if errors.len() > 0 {
        return Err(anyhow!("verify.route_sources reported errors: {:#?}", errors));
    }

    Ok(route_sources_results.deps)
}

pub async fn verify(
    cmd: &Command,
    tmp_dir: Option<&PathBuf>,
    recovery: bool,
) -> Result<HashSet<PathBuf>> {
    let mut query = Query::try_from(cmd)?.with_temporary_directory(tmp_dir);
    if recovery {
        query = query.with_recovery_artifacts();
    }
    let deps = verify_route_sources(query)?;
    Ok(deps)
}

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, anyhow};
use ffx_scrutiny_verify_args::pre_signing::Command;
use scrutiny_frontend::Scrutiny;
use std::collections::HashSet;
use std::path::PathBuf;

pub async fn verify(cmd: &Command, recovery: bool) -> Result<HashSet<PathBuf>> {
    let mut deps = HashSet::new();
    let policy_path =
        &cmd.policy.to_str().context("failed to convert policy PathBuf to string")?.to_owned();
    let golden_files_dir = &cmd
        .golden_files_dir
        .to_str()
        .context("failed to convert golden_files_dir PathBuf to string")?
        .to_owned();
    let artifacts = if recovery {
        Scrutiny::from_product_bundle_recovery(&cmd.product_bundle)
    } else {
        Scrutiny::from_product_bundle(&cmd.product_bundle)
    }?
    .collect()?;

    let response =
        artifacts.collect_presigning_errors(policy_path.clone(), golden_files_dir.clone())?;
    if response.errors.len() > 0 {
        println!(
            "The build has failed pre-signing checks defined by the policy file: {:?}",
            policy_path
        );
        println!("");
        for e in response.errors {
            println!("{}", e);
        }
        println!("");
        return Err(anyhow!("Pre-signing verification failed."));
    }

    deps.insert(cmd.policy.clone());
    Ok(deps)
}

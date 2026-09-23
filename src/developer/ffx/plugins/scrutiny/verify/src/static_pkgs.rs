// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use ffx_scrutiny_verify_args::static_pkgs::Command;
use scrutiny_frontend::Scrutiny;
use scrutiny_utils::golden::{CompareResult, GoldenFile};
use std::collections::HashSet;
use std::path::PathBuf;

const SOFT_TRANSITION_MSG : &str = "
If you are making a change in fuchsia.git that causes this, you need to perform a soft transition:
1: Instead of adding lines as written above, add each line prefixed with a question mark to mark it as transitional.
2: Instead of removing lines as written above, prefix the line with a question mark to mark it as transitional.
3: Check in your fuchsia.git change.
4: For each new line you added in 1, remove the question mark.
5: For each existing line you modified in 2, remove the line.
";

struct Query {
    product_bundle: PathBuf,
    recovery: bool,
}

fn verify_static_pkgs(query: &Query, golden_files: &Vec<PathBuf>) -> Result<HashSet<PathBuf>> {
    let artifacts = if query.recovery {
        Scrutiny::from_product_bundle_recovery(&query.product_bundle)
    } else {
        Scrutiny::from_product_bundle(&query.product_bundle)
    }?
    .collect()?;

    let static_pkgs_result = artifacts.get_static_packages()?;
    if static_pkgs_result.errors.len() > 0 {
        return Err(anyhow!("static.pkgs reported errors: {:#?}", static_pkgs_result.errors));
    }
    if static_pkgs_result.static_pkgs.is_none() {
        return Err(anyhow!("static.pkgs returned empty result"));
    }
    let static_pkgs = static_pkgs_result.static_pkgs.unwrap();

    // Extract package names from static package descriptions.
    let static_package_names: Vec<String> = static_pkgs
        .into_iter()
        .map(|((name, _variant), _hash)| name.as_ref().to_string())
        .collect();

    let golden_file =
        GoldenFile::from_files(golden_files).context("Failed to parse golden files")?;

    match golden_file.compare(static_package_names) {
        CompareResult::Matches => {
            let mut deps = static_pkgs_result.deps;
            deps.extend(golden_files.clone());
            Ok(deps)
        }
        CompareResult::Mismatch { errors } => {
            println!("Static package file mismatch");
            println!("");
            for error in errors.iter() {
                println!("{}", error);
            }
            println!("");
            println!(
                "If you intended to change the static package contents, please acknowledge it by updating {:?} with the added or removed lines.",
                golden_files[0]
            );
            println!("{}", SOFT_TRANSITION_MSG);
            Err(anyhow!("static file mismatch"))
        }
    }
}

pub async fn verify(cmd: &Command, recovery: bool) -> Result<HashSet<PathBuf>> {
    let product_bundle = cmd.product_bundle.clone();
    let query = Query { product_bundle, recovery };
    Ok(verify_static_pkgs(&query, &cmd.golden)?)
}

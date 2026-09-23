// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use ffx_scrutiny_verify_args::bootfs::Command;
use scrutiny_frontend::Scrutiny;
use scrutiny_utils::golden::{CompareResult, GoldenFile};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const SOFT_TRANSITION_MSG : &str = "
If you are making a change in fuchsia.git that causes this, you need to perform a soft transition:
1: Instead of adding lines as written above, add each line prefixed with a question mark to mark it as transitional.
2: Instead of removing lines as written above, prefix the line with a question mark to mark it as transitional.
3: Check in your fuchsia.git change.
4: For each new line you added in 1, remove the question mark.
5: For each existing line you modified in 2, remove the line.
";

pub async fn verify(cmd: &Command, recovery: bool) -> Result<HashSet<PathBuf>> {
    if cmd.golden.len() == 0 && cmd.golden_packages.len() == 0 {
        bail!("Must specify at least one --golden or --golden-packages");
    }
    let mut deps = HashSet::new();

    let artifacts = if recovery {
        Scrutiny::from_product_bundle_recovery(&cmd.product_bundle)
    } else {
        Scrutiny::from_product_bundle(&cmd.product_bundle)
    }?
    .collect()?;

    let unscrutinized_dirs = std::collections::HashSet::from([
        // blob dir files are unscrutinized, because their presence is dictated
        // by the boot package index, which itself is scrutinized the same way
        // static_packages is scrutinized.
        Some(Path::new("blob")),
    ]);

    let bootfs_file_names = artifacts.get_bootfs_files()?;
    let total_bootfs_file_count = bootfs_file_names.len();

    let non_blob_files = bootfs_file_names
        .into_iter()
        .filter(|filename| !unscrutinized_dirs.contains(&Path::new(filename).parent()))
        .collect::<Vec<String>>();

    let golden_file =
        GoldenFile::from_files(&cmd.golden).context("Failed to open the golden files")?;
    match golden_file.compare(non_blob_files.clone()) {
        CompareResult::Matches => Ok(()),
        CompareResult::Mismatch { errors } => {
            println!("Bootfs file mismatch\n");
            for error in errors.iter() {
                println!("{}", error);
            }
            println!(
                "\nIf you intended to change the bootfs contents, please acknowledge it by updating {:?} with the added or removed lines",
                cmd.golden[0],
            );
            println!("{}", SOFT_TRANSITION_MSG);
            Err(anyhow!("bootfs file mismatch"))
        }
    }?;

    deps.extend(cmd.golden.clone());

    let bootfs_packages = artifacts.get_bootfs_packages()?;

    // TODO(https://fxbug.dev/42179754) After the first bootfs package is migrated to a component, an
    // absence of a bootfs package index is an error.
    if bootfs_packages.is_none() {
        if non_blob_files.len() != total_bootfs_file_count {
            return Err(anyhow!(
                "tool.zbi.extract.bootfs.packages returned empty result, but there were blobs in the bootfs."
            ));
        } else {
            return Ok(deps);
        }
    }

    // Extract package names from bootfs package descriptions.
    let bootfs_packages = bootfs_packages.unwrap();
    let bootfs_package_names: Vec<String> = bootfs_packages
        .into_iter()
        .map(|((name, _variant), _hash)| name.as_ref().to_string())
        .collect();

    let golden_packages = GoldenFile::from_files(&cmd.golden_packages)
        .context("Failed to open the golden packages lists")?;
    match golden_packages.compare(bootfs_package_names) {
        CompareResult::Matches => Ok(()),
        CompareResult::Mismatch { errors } => {
            println!("Bootfs package index mismatch \n");
            for error in errors.iter() {
                println!("{}", error);
            }
            println!(
                "\nIf you intended to change the bootfs contents, please acknowledge it by updating {:?} with the added or removed lines.",
                cmd.golden_packages[0],
            );
            println!("{}", SOFT_TRANSITION_MSG);
            Err(anyhow!("bootfs package index mismatch"))
        }
    }?;
    deps.extend(cmd.golden_packages.clone());

    Ok(deps)
}

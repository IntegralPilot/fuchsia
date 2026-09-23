// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use ffx_scrutiny_verify_args::kernel_cmdline::Command;
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

// Query information common to multiple verification passes that may run against different golden
// files.
struct Query {
    // A host filesystem path to the product bundle.
    product_bundle: PathBuf,
    // Whether to build scrutiny model based on recovery-mode build artifacts.
    recovery: bool,
}

fn verify_kernel_cmdline<P: AsRef<Path>>(query: &Query, golden_paths: &Vec<P>) -> Result<()> {
    let artifacts = if query.recovery {
        Scrutiny::from_product_bundle_recovery(&query.product_bundle)
    } else {
        Scrutiny::from_product_bundle(&query.product_bundle)
    }?
    .collect()?;

    let cmdline = artifacts.get_cmdline()?;

    let golden_file =
        GoldenFile::from_files(&golden_paths).context("Failed to open golden files")?;
    match golden_file.compare(cmdline) {
        CompareResult::Matches => Ok(()),
        CompareResult::Mismatch { errors } => {
            println!("Kernel cmdline mismatch");
            println!("");
            for error in errors.iter() {
                println!("{}", error);
            }
            println!("");
            println!(
                "If you intended to change the kernel command line, please acknowledge it by updating {:?} with the added or removed lines.",
                golden_paths[0].as_ref()
            );
            println!("{}", SOFT_TRANSITION_MSG);
            Err(anyhow!("kernel cmdline mismatch"))
        }
    }
}

pub async fn verify(cmd: &Command, recovery: bool) -> Result<HashSet<PathBuf>> {
    if cmd.golden.len() == 0 {
        bail!("Must specify at least one --golden");
    }
    let mut deps = HashSet::new();

    let query = Query { product_bundle: cmd.product_bundle.clone(), recovery };
    verify_kernel_cmdline(&query, &cmd.golden)?;

    deps.extend(cmd.golden.clone());
    Ok(deps)
}

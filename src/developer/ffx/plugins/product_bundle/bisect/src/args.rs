// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;
use pbms::AuthFlowChoice;

/// Generate a list of released versions between [--from-success] and [--to-failure] for every
/// artifact contained within the given product bundle (e.g. platform, product, board).
///
/// Assemble intermediate product bundles using combinations of the artifacts, and facilitate
/// running tests on those PBs to quickly identify which artifact is the source of a bug or
/// behavior change.
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "bisect",
    note = "\
    VERSION formats:
  This parameter type (from-success, to-failure) refers to the product bundle version.

  Steps to retrieve this information depends on which product bundle is being bisected,
  but it can always be retrieved from the product bundle itself by running:

    ffx product-bundle get-version <path/to/product/bundle>
    ",
    note = "\
    For more information about how to use this tool, see go/fuchsia-product-bisection-userguide
    ",
    note = "\
    VALIDATION SCRIPT REQUIREMENTS:
  If a --script is provided, it will be executed for each bisection step.
  The script will receive the following arguments:
    --pb <path>  : The absolute path to the assembled product bundle for the current step.

  The script must exit with one of the following codes:
    0          : Pass (The bug is NOT present in this version)
    1-127      : Fail (The bug IS present in this version)
    128+       : Infrastructure failure (A critical infrastructure failure occurred)
    ",
    example = "\
    // 1. Bisect the core.vim3 product bundle between two provided versions.
// (Note: core.vim3 is not yet supported: https://fxbug.dev/485981469 .)

ffx product-bundle bisect core.vim3 \\
    --from-success 29.20250826.6.1 \\
    --to-failure 29.20250905.6.1


// 2. Automate bisection with a validation script.
// Create a file called `validate.sh` with the following content:

#!/bin/bash
# Extract the PB path from the arguments
while [[ \"$#\" -gt 0 ]]; do
    case $1 in
        --pb) PB_PATH=\"$2\"; shift 2 ;;
        *) shift ;;
    esac
done

# Flash the device with the assembled product bundle
ffx target flash \"$PB_PATH\"
if [ $? -ne 0 ]; then
    echo \"Failed to flash, skipping...\"
    exit 128 # Infrastructure failure
fi

# <this is where the test code goes>
# e.g., ffx test run ...

# Return 0 for Pass, 1 for Fail
exit 0

// Then pass the script to the bisection tool:
ffx product-bundle bisect core.vim3 \\
    --from-success 29.20250826.6.1 \\
    --to-failure 29.20250905.6.1 \\
    --script ./validate.sh
    "
)]
pub struct BisectCommand {
    /// name of the product bundle being bisected.
    #[argh(positional)]
    pub name: String,

    /// known-good version of the product bundle. The bisection search window begins here.
    #[argh(option)]
    pub from_success: String,

    /// known-bad version of the product bundle. The bisection search window ends here.
    #[argh(option)]
    pub to_failure: String,

    /// slot to bisect over (a or r). Defaults to slot A.
    #[argh(option, default = "Default::default()")]
    pub slot: assembly_artifact_cache::Slot,

    /// directory to write assembled images and other artifacts. Defaults to ~/<plan_directory>/out
    #[argh(option)]
    pub out_dir: Option<Utf8PathBuf>,

    /// directory to write intermediate files. Defaults to ~/<plan_directory>/gen
    #[argh(option)]
    pub gen_dir: Option<Utf8PathBuf>,

    /// script to run for automated bisection.
    #[argh(option)]
    pub script: Option<Utf8PathBuf>,

    /// authentication method to use.
    #[argh(option, default = "AuthFlowChoice::Default")]
    pub auth: AuthFlowChoice,
}

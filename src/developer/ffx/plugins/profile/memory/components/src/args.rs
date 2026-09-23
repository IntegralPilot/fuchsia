// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Argument-parsing specification for the `components` subcommand.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

/// Components.
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "components")]
pub struct ComponentsCommand {
    #[argh(switch, description = "loads the unprocessed memory information as json from stdin.")]
    pub stdin_input: bool,

    /// Uses the same format as the FIDL table Snapshot in
    /// //sdk/fidl/fuchsia.memory.attribution.plugin/plugin.fidl.
    #[argh(
        switch,
        description = "outputs the unprocessed memory information from the device as json."
    )]
    pub debug_json: bool,

    #[argh(switch, description = "outputs data in csv format.")]
    pub csv: bool,

    #[argh(switch, short = 'b', description = "prints a bucketized digest of the memory usage.")]
    pub buckets: bool,

    #[argh(
        switch,
        short = 'l',
        description = "used in conjunction with --buckets to print the content of each bucket."
    )]
    pub list_vmos: bool,

    #[argh(switch, description = "outputs a detailed output, machine only.")]
    pub detailed: bool,

    #[argh(
        option,
        description = "outputs system-wide statistics at regular intervals (in seconds)"
    )]
    pub stats_only: Option<u64>,

    #[argh(
        option,
        description = "path to the assembly manifest. Adds blob manifest and file path to the detailed json output. Locate manifest in the build output with: cat \"assembly_manifests.json\" | jq -r '.[] | select(.image_name==\"fuchsia\").assembly_manifest_path'"
    )]
    pub assembly_manifest: Option<Utf8PathBuf>,

    #[argh(
        switch,
        description = "use the abridged memory collection that omits Starnix processes."
    )]
    pub abridged: bool,
}

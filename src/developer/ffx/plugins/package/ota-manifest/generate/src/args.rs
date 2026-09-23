// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "generate", description = "Generate a new OTA Manifest.")]
pub struct GenerateCommand {
    /// path to the target product bundle directory or an existing manifest file
    #[argh(option)]
    pub target: Utf8PathBuf,

    /// output file path for the new manifest
    #[argh(option, short = 'o')]
    pub output: Utf8PathBuf,

    /// optional new blob base url
    #[argh(option)]
    pub blob_base_url: Option<String>,

    /// optional vbmeta asset image file to replace the existing one
    #[argh(option)]
    pub vbmeta: Option<Utf8PathBuf>,

    /// optional zbi asset image file to replace the existing one
    #[argh(option)]
    pub zbi: Option<Utf8PathBuf>,

    /// optional manifest private key file (PEM or PKCS8).
    /// If not provided, the default dev key is used.
    #[argh(option)]
    pub key: Option<Utf8PathBuf>,

    /// optional file containing the signature of the manifest public key.
    /// If not provided, the manifest private key is used as root key to sign its own public key.
    #[argh(option)]
    pub key_signature: Option<Utf8PathBuf>,
}

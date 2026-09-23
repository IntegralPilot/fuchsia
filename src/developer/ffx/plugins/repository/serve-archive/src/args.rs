// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;
use ffx_repository_server_start_args::default_address;
use std::net::SocketAddr;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "serve-archive",
    description = "Serve a package archive to a target device"
)]
pub struct ServeArchiveCommand {
    #[argh(positional, description = "path to the package archive (.far)")]
    pub archive: Utf8PathBuf,

    #[argh(
        option,
        short = 'r',
        default = "String::from(\"devhost\")",
        description = "repository name. Default is `devhost`."
    )]
    pub repository: String,

    #[argh(
        option,
        default = "default_address()",
        description = "address on which to serve the repository. Default is `[::]:8083`."
    )]
    pub address: SocketAddr,

    #[argh(
        option,
        description = "set up a rewrite rule mapping each `alias` host to the repository identified by `name`."
    )]
    pub alias: Vec<String>,

    #[argh(
        option,
        description = "the address used to listen on target-side when tunneling is used."
    )]
    pub tunnel_addr: Option<SocketAddr>,

    #[argh(
        switch,
        description = "if true, will not register repositories to device. Default is `false`."
    )]
    pub no_device: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_parse_args() {
        let cmd =
            ServeArchiveCommand::from_args(&["serve-archive"], &["/path/to/archive.far"]).unwrap();

        assert_eq!(cmd.archive, Utf8PathBuf::from("/path/to/archive.far"));
        assert_eq!(cmd.repository, "devhost");
        assert_eq!(cmd.address, default_address());
        assert_eq!(cmd.tunnel_addr, None);
        assert_eq!(cmd.alias, Vec::<String>::new());
        assert_eq!(cmd.no_device, false);
    }
}

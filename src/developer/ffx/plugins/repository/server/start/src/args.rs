// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use core::net::Ipv6Addr;
use ffx_core::ffx_command;
use fidl_fuchsia_pkg_ext::{RepositoryRegistrationAliasConflictMode, RepositoryStorageType};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "start", description = "Starts the package repository server.")]
pub struct StartCommand {
    /// address on which to serve the repository.
    /// Note that this can be either IPV4 or IPV6.
    /// For example, [::]:8083 or 127.0.0.1:8083
    /// Default is `[::]:8083`.
    #[argh(option)]
    pub address: Option<SocketAddr>,

    /// run server as a background process. This is mutually exclusive with
    /// --foreground.
    #[argh(switch)]
    pub background: bool,

    /// run server as a foreground process.  This is the default mode. This is
    /// mutually exclusive with --background.
    #[argh(switch)]
    pub foreground: bool,

    /// option used to indicate running as a detached process. Hidden from help.
    #[argh(switch, hidden_help)]
    pub disconnected: bool,

    // LINT.IfChange
    #[argh(option, short = 'r')]
    /// register this repository.
    /// Default is `devhost`.
    pub repository: Option<String>,
    // LINT.ThenChange(/src/developer/ffx/lib/pkg/src/config.rs:devhost_name)
    /// path to the root metadata that was used to sign the
    /// repository TUF metadata. This establishes the root of
    /// trust for this repository. If the TUF metadata was not
    /// signed by this root metadata, running this command
    /// will result in an error.
    /// Default is to use 1.root.json from the repository.
    #[argh(option)]
    pub trusted_root: Option<Utf8PathBuf>,

    /// location of the package repo.
    /// Default is given by the build directory
    /// obtained from the ffx context.
    #[argh(option)]
    pub repo_path: Option<Utf8PathBuf>,

    /// location of product bundle.
    #[argh(option)]
    pub product_bundle: Option<Utf8PathBuf>,

    /// set up a rewrite rule mapping each `alias` host to
    /// the repository identified by `name`.
    #[argh(option)]
    pub alias: Vec<String>,

    /// enable persisting this repository across reboots.
    /// Default is `Ephemeral`.
    #[argh(option, from_str_fn(parse_storage_type))]
    pub storage_type: Option<RepositoryStorageType>,

    /// resolution mechanism when alias registrations conflict. Must be either
    /// `error-out` or `replace`.
    /// Default is `replace`.
    #[argh(
        option,
        default = "default_alias_conflict_mode()",
        from_str_fn(parse_alias_conflict_mode)
    )]
    pub alias_conflict_mode: RepositoryRegistrationAliasConflictMode,

    /// location to write server port information to, in case port dynamically
    /// instantiated.
    #[argh(option)]
    pub port_path: Option<PathBuf>,

    /// the address used to listen on target-side when tunneling is used.
    #[argh(option)]
    pub tunnel_addr: Option<SocketAddr>,

    /// if true, will not register repositories to device.
    /// Default is `false`.
    #[argh(switch)]
    pub no_device: bool,

    /// refresh repository metadata during startup.
    /// Note that this is not necessary if package-tool runs in the background
    /// taking care of it, e.g. as part of `fx serve`.
    /// Default is `false`.
    #[argh(switch)]
    pub refresh_metadata: bool,

    /// auto publish packages listed in the given manifest. This uses time based versioning when publishing
    /// and ignores missing packages. The manifest is a json file a single member "content", which
    /// contains a list named "manifest". The list is a list of package manifests, relative paths are
    /// relative to the directory of the auto-publish manifest.
    #[argh(option)]
    pub auto_publish: Option<Utf8PathBuf>,
}

pub fn default_address() -> SocketAddr {
    (Ipv6Addr::UNSPECIFIED, 8083).into()
}

pub fn default_tunnel_addr() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}

pub fn parse_storage_type(arg: &str) -> Result<RepositoryStorageType, String> {
    match arg {
        "ephemeral" => Ok(RepositoryStorageType::Ephemeral),
        "persistent" => Ok(RepositoryStorageType::Persistent),
        _ => Err(format!("unknown storage type {}", arg)),
    }
}

pub fn default_alias_conflict_mode() -> RepositoryRegistrationAliasConflictMode {
    RepositoryRegistrationAliasConflictMode::Replace
}

pub fn parse_alias_conflict_mode(
    arg: &str,
) -> Result<RepositoryRegistrationAliasConflictMode, String> {
    match arg {
        "error-out" => Ok(RepositoryRegistrationAliasConflictMode::ErrorOut),
        "replace" => Ok(RepositoryRegistrationAliasConflictMode::Replace),
        _ => Err(format!("unknown alias conflict mode {}", arg)),
    }
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fidl_fuchsia_pkg_ext::{RepositoryRegistrationAliasConflictMode, RepositoryStorageType};
use std::net::SocketAddr;
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug, PartialEq)]
pub enum JsonURI {
    LocalFile(PathBuf),
    WebURL(Url),
}

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "register",
    description = "Make the target aware of a specific repository"
)]
pub struct RegisterCommand {
    /// register this repository, rather than the default.
    #[argh(option, short = 'r')]
    pub repository: Option<String>,

    /// repository server port number.
    /// Required to disambiguate multiple repositories with the same name.
    #[argh(option, short = 'p')]
    pub port: Option<u16>,

    /// repository server address override.
    /// When provided, overrides the server address registered on the target.
    /// Required when the server's listening address is not directly reachable
    /// by the target.
    #[argh(option)]
    pub address_override: Option<SocketAddr>,

    /// enable persisting this repository across reboots.
    #[argh(option, from_str_fn(parse_storage_type))]
    pub storage_type: Option<RepositoryStorageType>,

    /// set up a rewrite rule mapping each `alias` host to
    /// to the repository identified by `name`.
    #[argh(option)]
    pub alias: Vec<String>,

    /// resolution mechanism when alias registrations conflict. Must be either
    /// `error-out` or `replace`. Default is `replace`.
    #[argh(
        option,
        default = "default_alias_conflict_mode()",
        from_str_fn(parse_alias_conflict_mode)
    )]
    pub alias_conflict_mode: RepositoryRegistrationAliasConflictMode,

    /// register a repository on the target device from a configuration in a json file. Must be
    /// either a path or an URL. Note that ffx will not manage this repository automatically.
    /// This flag is mutually exclusive with the other flags, since it reads all repo config data
    /// from the provided configuration file.
    #[argh(option, from_str_fn(parse_json_uri))]
    pub json_uri: Option<JsonURI>,
}

fn parse_storage_type(arg: &str) -> Result<RepositoryStorageType, String> {
    match arg {
        "ephemeral" => Ok(RepositoryStorageType::Ephemeral),
        "persistent" => Ok(RepositoryStorageType::Persistent),
        _ => Err(format!("unknown storage type {}", arg)),
    }
}

fn default_alias_conflict_mode() -> RepositoryRegistrationAliasConflictMode {
    RepositoryRegistrationAliasConflictMode::Replace
}

fn parse_alias_conflict_mode(arg: &str) -> Result<RepositoryRegistrationAliasConflictMode, String> {
    match arg {
        "error-out" => Ok(RepositoryRegistrationAliasConflictMode::ErrorOut),
        "replace" => Ok(RepositoryRegistrationAliasConflictMode::Replace),
        _ => Err(format!("unknown alias conflict mode {}", arg)),
    }
}

pub fn parse_json_uri(arg: &str) -> Result<JsonURI, String> {
    if let Ok(url) = Url::parse(arg) {
        if url.scheme() == "file" {
            Ok(JsonURI::LocalFile(PathBuf::from(url.path())))
        } else {
            Ok(JsonURI::WebURL(url))
        }
    } else {
        Ok(JsonURI::LocalFile(PathBuf::from(arg)))
    }
}

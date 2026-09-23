// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
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
    name = "replace",
    description = "Replace all dynamic rules with the provided rules"
)]
pub struct ReplaceCommand {
    /// read the rules from a configuration in a json file. Must be either a path or an URL. This is
    /// mutually exclusive with the `rule` option.
    #[argh(option, short = 'u', from_str_fn(parse_json_uri))]
    pub json_uri: Option<JsonURI>,

    /// read the rules from a string provided in the argument. Must be valid JSON. This is mutually
    /// exclusive with the `json_uri` option.
    #[argh(option, short = 'r')]
    pub rule: Option<String>,
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

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use ffx_scrutiny_shell_args::ScrutinyShellCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use scrutiny_frontend::{
    BlobFsExtractController, FarMetaExtractController, FvmExtractController,
    ZbiExtractBootfsPackageIndex, ZbiExtractCmdlineController, ZbiExtractController,
    ZbiListBootfsController,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

#[derive(FfxTool)]
#[target(None)]
pub struct ScrutinyShellTool {
    #[command]
    pub cmd: ScrutinyShellCommand,
}

fho::embedded_plugin!(ScrutinyShellTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScrutinyShellTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let (namespace, args) =
            parse_command(self.cmd.command).map_err(|e| fho::Error::User(e.into()))?;
        let value = match namespace.as_str() {
            "tool.blobfs.extract" => {
                BlobFsExtractController::extract(args.input, args.output.unwrap())
            }
            "tool.far.extract.meta" => FarMetaExtractController::extract(args.input),
            "tool.fvm.extract" => FvmExtractController::extract(args.input, args.output.unwrap()),
            "tool.zbi.extract" => ZbiExtractController::extract(args.input, args.output.unwrap()),
            "tool.zbi.extract.cmdline" => ZbiExtractCmdlineController::extract(args.input),
            "tool.zbi.list.bootfs" => ZbiListBootfsController::extract(args.input),
            "tool.zbi.extract.bootfs.packages" => ZbiExtractBootfsPackageIndex::extract(args.input),
            _ => Err(anyhow!("Invalid command: {}", namespace)),
        }?;
        let s = serde_json::to_string_pretty(&value).map_err(|e| fho::Error::User(e.into()))?;
        println!("{}", s);
        Ok(())
    }
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct ToolArgs {
    input: PathBuf,
    output: Option<PathBuf>,
}

fn parse_command(command: String) -> Result<(String, ToolArgs)> {
    let mut tokens: VecDeque<String> =
        command.split_whitespace().map(|s| String::from(s)).collect();
    if tokens.len() == 0 {
        bail!("Empty command");
    }
    let namespace = tokens.pop_front().unwrap();

    // Parse the command arguments.
    let empty_command: HashMap<String, String> = HashMap::new();
    let mut query = json!(empty_command);
    if !tokens.is_empty() {
        if tokens.front().unwrap().starts_with("--") {
            query = args_to_json(&tokens);
        }
    }
    let args: ToolArgs = serde_json::from_value(query)?;
    Ok((namespace, args))
}

/// Converts a series of tokens into a single json value. For example:
/// --foo bar --baz a b would produce the json:
/// {
///     "foo": "bar",
///     "baz": "a b",
/// }
pub fn args_to_json(tokens: &VecDeque<String>) -> Value {
    let mut name: Option<String> = None;
    let mut map: HashMap<String, String> = HashMap::new();

    for token in tokens.iter() {
        if token.starts_with("--") {
            let stripped_name = token.strip_prefix("--").unwrap().to_string();
            map.insert(stripped_name.clone(), String::new());
            name = Some(stripped_name);
        } else if let Some(name) = &name {
            if let Some(entry) = map.get_mut(name) {
                if !entry.is_empty() {
                    entry.push_str(" ");
                }
                entry.push_str(token);
            }
        }
    }
    json!(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command() {
        assert_eq!(parse_command("foo".into()).is_ok(), false);
        assert_eq!(
            parse_command("foo --input in".into()).unwrap(),
            ("foo".into(), ToolArgs { input: "in".into(), output: None })
        );
        assert_eq!(
            parse_command("foo --input in --output out".into()).unwrap(),
            ("foo".into(), ToolArgs { input: "in".into(), output: Some("out".into()) })
        );
        assert_eq!(parse_command("foo --output out".into()).is_ok(), false);
    }
}

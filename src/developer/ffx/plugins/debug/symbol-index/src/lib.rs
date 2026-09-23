// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use errors::ffx_bail;
use ffx_debug_symbol_index_args::*;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use symbol_index::*;

#[derive(Debug)]
struct SymbolIndexPath {
    inner: String,
}

#[async_trait(?Send)]
impl fho::TryFromEnv for SymbolIndexPath {
    type Error = fho::Error;
    async fn try_from_env(_env: &fho::FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        Ok(SymbolIndexPath { inner: global_symbol_index_path().bug()? })
    }
}

#[derive(FfxTool)]
pub struct SymbolIndexTool {
    #[command]
    cmd: SymbolIndexCommand,
    path: SymbolIndexPath,
}

fho::embedded_plugin!(SymbolIndexTool);

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Ok,
    /// Successful execution with information strings.
    Index(SymbolIndex),
}

impl CommandStatus {
    fn index(&self) -> Option<&SymbolIndex> {
        match self {
            Self::Ok => None,
            Self::Index(s) => Some(s),
        }
    }
}

#[async_trait(?Send)]
impl FfxMain for SymbolIndexTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        match self.cmd.sub_command {
            SymbolIndexSubCommand::List(cmd) => {
                let result = CommandStatus::Index(list(cmd, &self.path.inner)?);
                // This is a little awkward: the command is intended to be printed in the normal
                // non-machine way as JSON output, so this prints it as JSON in both ways, with
                // just one being wrapped in the `CommandStatus` schema.
                writer
                    .machine_or_else(&result, || {
                        serde_json::to_string_pretty(result.index().unwrap())
                            .expect("serializing json")
                    })
                    .bug()?;
            }
            SymbolIndexSubCommand::Add(cmd) => {
                add(cmd, &self.path.inner)?;
                writer.machine(&CommandStatus::Ok).bug()?;
            }
            SymbolIndexSubCommand::Remove(cmd) => {
                remove(cmd, &self.path.inner)?;
                writer.machine(&CommandStatus::Ok).bug()?;
            }
            SymbolIndexSubCommand::Clean(cmd) => {
                clean(cmd, &self.path.inner)?;
                writer.machine(&CommandStatus::Ok).bug()?;
            }
        }
        Ok(())
    }
}

fn list(cmd: ListCommand, global_symbol_index_path: &str) -> Result<SymbolIndex> {
    let index = if cmd.aggregated {
        SymbolIndex::load_aggregate(global_symbol_index_path)?
    } else {
        SymbolIndex::load(global_symbol_index_path)?
    };
    Ok(index)
}

fn add(cmd: AddCommand, global_symbol_index_path: &str) -> Result<()> {
    // Create a new one if the global symbol-index.json doesn't exist or is malformed.
    let mut index =
        SymbolIndex::load(global_symbol_index_path).unwrap_or_else(|_| SymbolIndex::new());

    // Determine if this is a path or a URL.
    if cmd.source.starts_with("https://") || cmd.source.starts_with("http://") {
        if index.debuginfod.iter().any(|server| server.url == cmd.source) {
            return Ok(());
        }

        index.debuginfod.push(DebugInfoD { url: cmd.source, require_authentication: false });
    } else {
        let path = resolve_path_from_cwd(&cmd.source)?;
        let build_dir = cmd.build_dir.map(|p| resolve_path_from_cwd(&p).ok()).flatten();

        if path.ends_with(".json") {
            if index.includes.contains(&path) {
                return Ok(());
            }
            if build_dir.is_some() {
                ffx_bail!(
                    "--build-dir cannot be specified for json files. Remove `--build-dir` when specifying a JSON symbol index."
                );
            }
            index.includes.push(path);
        } else if path.ends_with("ids.txt") {
            if index.ids_txts.iter().any(|ids_txt| ids_txt.path == path) {
                return Ok(());
            }
            index.ids_txts.push(IdsTxt { path, build_dir });
        } else if Path::new(&path).is_dir() {
            if index.build_id_dirs.iter().any(|build_id_dir| build_id_dir.path == path) {
                return Ok(());
            }
            index.build_id_dirs.push(BuildIdDir { path, build_dir });
        } else {
            ffx_bail!("Unsupported format: {}", path);
        }
    }

    index.save(global_symbol_index_path)?;
    Ok(())
}

fn remove(cmd: RemoveCommand, global_symbol_index_path: &str) -> Result<()> {
    let mut index = SymbolIndex::load(global_symbol_index_path)?;
    if cmd.source.starts_with("https://") || cmd.source.starts_with("http://") {
        index.debuginfod.retain(|server| server.url != cmd.source);
    } else {
        let path = resolve_path_from_cwd(&cmd.source)?;
        index.includes.retain(|include| include != &path);
        index.ids_txts.retain(|ids_txt| ids_txt.path != path);
        index.build_id_dirs.retain(|build_id_dir| build_id_dir.path != path);
    }
    index.save(global_symbol_index_path)?;
    Ok(())
}

fn clean(_cmd: CleanCommand, global_symbol_index_path: &str) -> Result<()> {
    let mut index = SymbolIndex::load(global_symbol_index_path)?;
    index.includes.retain(|include| Path::new(include).exists());
    index.ids_txts.retain(|ids_txt| Path::new(&ids_txt.path).exists());
    index.build_id_dirs.retain(|build_id_dir| Path::new(&build_id_dir.path).exists());
    index.save(global_symbol_index_path)?;
    Ok(())
}

/// Resovle a relative from current_dir. Do nothing if |relative| is actually absolute.
fn resolve_path_from_cwd(relative: &str) -> Result<String> {
    if Path::new(relative).is_absolute() {
        Ok(relative.to_owned())
    } else {
        Ok(resolve_path(&std::env::current_dir()?, relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};
    use std::fs::*;
    use tempfile::TempDir;

    const LIST_RESULT_MAIN_PATH: &'static str =
        "../../src/developer/ffx/lib/symbol-index/test_data/main.json";

    #[fuchsia::test]
    async fn test_list_no_aggregate() {
        let cmd = SymbolIndexCommand {
            sub_command: SymbolIndexSubCommand::List(ListCommand { aggregated: false }),
        };
        let path = SymbolIndexPath { inner: LIST_RESULT_MAIN_PATH.to_owned() };
        let tool = SymbolIndexTool { cmd, path };
        let machine_buffers = TestBuffers::default();
        let machine_writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &machine_buffers);
        tool.main(machine_writer).await.expect("command success");
        let (stdout, _stderr) = machine_buffers.into_strings();
        let data: CommandStatus = serde_json::from_str(&stdout).unwrap();
        let list_result = data.index().expect("data index");
        let output = serde_json::to_string_pretty(&list_result).unwrap();
        let expected_idx = SymbolIndex::load(LIST_RESULT_MAIN_PATH).unwrap();
        let expected_out = serde_json::to_string_pretty(&expected_idx).unwrap();
        assert_eq!(expected_out, output);
    }

    #[fuchsia::test]
    async fn test_list_aggregate() {
        let cmd = SymbolIndexCommand {
            sub_command: SymbolIndexSubCommand::List(ListCommand { aggregated: true }),
        };
        let path = SymbolIndexPath { inner: LIST_RESULT_MAIN_PATH.to_owned() };
        let tool = SymbolIndexTool { cmd, path };
        let machine_buffers = TestBuffers::default();
        let machine_writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &machine_buffers);
        tool.main(machine_writer).await.expect("command success");
        let (stdout, _stderr) = machine_buffers.into_strings();
        let data: CommandStatus = serde_json::from_str(&stdout).unwrap();
        let list_result = data.index().expect("data index");
        let output = serde_json::to_string_pretty(&list_result).unwrap();
        let expected_idx = SymbolIndex::load_aggregate(LIST_RESULT_MAIN_PATH).unwrap();
        let expected_out = serde_json::to_string_pretty(&expected_idx).unwrap();
        assert_eq!(expected_out, output);
    }

    #[test]
    fn test_add_remove_clean() {
        let tempdir = TempDir::new().unwrap();
        let tempdir_path = tempdir.path().to_str().unwrap();
        let build_id_dir = tempdir_path.to_owned() + "/.build-id";
        let ids_txt = tempdir_path.to_owned() + "/ids.txt";
        let package_json = tempdir_path.to_owned() + "/package.symbol-index.json";
        let nonexistent = tempdir_path.to_owned() + "/nonexistent.json";
        let build_dir = Some(tempdir_path.to_owned());
        let index_path = tempdir_path.to_owned() + "/symbol-index.json";

        create_dir(&build_id_dir).unwrap();
        File::create(&package_json).unwrap();
        File::create(&ids_txt).unwrap();

        // Test add.
        add(AddCommand { build_dir: None, source: ids_txt.clone() }, &index_path).unwrap();
        add(AddCommand { build_dir: build_dir.clone(), source: build_id_dir.clone() }, &index_path)
            .unwrap();
        // Duplicated adding should be a noop
        add(AddCommand { build_dir: None, source: build_id_dir.clone() }, &index_path).unwrap();
        // build_dir cannot be supplied for json files
        assert!(
            add(AddCommand { build_dir: build_dir, source: package_json.clone() }, &index_path)
                .is_err()
        );
        add(AddCommand { build_dir: None, source: package_json.clone() }, &index_path).unwrap();
        // Duplicated adding should be a noop.
        add(AddCommand { build_dir: None, source: package_json }, &index_path).unwrap();
        // Adding a non-existent item is not an error.
        add(AddCommand { build_dir: None, source: nonexistent }, &index_path).unwrap();
        // Adding a relative path.
        add(AddCommand { build_dir: None, source: ".".to_owned() }, &index_path).unwrap();

        add(
            AddCommand { build_dir: None, source: "https://debuginfod.debian.net".to_owned() },
            &index_path,
        )
        .unwrap();

        let symbol_index = SymbolIndex::load(&index_path).unwrap();
        assert_eq!(symbol_index.ids_txts.len(), 1);
        assert_eq!(symbol_index.build_id_dirs.len(), 2);
        assert!(symbol_index.build_id_dirs[0].build_dir.is_some());
        assert_eq!(symbol_index.includes.len(), 2);
        assert_eq!(symbol_index.gcs_flat.len(), 0);
        assert_eq!(symbol_index.debuginfod.len(), 1);

        // Test remove.
        assert!(remove(RemoveCommand { source: ids_txt }, &index_path).is_ok());
        // Removing a relative path.
        assert!(remove(RemoveCommand { source: ".".to_owned() }, &index_path).is_ok());
        // Removing a URL.
        assert!(
            remove(
                RemoveCommand { source: "https://debuginfod.debian.net".to_owned() },
                &index_path
            )
            .is_ok()
        );
        let symbol_index = SymbolIndex::load(&index_path).unwrap();
        assert_eq!(symbol_index.ids_txts.len(), 0);
        assert_eq!(symbol_index.build_id_dirs.len(), 1);
        assert_eq!(symbol_index.debuginfod.len(), 0);

        // Test clean.
        remove_dir(build_id_dir).unwrap();
        assert!(clean(CleanCommand {}, &index_path).is_ok());
        let symbol_index = SymbolIndex::load(&index_path).unwrap();
        assert_eq!(symbol_index.build_id_dirs.len(), 0);
        assert_eq!(symbol_index.debuginfod.len(), 0);
        assert_eq!(symbol_index.includes.len(), 1);
    }
}

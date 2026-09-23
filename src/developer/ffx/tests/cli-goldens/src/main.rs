// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;
use ffx_command::CliArgsInfo;
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

#[derive(FromArgs)]
/// CLI tool for generating golden JSON files for
/// ffx commands.
struct Args {
    #[argh(switch)]
    /// only generate the metadata, do not write golden files.
    pub describe_only: bool,
    #[argh(switch)]
    /// write out command list file and exit.
    pub commandlist_only: bool,
    #[argh(option)]
    /// the output directory
    pub out_dir: Option<PathBuf>,
    #[argh(option)]
    /// base dir of the golden files in source. This is used to
    /// create dependencies for the goldens
    pub base_golden_src_dir: Option<String>,
    #[argh(option)]
    /// generate the comparisons file for the goldens
    pub gen_comparisons: Option<PathBuf>,
    #[argh(option)]
    /// path to ffx manifest
    pub ffx_path: PathBuf,
    #[argh(option)]
    /// path to ffx-tools manifest
    pub tool_list: PathBuf,
    #[argh(option)]
    /// path to write out the list of golden files.
    pub golden_file_list: Option<PathBuf>,
    #[argh(option)]
    /// path to write out the list of top level commands.
    pub command_list: Option<PathBuf>,
    #[argh(option)]
    /// top level command to use to filter output
    pub filter_command: Option<String>,
    #[argh(option)]
    /// GN depfile recording dependencies of the input.
    pub depfile: Option<PathBuf>,
}

#[derive(Eq, PartialEq, Serialize, Ord, PartialOrd)]
struct Comparison {
    pub candidate: String,
    pub golden: String,
}

// Note: setting logging to "info" will print superfluous information to stdout.
#[fuchsia::main(logging_minimum_severity = "warn")]
async fn main() -> Result<()> {
    let args = argh::from_env::<Args>();

    // Run ffx and get the JSON encoded help data.
    let mut cmd = Command::new(args.ffx_path);
    cmd.args([
        "--no-environment",
        "--config",
        &format!("ffx.subtool-manifest={}", args.tool_list.to_string_lossy()),
        "--machine",
        "json-pretty",
        "--help",
    ]);

    let out = cmd.output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    let value: CliArgsInfo = serde_json::from_str(&stdout)
        .or_else(|e| -> Result<CliArgsInfo> {
            panic!("{e}\n<start>{}\n<end>\n<starterr>\n{}\n<enderr>\n", stdout, stderr)
        })
        .expect("json");

    // Parse the ffx_tools.json file which lists the external subtools.
    // This is used to filter which commands are listed, and to
    // complete the depfile contents, which is deps of the inputs. In
    // this case, it is the contents of the ffx_tools.json file.
    let data = serde_json::from_str::<Value>(&fs::read_to_string(&args.tool_list)?)?;
    let tools: Vec<&Map<String, Value>> =
        data.as_array().unwrap().iter().filter_map(|t| t.as_object().clone()).collect();

    if let Some(depfile) = args.depfile {
        write_depfile(depfile, args.tool_list.display().to_string(), &tools)?;
    }

    if args.commandlist_only {
        // Get the internal subtools, and make sure
        // not of the tools listed are internal.
        let tool_list: Vec<_> = tools
            .iter()
            .filter(|t| t["category"].as_str() == Some("internal"))
            .filter_map(|t| t["name"].as_str())
            .collect();
        let mut out = fs::File::create(args.command_list.expect("command list path"))?;
        let mut commands: Vec<String> = value
            .commands
            .iter()
            .filter(|c| tool_list.iter().all(|t| t != &format!("ffx-{}", c.name)))
            .map(|c| c.name.clone())
            .collect();
        commands.sort();
        writeln!(&mut out, "{}", commands.join("\n"))?;
        return Ok(());
    }

    let cmd_name = "ffx";

    if let Some(golden_root) = args.out_dir {
        let golden_files = generate_goldens(
            &value,
            cmd_name,
            &golden_root,
            args.filter_command,
            !args.describe_only,
        )?;

        if let Some(golden_src_dir) = &args.base_golden_src_dir {
            let mut comparisons: Vec<Comparison> = golden_files
                .iter()
                .map(|p| {
                    let golden = PathBuf::from(golden_src_dir)
                        .join(p.strip_prefix(&golden_root).expect("relative golden path"));
                    Comparison {
                        golden: golden.to_string_lossy().into(),
                        candidate: p.to_string_lossy().into(),
                    }
                })
                .collect();

            comparisons.sort();

            if let Some(comparision_file) = &args.gen_comparisons {
                fs::write(comparision_file, serde_json::to_string_pretty(&comparisons)?)?;
            }
            if let Some(golden_file_list) = &args.golden_file_list {
                let mut out = fs::File::create(golden_file_list)?;
                let prefix = format!("{golden_src_dir}/");
                for c in comparisons {
                    let f = c.golden.strip_prefix(&prefix).unwrap_or(&c.golden);
                    writeln!(&mut out, "{f}")?;
                }
            }
        }
    }

    Ok(())
}

/// Writes a GN dep file for the manifest and its dependencies.
fn write_depfile(
    depfile: PathBuf,
    manifest_filename: String,
    subtools: &Vec<&Map<String, Value>>,
) -> Result<()> {
    let mut f = File::create(depfile)?;

    for tool in subtools {
        writeln!(
            f,
            "{manifest_filename} : {} {}",
            tool["executable"].as_str().unwrap(),
            tool["executable_metadata"].as_str().unwrap()
        )?;
    }

    Ok(())
}

fn generate_goldens(
    value: &CliArgsInfo,
    cmd: &str,
    out_dir: &PathBuf,
    filter_command: Option<String>,
    save_goldens: bool,
) -> Result<Vec<PathBuf>> {
    let mut files_written: Vec<PathBuf> = vec![];
    let file_name = out_dir.join(format!("{cmd}.golden"));

    // write out everything except the sub commands, which are broken out into separate files.
    let golden_data = CliArgsInfo {
        name: value.name.clone(),
        description: value.description.clone(),
        examples: value.examples.clone(),
        flags: value.flags.clone(),
        notes: value.notes.clone(),
        commands: vec![],
        positionals: value.positionals.clone(),
        error_codes: value.error_codes.clone(),
    };

    if filter_command.is_none() || Some(cmd) == filter_command.as_deref() {
        files_written.push(file_name.clone());
        if save_goldens {
            std::fs::create_dir_all(&out_dir)?;
            fs::write(&file_name, serde_json::to_string_pretty(&golden_data)?)?;
        }
    }

    if value.commands.len() > 0 {
        let subcmd_path = out_dir.join(cmd);
        if let Some(subcmd) = value.commands.iter().find(|c| Some(c.name.clone()) == filter_command)
        {
            if save_goldens {
                std::fs::create_dir_all(&subcmd_path)?;
            }
            files_written.extend(generate_goldens(
                &subcmd.command,
                &subcmd.name,
                &subcmd_path,
                None,
                save_goldens,
            )?);
        } else if filter_command.is_none() {
            if save_goldens {
                std::fs::create_dir_all(&subcmd_path)?;
            }

            // Now recurse on subcommands
            for subcmd in &value.commands {
                files_written.extend(generate_goldens(
                    &subcmd.command,
                    &subcmd.name,
                    &subcmd_path,
                    None,
                    save_goldens,
                )?);
            }
        }
    }

    Ok(files_written)
}

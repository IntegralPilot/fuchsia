// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{HEADER, escape_text, md_path};
use anyhow::{Context, Result, bail};
use ffx_command::{CliArgsInfo, ErrorCodeInfo, FlagInfo, SubCommandInfo};
use log::debug;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;

//LINT.IfChange(clidoc_subtool_manifest)
/// Subtool manifest entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SubToolManifestEntry {
    pub(crate) category: String,
    pub(crate) executable: PathBuf,
    pub(crate) executable_metadata: PathBuf,
    pub(crate) name: String,
}
// LINT.ThenChange(//src/developer/ffx/build/ffx_tool.gni:clidoc_subtool_manifest)

pub(crate) fn write_formatted_output_for_ffx(
    cmd_path: &PathBuf,
    output_path: &PathBuf,
    isolate_dir_path: &Option<PathBuf>,
    subtool_manifest_path: &Option<PathBuf>,
) -> Result<(String, String, PathBuf)> {
    // Get name of command from full path to the command executable.
    let cmd_name = cmd_path.file_name().expect("Could not get file name for command");
    let output_md_path = md_path(&cmd_name, &output_path);
    debug!("Generating docs for {:?} to {:?}", cmd_path, output_md_path);

    let mut cmd = Command::new(&cmd_path);
    cmd.args(["--no-environment"]);

    if let Some(subtool_manifest) = subtool_manifest_path {
        cmd.args([
            "--config",
            &format!("ffx.subtool-manifest={}", subtool_manifest.to_string_lossy()),
        ]);
    }
    if let Some(isolate_dir) = isolate_dir_path {
        cmd.args(["--isolate-dir", &isolate_dir.to_string_lossy()]);
    }
    let output = cmd
        .args(["--machine", "json-pretty", "--help"])
        .output()
        .context(format!("Command failed for {cmd_path:?}"))
        .expect("get output");

    if !output.status.success() {
        bail!("{cmd_path:?} failed {}: {}", output.status, String::from_utf8(output.stderr)?);
    } else if !output.stderr.is_empty() {
        log::info!("stderr for ffx is {}", String::from_utf8(output.stderr.clone())?);
    }

    let value: CliArgsInfo = serde_json::from_slice(&output.stdout)
        .or_else(|e| -> Result<CliArgsInfo> {
            panic!(
                "{e}\n<start>{}\n<end>\n<starterr>\n{}\n<enderr>\n",
                String::from_utf8(output.stdout)?,
                String::from_utf8(output.stderr)?
            )
        })
        .expect("json");

    // Create a buffer writer to format and write consecutive lines to a file.
    let file = File::create(&output_md_path).context(format!("create {:?}", output_md_path))?;
    let output_writer = &mut BufWriter::new(file);

    writeln!(output_writer, "{}", HEADER)?;
    write_command(output_writer, 1, "", &value)?;
    Ok((value.name.into(), value.description.into(), output_md_path))
}

fn write_command(
    output_writer: &mut BufWriter<File>,
    level: usize,
    parent_cmd: &str,
    command: &CliArgsInfo,
) -> Result<()> {
    let code_block_start =
        r#"```none {: style="white-space: break-spaces;" .devsite-disable-click-to-copy}"#;
    let mut sorted_commands = command.commands.clone();
    sorted_commands.sort_by(|a, b| a.name.cmp(&b.name));
    let heading_level = "#".repeat(level);
    let anchor = build_anchor(parent_cmd, &command.name.to_lowercase());
    writeln!(output_writer, "{heading_level} {} {{#{anchor}}}\n", command.name.to_lowercase())?;
    writeln!(output_writer, "{}\n", escape_text(&command.description))?;
    writeln!(output_writer, "{code_block_start}\n")?;
    writeln!(output_writer, "Usage: {}\n", build_usage_string(parent_cmd, &command)?)?;
    writeln!(output_writer, "```\n")?;

    write_flags(output_writer, &command.flags)?;
    write_subcommand_list(
        output_writer,
        &format!("{parent_cmd} {}", command.name.to_lowercase()),
        &sorted_commands,
    )?;
    write_examples(output_writer, &command.examples)?;
    write_notes(output_writer, &command.notes)?;
    write_errors(output_writer, &command.error_codes)?;

    for cmd in &sorted_commands {
        write_command(
            output_writer,
            level + 1,
            &format!("{parent_cmd} {}", command.name.to_lowercase()),
            &cmd.command,
        )?;
    }
    Ok(())
}

fn build_usage_string(parent_cmd: &str, value: &CliArgsInfo) -> Result<String> {
    let mut buf = Vec::<u8>::new();
    if !parent_cmd.is_empty() {
        write!(buf, "{parent_cmd} ")?;
    }
    write!(buf, "{}", value.name.to_lowercase())?;
    for flag in &value.flags {
        if flag.hidden || flag.long == "--help" {
            continue;
        }
        let flag_name = if let Some(short_flag) = flag.short {
            format!("-{short_flag}")
        } else {
            format!("{}", flag.long)
        };
        match &flag.kind {
            ffx_command::FlagKind::Option { arg_name } => match flag.optionality {
                ffx_command::Optionality::Greedy | ffx_command::Optionality::Required => {
                    write!(buf, " {flag_name} <{arg_name}>")?
                }
                ffx_command::Optionality::Optional => write!(buf, " [{flag_name} <{arg_name}>]")?,
                ffx_command::Optionality::Repeating => {
                    write!(buf, " [{flag_name} <{arg_name}...>]")?
                }
            },
            ffx_command::FlagKind::Switch => match flag.optionality {
                ffx_command::Optionality::Greedy | ffx_command::Optionality::Required => {
                    write!(buf, " {flag_name}")?
                }
                ffx_command::Optionality::Optional => write!(buf, " [{flag_name}]")?,
                ffx_command::Optionality::Repeating => write!(buf, " [{flag_name}...]")?,
            },
        }
    }
    if value.commands.is_empty() {
        for pos in &value.positionals {
            if pos.hidden {
                continue;
            }
            match pos.optionality {
                ffx_command::Optionality::Greedy | ffx_command::Optionality::Required => {
                    write!(buf, " {}", pos.name)?
                }
                ffx_command::Optionality::Optional => write!(buf, " [{}]", pos.name)?,
                ffx_command::Optionality::Repeating => write!(buf, " [{}...]", pos.name)?,
            }
        }
    } else {
        write!(buf, " [subcommand...]")?;
    }

    Ok(String::from_utf8(buf)?)
}

fn write_flags<W: Write>(output_writer: &mut BufWriter<W>, flags: &Vec<FlagInfo>) -> Result<()> {
    if flags.is_empty() {
        writeln!(output_writer, "\n")?;
        return Ok(());
    }
    writeln!(output_writer, "__Options__ | &nbsp;")?;
    writeln!(output_writer, "----------- | ------")?;

    for flag in flags {
        if flag.hidden {
            continue;
        }
        if let Some(s) = flag.short {
            writeln!(
                output_writer,
                "| <nobr>-{s}, {}</nobr> | {}",
                flag.long,
                escape_text(&flag.description)
            )?;
        } else {
            writeln!(
                output_writer,
                "| <nobr>{}</nobr> | {}",
                flag.long,
                escape_text(&flag.description)
            )?;
        }
    }
    writeln!(output_writer, "\n")?;
    Ok(())
}

fn write_examples(output_writer: &mut BufWriter<File>, examples: &Vec<String>) -> Result<()> {
    if examples.is_empty() {
        writeln!(output_writer, "\n")?;
        return Ok(());
    }
    writeln!(output_writer, "__Examples__\n")?;
    for ex in examples {
        writeln!(output_writer, "\n<pre>{}</pre>\n", escape_text(ex))?;
    }
    writeln!(output_writer, "\n")?;
    Ok(())
}

fn write_notes(output_writer: &mut BufWriter<File>, notes: &Vec<String>) -> Result<()> {
    if notes.is_empty() {
        writeln!(output_writer, "\n")?;
        return Ok(());
    }
    writeln!(output_writer, "__Notes__\n")?;
    for note in notes {
        writeln!(output_writer, "* <pre>{}</pre>", escape_text(note))?;
    }
    writeln!(output_writer, "\n")?;
    Ok(())
}

fn write_errors(output_writer: &mut BufWriter<File>, errors: &Vec<ErrorCodeInfo>) -> Result<()> {
    if errors.is_empty() {
        writeln!(output_writer, "\n")?;
        return Ok(());
    }
    writeln!(output_writer, "__Errors__ | &nbsp;")?;
    writeln!(output_writer, "----------- | ------")?;

    for e in errors {
        writeln!(output_writer, "| {} | {}", e.code, escape_text(&e.description))?;
    }
    writeln!(output_writer, "\n")?;
    Ok(())
}

fn write_subcommand_list<W: Write>(
    output_writer: &mut BufWriter<W>,
    parent_cmd: &str,
    commands: &Vec<SubCommandInfo>,
) -> Result<()> {
    if commands.is_empty() {
        writeln!(output_writer, "\n")?;
        return Ok(());
    }
    writeln!(output_writer, "__Subcommands__ | &nbsp;")?;
    writeln!(output_writer, "----------- | ------")?;

    for cmd in commands {
        let anchor = build_anchor(parent_cmd, &cmd.name);
        let first_line = cmd.command.description.lines().next().unwrap_or(&cmd.command.description);
        writeln!(output_writer, "| [{}](#{anchor}) | {}", cmd.name, escape_text(first_line))?;
    }
    writeln!(output_writer, "\n")?;
    Ok(())
}

fn build_anchor(parent_cmd: &str, cmd: &str) -> String {
    let suffix = cmd.trim();
    let prefix = parent_cmd.trim();
    if prefix.is_empty() {
        suffix.replace(" ", "_")
    } else {
        format!("{prefix}_{cmd}").replace(" ", "_")
    }
}

pub(crate) fn list_subtool_files(subtool_manifest: &PathBuf) -> Vec<PathBuf> {
    let file = fs::File::open(subtool_manifest).expect("file should open");
    let entries: Vec<SubToolManifestEntry> =
        serde_json::from_reader(file).expect("manifest should be json");

    entries
        .iter()
        .map(|item| vec![item.executable.clone(), item.executable_metadata.clone()])
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_command::{Optionality, PositionalInfo};

    #[test]
    fn test_write_subcommand_list() {
        let buf = Vec::<u8>::new();
        let mut writer = BufWriter::new(buf);

        let parent_cmd = "ffx something";
        let commands = vec![
            SubCommandInfo { name: "one".into(), command: CliArgsInfo::default() },
            SubCommandInfo { name: "two".into(), command: CliArgsInfo::default() },
        ];

        write_subcommand_list(&mut writer, parent_cmd, &commands).expect("subcommands written");
        writer.flush().expect("flush buffer");
        let actual = String::from_utf8(writer.get_ref().to_vec()).expect("string from utf8");

        let expected = "__Subcommands__ | &nbsp;\n----------- | ------\n| [one](#ffx_something_one) | \n| [two](#ffx_something_two) | \n\n\n";

        assert_eq!(actual, expected);
    }
    #[test]
    fn test_write_errors() {}

    #[test]
    fn test_write_notes() {}

    #[test]
    fn test_write_examples() {}

    #[test]
    fn test_write_flags() {
        let test_data = [
            (vec![], "\n\n"),
            (
                vec![
                    FlagInfo {
                        kind: ffx_command::FlagKind::Switch,
                        optionality: Optionality::Optional,
                        long: "--help".into(),
                        short: None,
                        description: "help".into(),
                        hidden: false,
                    },
                    FlagInfo {
                        kind: ffx_command::FlagKind::Switch,
                        optionality: Optionality::Optional,
                        long: "--hidden".into(),
                        short: None,
                        description: "hidden".into(),
                        hidden: true,
                    },
                ],
                r#"__Options__ | &nbsp;
----------- | ------
| <nobr>--help</nobr> | help


"#,
            ),
            (
                vec![
                    FlagInfo {
                        kind: ffx_command::FlagKind::Switch,
                        optionality: Optionality::Optional,
                        long: "--something".into(),
                        short: None,
                        description: "something".into(),
                        hidden: false,
                    },
                    FlagInfo {
                        kind: ffx_command::FlagKind::Switch,
                        optionality: Optionality::Repeating,
                        long: "--something-really-long".into(),
                        short: Some('s'),
                        description: "something".into(),
                        hidden: false,
                    },
                ],
                r#"__Options__ | &nbsp;
----------- | ------
| <nobr>--something</nobr> | something
| <nobr>-s, --something-really-long</nobr> | something


"#,
            ),
        ];

        for (flags, expected) in test_data {
            let buf = Vec::<u8>::new();
            let mut writer = BufWriter::new(buf);
            write_flags(&mut writer, &flags).expect("write_flags");
            writer.flush().expect("flush buffer");
            let actual = String::from_utf8(writer.get_ref().to_vec()).expect("string from utf8");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_build_usage_string_top() {
        let test_data = [
            (
                "",                     //parent_cmd
                CliArgsInfo::default(), //value
                String::from(""),       //expected
            ),
            (
                "", //parent_cmd
                CliArgsInfo {
                    name: "cmd1".into(),
                    description: "the cmd, 1".into(),
                    flags: vec![],
                    commands: vec![],
                    positionals: vec![],
                    ..Default::default()
                },
                String::from("cmd1"), //expected
            ),
            (
                "", //parent_cmd
                CliArgsInfo {
                    name: "cmd1".into(),
                    description: "the cmd, 1".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--help".into(),
                            short: None,
                            description: "help".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                    ],
                    commands: vec![],
                    positionals: vec![],
                    ..Default::default()
                },
                String::from("cmd1"), //expected
            ),
            (
                "parent", //parent_cmd
                CliArgsInfo {
                    name: "cmd2".into(),
                    description: "the cmd, 2".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--help".into(),
                            short: None,
                            description: "help".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--something".into(),
                            short: None,
                            description: "something".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Repeating,
                            long: "--something-really-long".into(),
                            short: Some('s'),
                            description: "something".into(),
                            hidden: false,
                        },
                    ],
                    commands: vec![],
                    positionals: vec![],
                    ..Default::default()
                },
                String::from("parent cmd2 [--something] [-s...]"), //expected
            ),
            (
                "parent", //parent_cmd
                CliArgsInfo {
                    name: "cmd3".into(),
                    description: "the cmd, 2".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Required,
                            long: "--path".into(),
                            short: Some('p'),
                            description: "path".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "the_arg".into() },
                            optionality: Optionality::Optional,
                            long: "--something".into(),
                            short: None,
                            description: "something".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "s_val".into() },
                            optionality: Optionality::Repeating,
                            long: "--something-really-long".into(),
                            short: Some('s'),
                            description: "something".into(),
                            hidden: false,
                        },
                    ],
                    commands: vec![],
                    positionals: vec![],
                    ..Default::default()
                },
                String::from("parent cmd3 -p [--something <the_arg>] [-s <s_val...>]"), //expected
            ),
            (
                "parent", //parent_cmd
                CliArgsInfo {
                    name: "cmd3".into(),
                    description: "the cmd, 2".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Required,
                            long: "--path".into(),
                            short: Some('p'),
                            description: "path".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "the_arg".into() },
                            optionality: Optionality::Optional,
                            long: "--something".into(),
                            short: None,
                            description: "something".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "s_val".into() },
                            optionality: Optionality::Repeating,
                            long: "--something-really-long".into(),
                            short: Some('s'),
                            description: "something".into(),
                            hidden: false,
                        },
                    ],
                    commands: vec![],
                    positionals: vec![PositionalInfo {
                        name: "pos1".into(),
                        description: "1".into(),
                        optionality: Optionality::Required,
                        hidden: false,
                    }],
                    ..Default::default()
                },
                String::from("parent cmd3 -p [--something <the_arg>] [-s <s_val...>] pos1"), //expected
            ),
            (
                "parent", //parent_cmd
                CliArgsInfo {
                    name: "cmd4".into(),
                    description: "the cmd, 4".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Required,
                            long: "--path".into(),
                            short: Some('p'),
                            description: "path".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "the_arg".into() },
                            optionality: Optionality::Optional,
                            long: "--something".into(),
                            short: None,
                            description: "something".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "s_val".into() },
                            optionality: Optionality::Repeating,
                            long: "--something-really-long".into(),
                            short: Some('s'),
                            description: "something".into(),
                            hidden: false,
                        },
                    ],
                    commands: vec![],
                    positionals: vec![
                        PositionalInfo {
                            name: "pos1".into(),
                            description: "1".into(),
                            optionality: Optionality::Required,
                            hidden: false,
                        },
                        PositionalInfo {
                            name: "other_pos".into(),
                            description: "others".into(),
                            optionality: Optionality::Repeating,
                            hidden: false,
                        },
                    ],
                    ..Default::default()
                },
                String::from(
                    "parent cmd4 -p [--something <the_arg>] [-s <s_val...>] pos1 [other_pos...]",
                ), //expected
            ),
            (
                "parent", //parent_cmd
                CliArgsInfo {
                    name: "cmd5".into(),
                    description: "the cmd, 5".into(),
                    flags: vec![
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Required,
                            long: "--path".into(),
                            short: Some('p'),
                            description: "path".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Switch,
                            optionality: Optionality::Optional,
                            long: "--hidden".into(),
                            short: None,
                            description: "hidden".into(),
                            hidden: true,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "the_arg".into() },
                            optionality: Optionality::Optional,
                            long: "--something".into(),
                            short: None,
                            description: "something".into(),
                            hidden: false,
                        },
                        FlagInfo {
                            kind: ffx_command::FlagKind::Option { arg_name: "s_val".into() },
                            optionality: Optionality::Repeating,
                            long: "--something-really-long".into(),
                            short: Some('s'),
                            description: "something".into(),
                            hidden: false,
                        },
                    ],
                    commands: vec![SubCommandInfo {
                        name: "another_cmd".into(),
                        command: CliArgsInfo { name: "another_cmd".into(), ..Default::default() },
                    }],
                    positionals: vec![
                        PositionalInfo {
                            name: "pos1".into(),
                            description: "1".into(),
                            optionality: Optionality::Required,
                            hidden: false,
                        },
                        PositionalInfo {
                            name: "other_pos".into(),
                            description: "others".into(),
                            optionality: Optionality::Repeating,
                            hidden: false,
                        },
                    ],
                    ..Default::default()
                },
                String::from(
                    "parent cmd5 -p [--something <the_arg>] [-s <s_val...>] [subcommand...]",
                ), //expected
            ),
        ];

        for (parent_cmd, value, expected) in test_data {
            let actual = build_usage_string(parent_cmd, &value).expect("usage string");
            assert_eq!(actual, expected);
        }
    }
}

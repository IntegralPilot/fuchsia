// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};

use anyhow::{Context, Result};
use argh::FromArgs;
use gtest_test_parser::{ChildGuard, TestResult, parse_str};

const TEST_OUTPUT_SUMMARY_PATH_ENV_KEY: &str = "TEST_OUTPUT_SUMMARY_PATH";

#[derive(FromArgs, Debug)]
/// Reads stdout from the test command (or from --file/stdin if no command given),
/// and writes a JSON formatted summary of parsed GTest cases.
struct Args {
    /// path to a log file to parse instead of running a subprocess command.
    #[argh(option)]
    file: Option<PathBuf>,

    /// path to write the output JSON summary to. Overrides TEST_OUTPUT_SUMMARY_PATH.
    #[argh(option)]
    output_summary: Option<PathBuf>,

    /// test command and arguments to execute.
    #[argh(positional, greedy)]
    command: Vec<String>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    if args.file.is_some() && !args.command.is_empty() {
        anyhow::bail!("Cannot specify both --file and a command to run");
    }

    let output_summary_path = args
        .output_summary
        .or_else(|| env::var_os(TEST_OUTPUT_SUMMARY_PATH_ENV_KEY).map(PathBuf::from));

    let mut exit_code = 0;
    let input_text: String = if let Some(file_path) = args.file {
        let bytes = fs::read(&file_path)
            .with_context(|| format!("error reading input file {}", file_path.display()))?;
        String::from_utf8_lossy(&bytes).into_owned()
    } else if args.command.is_empty() {
        let mut buffer = Vec::new();
        io::stdin().read_to_end(&mut buffer).context("error reading stdin")?;
        String::from_utf8_lossy(&buffer).into_owned()
    } else {
        let binary = &args.command[0];
        let command_arguments = &args.command[1..];

        let mut child = Command::new(binary)
            .args(command_arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("error running test binary {binary}"))?;

        let child_stdout = child.stdout.take().expect("child stdout was piped");
        let guard = ChildGuard::new(child);

        let mut reader = BufReader::new(child_stdout);
        let mut captured_lines = Vec::new();
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let mut line_buf = Vec::new();

        while reader.read_until(b'\n', &mut line_buf)? > 0 {
            out.write_all(&line_buf)?;
            out.flush()?;
            let line_str = String::from_utf8_lossy(&line_buf);
            captured_lines.push(line_str.trim_end_matches(&['\r', '\n'][..]).to_string());
            line_buf.clear();
        }

        let status = guard.wait().context("error waiting for test binary process")?;
        if !status.success() {
            exit_code = status.code().unwrap_or(1);
        }

        captured_lines.join("\n")
    };

    let cases = parse_str(&input_text);
    let result = TestResult { cases };

    if let Some(summary_path) = output_summary_path {
        if let Some(parent_dir) = summary_path.parent() {
            if !parent_dir.as_os_str().is_empty() {
                fs::create_dir_all(parent_dir).with_context(|| {
                    format!("error creating directory {}", parent_dir.display())
                })?;
            }
        }
        let file = fs::File::create(&summary_path)
            .with_context(|| format!("error creating summary file {}", summary_path.display()))?;
        let mut writer = io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &result)
            .context("error writing test summary JSON to file")?;
        writer.flush().context("error flushing test summary file")?;
    } else if args.command.is_empty() {
        // In --file or stdin mode, logs were not mirrored to stdout, so outputting
        // JSON summary to stdout is clean and expected. If a command was executed
        // without an output summary path, we intentionally skip emitting JSON to
        // avoid mixing test execution logs with JSON.
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        serde_json::to_writer_pretty(&mut handle, &result)
            .context("error writing test summary JSON to stdout")?;
        writeln!(handle)?;
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::fs::{create_dir_all, read_to_string, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{bail, Context as _, Result};
use argh::FromArgs;
use serde::{Deserialize, Serialize};

const OVERRIDE_ARGS: &str = "\
restat_rust = false
check_output_dir_leaks = false
";

trait CommandExt {
    fn success(&mut self) -> Result<()> {
        self.success_output().map(|_| ())
    }

    fn success_output(&mut self) -> Result<Output>;
}

impl CommandExt for Command {
    fn success_output(&mut self) -> Result<Output> {
        let output = self.output().context(format!("failed to run {self:?}"))?;
        if !output.status.success() {
            let stdout = std::str::from_utf8(&output.stdout)?;
            let stderr = std::str::from_utf8(&output.stderr)?;
            bail!("command failed: {self:?}\nstdout: {stdout}\nstderr: {stderr}");
        }
        Ok(output)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RustcBuildStep {
    env: HashMap<String, String>,
    args: Vec<String>,
}

impl RustcBuildStep {
    fn parse(input: &str) -> Result<Self> {
        let mut env = HashMap::new();
        let mut args = Vec::new();

        let mut pieces = input.split(' ').peekable();

        // Parse environment first
        while let Some(piece) = pieces.peek() {
            if let Some((key, value)) = piece.split_once('=') {
                env.insert(key.to_string(), value.to_string());
                pieces.next();
            } else {
                break;
            }
        }

        // Check that rustc command follows
        let Some(rustc_path) = pieces.next() else {
            bail!("unexpected end after environment variables");
        };
        if Path::new(rustc_path).file_name().context("expected path after environment variables")?
            != "rustc"
        {
            bail!("expected rustc path after environment variables");
        }

        // Everything else before an `&&` is an arg
        args.extend(
            pieces.filter(|p| !p.is_empty()).take_while(|p| *p != "&&").map(str::to_string),
        );

        Ok(Self { env, args })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BuildStep {
    Rustc(RustcBuildStep),
}

impl BuildStep {
    fn filter_parse(input: &str) -> Option<Result<Self>> {
        let input = input.trim();
        let (first, _) = input.split_once(' ')?;
        match first {
            "../../build/scripts/no_op.sh" => None,
            "touch" => None,
            _ => match RustcBuildStep::parse(input) {
                Ok(build_step) => Some(Ok(Self::Rustc(build_step))),
                Err(e) => Some(Err(e)),
            },
        }
    }
}

#[derive(Debug, FromArgs)]
/// Extract a reproduction for a failing Rust build from the Fuchsia tree
struct Args {
    /// the ninja target to extract Rust build steps for
    #[argh(positional)]
    target: String,

    /// the output path to write the Rust build steps to
    #[argh(option, short = 'o')]
    output: String,
}

const OUT_DIR: &str = "out/rust_extract";

fn write_gn_args() -> Result<()> {
    let gn_args = read_to_string("out/default/args.gn").context("failed to open GN args file")?;

    create_dir_all(OUT_DIR).context("failed to create new out directory")?;
    let new_args_gn_path = Path::new(OUT_DIR).join("args.gn");
    let mut new_gn_args = BufWriter::new(
        File::create(new_args_gn_path).context("failed to create new GN args file")?,
    );
    writeln!(new_gn_args, "{gn_args}\n# rust_extract configuration\n{OVERRIDE_ARGS}")?;

    Ok(())
}

fn main() -> Result<()> {
    let args = argh::from_env::<Args>();

    write_gn_args()?;

    Command::new("fx").args(["gn", "gen", OUT_DIR]).success()?;

    let ninja_output = Command::new("fx")
        .args(["ninja", "-t", "commands", &args.target])
        .current_dir(OUT_DIR)
        .success_output()?;
    let ninja_stdout = std::str::from_utf8(&ninja_output.stdout)?;

    let build_steps =
        ninja_stdout.lines().filter_map(BuildStep::filter_parse).collect::<Result<Vec<_>>>()?;

    let output =
        BufWriter::new(File::create(&args.output).context("failed to create output file")?);
    serde_json::to_writer(output, &build_steps)
        .context("failed to serialize build steps to JSON")?;

    Ok(())
}

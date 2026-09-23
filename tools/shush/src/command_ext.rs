// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Result};

pub trait CommandExt {
    /// Runs the command, returning `Err` if the spawning process fails or the
    /// exit status is not success.
    fn run(&mut self) -> Result<String>;

    /// Runs the command with the given input as stdin, returning `Err` if
    /// spawning the process fails or the exit status is not success.
    fn run_with(&mut self, input: &str) -> Result<String>;
}

fn check_output(command: &Command, output: Output) -> Result<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        bail!("failed to run command: {command:?}\nstdout:\n{stdout}\nstderr:{stderr}")
    }
    Ok(stdout.to_string())
}

impl CommandExt for Command {
    fn run(&mut self) -> Result<String> {
        let output = self.output()?;
        check_output(self, output)
    }

    fn run_with(&mut self, input: &str) -> Result<String> {
        let mut child =
            self.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes())?;
        drop(stdin);

        let output = child.wait_with_output()?;
        check_output(self, output)
    }
}

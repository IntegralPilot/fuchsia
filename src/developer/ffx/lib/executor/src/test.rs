// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CommandOutput, ExecutionError, FfxExecutor};

#[derive(thiserror::Error, Debug)]
pub enum TestingError {
    #[error("Unexpected exit code. Expected {2}, got {3}, stdout: ```{0}```, stderr: ```{1}```")]
    UnexpectedExitCode(String, String, i32, i32),
    #[error("Error executing command: {0:?}")]
    ExecutionError(ExecutionError),
    #[error("Error constructing command: {0:?}")]
    CommandConstruction(#[from] crate::CommandConstructionError),
    #[error("IO error {0:?}")]
    IoError(std::io::Error),
    #[error("Output did not match expected: '{0}'")]
    MatchingError(String),
    #[error("Error parsing command output: {0}")]
    ParsingError(ffx_writer::Error),
}

/// Struct defining a command line for executing as part of a test.
pub struct TestCommandLineInfo<'a> {
    /// args is the `ffx` command line arguments, not including `ffx`
    pub args: Vec<&'a str>,
    /// stdout_check and stderr_check are functions or closures to
    /// check the contents of stdout and stderr. This allows for
    /// flexibility of rigor.
    pub output_check: fn(CommandOutput) -> Result<(), TestingError>,
}

impl<'a> TestCommandLineInfo<'a> {
    pub fn new(
        args: Vec<&'a str>,
        output_check: fn(CommandOutput) -> Result<(), TestingError>,
    ) -> Self {
        TestCommandLineInfo { args, output_check }
    }

    pub async fn run_command_lines(
        executor: &dyn FfxExecutor<Error = crate::CommandConstructionError>,
        test_data: Vec<TestCommandLineInfo<'_>>,
    ) -> Result<(), TestingError> {
        for test in test_data {
            test.run_command_with_checks(executor).await?;
        }
        Ok(())
    }

    async fn run_command_with_checks(
        &self,
        executor: &dyn FfxExecutor<Error = crate::CommandConstructionError>,
    ) -> Result<String, TestingError> {
        let cmd = executor.make_ffx_cmd(&self.args)?;
        let output = executor.exec_ffx(cmd).await.map_err(|e| TestingError::ExecutionError(e))?;
        log::info!("Ran command {:?}, output: {}\n{}", self.args, output.stdout, output.stderr);
        (self.output_check)(output.clone())?;
        Ok(output.stdout)
    }
}

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Serialize;
use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use tokio::process::Command;

/// A log of command invocations used to record details regarding the execution of the
/// test and post-processors. This is potentially useful for debugging issues with the
/// test framework itself.
#[derive(Serialize, Debug, Default)]
pub struct CommandInvocationLog {
    /// The invocation of the test itself.
    pub test: CommandInvocation,

    /// The invocations of postprocessors by name.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub postprocessors: HashMap<String, CommandInvocation>,
}

impl CommandInvocationLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_test(
        &mut self,
        command: &Command,
        exit_status: ExitStatus,
        stdout: Option<PathBuf>,
        stderr: Option<PathBuf>,
    ) {
        self.test = CommandInvocation::new(command, exit_status, stdout, stderr);
    }

    pub fn record_postprocessor(
        &mut self,
        command: &Command,
        exit_status: ExitStatus,
        stdout: Option<PathBuf>,
        stderr: Option<PathBuf>,
    ) {
        self.postprocessors.insert(
            String::from(command.as_std().get_program().to_str().expect("program has UTF-8 name")),
            CommandInvocation::new(command, exit_status, stdout, stderr),
        );
    }
}

/// Describes a single command invocation.
#[derive(Serialize, Debug, Default)]
pub struct CommandInvocation {
    /// The path of the invoked program.
    pub program_path: PathBuf,

    /// The arguments passed to the program.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// The environment provided to the program.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub envs: HashMap<String, Option<String>>,

    /// The directory in which the command was invoked.
    pub current_dir: PathBuf,

    /// The exit status of the command invocation.
    pub exit_status: i32,

    /// The stdout artifact, if anything was written to stdout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_artifact: Option<PathBuf>,

    /// The stderr artifact, if anything was written to stderr.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_artifact: Option<PathBuf>,
}

impl CommandInvocation {
    pub fn new(
        command: &Command,
        exit_status: ExitStatus,
        stdout: Option<PathBuf>,
        stderr: Option<PathBuf>,
    ) -> Self {
        let cmd = command.as_std();
        Self {
            program_path: PathBuf::from(cmd.get_program()),
            args: cmd
                .get_args()
                .map(|arg| String::from(arg.to_str().expect("args are UTF-8")))
                .collect(),
            envs: cmd
                .get_envs()
                .map(|(k, v)| {
                    (
                        String::from(k.to_str().expect("env variables have UTF-8 names")),
                        v.map(|val| {
                            String::from(val.to_str().expect("env variables have UTF-8 values"))
                        }),
                    )
                })
                .collect(),
            current_dir: cmd.get_current_dir().expect("command has current dir").to_path_buf(),
            exit_status: exit_status.into_raw(),
            stdout_artifact: stdout,
            stderr_artifact: stderr,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[fuchsia::test]
    fn test_execution_log() {
        let mut under_test = CommandInvocationLog::new();

        let mut command = Command::new("/etc/test_binary");
        command.arg("test_arg1").arg("test_arg2");
        command.env("test_env1", "test_env1_value").env("test_env2", "test_env2_value");
        command.current_dir("/tmp");

        under_test.record_test(
            &command,
            ExitStatus::from_raw(123),
            Some(PathBuf::from("test_stdout")),
            Some(PathBuf::from("test_stderr")),
        );

        let mut command = Command::new("/etc/postprocessor1_binary");
        command.arg("pp1_arg1");
        command.env("pp1_env1", "pp1_env1_value");
        command.current_dir("/etc");

        under_test.record_postprocessor(&command, ExitStatus::from_raw(124), None, None);

        let value = serde_json::to_value(&under_test)
            .expect("invocation log conversion to value should succeed");
        assert_eq!(
            value,
            json!({
                "test": {
                    "program_path": "/etc/test_binary",
                    "args": ["test_arg1", "test_arg2"],
                    "envs": {
                        "test_env1": "test_env1_value",
                        "test_env2": "test_env2_value"
                    },
                    "current_dir": "/tmp",
                    "exit_status": 123,
                    "stdout_artifact": "test_stdout",
                    "stderr_artifact": "test_stderr"
                },
                "postprocessors": {
                    "/etc/postprocessor1_binary": {
                        "program_path": "/etc/postprocessor1_binary",
                        "args": ["pp1_arg1"],
                        "envs": {
                            "pp1_env1": "pp1_env1_value"
                        },
                        "current_dir": "/etc",
                        "exit_status": 124,
                    }
                }
            })
        );
    }
}

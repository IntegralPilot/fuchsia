// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Tool, ToolCommand, ToolCommandLog, ToolProvider};

use anyhow::{Context, Result, anyhow};
use camino::Utf8PathBuf;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use utf8_path::PathToStringExt;

/// A provider for tools from a platform artifacts directory.
#[derive(Clone)]
pub struct PlatformToolProvider {
    tools_dir: Utf8PathBuf,
    log: ToolCommandLog,
}

impl PlatformToolProvider {
    /// Construct a new PlatformToolProvider.
    pub fn new(platform_artifacts_dir: Utf8PathBuf) -> Self {
        Self { tools_dir: platform_artifacts_dir.join("tools"), log: ToolCommandLog::default() }
    }
}

impl ToolProvider for PlatformToolProvider {
    fn get_tool(&self, name: &str) -> Result<Box<dyn Tool>> {
        let path = self.get_tool_path(name)?;
        self.get_tool_with_path(path)
    }

    fn get_tool_with_path(&self, path: PathBuf) -> Result<Box<dyn Tool>> {
        let tool = PlatformTool::new(path, self.log.clone());
        Ok(Box::new(tool))
    }

    fn log(&self) -> &ToolCommandLog {
        &self.log
    }

    fn get_tool_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.tools_dir.join(name).as_std_path().to_path_buf())
    }
}

#[derive(Clone, Debug)]
struct PlatformTool {
    path: PathBuf,
    log: ToolCommandLog,
}

impl PlatformTool {
    pub fn new(path: PathBuf, log: ToolCommandLog) -> Self {
        Self { path, log }
    }
}

impl Tool for PlatformTool {
    fn run(&self, args: &[String]) -> Result<()> {
        let path = self.path.path_to_string()?;
        self.log.add(ToolCommand::new(path.clone(), args.into()));
        let output = Command::new(&self.path)
            .args(args)
            .output()
            .context(format!("Failed to run the tool: {}", path))?;
        if output.status.success() {
            // Output stderr and stdout from the tool on success cases as well as failures.

            if !output.stderr.is_empty() {
                std::io::stderr()
                    .write_all(&output.stderr)
                    .context("writing tool stderr on success.")?;
            }
            if !output.stdout.is_empty() {
                std::io::stdout()
                    .write_all(&output.stdout)
                    .context("writing tool stdout on success.")?;
            }
        } else {
            let command = format!("{} {}", path, args.join(" "));
            return Err(anyhow!("{} exited with status: {}", path, output.status)
                .context(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)))
                .context(format!("stdout: {}", String::from_utf8_lossy(&output.stdout)))
                .context(command));
        }
        Ok(())
    }
}

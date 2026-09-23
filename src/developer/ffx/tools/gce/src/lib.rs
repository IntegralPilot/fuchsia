// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fho::subtool_suite::{FfxSubtoolSuite, Subtool, SubtoolBox, SubtoolSuite, ToolSuiteCommand};
use fho::{FfxTool, FhoEnvironment, Result};

mod args;
mod subtools;

pub use args::{GceCommand, GceSubCommand, ListCommand, SerialCommand, ShowCommand, StopCommand};
pub use subtools::{ListTool, SerialTool, ShowTool, StopTool};

impl ToolSuiteCommand for GceCommand {
    type SubCommand = GceSubCommand;
    fn into_subcommand(self) -> Self::SubCommand {
        self.subcommand
    }
}

pub struct GceSuite;

#[async_trait::async_trait(?Send)]
impl SubtoolSuite for GceSuite {
    type Command = GceCommand;

    async fn new_subtool(
        env: FhoEnvironment,
        subcommand: GceSubCommand,
    ) -> Result<Box<dyn SubtoolBox>> {
        Ok(match subcommand {
            GceSubCommand::List(cmd) => Subtool::new(ListTool::from_env(env, cmd).await?),
            GceSubCommand::Show(cmd) => Subtool::new(ShowTool::from_env(env, cmd).await?),
            GceSubCommand::Serial(cmd) => Subtool::new(SerialTool::from_env(env, cmd).await?),
            GceSubCommand::Stop(cmd) => Subtool::new(StopTool::from_env(env, cmd).await?),
        })
    }
}

pub type GceSuiteTool = FfxSubtoolSuite<GceSuite>;

#[cfg(test)]
mod tests {
    use super::*;
    use argh::FromArgs;

    #[test]
    fn test_parse_list_command() {
        let cmd = GceCommand::from_args(&["gce"], &["list", "--zone", "us-east1-c"])
            .expect("parsed list");
        match cmd.subcommand {
            GceSubCommand::List(l) => {
                assert_eq!(l.zone.as_deref(), Some("us-east1-c"));
            }
            _ => panic!("expected List subcommand"),
        }
    }

    #[test]
    fn test_parse_show_command() {
        let cmd = GceCommand::from_args(&["gce"], &["show", "test-vm"]).expect("parsed show");
        match cmd.subcommand {
            GceSubCommand::Show(s) => {
                assert_eq!(s.name, "test-vm");
            }
            _ => panic!("expected Show subcommand"),
        }
    }

    #[test]
    fn test_parse_serial_command() {
        let cmd =
            GceCommand::from_args(&["gce"], &["serial", "test-vm", "--follow", "--port", "1"])
                .expect("parsed serial");
        match cmd.subcommand {
            GceSubCommand::Serial(s) => {
                assert_eq!(s.name, "test-vm");
                assert!(s.follow);
                assert_eq!(s.port, 1);
            }
            _ => panic!("expected Serial subcommand"),
        }
    }

    #[test]
    fn test_parse_stop_command() {
        let cmd =
            GceCommand::from_args(&["gce"], &["stop", "my-vm", "--keep"]).expect("parsed stop");
        match cmd.subcommand {
            GceSubCommand::Stop(s) => {
                assert_eq!(s.name, "my-vm");
                assert!(s.keep);
            }
            _ => panic!("expected Stop subcommand"),
        }
    }

    #[test]
    fn test_parse_stop_command_requires_name() {
        assert!(GceCommand::from_args(&["gce"], &["stop"]).is_err());
    }
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use emulator_instance::EmulatorInstances;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_config::keys::EMU_INSTANCE_ROOT_DIR;
use ffx_emulator_config::EngineConsoleType;
use ffx_emulator_console_args::ConsoleCommand;
use ffx_emulator_engines::EngineBuilder;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, bug};
/// Sub-sub tool for `emu console`
#[derive(FfxTool)]
pub struct EmuConsoleTool {
    #[command]
    cmd: ConsoleCommand,
    context: EnvironmentContext,
}

// Since this is a part of a legacy plugin, add
// the legacy entry points. If and when this
// is migrated to a subcommand, this macro can be
// removed.
fho::embedded_plugin!(EmuConsoleTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for EmuConsoleTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        if writer.is_machine() {
            return Err(fho::user_error!("Interactive console not supported in machine mode"));
        }
        let instance_dir: PathBuf =
            self.context.get(EMU_INSTANCE_ROOT_DIR).map_err(|e| bug!("{e}"))?;
        let builder = EngineBuilder::new(&self.context, EmulatorInstances::new(instance_dir));

        let console = match get_console_type(&self.cmd) {
            Ok(c) => c,
            Err(e) => ffx_bail!("{:?}", e),
        };
        let mut name: Option<String> = self.cmd.name.clone();
        match builder.get_engine_by_name(&mut name) {
            Ok(Some(engine)) => engine.attach(console),
            Ok(None) => {
                if let Some(name) = self.cmd.name {
                    println!("Instance {name} not found.");
                } else {
                    println!("No instances found");
                }

                Ok(())
            }
            Err(e) => ffx_bail!("{:?}", e),
        }
    }
}

fn get_console_type(cmd: &ConsoleCommand) -> Result<EngineConsoleType> {
    let mut result = cmd.console_type;
    if cmd.serial {
        if result == EngineConsoleType::None || result == EngineConsoleType::Serial {
            result = EngineConsoleType::Serial;
        } else {
            return Err(anyhow!("Only one of --serial, --command, or --machine may be specified."));
        }
    }
    if cmd.command {
        if result == EngineConsoleType::None || result == EngineConsoleType::Command {
            result = EngineConsoleType::Command;
        } else {
            return Err(anyhow!("Only one of --serial, --command, or --machine may be specified."));
        }
    }
    if cmd.machine {
        if result == EngineConsoleType::None || result == EngineConsoleType::Machine {
            result = EngineConsoleType::Machine;
        } else {
            return Err(anyhow!("Only one of --serial, --command, or --machine may be specified."));
        }
    }
    if result == EngineConsoleType::None {
        result = EngineConsoleType::Serial;
    }
    Ok(result)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_console_type() -> Result<()> {
        let mut cmd = ConsoleCommand::default();

        // Nothing is selected, so it defaults to Serial.
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Serial);

        // Check each value of the --console_type flag
        cmd.console_type = EngineConsoleType::Command;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Command);

        cmd.console_type = EngineConsoleType::Machine;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Machine);

        cmd.console_type = EngineConsoleType::Serial;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Serial);

        // Check that each of the standalone flags work.
        cmd.console_type = EngineConsoleType::None;
        cmd.command = true;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Command);
        cmd.command = false;

        cmd.machine = true;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Machine);
        cmd.machine = false;

        cmd.serial = true;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Serial);
        cmd.serial = false;

        // Check that if the console_type is set, and the matching stand-alone is set, it's still ok
        cmd.command = true;
        cmd.console_type = EngineConsoleType::Command;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Command);
        cmd.command = false;

        cmd.machine = true;
        cmd.console_type = EngineConsoleType::Machine;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Machine);
        cmd.machine = false;

        cmd.serial = true;
        cmd.console_type = EngineConsoleType::Serial;
        let result = get_console_type(&cmd);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EngineConsoleType::Serial);
        cmd.serial = false;

        // Check that if any two standalones are set, or all three, it's an error
        cmd.command = true;
        cmd.serial = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.serial = false;
        cmd.command = false;

        cmd.command = true;
        cmd.machine = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.machine = false;
        cmd.command = false;

        cmd.machine = true;
        cmd.serial = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.machine = false;
        cmd.serial = false;

        cmd.command = true;
        cmd.machine = true;
        cmd.serial = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.machine = false;
        cmd.serial = false;
        cmd.command = false;

        // Check that if console_type is set, and also a non-matching standalone, it's an error
        cmd.console_type = EngineConsoleType::Serial;
        cmd.command = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.command = false;
        cmd.machine = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.machine = false;

        cmd.console_type = EngineConsoleType::Machine;
        cmd.command = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.command = false;
        cmd.serial = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.serial = false;

        cmd.console_type = EngineConsoleType::Command;
        cmd.serial = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.serial = false;
        cmd.machine = true;
        assert!(get_console_type(&cmd).is_err());
        cmd.machine = false;

        Ok(())
    }
}

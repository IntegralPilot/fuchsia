// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use emulator_instance::{EmulatorInstances, EngineState};
use ffx_config::EnvironmentContext;
use ffx_config::keys::EMU_INSTANCE_ROOT_DIR;
use ffx_emulator_config::EmulatorEngine;
use ffx_emulator_engines::EngineBuilder;
use ffx_emulator_screenshot_args::ScreenshotCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, Result as FhoResult, bug, user_error};
use schemars::JsonSchema;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, JsonSchema)]
pub struct ScreenshotResult {
    pub path: PathBuf,
}

#[derive(FfxTool)]
#[target(None)]
pub struct ScreenshotTool {
    #[command]
    pub cmd: ScreenshotCommand,
    pub context: EnvironmentContext,
}

fho::embedded_plugin!(ScreenshotTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScreenshotTool {
    type Writer = VerifiedMachineWriter<ScreenshotResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> FhoResult<()> {
        let mut engine = self.get_engine().await?;
        let absolute_path = self.resolve_path()?;

        self.validate_preconditions(&*engine, &absolute_path)?;

        engine
            .screenshot(&absolute_path)
            .await
            .map_err(|e| user_error!("Failed to take screenshot: {e}"))?;

        if writer.is_machine() {
            writer.machine(&ScreenshotResult { path: absolute_path })?;
        } else {
            let output_name = self.cmd.output.to_string_lossy();
            writer.line(format!("Screenshot saved to {}", output_name)).map_err(|e| bug!("{e}"))?;
        }
        Ok(())
    }
}

impl ScreenshotTool {
    async fn get_engine(&self) -> FhoResult<Box<dyn EmulatorEngine>> {
        let instance_dir: PathBuf =
            self.context.get(EMU_INSTANCE_ROOT_DIR).map_err(|e| bug!("{e}"))?;
        let emu_instances = EmulatorInstances::new(instance_dir);
        let builder = EngineBuilder::new(&self.context, emu_instances);

        let mut name = self.cmd.name.clone();
        builder
            .get_engine_by_name(&mut name)
            .map_err(|e| user_error!("Failed to get emulator engine: {e}"))?
            .ok_or_else(|| user_error!("Could not find emulator instance."))
    }

    fn resolve_path(&self) -> FhoResult<PathBuf> {
        if self.cmd.output.is_absolute() {
            Ok(self.cmd.output.clone())
        } else {
            std::env::current_dir()
                .context("Getting current dir")
                .map_err(|e| bug!("{e}"))
                .map(|path| path.join(&self.cmd.output))
        }
    }

    fn validate_preconditions(
        &self,
        engine: &dyn EmulatorEngine,
        absolute_path: &Path,
    ) -> FhoResult<()> {
        // Precondition checks to avoid triggering panics in the engine.
        if engine.engine_state() != EngineState::Running {
            return Err(user_error!(
                "Emulator is not running (Current state: {:?}). \
                Please start the emulator before taking a screenshot.",
                engine.engine_state()
            ));
        }

        let config = engine.emu_config();
        if config.device.screen.width == 0 || config.device.screen.height == 0 {
            return Err(user_error!(
                "The emulator virtual display has an invalid resolution ({}x{}). \
                Screenshots require a valid screen resolution.",
                config.device.screen.width,
                config.device.screen.height
            ));
        }

        if absolute_path.is_dir() {
            return Err(user_error!(
                "The output path is a directory: {}. Please specify a file path.",
                absolute_path.display()
            ));
        }

        if let Some(parent) = absolute_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                user_error!("Failed to create directory {}: {}", parent.display(), e)
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_instance::EmulatorConfiguration;
    use tempfile::tempdir;

    struct StubEngine {
        state: EngineState,
        config: EmulatorConfiguration,
    }

    #[async_trait::async_trait(?Send)]
    impl EmulatorEngine for StubEngine {
        fn engine_state(&self) -> EngineState {
            self.state
        }
        fn emu_config(&self) -> &EmulatorConfiguration {
            &self.config
        }
    }

    #[fuchsia::test]
    async fn test_get_engine_no_instance() {
        const TEST_OUT_PATH: &str = "test.png";
        let env = ffx_config::test_init().unwrap();
        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: PathBuf::from(TEST_OUT_PATH) },
            context: env.context.clone(),
        };
        let res = tool.get_engine().await;
        assert!(res.is_err());
        if let Err(e) = res {
            assert!(e.to_string().contains("Could not find emulator instance"));
        }
    }

    #[fuchsia::test]
    async fn test_resolve_path_absolute() {
        assert!(!cfg!(windows), "Windows is not supported for these tests.");
        const ABSOLUTE_PATH: &str = "/tmp/test.png";
        let env = ffx_config::test_init().unwrap();
        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: PathBuf::from(ABSOLUTE_PATH) },
            context: env.context.clone(),
        };
        let res = tool.resolve_path().unwrap();
        assert_eq!(res, PathBuf::from(ABSOLUTE_PATH));
    }

    #[fuchsia::test]
    async fn test_resolve_path_relative() {
        const RELATIVE_PATH: &str = "test.png";
        let env = ffx_config::test_init().unwrap();
        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: PathBuf::from(RELATIVE_PATH) },
            context: env.context.clone(),
        };
        let res = tool.resolve_path().unwrap();
        let expected = std::env::current_dir().unwrap().join(RELATIVE_PATH);
        assert_eq!(res, expected);
    }

    #[fuchsia::test]
    async fn test_validate_preconditions_not_running() {
        const TEST_OUT_PATH: &str = "test.png";
        const NON_EXISTENT_PATH: &str = "nonexistent.png";
        let env = ffx_config::test_init().unwrap();
        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: PathBuf::from(TEST_OUT_PATH) },
            context: env.context.clone(),
        };
        let engine =
            StubEngine { state: EngineState::Staged, config: EmulatorConfiguration::default() };
        let res = tool.validate_preconditions(&engine, Path::new(NON_EXISTENT_PATH));
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Emulator is not running"));
    }

    #[fuchsia::test]
    async fn test_validate_preconditions_headless() {
        const TEST_OUT_PATH: &str = "test.png";
        let env = ffx_config::test_init().unwrap();
        let temp = tempdir().unwrap();
        let file_path = temp.path().join(TEST_OUT_PATH);

        let mut config = EmulatorConfiguration::default();
        config.runtime.headless = true;
        config.device.screen.width = 800;
        config.device.screen.height = 600;
        let engine = StubEngine { state: EngineState::Running, config };

        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: file_path.clone() },
            context: env.context.clone(),
        };

        let res = tool.validate_preconditions(&engine, &file_path);
        assert!(res.is_ok(), "Expected OK for headless with valid resolution, got {:?}", res.err());
    }

    #[fuchsia::test]
    async fn test_validate_preconditions_invalid_resolution() {
        const TEST_OUT_PATH: &str = "test.png";
        const NON_EXISTENT_PATH: &str = "nonexistent.png";
        let env = ffx_config::test_init().unwrap();
        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: PathBuf::from(TEST_OUT_PATH) },
            context: env.context.clone(),
        };
        let mut config = EmulatorConfiguration::default();
        config.runtime.headless = false;
        config.device.screen.width = 0;
        config.device.screen.height = 0;
        let engine = StubEngine { state: EngineState::Running, config };
        let res = tool.validate_preconditions(&engine, Path::new(NON_EXISTENT_PATH));
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("invalid resolution"));
    }

    #[fuchsia::test]
    async fn test_validate_preconditions_overwrites_file() {
        const EXISTING_PATH: &str = "existing.png";
        let env = ffx_config::test_init().unwrap();
        let temp = tempdir().unwrap();
        let file_path = temp.path().join(EXISTING_PATH);
        std::fs::write(&file_path, "data").unwrap();

        let mut config = EmulatorConfiguration::default();
        config.device.screen.width = 800;
        config.device.screen.height = 600;
        let engine = StubEngine { state: EngineState::Running, config };

        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: file_path.clone() },
            context: env.context.clone(),
        };

        let res = tool.validate_preconditions(&engine, &file_path);
        assert!(res.is_ok(), "Expected OK for existing file, got {:?}", res.unwrap_err());
    }

    #[fuchsia::test]
    async fn test_validate_preconditions_is_dir() {
        let env = ffx_config::test_init().unwrap();
        let temp = tempdir().unwrap();
        let dir_path = temp.path().to_path_buf();

        let mut config = EmulatorConfiguration::default();
        config.device.screen.width = 800;
        config.device.screen.height = 600;
        let engine = StubEngine { state: EngineState::Running, config };

        let tool = ScreenshotTool {
            cmd: ScreenshotCommand { name: None, output: dir_path.clone() },
            context: env.context.clone(),
        };

        let res = tool.validate_preconditions(&engine, &dir_path);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("is a directory"));
    }
}

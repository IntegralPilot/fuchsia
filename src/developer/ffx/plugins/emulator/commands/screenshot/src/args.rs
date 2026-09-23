// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "screenshot",
    description = "Take a screenshot of the emulator's primary display.",
    example = "Capture a screenshot from the default emulator:
  $ ffx emu screenshot --output capture.png

Capture and save to a specific directory using the short flag:
  $ ffx emu screenshot -o ./captures/debug.png

Capture and save to an absolute path:
  $ ffx emu screenshot --output /tmp/screenshot.png

Capture from a specific emulator instance:
  $ ffx emu screenshot --output result.png fuchsia-emulator",
    note = "The screenshot is captured as a PNG file in RGB color space.
The resolution is the same as the emulator's primary display device.

This command will fail if:
- The emulator is not in the 'Running' state.
- A virtual display is not enabled in the emulator configuration.
- The output path is a directory.
- The parent directory is not writable.

If the parent directory of the output path does not exist, ffx will
attempt to create it automatically."
)]
pub struct ScreenshotCommand {
    /// name of the emulator instance to take a screenshot on.
    /// If not specified, the default instance is used.
    /// See a list of available instances by running `ffx emu list`.
    #[argh(positional)]
    pub name: Option<String>,

    /// path to save the screenshot. Relative (to the current directory) or absolute paths
    /// are both supported. The output will always be in PNG format.
    #[argh(option, short = 'o')]
    pub output: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screenshot_args() {
        const OUT_PATH: &str = "test.png";
        let args = ["screenshot", "--output", OUT_PATH];
        let cmd = ScreenshotCommand::from_args(&["screenshot"], &args).unwrap();
        assert_eq!(cmd.output, PathBuf::from(OUT_PATH));
        assert_eq!(cmd.name, None);
    }

    #[test]
    fn test_screenshot_with_name() {
        const OUT_PATH: &str = "test.png";
        const EMU_NAME: &str = "fuchsia-emulator";
        let args = ["screenshot", "--output", OUT_PATH, EMU_NAME];
        let cmd = ScreenshotCommand::from_args(&["screenshot"], &args).unwrap();
        assert_eq!(cmd.output, PathBuf::from(OUT_PATH));
        assert_eq!(cmd.name, Some(EMU_NAME.to_string()));
    }

    #[test]
    fn test_screenshot_args_short() {
        const OUT_PATH: &str = "test.png";
        let args = ["screenshot", "-o", OUT_PATH];
        let cmd = ScreenshotCommand::from_args(&["screenshot"], &args).unwrap();
        assert_eq!(cmd.output, PathBuf::from(OUT_PATH));
        assert_eq!(cmd.name, None);
    }

    #[test]
    fn test_screenshot_with_name_short() {
        const OUT_PATH: &str = "test.png";
        const EMU_NAME: &str = "fuchsia-emulator";
        let args = ["screenshot", "-o", OUT_PATH, EMU_NAME];
        let cmd = ScreenshotCommand::from_args(&["screenshot"], &args).unwrap();
        assert_eq!(cmd.output, PathBuf::from(OUT_PATH));
        assert_eq!(cmd.name, Some(EMU_NAME.to_string()));
    }

    #[test]
    fn test_screenshot_missing_output() {
        let args = ["screenshot"];
        let result = ScreenshotCommand::from_args(&["screenshot"], &args);
        assert!(result.is_err());
    }
}

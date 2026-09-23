// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This file defines some e2e tests for ffx emu related workflows.

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use ffx_emulator_stop_command_output::CommandStatus;
    use ffx_executor::strict::{LogOutputLocation, StrictContext};
    use ffx_executor::test::{TestCommandLineInfo, TestingError};
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::TempDir;

    const FFX_PATH: &str = env!("FFX_PATH");

    fn ssh_key_path(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("ssh_private_key")
    }

    async fn new_strict_context() -> Result<(StrictContext, TempDir)> {
        let temp_dir = tempfile::TempDir::new()?;
        assert!(temp_dir.path().exists());
        let emu_dir = temp_dir.path().join("emulators");
        std::fs::create_dir_all(&emu_dir)?;
        let ssh_priv_key = ssh_key_path(&temp_dir);
        let ssh_pub_key = temp_dir.path().join("ssh_public_key");
        let ffx_binary = PathBuf::from_str(FFX_PATH).unwrap();
        let mut ffx_root_dir = ffx_binary.clone();
        ffx_root_dir.pop();
        let ffx_strict_context = StrictContext::new(
            ffx_binary,
            LogOutputLocation::Stderr,
            [
                ("log.level", "debug"),
                ("ssh.priv", &ssh_priv_key.to_string_lossy()),
                ("ssh.pub", &ssh_pub_key.to_string_lossy()),
                ("ffx.subtool-search-paths", &ffx_root_dir.to_string_lossy()),
                ("emu.instance_dir", &emu_dir.to_string_lossy()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_string()))
            .collect(),
        );
        Ok((ffx_strict_context, temp_dir))
    }

    #[fuchsia::test]
    async fn test_strict_stop_no_emulators() {
        let (ffx_strict_ctx, _temp_dir) =
            new_strict_context().await.expect("creating ffx_strict context");
        // Expect a failed exit code.
        let test_data =
            vec![TestCommandLineInfo::new(vec!["emu", "stop"], |mut command_output| {
                // TODO(b/395982857): The tool should not print out two error messages upon
                // failure. For now, that is why the split is happening. The fix for this is
                // going to be complicated.
                let mut s_arr = command_output.stdout.split("\n");
                command_output.stdout = s_arr.next().unwrap().to_owned();

                let err: CommandStatus =
                    command_output.machine_output().map_err(TestingError::ParsingError)?;
                let CommandStatus::UserError { ref message } = err else {
                    return Err(TestingError::MatchingError(format!(
                        "wrong error returned. Expected `UserError`, Got: `{err:?}`"
                    )));
                };
                let expected = "does not exist";
                if message.contains(expected) {
                    Ok(())
                } else {
                    Err(TestingError::MatchingError(format!(
                        "Expected: '{}', Got: '{}'",
                        expected.to_owned(),
                        message
                    )))
                }
            })];
        TestCommandLineInfo::run_command_lines(&ffx_strict_ctx, test_data)
            .await
            .expect("run commands");
    }

    #[fuchsia::test]
    async fn test_target_list_and_show_with_serial_numbers() {
        use ffx_e2e_emu::IsolatedEmulator;
        use serde_json::Value;

        // 1. Start emulator with serial numbers enabled (default).
        let emu_with_serial =
            IsolatedEmulator::start("emu-with-serial").await.expect("start emu-with-serial");
        let name_with_serial = emu_with_serial.emu_name();

        // Verify serial number is listed in target list.
        let list_with_serial: Value =
            emu_with_serial.ffx_json(&["target", "list", name_with_serial]).await.unwrap();
        let targets_with_serial = list_with_serial.as_array().expect("target list is array");
        assert_eq!(targets_with_serial.len(), 1);
        let target_with_serial = &targets_with_serial[0];
        let serial = target_with_serial["serial"].as_str().expect("serial is string");
        assert_eq!(serial.len(), 12);
        assert!(serial.starts_with("EM-"));
        assert!(
            serial[3..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && (c.is_numeric() || c.is_uppercase()))
        );

        // Verify serial number is shown in target show.
        let show_with_serial: Value = emu_with_serial.ffx_json(&["target", "show"]).await.unwrap();
        let show_serial =
            show_with_serial["device"]["serial_number"].as_str().expect("serial_number is string");
        assert_eq!(show_serial, serial);

        // 2. Start emulator with serial numbers disabled.
        let emu_no_serial = IsolatedEmulator::start_with_serial_enabled("emu-no-serial", false)
            .await
            .expect("start emu-no-serial");
        let name_no_serial = emu_no_serial.emu_name();

        // Verify target list shows `<unknown>` or no serial.
        let list_no_serial: Value =
            emu_no_serial.ffx_json(&["target", "list", name_no_serial]).await.unwrap();
        let targets_no_serial = list_no_serial.as_array().expect("target list is array");
        assert_eq!(targets_no_serial.len(), 1);
        let target_no_serial = &targets_no_serial[0];
        let serial_no_serial = target_no_serial["serial"].as_str();
        assert!(serial_no_serial.is_none() || serial_no_serial == Some("<unknown>"));

        // Verify target show has null or empty/missing serial_number.
        let show_no_serial: Value = emu_no_serial.ffx_json(&["target", "show"]).await.unwrap();
        let show_serial_no_serial = show_no_serial["device"]["serial_number"].as_str();
        assert!(show_serial_no_serial.is_none());
    }
}

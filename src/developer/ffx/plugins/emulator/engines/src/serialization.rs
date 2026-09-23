// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::EngineBuilder;
use emulator_instance::{EmulatorInstances, EngineOption, read_from_disk};
use ffx_config::EnvironmentContext;
use ffx_emulator_config::EmulatorEngine;
use fho::{Result, return_bug, return_user_error};

pub async fn read_engine_from_disk(
    context: &EnvironmentContext,
    emu_instances: &EmulatorInstances,
    name: &str,
) -> Result<Box<dyn EmulatorEngine>> {
    let instance_dir =
        emu_instances.get_instance_dir(name, false).map_err(|e| anyhow::Error::from(e))?;
    let builder = EngineBuilder::new(context, emu_instances.clone());
    match read_from_disk(&instance_dir) {
        Ok(EngineOption::DoesExist(data)) => Ok(builder.from_data(*data)),
        Ok(EngineOption::DoesNotExist(_)) => return_user_error!("{name} instance does not exist"),
        Err(e) => return_bug!("Could not read engine {name} from disk: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FemuEngine;
    use crate::qemu_based::qemu::QemuEngine;
    use emulator_instance::{EmulatorInstanceData, EngineState, EngineType};
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_write_then_read() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");
        let env = ffx_config::test_env().build()?;
        // Create a test directory in TempFile::tempdir.
        let qemu_name = "qemu_test_write_then_read";
        let femu_name = "femu_test_write_then_read";

        let instance_dir = temp_dir.path().to_path_buf();
        let emu_instances = EmulatorInstances::new(instance_dir.clone());

        // Set up some test data.
        let mut qemu_instance_data =
            EmulatorInstanceData::new_with_state(qemu_name, EngineState::New);
        qemu_instance_data.set_engine_type(EngineType::Qemu);
        let q_engine = QemuEngine::new(&env.context, qemu_instance_data, emu_instances.clone());
        let mut femu_instance_data =
            EmulatorInstanceData::new_with_state(femu_name, EngineState::New);
        femu_instance_data.set_engine_type(EngineType::Qemu);
        let f_engine = FemuEngine::new(&env.context, femu_instance_data, emu_instances.clone());

        // Serialize the QEMU engine to disk.
        q_engine.save_to_disk().await.expect("Problem serializing QEMU engine to disk.");

        let qemu_copy = read_engine_from_disk(&env.context, &emu_instances, qemu_name)
            .await
            .expect("Problem reading QEMU engine from disk.");

        assert_eq!(qemu_copy.get_instance_data(), q_engine.get_instance_data());

        // Serialize the FEMU engine to disk.
        f_engine.save_to_disk().await.expect("Problem serializing FEMU engine to disk.");
        let box_engine = read_engine_from_disk(&env.context, &emu_instances, femu_name).await;
        assert!(box_engine.is_ok(), "Read from disk failed for FEMU: {:?}", box_engine.err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_read_unknown_engine_type() -> Result<()> {
        let unknown_engine_type = include_str!("../test_data/unknown_engine_type_engine.json");
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");
        let instance_root = PathBuf::from(temp_dir.path());

        let emulator_instances = EmulatorInstances::new(instance_root);

        let name = "unknown-type";

        {
            // stage the instance data since we can't write it via the emulator_instance API.

            let instance_path =
                emulator_instances.get_instance_dir(name, true).expect("get instance dir");
            let data_path = instance_path.join("engine.json");
            let mut file = File::create(&data_path).expect("create data file");
            write!(file, "{}", &unknown_engine_type).expect("writing unknown engine type");
        }

        let box_engine = read_from_disk(
            &emulator_instances.get_instance_dir(name, false).expect("get instance dir"),
        );
        assert!(box_engine.is_err());

        Ok(())
    }
}

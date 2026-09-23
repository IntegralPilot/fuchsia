// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::{EmulatorInstanceData, EmulatorInstanceError, EngineOption, EngineState, Result};
use std::fs::{File, create_dir_all};
use std::path::PathBuf;

pub(crate) const SERIALIZE_FILE_NAME: &str = "engine.json";

#[derive(Debug, Clone)]
pub struct EmulatorInstances {
    instance_root: PathBuf,
}

impl EmulatorInstances {
    pub fn new(instance_root: PathBuf) -> Self {
        EmulatorInstances { instance_root }
    }
    /// Return a PathBuf with the path to the instance directory for this engine. If the "create" flag
    /// is set, the directory and its ancestors will be created if it doesn't already exist.
    pub fn get_instance_dir(&self, instance_name: &str, create: bool) -> Result<PathBuf> {
        let path = self.instance_root.join(&instance_name);
        if !path.exists() {
            if create {
                log::debug!("Creating {:?} for {}", path, instance_name);
                create_dir_all(&path.as_path())?;
            } else {
                log::debug!(
                    "Path {} doesn't exist. Check the spelling of the instance name.",
                    instance_name
                );
            }
        }
        Ok(path)
    }

    /// Given an instance name, empty and remove the instance directory associated with that name.
    /// Fails if the directory can't be removed; returns Ok(()) if the directory doesn't exist.
    pub fn clean_up_instance_dir(&self, instance_name: &str) -> Result<()> {
        let path = self.get_instance_dir(instance_name, false)?;
        if path.exists() {
            log::debug!("Removing {:?} for {:?}", path, path.as_path().file_name().unwrap());
            std::fs::remove_dir_all(&path.as_path()).map_err(|e| {
                EmulatorInstanceError::RemoveDirectory { path: path.to_path_buf(), source: e }
            })
        } else {
            // It's already gone, so just return Ok(()).
            Ok(())
        }
    }

    /// Retrieve a list of all of the names of instances currently present on the local system.
    pub fn get_all_instances(&self) -> Result<Vec<EmulatorInstanceData>> {
        let mut result = Vec::new();
        let root = self.instance_root.as_path();
        if root.is_dir() {
            for entry in root.read_dir()? {
                if let Ok(entry) = entry {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    if entry.path().join(SERIALIZE_FILE_NAME).exists() {
                        if let Some(name_as_os_str) = entry.path().file_name() {
                            if let Some(name) = name_as_os_str.to_str() {
                                match read_from_disk(&entry.path()) {
                                    Ok(EngineOption::DoesExist(data)) => result.push(*data),
                                    Ok(EngineOption::DoesNotExist(name)) => {
                                        result.push(EmulatorInstanceData::new_with_state(
                                            &name,
                                            EngineState::Error,
                                        ))
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Cannot read emulator instance data for {}: {:?}",
                                            name,
                                            e
                                        );
                                        result.push(EmulatorInstanceData::new_with_state(
                                            name,
                                            EngineState::Error,
                                        ));
                                    }
                                };
                            }
                        }
                    }
                }
            }
        }
        return Ok(result);
    }
}

pub fn read_from_disk(instance_directory: &PathBuf) -> Result<EngineOption> {
    let filepath = instance_directory.join(SERIALIZE_FILE_NAME);

    // Read the engine.json file and deserialize it from disk into a new TypedEngine instance
    if filepath.exists() {
        let file = File::open(&filepath)
            .map_err(|e| EmulatorInstanceError::OpenFile { path: filepath.clone(), source: e })?;
        let res: Result<EmulatorInstanceData> = serde_json::from_reader(file)
            .map_err(|e| EmulatorInstanceError::ParseJson { path: filepath.clone(), source: e });
        if res.is_err() {
            log::warn!("Failed to parse emulator instance: {res:?}");
        }
        let value = res?;
        Ok(EngineOption::DoesExist(Box::new(value)))
    } else {
        Ok(EngineOption::DoesNotExist(filepath.to_string_lossy().into()))
    }
}

pub fn read_from_disk_untyped(instance_directory: &PathBuf) -> Result<serde_json::Value> {
    // Get the engine's location, which is in the instance directory.
    let filepath = instance_directory.join(SERIALIZE_FILE_NAME);

    // Read the engine.json file and deserialize it from disk into a new TypedEngine instance
    if filepath.exists() {
        let file = File::open(&filepath)
            .map_err(|e| EmulatorInstanceError::OpenFile { path: filepath.clone(), source: e })?;
        let value: serde_json::Value = serde_json::from_reader(file)
            .map_err(|e| EmulatorInstanceError::ParseJson { path: filepath.clone(), source: e })?;
        Ok(value)
    } else {
        Err(EmulatorInstanceError::MissingEngineFile(filepath))
    }
}

pub fn write_to_disk(data: &EmulatorInstanceData, instance_directory: &PathBuf) -> Result<()> {
    // The engine's serialized form will be saved in ${EMU_INSTANCE_ROOT_DIR}/${runtime.name}.
    // This is the path set up by the EngineBuilder, so it's expected to already exist.

    // Create the engine.json file to hold the serialized data, and write it out to disk,
    let filepath = instance_directory.join(SERIALIZE_FILE_NAME);
    let file = File::create(&filepath)
        .map_err(|e| EmulatorInstanceError::CreateFile { path: filepath.clone(), source: e })?;
    log::debug!("Writing serialized engine out to {:?}", filepath);
    match serde_json::to_writer(file, data) {
        Ok(_) => Ok(()),
        Err(e) => Err(EmulatorInstanceError::SerializeJson(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmulatorInstanceInfo, EngineType};
    use std::fs::remove_file;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_get_instance_dir() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emu_instances = EmulatorInstances::new(instance_root.clone());

        // Create a new directory.
        let path1 = emu_instances.get_instance_dir("create_me", true)?;
        assert_eq!(path1, instance_root.join("create_me"));
        assert!(path1.exists());

        // Look for a dir that doesn't exist, but don't create it.
        let path2 = emu_instances.get_instance_dir("dont_create", false)?;
        assert!(!path2.exists());

        // Look for a dir that already exists, but don't allow creation.
        let mut path3 = emu_instances.get_instance_dir("create_me", false)?;
        assert_eq!(path3, instance_root.join("create_me"));
        assert!(path3.exists());

        // Get an existing directory, but set the create flag too. Make sure it didn't get replaced.
        path3 = path3.join("foo.txt");
        let _ = File::create(&path3)?;
        let path4 = emu_instances.get_instance_dir("create_me", true)?;
        assert!(path4.exists());
        assert!(path3.exists());
        assert_eq!(path4, instance_root.join("create_me"));
        for entry in path4.as_path().read_dir()? {
            assert_eq!(entry?.path(), path3);
        }

        Ok(())
    }

    #[test]
    fn test_get_all_instances() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emu_instances = EmulatorInstances::new(instance_root.clone());

        // Create three mock instance directories, and make sure they're all included.
        let path1 = instance_root.join("path1");
        create_dir_all(path1.as_path())?;
        let file1_path = path1.join(SERIALIZE_FILE_NAME);
        let _file1 = File::create(&file1_path)?;

        let path2 = instance_root.join("path2");
        create_dir_all(path2.as_path())?;
        let file2_path = path2.join(SERIALIZE_FILE_NAME);
        let _file2 = File::create(&file2_path)?;

        let path3 = instance_root.join("path3");
        create_dir_all(path3.as_path())?;
        let file3_path = path3.join(SERIALIZE_FILE_NAME);
        let _file3 = File::create(&file3_path)?;

        let instances = emu_instances.get_all_instances()?;
        assert!(instances.iter().any(|e| e.get_name() == "path1"));
        assert!(instances.iter().any(|e| e.get_name() == "path2"));
        assert!(instances.iter().any(|e| e.get_name() == "path3"));

        // If the directory doesn't contain an engine.json file, it's not an instance.
        // Remove the file for path2, and make sure it's excluded from the results.
        assert!(remove_file(&file2_path).is_ok());

        let instances = emu_instances.get_all_instances()?;
        assert!(instances.iter().any(|e| e.get_name() == "path1"));
        assert!(!instances.iter().any(|e| e.get_name() == "path2"));
        assert!(instances.iter().any(|e| e.get_name() == "path3"));

        // Other files in the root shouldn't be included either. Create an empty file in the root
        // and make sure it's excluded too.
        let file_path = instance_root.join("empty_file");
        let _empty_file = File::create(&file_path)?;

        let instances = emu_instances.get_all_instances()?;
        assert!(instances.iter().any(|e| e.get_name() == "path1"));
        assert!(!instances.iter().any(|e| e.get_name() == "path2"));
        assert!(instances.iter().any(|e| e.get_name() == "path3"));

        Ok(())
    }

    #[test]
    fn test_clean_up_instance_dir() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emu_instances = EmulatorInstances::new(instance_root.clone());

        let path1 = instance_root.join("path1");
        create_dir_all(path1.as_path())?;
        assert!(path1.exists());

        let path2 = instance_root.join("path2");
        create_dir_all(path2.as_path())?;
        assert!(path2.exists());

        let file_path = path2.join("foo.txt");
        let _ = File::create(&file_path)?;
        assert!(file_path.exists());

        // Clean up an existing, empty directory
        emu_instances.clean_up_instance_dir("path1").expect("cleanup path1");
        assert!(!path1.exists());
        assert!(path2.exists());

        // Clean up an existing, populated directory
        emu_instances.clean_up_instance_dir("path2").expect("cleanup path2");
        assert!(!path2.exists());
        assert!(!file_path.exists());

        // Clean up an non-existing directory
        emu_instances.clean_up_instance_dir("path3").expect("cleanup path3");
        assert!(!path1.exists());
        Ok(())
    }

    #[test]
    fn test_broken_reads() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emu_instances = EmulatorInstances::new(instance_root.clone());
        // Create a test directory in TempFile::tempdir.
        let name = "test_write_then_read";
        let temp_path = instance_root.join(name);
        let file_path = temp_path.join(SERIALIZE_FILE_NAME);
        create_dir_all(&temp_path)?;

        let bad_json = "This is not valid JSON";
        let no_pid = r#"{ "engine_type":"femu" }"#;
        let bad_pid = r#"{ "engine_type":"femu","pid":"string" }"#;
        let has_pid = r#"{ "engine_type":"femu","pid":123456 }"#;

        // Note: This string is a currently valid and complete instance of a FEMU config as it
        // would be serialized to disk. The test on this string should fail if a change (to the
        // EmulatorConfiguration data structure, for example) would break deserialization of
        // existing emulation instances. If your change causes this test to fail, consider wrapping
        // the fields you changed in Option<foo>, or providing a default value for the field to
        // deserialize with. Do not simply update this text to match your change, or users will
        // see [Broken] emulators on their next update. Wait until the field has had time to "bake"
        // before updating this text for your changes.
        let valid_femu = r#"{"emulator_configuration":{"device":{"audio":{"model":"hda"},"cpu":{
            "architecture":"x64","count":0},"memory":{"quantity":8192,"units":"megabytes"},
            "pointing_device":"mouse","screen":{"height":800,"width":1280,"units":"pixels"},
            "vsock":{"enabled":false,"cid":0},
            "storage":{"quantity":2,"units":"gigabytes"}},"flags":{"args":[],"envs":{},"features":[],
            "kernel_args":[],"options":[]},"guest":{"fvm_image":"/path/to/fvm.blk","kernel_image":
            "/path/to/multiboot.bin","zbi":"/path/to/fuchsia.zbi"},"host":{"acceleration":
            "hyper","architecture":"x64","gpu":"auto","log":"/path/to/emulator.log","networking"
            :"tap","os":"linux","port_map":{}},"runtime":{"console":"none","debugger":false,
            "dry_run":false,"headless":true,"hidpi_scaling":false,"instance_directory":"/some/dir",
            "log_level":"info","mac_address":"52:54:47:5e:82:ef","name":"fuchsia-emulator",
            "startup_timeout":{"secs":60,"nanos":0},"template":"/path/to/config","upscript":null}},
            "pid":657042,"engine_type":"femu"}"#;

        let mut file = File::create(&file_path)?;
        write!(file, "{}", &bad_json)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_err());
        let value = read_from_disk_untyped(&temp_path);
        assert!(value.is_err());

        remove_file(&file_path).expect("Problem removing serialized file during test.");
        let mut file = File::create(&file_path)?;
        write!(file, "{}", &no_pid)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_err());
        let value = read_from_disk_untyped(&temp_path);
        assert!(value.is_ok(), "{:?}", value);
        assert!(value.unwrap().get("pid").is_none());

        remove_file(&file_path).expect("Problem removing serialized file during test.");
        let mut file = File::create(&file_path)?;
        write!(file, "{}", &bad_pid)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_err());
        let value = read_from_disk_untyped(&temp_path);
        assert!(value.is_ok(), "{:?}", value);
        assert!(value.as_ref().unwrap().get("pid").is_some());
        assert!(value.unwrap().get("pid").unwrap().as_i64().is_none());

        remove_file(&file_path).expect("Problem removing serialized file during test.");
        let mut file = File::create(&file_path)?;
        write!(file, "{}", &has_pid)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_err());
        let value = read_from_disk_untyped(&temp_path);
        assert!(value.is_ok(), "{:?}", value);
        assert!(value.as_ref().unwrap().get("pid").is_some());
        assert!(value.as_ref().unwrap().get("pid").unwrap().as_i64().is_some());
        assert_eq!(value.unwrap().get("pid").unwrap().as_i64().unwrap(), 123456);

        remove_file(&file_path).expect("Problem removing serialized file during test.");
        let mut file = File::create(&file_path)?;
        write!(file, "{}", &valid_femu)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_ok(), "{:?}", box_engine.err());
        match box_engine? {
            EngineOption::DoesExist(data) => assert_eq!(data.get_engine_type(), EngineType::Femu),
            other => panic!("Expected DoesExist, got {other:?}"),
        }

        Ok(())
    }

    #[test]
    fn test_no_vsock() -> Result<()> {
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emu_instances = EmulatorInstances::new(instance_root.clone());
        // Create a test directory in TempFile::tempdir.
        let name = "test_no_vsock";
        let temp_path = instance_root.join(name);
        let file_path = temp_path.join(SERIALIZE_FILE_NAME);
        create_dir_all(&temp_path)?;

        // Note: This string is a currently valid and complete instance of a FEMU config as it
        // would be serialized to disk. The test on this string should fail if a change (to the
        // EmulatorConfiguration data structure, for example) would break deserialization of
        // existing emulation instances. If your change causes this test to fail, consider wrapping
        // the fields you changed in Option<foo>, or providing a default value for the field to
        // deserialize with. Do not simply update this text to match your change, or users will
        // see [Broken] emulators on their next update. Wait until the field has had time to "bake"
        // before updating this text for your changes.
        let valid_femu = r#"{"emulator_configuration":{"device":{"audio":{"model":"hda"},"cpu":{
            "architecture":"x64","count":0},"memory":{"quantity":8192,"units":"megabytes"},
            "pointing_device":"mouse","screen":{"height":800,"width":1280,"units":"pixels"},
            "storage":{"quantity":2,"units":"gigabytes"}},"flags":{"args":[],"envs":{},"features":[],
            "kernel_args":[],"options":[]},"guest":{"fvm_image":"/path/to/fvm.blk","kernel_image":
            "/path/to/multiboot.bin","zbi":"/path/to/fuchsia.zbi"},"host":{"acceleration":
            "hyper","architecture":"x64","gpu":"auto","log":"/path/to/emulator.log","networking"
            :"tap","os":"linux","port_map":{}},"runtime":{"console":"none","debugger":false,
            "dry_run":false,"headless":true,"hidpi_scaling":false,"instance_directory":"/some/dir",
            "log_level":"info","mac_address":"52:54:47:5e:82:ef","name":"fuchsia-emulator",
            "startup_timeout":{"secs":60,"nanos":0},"template":"/path/to/config","upscript":null}},
            "pid":657042,"engine_type":"femu"}"#;

        let mut file = File::create(&file_path)?;
        write!(file, "{}", &valid_femu)?;
        let box_engine = read_from_disk(&emu_instances.get_instance_dir(name, false)?);
        assert!(box_engine.is_ok(), "{:?}", box_engine.err());
        match box_engine? {
            EngineOption::DoesExist(data) => assert_eq!(data.get_engine_type(), EngineType::Femu),
            other => panic!("Expected DoesExist, got {other:?}"),
        }

        Ok(())
    }
}

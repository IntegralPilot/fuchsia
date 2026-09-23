// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow, bail};
use assembly_package_copy::PackageCopier;
use assembly_util::copy_dir;
use camino::{Utf8Path, Utf8PathBuf};
use depfile::Depfile;
use pathdiff::diff_paths;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A container of assembly artifacts that has a top-level config json file.
/// Use #[assembly_container(config.json)] to implement this trait.
/// The struct must derive Deserialize, Serialize, and WalkPaths.
pub trait AssemblyContainer {
    /// Get the filename of the top-level config, which is used during
    /// serialization and deserialization. This is implemented by the proc-macro
    ///   #[assembly_container(config.json)]
    fn get_config_filename() -> &'static str;

    /// Construct an assembly container from a config file.
    /// It is assumed that the paths in `config_path` are absolute.
    ///
    /// This should only be used during initial construction of the container,
    /// such as from the bazel rules, because otherwise the container should
    /// already be hermetic and the config will contain relative paths.
    fn from_config_path(config_path: impl AsRef<Utf8Path>) -> Result<Self>
    where
        Self: Sized + WalkPaths + DeserializeOwned,
    {
        // Read the config to a string first because it offers better
        // performance for serde.
        let data = std::fs::read_to_string(config_path.as_ref())
            .with_context(|| format!("Reading config: {}", config_path.as_ref()))?;
        let config = serde_json5::from_str(&data)
            .with_context(|| format!("Parsing config: {}", config_path.as_ref()))?;

        // We assume that the paths are already absolute, because we are loading
        // from a config file, rather than a hermetic directory.
        Ok(config)
    }

    /// Construct an assembly container from a config file, where the paths
    /// inside the config file are relative to the config file's directory.
    fn from_config_path_relative_paths(config_path: impl AsRef<Utf8Path>) -> Result<Self>
    where
        Self: Sized + WalkPaths + DeserializeOwned,
    {
        let mut config = Self::from_config_path(&config_path)?;
        if let Some(dir) = config_path.as_ref().parent() {
            config
                .walk_paths(&mut |path: &mut Utf8PathBuf,
                                  _dest: Utf8PathBuf,
                                  _filetype: FileType| {
                    *path = dir.join(&path);
                    Ok(())
                })
                .context("Making all config paths absolute")?;
        }
        Ok(config)
    }

    /// Parse an assembly container from a hermetic directory on disk.
    /// It is assumed that the paths in the config file are relative, and when
    /// parsing them they will be transformed to absolute to make them easier
    /// to manipulate.
    fn from_dir(dir: impl AsRef<Utf8Path>) -> Result<Self>
    where
        Self: Sized + WalkPaths + DeserializeOwned,
    {
        let config_path = dir.as_ref().join(Self::get_config_filename());
        let mut config = Self::from_config_path(config_path)?;

        // We assume that the paths are relative, because we are loading from a
        // hermetic directory. They need to be made absolute.
        config
            .walk_paths(&mut |path: &mut Utf8PathBuf, _dest: Utf8PathBuf, _filetype: FileType| {
                *path = dir.as_ref().join(&path);
                Ok(())
            })
            .context("Making all config paths absolute")?;
        Ok(config)
    }

    /// Write an assembly container to a directory on disk.
    ///
    /// The paths will be transformed from absolute to relative before writing
    /// them to disk.
    ///
    /// A depfile will also be written that includes all the files that were
    /// opened or copied.
    fn write_to_dir(
        self,
        dir: impl AsRef<Utf8Path>,
        depfile_path: Option<impl AsRef<Utf8Path>>,
    ) -> Result<Self>
    where
        Self: Sized + WalkPaths + Serialize,
    {
        if let Some(depfile_path) = depfile_path {
            let config_path = dir.as_ref().join(Self::get_config_filename());

            let mut depfile = Depfile::new_with_output(&config_path);
            let s = self.write_to_dir_with_depfile(&dir, Some(&mut depfile))?;

            depfile
                .write_to(&depfile_path)
                .with_context(|| format!("Writing depfile: {}", depfile_path.as_ref()))?;
            Ok(s)
        } else {
            self.write_to_dir_with_depfile(&dir, None)
        }
    }

    /// Write an assembly container to a directory on disk, using the provided
    /// depfile to track inputs and outputs.
    ///
    /// The paths will be transformed from absolute to relative before writing
    /// them to disk.
    fn write_to_dir_with_depfile(
        mut self,
        dir: impl AsRef<Utf8Path>,
        depfile: Option<&mut Depfile>,
    ) -> Result<Self>
    where
        Self: Sized + WalkPaths + Serialize,
    {
        let config_path = dir.as_ref().join(Self::get_config_filename());
        let mut depfile = depfile;

        // Ignore failures to remove the directory.
        let _ = std::fs::remove_dir_all(dir.as_ref());
        std::fs::create_dir_all(dir.as_ref())?;

        // Prepare a packages copier with directories at the top of the container.
        let blobs_dir = dir.as_ref().join("blobs");
        let packages_dir = dir.as_ref().join("packages");
        let subpackages_dir = dir.as_ref().join("subpackages");
        std::fs::create_dir_all(&blobs_dir)?;
        std::fs::create_dir_all(&packages_dir)?;
        std::fs::create_dir_all(&subpackages_dir)?;
        let mut package_copier = PackageCopier::new(&packages_dir, &subpackages_dir, &blobs_dir);

        // Collect all output paths to ensure we don't have any duplicates.
        let mut outputs = BTreeSet::new();

        // Copy each file referenced by the config into `dir`.
        self.walk_paths(&mut |path: &mut Utf8PathBuf, dest: Utf8PathBuf, filetype: FileType| {
            // Bazel does not like colons in filenames.
            // Replace any colons in `dest` with directory separators.
            let dest = dest
                .into_string()
                .split(|c| c == ':')
                .filter(|s| !s.is_empty())
                .fold(Utf8PathBuf::new(), |dest, part| dest.join(part));

            match filetype {
                // Package manifests are copied via the PackageCopier.
                FileType::PackageManifest => {
                    let new_path = package_copier.add_package_from_manifest_path(&path)?;
                    let new_path = diff_paths(&new_path, dir.as_ref()).ok_or_else(|| {
                        anyhow!("Failed to make the path relative: {}", new_path)
                    })?;
                    *path = Utf8PathBuf::try_from(new_path)?;
                }

                FileType::Directory => {
                    let new_path = dir.as_ref().join(&dest);
                    copy_dir(path.as_std_path(), new_path.as_std_path())
                        .with_context(|| format!("Copying directory: {}", path))?;
                    *path = dest;
                }

                // All other files are copied directly.
                FileType::Unknown => {
                    if let Some(depfile) = depfile.as_deref_mut() {
                        depfile.add_input(&path);
                    }
                    let name = path
                        .file_name()
                        .ok_or_else(|| anyhow!("Path is missing a filename: {}", path))?;

                    // Copy the file to the right place and ensure that we don't copy two files to
                    // the same location.
                    let absolute_dir = dir.as_ref().join(&dest);
                    let absolute_path = absolute_dir.join(&name);
                    if !outputs.insert(absolute_path.clone()) {
                        bail!(
                            "Two files in the container will clobber each other at destination: {}",
                            absolute_path
                        );
                    }
                    std::fs::create_dir_all(&absolute_dir)?;
                    assembly_util::fast_copy(&path, &absolute_path)
                        .with_context(|| format!("Copying file: {}", path))?;

                    // Replace the path with the relative path.
                    // We always want to write the paths to disk as relative.
                    let relative_path = dest.join(&name);
                    *path = relative_path;
                }
            }

            Ok(())
        })
        .with_context(|| {
            format!("Copying all files referenced by the config into: {}", dir.as_ref())
        })?;

        // Copy all the packages.
        let package_deps = package_copier.perform_copy().with_context(|| {
            format!("Copying all packages reference by the config into: {}", dir.as_ref())
        })?;
        if let Some(depfile) = depfile {
            depfile.add_inputs(package_deps);
        }

        // Write the new config to the `dir`.
        let config_file = std::fs::File::create(config_path)?;
        serde_json::to_writer_pretty(config_file, &self)
            .with_context(|| format!("Writing the config file into: {}", dir.as_ref()))?;

        // Update the paths to be absolute.
        // The paths should always be absolute when read into memory.
        self.walk_paths(&mut |path: &mut Utf8PathBuf, _dest: Utf8PathBuf, _filetype: FileType| {
            *path = dir.as_ref().join(&path);
            Ok(())
        })
        .context("Making all the paths absolute")?;

        Ok(self)
    }

    /// Merge the values from `overrides` into self and return a new self.
    fn apply_overrides(self, overrides: serde_json::Value) -> Result<Self>
    where
        Self: Sized + DeserializeOwned + Serialize,
    {
        let config = serde_json::to_value(self).context("Parsing the config to a value")?;
        crate::merge::try_merge_into::<Self>(config, overrides).context("Applying overrides")
    }
}

/// The type of the file, which is passed to the `found` function in WalkPaths.
#[derive(Debug, PartialEq)]
pub enum FileType {
    /// An unknown file type.
    Unknown,

    /// A package manifest.
    PackageManifest,

    /// An opaque directory.
    Directory,
}

/// A function that is called whenever a path is found in the config.
pub trait WalkPathsFn: FnMut(&mut Utf8PathBuf, Utf8PathBuf, FileType) -> Result<()> {}
impl<T: FnMut(&mut Utf8PathBuf, Utf8PathBuf, FileType) -> Result<()>> WalkPathsFn for T {}

/// A type that implements `WalkPaths` can be traversed to find all the paths
/// inside itself. Whenever a path is found, `found` will be called with a
/// mutable reference to the path (so that it can be manipulated), and a desired
/// destination directory for that path. The type gets to defined what that
/// desired destination directory should be.
pub trait WalkPaths {
    /// Walk all the paths in the type, and call `found` when one is found.
    fn walk_paths<F: WalkPathsFn>(&mut self, found: &mut F) -> Result<()> {
        self.walk_paths_with_dest(found, Utf8PathBuf::default())
    }

    /// Walk all the paths in the type, and call `found` when one is found, and
    /// append to `dest` if desired.
    ///
    /// A type may choose to append to `dest` in order to disambiguate child
    /// fields when recursing. For example, a struct with a field `my_field`,
    /// would append `/my_field`.
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()>;
}

impl WalkPaths for Utf8PathBuf {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        found(self, dest, FileType::Unknown)?;
        Ok(())
    }
}

impl<T> WalkPaths for Option<T>
where
    T: WalkPaths,
{
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        if let Some(value) = self {
            value.walk_paths_with_dest(found, dest)?;
        }
        Ok(())
    }
}

impl<T> WalkPaths for Vec<T>
where
    T: WalkPaths,
{
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        for (index, value) in self.iter_mut().enumerate() {
            // We add the index in order to avoid collisions between files with the same name.
            let dest = dest.join(format!("{}", index));
            value.walk_paths_with_dest(found, dest)?;
        }
        Ok(())
    }
}

impl<K, V> WalkPaths for HashMap<K, V>
where
    K: AsRef<Utf8Path>,
    V: WalkPaths,
{
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        for (key, value) in self {
            let dest = dest.join(key);
            value.walk_paths_with_dest(found, dest)?;
        }
        Ok(())
    }
}

impl<K, V> WalkPaths for BTreeMap<K, V>
where
    K: AsRef<Utf8Path>,
    V: WalkPaths,
{
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        for (key, value) in self {
            let dest = dest.join(key);
            value.walk_paths_with_dest(found, dest)?;
        }
        Ok(())
    }
}

pub fn path_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    let mut schema: schemars::schema::SchemaObject = <String>::json_schema(generator).into();
    schema.format = Some("Utf8PathBuf".to_owned());
    schema.into()
}

/// A path to a directory that should be copied wholesale inside an
/// AssemblyContainer.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, JsonSchema)]
pub struct DirectoryPathBuf(#[schemars(schema_with = "path_schema")] Utf8PathBuf);

impl WalkPaths for DirectoryPathBuf {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> Result<()> {
        found(&mut self.0, dest, FileType::Directory)
    }
}

impl std::fmt::Display for DirectoryPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl DirectoryPathBuf {
    /// Construct a new DirectoryPathBuf from a Utf8PathBuf.
    pub fn new(path: Utf8PathBuf) -> Self {
        Self(path)
    }

    /// Get the inner Utf8PathBuf.
    pub fn as_utf8_path_buf(&self) -> &Utf8PathBuf {
        &self.0
    }
}

impl AsRef<Utf8Path> for DirectoryPathBuf {
    fn as_ref(&self) -> &Utf8Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for DirectoryPathBuf {
    fn as_ref(&self) -> &std::path::Path {
        self.0.as_std_path()
    }
}

#[cfg(test)]
mod tests {
    use super::copy_dir;
    use crate::{AssemblyContainer, FileType, WalkPaths, assembly_container};
    use camino::Utf8PathBuf;
    use fuchsia_hash::{HASH_SIZE, Hash};
    use fuchsia_pkg::{BlobInfo, MetaPackage, PackageManifestBuilder, PackageName};
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};
    use std::fs::File;
    use std::io::Read;
    use std::str::FromStr;
    use tempfile::{NamedTempFile, tempdir};

    #[derive(Debug, Deserialize, Serialize, WalkPaths, PartialEq, Eq)]
    #[assembly_container(nested_structs.json)]
    struct Config {
        #[walk_paths]
        alpha: Alpha,
    }

    #[derive(Debug, Deserialize, Serialize, WalkPaths, PartialEq, Eq)]
    struct Alpha {
        #[walk_paths]
        beta: Beta,
    }

    #[derive(Debug, Deserialize, Serialize, WalkPaths, PartialEq, Eq)]
    struct Beta {
        #[walk_paths]
        gamma: Utf8PathBuf,
    }

    #[test]
    fn test_read_from_config() {
        // Write a config to disk using absolute paths.
        let gamma_file = NamedTempFile::new().unwrap();
        let gamma_path = Utf8PathBuf::from_path_buf(gamma_file.path().to_path_buf()).unwrap();
        let config_value = json!({
            "alpha": {
                "beta": {
                    "gamma": gamma_path.clone(),
                }
            }
        });
        let config_file = NamedTempFile::new().unwrap();
        let config_path = Utf8PathBuf::from_path_buf(config_file.path().to_path_buf()).unwrap();
        serde_json::to_writer(&config_file, &config_value).unwrap();

        // Parse the config into memory and ensure it is correct.
        // The config in memory should have absolute paths.
        let expected: Config = serde_json::from_value(config_value).unwrap();
        let config = Config::from_config_path(&config_path).unwrap();
        assert_eq!(expected, config);
    }

    #[test]
    fn test_read_from_json5_config() {
        // Write a config to disk using absolute paths.
        let gamma_file = NamedTempFile::new().unwrap();
        let gamma_path = Utf8PathBuf::from_path_buf(gamma_file.path().to_path_buf()).unwrap();
        let config_value = json!({
            "alpha": {
                "beta": {
                    "gamma": gamma_path.clone(),
                }
            }
        });
        let config_file = NamedTempFile::new().unwrap();
        let config_path = Utf8PathBuf::from_path_buf(config_file.path().to_path_buf()).unwrap();
        serde_json5::to_writer(&config_file, &config_value).unwrap();

        // Parse the config into memory and ensure it is correct.
        // The config in memory should have absolute paths.
        let expected: Config = serde_json::from_value(config_value).unwrap();
        let config = Config::from_config_path(&config_path).unwrap();
        assert_eq!(expected, config);
    }

    #[test]
    fn test_nested_structs_write_then_read() {
        // Construct a config in memory.
        let gamma_file = NamedTempFile::new().unwrap();
        let gamma_path = Utf8PathBuf::from_path_buf(gamma_file.path().to_path_buf()).unwrap();
        let gamma_name = gamma_path.file_name().unwrap();
        let c = Config { alpha: Alpha { beta: Beta { gamma: gamma_path.clone() } } };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to disk.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let c = c.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the returned config is correct.
        // The config in memory should have absolute paths.
        let new_gamma_path = dir_path.join("alpha/beta/gamma").join(&gamma_name);
        let expected = Config { alpha: Alpha { beta: Beta { gamma: new_gamma_path.clone() } } };
        assert_eq!(expected, c);

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("nested_structs.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "alpha": {
                "beta": {
                    "gamma": format!("alpha/beta/gamma/{}", &gamma_name),
                }
            }
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure gamma exists in the right location.
        assert!(new_gamma_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &gamma_path), deps);

        // Parse the config on disk back into memory and ensure it is correct.
        let parsed_c = Config::from_dir(&dir_path).unwrap();
        assert_eq!(c, parsed_c);
    }

    #[test]
    fn test_custom_walk() {
        #[derive(Deserialize, Serialize)]
        #[assembly_container(custom_walk.json)]
        struct ConfigWithCustomWalk {
            path: Utf8PathBuf,
        }

        impl WalkPaths for ConfigWithCustomWalk {
            fn walk_paths_with_dest<F: crate::WalkPathsFn>(
                &mut self,
                found: &mut F,
                dest: Utf8PathBuf,
            ) -> anyhow::Result<()> {
                // Put the files under "custom_dir".
                let dest = dest.join("custom_dir");
                found(&mut self.path, dest.clone(), FileType::Unknown)?;
                Ok(())
            }
        }

        // Create a config in memory.
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let name = path.file_name().unwrap();
        let config = ConfigWithCustomWalk { path: path.clone() };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        config.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("custom_walk.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "path": format!("custom_dir/{}", &name),
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = dir_path.join("custom_dir").join(&name);
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &path), deps);
    }

    #[test]
    fn test_enum() {
        #[derive(Deserialize, Serialize, WalkPaths)]
        #[assembly_container(enums.json)]
        struct ConfigWithEnum {
            #[walk_paths]
            field1: MyEnum,
            #[walk_paths]
            field2: MyEnum,
        }

        #[derive(Deserialize, Serialize, WalkPaths)]
        enum MyEnum {
            Unnamed(Utf8PathBuf),
            Named { key: Utf8PathBuf },
        }

        // Create a config in memory.
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let name = path.file_name().unwrap();
        let config = ConfigWithEnum {
            field1: MyEnum::Unnamed(path.clone()),
            field2: MyEnum::Named { key: path.clone() },
        };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        config.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("enums.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "field1": {
                "Unnamed": format!("field1/Unnamed/{}", &name),
            },
            "field2" : {
                "Named": {
                    "key": format!("field2/Named/key/{}", &name),
                },
            },
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = dir_path.join("field1").join("Unnamed").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path.join("field2").join("Named").join("key").join(&name);
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &path), deps);
    }

    #[test]
    fn test_map() {
        #[derive(Deserialize, Serialize, WalkPaths)]
        #[assembly_container(maps.json)]
        struct ConfigWithMap {
            #[walk_paths]
            hash_map: HashMap<String, Utf8PathBuf>,
            #[walk_paths]
            btree_map: BTreeMap<String, Utf8PathBuf>,
            #[walk_paths]
            btree_map_with_colons: BTreeMap<String, Utf8PathBuf>,
        }

        // Create a config in memory.
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let name = path.file_name().unwrap();
        let config = ConfigWithMap {
            hash_map: [("key".into(), path.clone())].into(),
            btree_map: [("key".into(), path.clone())].into(),
            btree_map_with_colons: [("key::with::colons".into(), path.clone())].into(),
        };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        config.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("maps.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "hash_map": {
                "key": format!("hash_map/key/{}", &name),
            },
            "btree_map": {
                "key": format!("btree_map/key/{}", &name),
            },
            "btree_map_with_colons": {
                "key::with::colons": format!("btree_map_with_colons/key/with/colons/{}", &name),
            },
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = dir_path.join("hash_map").join("key").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path.join("btree_map").join("key").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path
            .join("btree_map_with_colons")
            .join("key")
            .join("with")
            .join("colons")
            .join(&name);
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &path), deps);
    }

    #[test]
    fn test_option() {
        #[derive(Deserialize, Serialize, WalkPaths)]
        #[assembly_container(options.json)]
        struct ConfigWithOption {
            #[walk_paths]
            option1: Option<Utf8PathBuf>,
            #[walk_paths]
            option2: Option<Utf8PathBuf>,
        }

        // Create a config in memory.
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let name = path.file_name().unwrap();
        let config = ConfigWithOption { option1: Some(path.clone()), option2: None };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        config.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("options.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "option1": format!("option1/{}", &name),
            "option2": None::<Utf8PathBuf>,
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = dir_path.join("option1").join(&name);
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &path), deps);
    }

    #[test]
    fn test_nested_vecs() {
        #[derive(Deserialize, Serialize, WalkPaths)]
        #[assembly_container(nested_vecs.json)]
        struct ConfigWithVec {
            #[walk_paths]
            top_vec: Vec<ParentStruct>,
        }

        #[derive(Deserialize, Serialize, WalkPaths)]
        struct ParentStruct {
            #[walk_paths]
            parent_vec: Vec<ChildStruct>,
        }

        #[derive(Deserialize, Serialize, WalkPaths)]
        struct ChildStruct {
            #[walk_paths]
            child_vec: Vec<Utf8PathBuf>,
        }

        // Create a config in memory.
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let name = path.file_name().unwrap();
        let config = ConfigWithVec {
            top_vec: vec![
                ParentStruct { parent_vec: vec![ChildStruct { child_vec: vec![path.clone()] }] },
                ParentStruct {
                    parent_vec: vec![
                        ChildStruct { child_vec: vec![path.clone()] },
                        ChildStruct { child_vec: vec![path.clone(), path.clone()] },
                    ],
                },
            ],
        };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        config.write_to_dir(&dir_path, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = dir_path.join("nested_vecs.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "top_vec": [
                {
                    "parent_vec": [{
                        "child_vec": [ format!("top_vec/0/parent_vec/0/child_vec/0/{}", &name) ],
                    }],
                },
                {
                    "parent_vec": [
                        {
                            "child_vec": [
                                format!("top_vec/1/parent_vec/0/child_vec/0/{}", &name),
                            ],
                        },
                        {
                            "child_vec": [
                                format!("top_vec/1/parent_vec/1/child_vec/0/{}", &name),
                                format!("top_vec/1/parent_vec/1/child_vec/1/{}", &name),
                            ],
                        },
                    ],
                },
            ],
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = dir_path.join("top_vec/0/parent_vec/0/child_vec/0").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path.join("top_vec/1/parent_vec/0/child_vec/0").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path.join("top_vec/1/parent_vec/1/child_vec/0").join(&name);
        assert!(new_path.exists());
        let new_path = dir_path.join("top_vec/1/parent_vec/1/child_vec/1").join(&name);
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(format!("{}: \\\n  {}\n", &config_path, &path), deps);
    }

    #[test]
    fn test_package_manifest() {
        #[derive(Deserialize, Serialize)]
        #[assembly_container(config.json)]
        struct ConfigWithPackageManifest {
            manifest: Utf8PathBuf,
        }

        // Use a custom walk so that we can return FileType::PackageManifest.
        impl WalkPaths for ConfigWithPackageManifest {
            fn walk_paths_with_dest<F: crate::WalkPathsFn>(
                &mut self,
                found: &mut F,
                dest: Utf8PathBuf,
            ) -> anyhow::Result<()> {
                found(&mut self.manifest, dest.join("manifest"), FileType::PackageManifest)
            }
        }

        // Prepare a directory for temporary files.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        // Create a config in memory.
        let blob_path = dir_path.join("file.txt");
        std::fs::write(&blob_path, "").unwrap();
        let package_name = PackageName::from_str("fake").unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero(package_name);
        let package_manifest_builder = PackageManifestBuilder::new(meta_package);
        let package_manifest_builder = package_manifest_builder.add_blob(BlobInfo {
            source_path: blob_path.to_string(),
            path: "data/file.txt".into(),
            merkle: Hash::from_array([0; HASH_SIZE]),
            size: 1,
        });
        let package_manifest = package_manifest_builder.build();
        let package_manifest_path = dir_path.join("fake_package_manifest.json");
        package_manifest.write_with_relative_paths(&package_manifest_path).unwrap();
        let config = ConfigWithPackageManifest { manifest: package_manifest_path.clone() };

        // Prepare a depfile.
        let mut depfile = NamedTempFile::new().unwrap();
        let depfile_path = Utf8PathBuf::from_path_buf(depfile.path().to_path_buf()).unwrap();

        // Write the config to a hermetic directory.
        let container_dir = dir_path.join("container");
        config.write_to_dir(&container_dir, Some(depfile_path)).unwrap();

        // Ensure the contents of the config on disk are correct.
        // The config on disk should have relative paths.
        let config_path = container_dir.join("config.json");
        let config_file = File::open(&config_path).unwrap();
        let expected = json!({
            "manifest": "packages/fake",
        });
        let actual: serde_json::Value = serde_json::from_reader(&config_file).unwrap();
        assert_eq!(expected, actual);

        // Ensure the files exists in the right location.
        let new_path = container_dir.join("packages/fake");
        assert!(new_path.exists());
        let new_path = container_dir
            .join("blobs/0000000000000000000000000000000000000000000000000000000000000000");
        assert!(new_path.exists());

        // Ensure the depfile is correct.
        let mut deps = String::new();
        depfile.read_to_string(&mut deps).unwrap();
        assert_eq!(
            format!("{}: \\\n  {} \\\n  {}\n", &config_path, &package_manifest_path, &blob_path),
            deps
        );
    }

    #[test]
    fn test_copy_dir() {
        // Prepare a directory for temporary files.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        // Write a directory with a file in a nested path.
        //  src
        //   |- nested
        //       |- file.txt
        let src_path = dir_path.join("src");
        let src_nested_path = src_path.join("nested");
        let src_file_path = src_nested_path.join("file.txt");
        std::fs::create_dir_all(&src_nested_path).unwrap();
        std::fs::write(src_file_path, "data").unwrap();

        // Copy to a new directory.
        let dst_path = dir_path.join("dst");
        copy_dir(src_path.as_std_path(), dst_path.as_std_path()).unwrap();

        // Ensure the new directory contains the file in the new location.
        let dst_file_path = dst_path.join("nested").join("file.txt");
        let data = std::fs::read_to_string(&dst_file_path).unwrap();
        assert_eq!("data", &data);
    }

    #[test]
    fn test_walk_error_for_duplicate_path() {
        #[derive(Deserialize, Serialize)]
        #[assembly_container(config.json)]
        struct ConfigWithDuplicate {
            one: Utf8PathBuf,
            two: Utf8PathBuf,
        }

        // Use a custom walk so that we can return two identical dests.
        impl WalkPaths for ConfigWithDuplicate {
            fn walk_paths_with_dest<F: crate::WalkPathsFn>(
                &mut self,
                found: &mut F,
                dest: Utf8PathBuf,
            ) -> anyhow::Result<()> {
                found(&mut self.one, dest.clone(), FileType::Unknown)?;
                found(&mut self.two, dest, FileType::Unknown)?;
                Ok(())
            }
        }

        // Prepare a directory for temporary files.
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        // Create a config in memory that uses the same file for both fields.
        // It doesn't have to be the same file to cause an error, but it does
        // have to have the same filename in order to collide.
        let input_path = dir_path.join("file.txt");
        std::fs::write(&input_path, "").unwrap();
        let config = ConfigWithDuplicate { one: input_path.clone(), two: input_path };

        // Write the config to a hermetic directory.
        let container_dir = dir_path.join("container");
        assert!(config.write_to_dir(&container_dir, None::<Utf8PathBuf>).is_err());
    }
}

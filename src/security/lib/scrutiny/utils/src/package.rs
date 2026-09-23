// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifact::ArtifactReader;
use crate::io::ReadSeek;
use crate::key_value::parse_key_value;
use anyhow::{Context, Result};
use fuchsia_archive::Utf8Reader;
use fuchsia_merkle::Hash;
use fuchsia_url::{PackageName, PackageVariant};
use serde::de::{self, Deserializer, Error as _, MapAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::{FromStr, from_utf8};
use thiserror::Error;

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum SystemImageError {
    #[error("additional boot config is missing the pkgfs cmd entry")]
    MissingPkgfsCmdEntry,
    #[error(
        "Unexpected number of pkgfs cmd entry arguments: expected {expected_len}; actual: {actual_len}"
    )]
    UnexpectedPkgfsCmdLen { expected_len: usize, actual_len: usize },
    #[error("Unexpected pkgfs command: expected {expected_cmd}; actual {actual_cmd}")]
    UnexpectedPkgfsCmd { expected_cmd: String, actual_cmd: String },
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum PackageError {
    #[error("Malformed package hash: expected hex-SHA256; actual {actual_hash}")]
    MalformedPackageHash { actual_hash: String },
    #[error("Failed to open package file: {package_path}: {io_error}")]
    FailedToOpenPackage { package_path: PathBuf, io_error: String },
    #[error("Failed to read package file: {package_path}: {io_error}")]
    FailedToReadPackage { package_path: PathBuf, io_error: String },
    #[error(
        "Failed to verify package file: expected merkle root: {expected_merkle_root}; computed merkle root: {computed_merkle_root}"
    )]
    FailedToVerifyPackage { expected_merkle_root: Hash, computed_merkle_root: Hash },
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ReadContentBlobError {
    #[error("Failed to read package's meta/contents file: {error}")]
    FailedToReadMetaContents { error: String },
    #[error("Failed to convert package's meta/contents file's raw bytes to string: {error}")]
    FailedToConvertMetaContentsToString { error: String },
    #[error("Failed to parse path=merkle pairs in package's meta/contents file: {error}")]
    FailedToParseMetaContents { error: String },
    #[error("File not found in package's meta/contents map: {file_path}")]
    MetaContentsDoesNotContainFile { file_path: String },
    #[error("Failed to read file from package: {file_path}, error: {error}")]
    FailedToReadFileFromPackage { file_path: String, error: String },
}

/// Path within a Fuchsia package that contains the package contents manifest.
pub static META_CONTENTS_PATH: &str = "meta/contents";
pub static PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY: &str = "zircon.system.pkgfs.cmd";
pub static PKGFS_BINARY_PATH: &str = "bin/pkgsvr";

/// Parse the system image merkle hash from the zircon.system.pkgfs.cmd value in additional boot args.
/// Assumption: the additional_boot_args provided have already split values by the `+` delimiter.
/// Example: zircon.system.pkgfs.cmd=bin/pkgsvr+a5e7a3756cca7fc664de30d9fe6cec96f7923562763a4678b1a7c69f84aedce3
pub fn extract_system_image_hash_string(
    additional_boot_args: &HashMap<String, Vec<String>>,
) -> Result<String, SystemImageError> {
    let pkgfs_cmd = additional_boot_args
        .get(PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY)
        .ok_or(SystemImageError::MissingPkgfsCmdEntry)?;

    if pkgfs_cmd.len() != 2 {
        return Err(SystemImageError::UnexpectedPkgfsCmdLen {
            expected_len: 2,
            actual_len: pkgfs_cmd.len(),
        });
    }
    let (pkgfs_binary_path, system_image_merkle_string) = (&pkgfs_cmd[0], &pkgfs_cmd[1]);

    if pkgfs_binary_path != PKGFS_BINARY_PATH {
        return Err(SystemImageError::UnexpectedPkgfsCmd {
            expected_cmd: PKGFS_BINARY_PATH.to_string(),
            actual_cmd: pkgfs_binary_path.clone(),
        });
    }
    Ok(system_image_merkle_string.to_string())
}

/// Verify that the merkle given for the package matches the computed merkle of the associated blob.
///
/// The package merkle strings may be extracted from:
/// 1. The zircon.system.pkgfs.cmd value from additional_boot_args for the system image blob.
/// 2. The package listing in data/static_packages from the system image blob's data.
/// 3. The bootfs package listing within a zbi from data/bootfs_packages.
///
/// The provided artifact_reader is assumed to have access to the set of blobs containing the package of interest.
pub fn verify_package_merkle(
    package_merkle_string: &String,
    artifact_reader: &mut Box<dyn ArtifactReader>,
) -> Result<(), PackageError> {
    // First, ensure the given string for the package merkle is a valid hash.
    let package_merkle = Hash::from_str(package_merkle_string).map_err(|_| {
        PackageError::MalformedPackageHash { actual_hash: package_merkle_string.clone() }
    })?;

    // Next, open the blob corresponding to the package merkle from the artifact reader.
    let package_path = Path::new(package_merkle_string);
    let pkg = artifact_reader.open(&Path::new(package_path)).map_err(|err| {
        PackageError::FailedToReadPackage {
            package_path: package_path.to_path_buf(),
            io_error: err.to_string(),
        }
    })?;

    // Compute the merkle from the package blob.
    let computed_package_merkle =
        fuchsia_merkle::root_from_reader(pkg).map_err(|err| PackageError::FailedToReadPackage {
            package_path: package_path.to_path_buf(),
            io_error: err.to_string(),
        })?;

    // Make sure the computed merkle matches the given value.
    if computed_package_merkle != package_merkle {
        return Err(PackageError::FailedToVerifyPackage {
            expected_merkle_root: package_merkle,
            computed_merkle_root: computed_package_merkle,
        });
    }
    Ok(())
}

pub fn open_update_package<P: AsRef<Path>>(
    update_package_path: P,
    artifact_reader: &mut Box<dyn ArtifactReader>,
) -> Result<Utf8Reader<Box<dyn ReadSeek>>> {
    let update_package_path = update_package_path.as_ref();
    let update_package_file = artifact_reader.open(update_package_path).with_context(|| {
        format!("Failed to open update package meta.far at {:?}", update_package_path)
    })?;

    let update_package_hash = fuchsia_merkle::root_from_reader(update_package_file)
        .with_context(|| {
            format!(
                "Failed to compute merkle root of update package meta.far at {:?}",
                update_package_path
            )
        })?
        .to_string();
    let far = artifact_reader.open(&Path::new(&update_package_hash)).with_context(|| {
        format!(
            "Failed to open update package meta.far at {:?} from artifact archives",
            update_package_path
        )
    })?;
    Utf8Reader::new(far).with_context(|| {
        format!(
            "Failed to initialize far reader for update package at {:?} with merkle root {}",
            update_package_path, update_package_hash
        )
    })
}

pub fn read_content_blob(
    far_reader: &mut Utf8Reader<impl ReadSeek>,
    artifact_reader: &mut Box<dyn ArtifactReader>,
    path: &str,
) -> Result<Vec<u8>, ReadContentBlobError> {
    let meta_contents = far_reader
        .read_file(META_CONTENTS_PATH)
        .map_err(|err| ReadContentBlobError::FailedToReadMetaContents { error: err.to_string() })?;
    let meta_contents = from_utf8(meta_contents.as_slice()).map_err(|err| {
        ReadContentBlobError::FailedToConvertMetaContentsToString { error: err.to_string() }
    })?;
    let paths_to_merkles = parse_key_value(meta_contents).map_err(|err| {
        ReadContentBlobError::FailedToParseMetaContents { error: err.to_string() }
    })?;
    let merkle_root = paths_to_merkles.get(path).ok_or_else(|| {
        ReadContentBlobError::MetaContentsDoesNotContainFile { file_path: path.to_string() }
    })?;
    artifact_reader.read_bytes(&Path::new(merkle_root)).map_err(|err| {
        ReadContentBlobError::FailedToReadFileFromPackage {
            file_path: path.to_string(),
            error: err.to_string(),
        }
    })
}

/// Package index files contain lines of the form:
/// [pkg-name-variant-path]=[merkle-root-hash].
pub type PackageIndexContents = HashMap<(PackageName, Option<PackageVariant>), Hash>;

/// Serialize package indices listing contents. A custom strategy is necessary because
/// map keys are stored as `(PackageName, Option<PackageVariant>)`, which must be manually converted
/// to a string representation.
pub fn serialize_pkg_index<S>(
    pkgs: &Option<PackageIndexContents>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match pkgs {
        None => serializer.serialize_none(),
        Some(pkgs) => {
            let mut map = HashMap::new();
            for ((name, variant), hash) in pkgs {
                match variant {
                    None => {
                        map.insert(name.to_string(), hash.to_string());
                    }
                    Some(variant) => {
                        map.insert(
                            format!("{}/{}", name.as_ref(), variant.as_ref()),
                            hash.to_string(),
                        );
                    }
                }
            }
            serializer.serialize_some(&map)
        }
    }
}

/// Deserialize package indices listing contents. A custom strategy is necessary because
/// map keys are stored as `(PackageName, Option<PackageVariant>)`, which must be manually converted
/// from a string representation.
pub fn deserialize_pkg_index<'de, D>(
    deserializer: D,
) -> Result<Option<PackageIndexContents>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptVisitor;
    struct MapVisitor;

    impl<'de> Visitor<'de> for OptVisitor {
        type Value = Option<PackageIndexContents>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("optional pkgs map")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            let visitor = MapVisitor;
            Ok(Some(deserializer.deserialize_any(visitor)?))
        }
    }

    impl<'de> Visitor<'de> for MapVisitor {
        type Value = PackageIndexContents;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("pkg index map")
        }

        fn visit_map<A>(self, mut map_access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut map = HashMap::with_capacity(map_access.size_hint().unwrap_or(0));
            loop {
                let entry: Option<(String, String)> = map_access.next_entry()?;
                match entry {
                    None => break,
                    Some((name_variant_string, hash_string)) => {
                        let mut name_parts = vec![];
                        let mut variant_part = None;
                        for part in name_variant_string.split("/") {
                            if let Some(prev_tail) = variant_part {
                                name_parts.push(prev_tail);
                            }
                            variant_part = Some(part);
                        }
                        let name_string = name_parts.join("/");
                        let name = PackageName::from_str(&name_string).map_err(|err| {
                            A::Error::custom(&format!(
                                "Failed to parse package name from string: {}: {}",
                                name_string, err
                            ))
                        })?;
                        let variant = variant_part
                            .map(|variant| {
                                PackageVariant::from_str(variant).map_err(|err| {
                                    A::Error::custom(&format!(
                                        "Failed to parse package variant from string: {}: {}",
                                        variant, err
                                    ))
                                })
                            })
                            .map_or(Ok(None), |r| r.map(Some))?;
                        let hash = Hash::from_str(&hash_string).map_err(|err| {
                            A::Error::custom(&format!(
                                "Failed to parse package hash from string: {}: {}",
                                hash_string, err
                            ))
                        })?;
                        map.insert((name, variant), hash);
                    }
                }
            }
            Ok(map)
        }
    }

    let visitor = OptVisitor;
    deserializer.deserialize_option(visitor)
}

#[cfg(test)]
mod tests {
    use super::{
        META_CONTENTS_PATH, PKGFS_BINARY_PATH, PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY, PackageError,
        SystemImageError, extract_system_image_hash_string, verify_package_merkle,
    };
    use crate::artifact::ArtifactReader;
    use crate::io::ReadSeek;
    use anyhow::{Result, anyhow};
    use fuchsia_archive::write as far_write;
    use fuchsia_merkle::{HASH_SIZE, Hash};
    use maplit::{btreemap, hashmap};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::io::{BufWriter, Cursor, Read};
    use std::path::{Path, PathBuf};

    struct TestArtifactReader {
        artifacts: HashMap<PathBuf, Vec<u8>>,
    }

    impl TestArtifactReader {
        fn new(artifacts: HashMap<PathBuf, Vec<u8>>) -> Self {
            Self { artifacts }
        }
    }

    impl ArtifactReader for TestArtifactReader {
        fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>> {
            if let Some(bytes) = self.artifacts.get(path) {
                return Ok(Box::new(Cursor::new(bytes.clone())));
            }
            Err(anyhow!("No artifact found for path: {:?}", path))
        }

        fn read_bytes(&mut self, _path: &Path) -> Result<Vec<u8>> {
            panic!("not implemented");
        }

        fn get_deps(&self) -> HashSet<PathBuf> {
            panic!("not implemented");
        }
    }

    fn create_package_far() -> Vec<u8> {
        let mut package_far = BufWriter::new(Vec::new());
        let meta_contents = "".to_string();
        let meta_contents_bytes = meta_contents.as_bytes();
        let meta_contents_reader: Box<dyn Read> = Box::new(meta_contents_bytes);
        let path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = btreemap! {
            META_CONTENTS_PATH =>
                (meta_contents_bytes.len() as u64, meta_contents_reader),
        };
        far_write(&mut package_far, path_content_map).unwrap();
        package_far.into_inner().unwrap()
    }

    // These pkgfs command parsing tests mirror the tests in static_pkgs collector, but are narrower in
    // scope as they test smaller functions.
    #[fuchsia::test]
    fn test_missing_pkgfs_cmd_entry() {
        let additional_boot_args = hashmap! {};
        let result = extract_system_image_hash_string(&additional_boot_args);
        match result {
            Err(SystemImageError::MissingPkgfsCmdEntry { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_pkgfs_cmd_too_short() {
        let additional_boot_args = hashmap! {
            PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY.to_string() => vec![PKGFS_BINARY_PATH.to_string()],
        };
        let result = extract_system_image_hash_string(&additional_boot_args);
        match result {
            Err(SystemImageError::UnexpectedPkgfsCmdLen { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_pkgfs_cmd_too_long() {
        let additional_boot_args = hashmap! {
            PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY.to_string() => vec![
                PKGFS_BINARY_PATH.to_string(),
                "param1".to_string(),
                "param2".to_string(),
            ],
        };
        let result = extract_system_image_hash_string(&additional_boot_args);
        match result {
            Err(SystemImageError::UnexpectedPkgfsCmdLen { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_bad_pkgfs_cmd() {
        let bad_cmd_name = "unexpected/pkgsvr/path";
        let additional_boot_args = hashmap! {
            PKGFS_CMD_ADDITIONAL_BOOT_CONFIG_KEY.to_string() => vec![
                bad_cmd_name.to_string(),
                Hash::from([0; HASH_SIZE]).to_string(),
            ],
        };
        let result = extract_system_image_hash_string(&additional_boot_args);
        match result {
            Err(SystemImageError::UnexpectedPkgfsCmd { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_invalid_package_merkle() {
        let mut test_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));
        let bad_merkle_root = "I am not a merkle root".to_string();
        let result = verify_package_merkle(&bad_merkle_root, &mut test_artifact_reader);
        match result {
            Err(PackageError::MalformedPackageHash { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_missing_package() {
        let mut test_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));
        let designated_system_image_hash = Hash::from([0; HASH_SIZE]);
        let result = verify_package_merkle(
            &designated_system_image_hash.to_string(),
            &mut test_artifact_reader,
        );
        match result {
            Err(PackageError::FailedToReadPackage { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }

    #[fuchsia::test]
    fn test_incorrect_package_merkle() {
        // This test sets up a TestArtifactReader to contain a representative artifact that
        // we can calculate a merkle hash for. The code under test will use this to compute
        // a merkle and match it against the given value (the `designated_package_hash`).
        let designated_package_hash = Hash::from([0; HASH_SIZE]);
        let package_contents = create_package_far();
        let package_hash = fuchsia_merkle::root_from_slice(&package_contents);
        assert!(designated_package_hash != package_hash);

        // Incorrectly map designated_package_hash` to `package_contents` (that's not its
        // content hash!).
        let test_artifacts = hashmap! {
            PathBuf::from(designated_package_hash.to_string()) => package_contents,
        };

        let mut test_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(test_artifacts));

        let result =
            verify_package_merkle(&designated_package_hash.to_string(), &mut test_artifact_reader);

        match result {
            Err(PackageError::FailedToVerifyPackage { .. }) => return,
            _ => panic!("Unexpected result: {:?}", result),
        };
    }
}

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::package_types::{ComponentManifest, PackageDefinition, PartialPackageDefinition};
use crate::package_utils::{is_cf_v2_config_values, is_cf_v2_manifest};
use anyhow::{Context, Result};
use fuchsia_archive::Utf8Reader as FarReader;
use fuchsia_hash::Hash;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use scrutiny_utils::artifact::ArtifactReader;
use scrutiny_utils::io::ReadSeek;
use scrutiny_utils::key_value::parse_key_value;
use scrutiny_utils::package::{META_CONTENTS_PATH, open_update_package, read_content_blob};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use update_package::parse_packages_json;

/// Trait that defines functions for the retrieval of the bytes that define package definitions.
/// Used primarily to allow for nicer testing via mocking out the backing `fx serve` instance.
pub trait PackageReader: Send + Sync {
    /// Returns the fully-qualified URLs of packages expected to be reachable
    /// via this reader.
    fn read_package_urls(&mut self) -> Result<Vec<PinnedAbsolutePackageUrl>>;
    /// Takes a package name and a merkle hash and returns the package definition
    /// for the specified merkle hash. All valid CF files specified by the FAR
    /// archive pointed to by the merkle hash are parsed and returned.
    /// The package name is not validated.
    ///
    /// Currently only CFv1 is supported, CFv2 support is tracked here (https://fxbug.dev/42130750).
    fn read_package_definition(
        &mut self,
        pkg_url: &PinnedAbsolutePackageUrl,
    ) -> Result<PackageDefinition>;
    /// Identical to `read_package_definition`, except read the update package bound to this reader.
    /// If reader is not bound to an update package or an error occurs reading the update package,
    /// return an `Err(...)`.
    fn read_update_package_definition(&mut self) -> Result<PartialPackageDefinition>;
    /// Gets the paths to files touched by read operations.
    fn get_deps(&self) -> HashSet<PathBuf>;
}

pub struct PackagesFromUpdateReader {
    update_package_path: PathBuf,
    blob_reader: Box<dyn ArtifactReader>,
    deps: HashSet<PathBuf>,
}

impl PackagesFromUpdateReader {
    pub fn new<P: AsRef<Path>>(
        update_package_path: P,
        blob_reader: Box<dyn ArtifactReader>,
    ) -> Self {
        Self {
            update_package_path: update_package_path.as_ref().to_path_buf(),
            blob_reader,
            deps: HashSet::new(),
        }
    }
}

impl PackageReader for PackagesFromUpdateReader {
    fn read_package_urls(&mut self) -> Result<Vec<PinnedAbsolutePackageUrl>> {
        self.deps.insert(self.update_package_path.clone());
        let mut far_reader = open_update_package(&self.update_package_path, &mut self.blob_reader)
            .context("Failed to open update meta.far for package reader")?;
        let packages_json_contents =
            read_content_blob(&mut far_reader, &mut self.blob_reader, "packages.json")
                .context("Failed to open update packages.json in update meta.far")?;
        log::info!(
            "packages.json contents: \"\"\"\n{}\n\"\"\"",
            std::str::from_utf8(packages_json_contents.as_slice()).unwrap()
        );
        parse_packages_json(packages_json_contents.as_slice())
            .context("Failed to parse packages.json file from update package")
    }

    fn read_package_definition(
        &mut self,
        pkg_url: &PinnedAbsolutePackageUrl,
    ) -> Result<PackageDefinition> {
        let meta_far = self
            .blob_reader
            .open(&Path::new(&pkg_url.hash().to_string()))
            .with_context(|| format!("Failed to open meta.far blob for package {}", pkg_url))?;
        read_package_definition(pkg_url, meta_far)
    }

    fn read_update_package_definition(&mut self) -> Result<PartialPackageDefinition> {
        self.deps.insert(self.update_package_path.clone());

        let update_package: Box<dyn ReadSeek> =
            self.blob_reader.open(&self.update_package_path).with_context(|| {
                format!("reading from blob_reader: {}", self.update_package_path.display())
            })?;

        read_partial_package_definition(update_package).with_context(|| {
            format!("reading update package: {}", self.update_package_path.display())
        })
    }

    fn get_deps(&self) -> HashSet<PathBuf> {
        self.deps.union(&self.blob_reader.get_deps()).map(PathBuf::clone).collect()
    }
}

pub fn read_package_definition(
    pkg_url: &PinnedAbsolutePackageUrl,
    data: impl ReadSeek,
) -> Result<PackageDefinition> {
    let partial = read_partial_package_definition(data)
        .with_context(|| format!("Failed to construct package definition for {:?}", pkg_url))?;
    Ok(PackageDefinition::new(pkg_url.clone().into(), partial))
}

pub fn read_partial_package_definition(rs: impl ReadSeek) -> Result<PartialPackageDefinition> {
    let mut far_reader =
        FarReader::new(rs).context("Failed to construct meta.far reader for package")?;

    let mut pkg_def = PartialPackageDefinition::default();

    // Find any parseable files from the archive.
    // Create a separate list of file names to read to avoid borrow checker
    // issues while iterating through the list.
    let mut cf_v2_files = Vec::<String>::new();
    let mut cf_v2_config_files = Vec::<String>::new();
    let mut contains_meta_contents = false;
    for item in far_reader.list().map(|e| e.path()) {
        if item == META_CONTENTS_PATH {
            contains_meta_contents = true;
        } else {
            let path_buf: PathBuf = OsStr::new(item).into();
            if is_cf_v2_manifest(&path_buf) {
                cf_v2_files.push(item.to_string());
            } else if is_cf_v2_config_values(&path_buf) {
                cf_v2_config_files.push(item.to_string());
            }
        }
    }

    // Meta files exist directly in the FAR and aren't represented by blobs.
    // Create a separate list of file names to read to avoid borrow checker
    // issues while iterating through the list.
    let meta_paths: Vec<String> = far_reader.list().map(|item| item.path().to_string()).collect();
    for meta_path in meta_paths.iter() {
        let meta_bytes = far_reader.read_file(meta_path).with_context(|| {
            format!("Failed to read file {} from meta.far for package", meta_path)
        })?;
        pkg_def.meta.insert(meta_path.into(), meta_bytes);
    }

    if contains_meta_contents {
        let content_bytes = far_reader
            .read_file(&META_CONTENTS_PATH)
            .context("Failed to read file meta/contents from meta.far for package")?;
        let content_str = std::str::from_utf8(&content_bytes)
            .context("Failed decode file meta/contents as UTF8-encoded string")?;
        pkg_def.contents = parse_key_value(content_str).with_context(|| {
            format!("Failed to parse file meta/contents for package as path=merkle pairs; file contents:\n\"\"\"\n{}\n\"\"\"", content_str)
        })?
        .into_iter()
        .map(|(key, value)| {
            let (path, hash) = (PathBuf::from(key), Hash::from_str(&value)?);
            Ok((path, hash))
        }).collect::<Result<Vec<(PathBuf, Hash)>>>().with_context(|| {
            format!("Failed to parse (path,hash) pairs from file meta/contents for package as path=merkle pairs; file contents:\n\"\"\"\n{}\n\"\"\"", content_str)
        })?
        .into_iter()
        .collect();
    }

    // CF manifest files are encoded with persistent FIDL.
    for cm_file in cf_v2_files.iter() {
        let decl_bytes = far_reader.read_file(cm_file).with_context(|| {
            format!("Failed to read file {} from meta.far for package", cm_file)
        })?;
        pkg_def.cms.insert(cm_file.into(), ComponentManifest::from(decl_bytes));
    }

    for cvf_file in &cf_v2_config_files {
        let values_bytes = far_reader.read_file(cvf_file).with_context(|| {
            format!("Failed to read file {} from meta.far for package", cvf_file)
        })?;
        pkg_def.cvfs.insert(cvf_file.into(), values_bytes);
    }

    Ok(pkg_def)
}

#[cfg(test)]
mod tests {
    use super::{PackageReader, PackagesFromUpdateReader};
    use crate::package_types::ComponentManifest;
    use fuchsia_archive::write;
    use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
    use fuchsia_url::{PackageName, PackageVariant};
    use scrutiny_testing::TEST_REPO_URL;
    use scrutiny_testing::artifact::MockArtifactReader;
    use scrutiny_utils::package::META_CONTENTS_PATH;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::{Cursor, Read};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use tempfile::tempdir;
    use update_package::serialize_packages_json;

    fn fake_update_package<P: AsRef<Path>>(
        path: P,
        pkg_urls: &[PinnedAbsolutePackageUrl],
    ) -> MockArtifactReader {
        let packages_json_contents = serialize_packages_json(pkg_urls).unwrap();
        let packages_json_merkle = fuchsia_merkle::root_from_slice(&packages_json_contents);
        let meta_contents_string = format!("packages.json={}\n", packages_json_merkle);
        let meta_contents_str = &meta_contents_string;
        let meta_contents_bytes = meta_contents_str.as_bytes();
        let mut path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        path_content_map.insert(
            META_CONTENTS_PATH,
            (meta_contents_bytes.len() as u64, Box::new(meta_contents_bytes)),
        );
        let mut update_pkg = File::create(path).unwrap();
        write(&mut update_pkg, path_content_map).unwrap();

        let mut mock_artifact_reader = MockArtifactReader::new();
        mock_artifact_reader
            .append_artifact(&packages_json_merkle.to_string(), packages_json_contents);
        mock_artifact_reader
    }

    #[fuchsia::test]
    fn read_package_definition_ignores_invalid_files() {
        // `meta/foo.cm`
        let foo_bytes = "foo".as_bytes();

        // `meta/bar.cm`
        let bar_bytes = "bar".as_bytes();

        // `meta/baz` and `grr.cm` with the same content.
        let baz_bytes = "baz\n".as_bytes();

        // Reuse paths "a", "c", and "1" as content of non-meta test files.
        let a_str = "a";
        let c_str = "c";
        let one_str = "1";
        let a_path = PathBuf::from(a_str);
        let c_path = PathBuf::from(c_str);
        let one_path = PathBuf::from(one_str);
        let a_hash = fuchsia_merkle::root_from_slice(a_str);
        let c_hash = fuchsia_merkle::root_from_slice(c_str);
        let one_hash = fuchsia_merkle::root_from_slice(one_str);
        let meta_contents_string =
            format!("{}={}\n{}={}\n{}={}\n", a_str, a_hash, c_str, c_hash, one_str, one_hash);
        let meta_contents_str = &meta_contents_string;
        let meta_contents_bytes = meta_contents_str.as_bytes();

        let mut path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        path_content_map.insert(
            META_CONTENTS_PATH,
            (meta_contents_bytes.len() as u64, Box::new(meta_contents_bytes)),
        );

        let meta_foo_cm_str = "meta/foo.cm";
        let meta_bar_cm_str = "meta/bar.cm";
        let meta_baz_str = "meta/baz";
        let grr_cm_str = "meta.cm";
        let meta_foo_cm_path = PathBuf::from(meta_foo_cm_str);
        let meta_bar_cm_path = PathBuf::from(meta_bar_cm_str);
        // No expectations require construction of a `meta_baz_path` or `grr_cm_path`.
        path_content_map.insert(meta_foo_cm_str, (foo_bytes.len() as u64, Box::new(foo_bytes)));
        path_content_map.insert(meta_bar_cm_str, (bar_bytes.len() as u64, Box::new(bar_bytes)));
        path_content_map.insert(meta_baz_str, (baz_bytes.len() as u64, Box::new(baz_bytes)));
        path_content_map.insert(grr_cm_str, (baz_bytes.len() as u64, Box::new(baz_bytes)));

        // Construct package named `foo`.
        let mut target = Cursor::new(Vec::new());
        write(&mut target, path_content_map).unwrap();
        let pkg_contents = target.get_ref();
        let pkg_merkle = fuchsia_merkle::root_from_slice(pkg_contents);
        let pkg_url = PinnedAbsolutePackageUrl::new(
            TEST_REPO_URL.clone(),
            PackageName::from_str("foo").unwrap(),
            Some(PackageVariant::zero()),
            pkg_merkle,
        );

        // Fake update package designates `foo` package defined above.
        let temp_dir = tempdir().unwrap();
        let update_pkg_path = temp_dir.path().join("update.far");
        let mut mock_artifact_reader =
            fake_update_package(&update_pkg_path, vec![pkg_url.clone()].as_slice());

        // Add all artifacts to the test artifact reader.
        mock_artifact_reader.append_artifact(&a_hash.to_string(), Vec::from(a_str.as_bytes()));
        mock_artifact_reader.append_artifact(&c_hash.to_string(), Vec::from(c_str.as_bytes()));
        mock_artifact_reader.append_artifact(&one_hash.to_string(), Vec::from(one_str.as_bytes()));
        mock_artifact_reader.append_artifact(&pkg_merkle.to_string(), pkg_contents.clone());

        let mut pkg_reader =
            PackagesFromUpdateReader::new(&update_pkg_path, Box::new(mock_artifact_reader));

        let result = pkg_reader.read_package_definition(&pkg_url).unwrap();
        assert_eq!(result.contents.len(), 3);
        assert_eq!(result.contents[&a_path], a_hash);
        assert_eq!(result.contents[&c_path], c_hash);
        assert_eq!(result.contents[&one_path], one_hash);
        assert_eq!(result.cms.len(), 2);
        if let ComponentManifest::Version2(b) = &result.cms[&meta_foo_cm_path] {
            assert_eq!(b, "foo".as_bytes());
        } else {
            panic!("Expected foo manifest to be Some()");
        }

        if let ComponentManifest::Version2(b) = &result.cms[&meta_bar_cm_path] {
            assert_eq!(b, "bar".as_bytes());
        } else {
            panic!("Expected bar manifest to be Some()");
        }
    }
}

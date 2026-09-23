// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use fuchsia_hash::Hash;
use fuchsia_pkg::package_sets::{PackageMap, PackageProperties, PackageSetType};
use fuchsia_pkg::{PackageManifest, PackagePath};
use fuchsia_url::RepositoryUrl;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

/// `WritablePackageList` represents a collection of packages that can be populated and
/// written into a file. This allows for gradual migration for packages index config
/// files (boot, base, cache) to JSON incrementally.
/// TODO(https://fxbug.dev/42176515): refactor out once base_packages are migrated to JSON format.
pub trait WritablePackageList {
    /// Add a new package with `name` and `merkle`.
    fn insert(
        &mut self,
        repository: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        merkle: Hash,
        package_set_type: Option<PackageSetType>,
    ) -> Result<()>;
    /// Generate the file to be used as the package index.
    fn write(&self, out: &mut impl Write) -> Result<()>;

    /// Returns whether the list has contents to write.
    fn is_empty(&self) -> bool;

    /// Pulls out the path and merkle from `package` and adds it to `packages` with a path to
    /// merkle mapping. This is a convenience function to simplify the case for implementations
    /// unaware of package set types.
    fn add_package(&mut self, package: PackageManifest) -> Result<()> {
        self.add_package_with_package_set(package, None)
    }

    /// Pulls out the path and merkle from `package` and adds it to `packages` with a path to
    /// merkle mapping.
    fn add_package_with_package_set(
        &mut self,
        package: PackageManifest,
        package_set_type: Option<PackageSetType>,
    ) -> Result<()> {
        let package_name = package.name().as_ref();
        if package_name == "system_image" || package_name == "update" {
            return Err(anyhow!("system_image and update packages are not allowed"));
        }

        let package_repository = package.repository();
        let path = package.package_path().to_string();
        package
            .blobs()
            .iter()
            .find(|blob| blob.path == "meta/")
            .ok_or_else(|| {
                anyhow!("Failed to add package {} to the list, unable to find meta blob", path)
            })
            .and_then(|meta_blob| {
                self.insert(package_repository, path, meta_blob.merkle, package_set_type)
            })
    }

    /// Helper fn to handle the (repeated) process of writing a list of packages
    /// out to the expected file, and returning a (destination, source) tuple
    /// for inclusion in the package's contents.
    fn write_index_file(
        &self,
        gendir: impl AsRef<Path>,
        name: &str,
        destination: impl AsRef<str>,
    ) -> Result<(String, String)> {
        // TODO(https://fxbug.dev/42156218) Decide on a consistent pattern for using gendir and
        //   how intermediate files should be named and where in gendir they should
        //   be placed.
        //
        // For a file of destination "data/foo.txt", and a gendir of "assembly/gendir",
        //   this creates "assembly/gendir/data/foo.txt".
        let path = gendir.as_ref().join(destination.as_ref());
        let path_str = path.to_str().ok_or_else(|| {
            anyhow!(format!("package index path is not valid UTF-8: {}", path.display()))
        })?;

        // Create any parent dirs necessary.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context(format!(
                "Failed to create parent dir {} for {} in gendir",
                parent.display(),
                destination.as_ref()
            ))?;
        }

        let mut index_file = File::create(&path)
            .context(format!("Failed to create the {} packages index file: {}", name, path_str))?;

        self.write(&mut index_file).context(format!(
            "Failed to write the {} package index file: {}",
            name,
            path.display()
        ))?;

        Ok((destination.as_ref().to_string(), path_str.to_string()))
    }
}

/// A list of mappings between package name and merkle, which can be written to
/// a file to be used as a package index.
#[derive(Default, Debug)]
pub struct PackageList {
    // Map between package name and merkle.
    packages: BTreeMap<String, Hash>,
}

impl WritablePackageList for PackageList {
    /// Add a new package with `name` and `merkle`.
    fn insert(
        &mut self,
        _repository: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        merkle: Hash,
        _package_set_type: Option<PackageSetType>,
    ) -> Result<()> {
        self.packages.insert(name.as_ref().to_string(), merkle);
        Ok(())
    }

    /// Generate the file to be used as a package index.
    fn write(&self, out: &mut impl Write) -> Result<()> {
        for (name, merkle) in self.packages.iter() {
            writeln!(out, "{}={}", name, merkle)?;
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

/// A list of package URLs pinned to a hash, which can be written to a file.
#[derive(Default, Debug)]
pub struct PackageUrlList {
    /// Using a BTreeSet to ensure output consistency, i.e. order of output package list is not
    /// subject to insertion order.
    packages: BTreeSet<PinnedAbsolutePackageUrl>,
}

impl PackageUrlList {
    /// Returns a reference to the list absolute package urls
    /// that this instance contains.
    pub fn get_packages(&self) -> Vec<&PinnedAbsolutePackageUrl> {
        return self.packages.iter().collect();
    }
}

impl WritablePackageList for PackageUrlList {
    /// Insert a new pinned to hash URL into the list.
    fn insert(
        &mut self,
        repository: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        merkle: Hash,
        _package_set_type: Option<PackageSetType>,
    ) -> Result<()> {
        let repository = repository
            .ok_or_else(|| anyhow!("Unable to create package url: empty repository field."))?;
        let path = PackagePath::from_str(name.as_ref())
            .map_err(|e| anyhow!("Failed to parse package path: {}", e))?;
        let url = PinnedAbsolutePackageUrl::new_with_path(
            RepositoryUrl::parse_host(repository.as_ref().to_string())
                .context("Failed to create repository url")?,
            &path.to_string(),
            merkle,
        )
        .map_err(|e| anyhow!("Failed to create package url: {}", e))?;
        self.packages.insert(url);
        Ok(())
    }

    /// Generate the file to be placed in the Base Package.
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        // If there are no packages, we should generate an empty file.
        if self.packages.is_empty() {
            return Ok(());
        }
        let contents = json!({
            "version": "1",
            "content": &self.packages,
        });
        serde_json::to_writer(writer, &contents).map_err(|e| anyhow!("Error writing JSON: {}", e))
    }

    fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

impl PartialEq<PackageList> for Vec<(String, Hash)> {
    fn eq(&self, other: &PackageList) -> bool {
        if self.len() == other.packages.len() {
            for item in self {
                match other.packages.get(&item.0) {
                    Some(hash) => {
                        if hash != &item.1 {
                            return false;
                        }
                    }
                    None => {
                        return false;
                    }
                }
            }
            return true;
        }
        return false;
    }
}

/// A map of package URLs pinned to a hash corresponding to sets, which can be written to a file.
/// This map can contain all pinned packages known to a system, but is initially used to implement
/// anchored packages as per RFC-0271.
#[derive(Default, Debug, PartialEq)]
pub struct PackageSetMap {
    packages: PackageMap,
}

impl WritablePackageList for PackageSetMap {
    fn insert(
        &mut self,
        repository: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        merkle: Hash,
        package_set_type: Option<PackageSetType>,
    ) -> Result<()> {
        // Verify if a PinnedAbsolutePackageUrl can be constructed from the given info.
        // If not, we will return an error.
        let repository =
            repository.context(format!("Unable to create package url: empty repository field."))?;
        let path = PackagePath::from_str(name.as_ref())
            .map_err(|e| anyhow!("Failed to parse package path: {}", e))?;
        let url = PinnedAbsolutePackageUrl::new_with_path(
            RepositoryUrl::parse_host(repository.as_ref().to_string())
                .context("Failed to create repository url")?,
            &path.to_string(),
            merkle,
        )
        .map_err(|e| anyhow!("Failed to create package url: {}", e))?;
        let package_set_type =
            package_set_type.context(format!("No package set type given for {}", url))?;
        let (u, h) = url.into_unpinned_and_hash();
        let map = self.packages.entry(package_set_type).or_insert_with(BTreeMap::new);
        if map.contains_key(&u) {
            return Err(anyhow!("Duplicate insert is not supported. Offending package URL: {}", u));
        }
        map.insert(u, PackageProperties { hash: h });
        Ok(())
    }

    fn write(&self, writer: &mut impl Write) -> Result<()> {
        serde_json::to_writer(writer, &self.packages)
            .map_err(|e| anyhow!("Error writing JSON: {}", e))
    }
    fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_pkg::package_sets::AnchoredPackageSetType;
    use fuchsia_pkg::{BlobInfo, MetaPackage, PackageManifestBuilder};

    #[test]
    fn package_list() {
        let mut out: Vec<u8> = Vec::new();
        let mut packages = PackageList::default();
        packages
            .insert(Some("testrepository.com"), "package2", Hash::from([34u8; 32]), None)
            .unwrap();
        packages
            .insert(Some("testrepository.com"), "package0", Hash::from([0u8; 32]), None)
            .unwrap();
        packages
            .insert(Some("testrepository.com"), "package1", Hash::from([17u8; 32]), None)
            .unwrap();
        packages.write(&mut out).unwrap();
        assert_eq!(
            b"package0=0000000000000000000000000000000000000000000000000000000000000000\n\
                    package1=1111111111111111111111111111111111111111111111111111111111111111\n\
                    package2=2222222222222222222222222222222222222222222222222222222222222222\n",
            &*out
        );
    }

    #[test]
    fn package_url_list() {
        let mut out: Vec<u8> = Vec::new();
        let mut packages = PackageUrlList::default();
        packages
            .insert(Some("testrepository.com"), "package2/0", Hash::from([34u8; 32]), None)
            .unwrap();
        packages
            .insert(Some("testrepository.com"), "package0/0", Hash::from([0u8; 32]), None)
            .unwrap();
        packages
            .insert(Some("testrepository.com"), "package1/0", Hash::from([17u8; 32]), None)
            .unwrap();
        packages.write(&mut out).unwrap();
        assert_eq!(
            br#"{"version":"1","content":["fuchsia-pkg://testrepository.com/package0/0?hash=0000000000000000000000000000000000000000000000000000000000000000","fuchsia-pkg://testrepository.com/package1/0?hash=1111111111111111111111111111111111111111111111111111111111111111","fuchsia-pkg://testrepository.com/package2/0?hash=2222222222222222222222222222222222222222222222222222222222222222"]}"#,
            &*out
        );
    }

    #[test]
    fn package_set_map() {
        let mut out: Vec<u8> = Vec::new();
        let mut packages = PackageSetMap::default();
        packages
            .insert(
                Some("testrepository.com"),
                "package2/0",
                Hash::from([34u8; 32]),
                Some(PackageSetType::Anchored(AnchoredPackageSetType::Automatic)),
            )
            .unwrap();
        packages
            .insert(
                Some("testrepository.com"),
                "package0/0",
                Hash::from([0u8; 32]),
                Some(PackageSetType::Anchored(AnchoredPackageSetType::OnDemand)),
            )
            .unwrap();
        packages
            .insert(
                Some("testrepository.com"),
                "package1/0",
                Hash::from([17u8; 32]),
                Some(PackageSetType::Anchored(AnchoredPackageSetType::Permanent)),
            )
            .unwrap();
        packages.write(&mut out).unwrap();
        let json = r#" {
            "anchored_automatic": {
                "fuchsia-pkg://testrepository.com/package2/0": {
                    "hash": "2222222222222222222222222222222222222222222222222222222222222222"
                }
            },
            "anchored_on_demand": {
                "fuchsia-pkg://testrepository.com/package0/0": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000"
                }
            },
             "anchored_permanent": {
                "fuchsia-pkg://testrepository.com/package1/0": {
                    "hash": "1111111111111111111111111111111111111111111111111111111111111111"
                }
            }
        }"#;
        let got: PackageMap = serde_json::from_str(json).unwrap();
        assert_eq!(packages, PackageSetMap { packages: got });
    }

    #[test]
    fn empty_package_url_list() {
        let mut out: Vec<u8> = Vec::new();
        let packages = PackageUrlList::default();
        packages.write(&mut out).unwrap();
        assert_eq!(b"", &*out);
    }

    #[test]
    fn empty_package_set_map() {
        let mut out: Vec<u8> = Vec::new();
        let packages = PackageSetMap::default();
        packages.write(&mut out).unwrap();
        assert_eq!("{}", String::from_utf8(out).unwrap());
    }

    #[test]
    fn test_add_package_to() {
        let system_image = generate_test_manifest("system_image", None);
        let update = generate_test_manifest("update", None);
        let valid = generate_test_manifest("valid", None);
        let mut packages = PackageList::default();
        assert!(WritablePackageList::add_package(&mut packages, system_image).is_err());
        assert!(WritablePackageList::add_package(&mut packages, update).is_err());
        assert!(WritablePackageList::add_package(&mut packages, valid).is_ok());
    }

    #[test]
    fn test_add_package_to_url_list() {
        let system_image = generate_test_manifest("system_image", None);
        let update = generate_test_manifest("update", None);
        let valid = generate_test_manifest("valid", None);
        let mut packages = PackageUrlList::default();
        assert!(WritablePackageList::add_package(&mut packages, system_image).is_err());
        assert!(WritablePackageList::add_package(&mut packages, update).is_err());
        assert!(WritablePackageList::add_package(&mut packages, valid).is_ok());
    }

    #[test]
    fn test_add_package_to_package_set_map() {
        let system_image = generate_test_manifest("system_image", None);
        let update = generate_test_manifest("update", None);
        let valid = generate_test_manifest("valid", None);
        let mut packages = PackageSetMap::default();
        assert!(WritablePackageList::add_package(&mut packages, system_image).is_err());
        assert!(WritablePackageList::add_package(&mut packages, update).is_err());
        // Unlike PackageList and PackageUrlList, PackageSetMap expects a package set type for each
        // package added.
        assert!(WritablePackageList::add_package(&mut packages, valid.clone()).is_err());
        assert!(
            WritablePackageList::add_package_with_package_set(
                &mut packages,
                valid,
                Some(PackageSetType::Anchored(AnchoredPackageSetType::Automatic))
            )
            .is_ok()
        );
    }

    // Generates a package manifest to be used for testing. The `name` is used in the blob file
    // names to make each manifest somewhat unique. If supplied, `file_path` will be used as the
    // non-meta-far blob source path, which allows the tests to use a real file.
    fn generate_test_manifest(name: &str, file_path: Option<&Path>) -> PackageManifest {
        let meta_source = format!("path/to/{}/meta.far", name);
        let file_source = match file_path {
            Some(path) => path.to_string_lossy().into_owned(),
            _ => format!("path/to/{}/file.txt", name),
        };
        let builder = PackageManifestBuilder::new(MetaPackage::from_name_and_variant_zero(
            name.parse().unwrap(),
        ));
        let builder = builder.repository("testrepository.com");
        let builder = builder.add_blob(BlobInfo {
            source_path: meta_source,
            path: "meta/".into(),
            merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
            size: 1,
        });
        let builder = builder.add_blob(BlobInfo {
            source_path: file_source,
            path: "data/file.txt".into(),
            merkle: "1111111111111111111111111111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            size: 1,
        });
        builder.build()
    }
}

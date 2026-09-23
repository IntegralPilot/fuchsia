// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::PackageManifest;
use anyhow::{Context as _, Result};
use camino::Utf8PathBuf;
use fuchsia_merkle::Hash;
use fuchsia_url::RelativePackageUrl;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::io;

/// Helper type for reading the build-time information based on the subpackage
/// declarations declared in a build file (such as the `subpackages` list in
/// a `fuchsia_package()` target, in `BUILD.gn`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubpackagesBuildManifest(SubpackagesBuildManifestV0);

impl SubpackagesBuildManifest {
    /// Return the subpackage manifest entries.
    pub fn entries(&self) -> &[SubpackagesBuildManifestEntry] {
        &self.0.entries
    }

    /// Open up each entry in the manifest and return the subpackage url and hash.
    pub fn to_subpackages(&self) -> Result<Vec<(RelativePackageUrl, Hash, Utf8PathBuf)>> {
        let mut entries = Vec::with_capacity(self.0.entries.len());
        for entry in &self.0.entries {
            // Read the merkle from the package manifest.
            let manifest = PackageManifest::try_load_from(&entry.package_manifest_file)
                .with_context(|| format!("reading {}", entry.package_manifest_file))?;
            let meta_far_blob = manifest
                .blobs()
                .iter()
                .find(|b| b.path == "meta/")
                .with_context(|| format!("finding meta/in {}", entry.package_manifest_file))?;

            // Read the name from the package manifest, but override with the name if provided.
            let url = match &entry.kind {
                SubpackagesBuildManifestEntryKind::Url(url) => url.clone(),
                SubpackagesBuildManifestEntryKind::Empty
                | SubpackagesBuildManifestEntryKind::MetaPackageFile(_) => {
                    manifest.name().clone().into()
                }
            };

            entries.push((url, meta_far_blob.merkle, entry.package_manifest_file.clone()));
        }
        Ok(entries)
    }

    /// Deserializes a `SubpackagesBuildManifest` from json.
    pub fn deserialize(reader: impl io::BufRead) -> Result<Self> {
        Ok(SubpackagesBuildManifest(serde_json::from_reader(reader)?))
    }

    /// Serializes a `SubpackagesBuildManifest` to json.
    pub fn serialize(&self, writer: impl io::Write) -> Result<()> {
        Ok(serde_json::to_writer(writer, &self.0.entries)?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SubpackagesBuildManifestV0 {
    entries: Vec<SubpackagesBuildManifestEntry>,
}

impl From<Vec<SubpackagesBuildManifestEntry>> for SubpackagesBuildManifest {
    fn from(entries: Vec<SubpackagesBuildManifestEntry>) -> Self {
        SubpackagesBuildManifest(SubpackagesBuildManifestV0 { entries })
    }
}

impl<'de> Deserialize<'de> for SubpackagesBuildManifestV0 {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(flatten)]
            helper_kind: Option<HelperKind>,
            package_manifest_file: Utf8PathBuf,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HelperKind {
            Name { name: RelativePackageUrl },
            File { meta_package_file: Utf8PathBuf },
        }

        let manifest_entries = Vec::<Helper>::deserialize(deserializer)?;

        let mut entries = vec![];
        for Helper { helper_kind, package_manifest_file } in manifest_entries {
            let kind = match helper_kind {
                Some(HelperKind::Name { name }) => SubpackagesBuildManifestEntryKind::Url(name),
                Some(HelperKind::File { meta_package_file }) => {
                    SubpackagesBuildManifestEntryKind::MetaPackageFile(meta_package_file)
                }
                None => SubpackagesBuildManifestEntryKind::Empty,
            };
            entries.push(SubpackagesBuildManifestEntry { kind, package_manifest_file });
        }

        Ok(SubpackagesBuildManifestV0 { entries })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubpackagesBuildManifestEntry {
    /// The subpackages build manifest entry's [EntryKind].
    pub kind: SubpackagesBuildManifestEntryKind,

    /// The package_manifest.json of the subpackage.
    pub package_manifest_file: Utf8PathBuf,
}

impl Serialize for SubpackagesBuildManifestEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            name: Option<&'a RelativePackageUrl>,
            #[serde(skip_serializing_if = "Option::is_none")]
            meta_package_file: Option<&'a Utf8PathBuf>,
            package_manifest_file: &'a Utf8PathBuf,
        }
        let mut helper = Helper {
            name: None,
            meta_package_file: None,
            package_manifest_file: &self.package_manifest_file,
        };
        match &self.kind {
            SubpackagesBuildManifestEntryKind::Empty => {}
            SubpackagesBuildManifestEntryKind::Url(url) => helper.name = Some(url),
            SubpackagesBuildManifestEntryKind::MetaPackageFile(path) => {
                helper.meta_package_file = Some(path)
            }
        }
        helper.serialize(serializer)
    }
}

impl SubpackagesBuildManifestEntry {
    /// Construct a new [SubpackagesBuildManifestEntry].
    pub fn new(
        kind: SubpackagesBuildManifestEntryKind,
        package_manifest_file: Utf8PathBuf,
    ) -> Self {
        Self { kind, package_manifest_file }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubpackagesBuildManifestEntryKind {
    Empty,
    Url(RelativePackageUrl),
    MetaPackageFile(Utf8PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetaPackage, PackageBuilder};
    use assert_matches::assert_matches;
    use camino::Utf8Path;
    use fuchsia_url::PackageName;
    use serde_json::json;
    use std::fs::File;

    const FAKE_ABI_REVISION: version_history::AbiRevision =
        version_history::AbiRevision::from_u64(0xdc6e746980ce30a9);

    #[test]
    fn test_deserialize() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Generate a subpackages build manifest.
        let pkg1_name = PackageName::try_from("pkg1".to_string()).unwrap();
        let pkg1_url = RelativePackageUrl::from(pkg1_name.clone());
        let pkg1_package_manifest_file = dir.join("pkg1-package_manifest.json");
        let pkg1_meta_far_file = dir.join("pkg1-meta.far");

        let pkg2_name = PackageName::try_from("pkg2".to_string()).unwrap();
        let pkg2_url = RelativePackageUrl::from(pkg2_name.clone());
        let pkg2_meta_package_file = dir.join("pkg2-meta-package");
        let pkg2_package_manifest_file = dir.join("pkg2-package_manifest.json");
        let pkg2_meta_far_file = dir.join("pkg2-meta.far");

        // Write out all the files.
        let meta_package = MetaPackage::from_name_and_variant_zero(pkg2_name.clone());
        meta_package.serialize(File::create(&pkg2_meta_package_file).unwrap()).unwrap();

        let mut builder = PackageBuilder::new(&pkg1_name, FAKE_ABI_REVISION);
        builder.manifest_path(&pkg1_package_manifest_file);
        let manifest = builder.build(dir, pkg1_meta_far_file).unwrap();
        let pkg1_hash = manifest.blobs().iter().find(|b| b.path == "meta/").unwrap().merkle;

        let mut builder = PackageBuilder::new(&pkg2_name, FAKE_ABI_REVISION);
        builder.manifest_path(&pkg2_package_manifest_file);
        let manifest = builder.build(dir, pkg2_meta_far_file).unwrap();
        let pkg2_hash = manifest.blobs().iter().find(|b| b.path == "meta/").unwrap().merkle;

        // Make sure we can deserialize from the manifest format.
        let subpackages_build_manifest_path = dir.join("subpackages-build-manifest");
        serde_json::to_writer(
            File::create(&subpackages_build_manifest_path).unwrap(),
            &json!([
                {
                    "package_manifest_file": pkg1_package_manifest_file.to_string(),
                },
                {
                    "name": pkg1_name.to_string(),
                    "package_manifest_file": pkg1_package_manifest_file.to_string(),
                },
                {
                    "meta_package_file": pkg2_meta_package_file.to_string(),
                    "package_manifest_file": pkg2_package_manifest_file.to_string(),
                },
            ]),
        )
        .unwrap();

        let manifest = SubpackagesBuildManifest::deserialize(io::BufReader::new(
            File::open(&subpackages_build_manifest_path).unwrap(),
        ))
        .unwrap();

        assert_eq!(
            manifest.0.entries,
            vec![
                SubpackagesBuildManifestEntry {
                    kind: SubpackagesBuildManifestEntryKind::Empty,
                    package_manifest_file: pkg1_package_manifest_file.clone(),
                },
                SubpackagesBuildManifestEntry {
                    kind: SubpackagesBuildManifestEntryKind::Url(pkg1_url.clone()),
                    package_manifest_file: pkg1_package_manifest_file.clone(),
                },
                SubpackagesBuildManifestEntry {
                    kind: SubpackagesBuildManifestEntryKind::MetaPackageFile(
                        pkg2_meta_package_file
                    ),
                    package_manifest_file: pkg2_package_manifest_file.clone(),
                },
            ]
        );

        // Make sure we can convert the manifest into subpackages.
        assert_eq!(
            manifest.to_subpackages().unwrap(),
            vec![
                (pkg1_url.clone(), pkg1_hash, pkg1_package_manifest_file.clone()),
                (pkg1_url, pkg1_hash, pkg1_package_manifest_file),
                (pkg2_url, pkg2_hash, pkg2_package_manifest_file),
            ]
        );
    }

    #[test]
    fn test_meta_package_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_meta_package_file = dir.join("pkg-meta-package");
        let pkg_package_manifest_file = dir.join("package_manifest.json");
        let pkg_meta_far_file = dir.join("meta.far");

        let subpackages_build_manifest_path = dir.join("subpackages-build-manifest");
        serde_json::to_writer(
            File::create(&subpackages_build_manifest_path).unwrap(),
            &json!([
                {
                    "meta_package_file": pkg_meta_package_file.to_string(),
                    "package_manifest_file": pkg_package_manifest_file.to_string(),
                },
            ]),
        )
        .unwrap();

        let manifest = SubpackagesBuildManifest::deserialize(io::BufReader::new(
            File::open(&subpackages_build_manifest_path).unwrap(),
        ))
        .unwrap();

        // We should error out if the package manifest file doesn't exist.
        assert_matches!(
            manifest.to_subpackages(),
            Err(err) if err.downcast_ref::<io::Error>().unwrap().kind() == io::ErrorKind::NotFound
        );

        // It should work once we write the files.
        let mut builder = PackageBuilder::new("pkg", FAKE_ABI_REVISION);
        builder.manifest_path(&pkg_package_manifest_file);
        let package_manifest = builder.build(dir, pkg_meta_far_file).unwrap();
        let pkg_hash = package_manifest.blobs().iter().find(|b| b.path == "meta/").unwrap().merkle;

        let pkg_name = PackageName::try_from("pkg".to_string()).unwrap();
        MetaPackage::from_name_and_variant_zero(pkg_name.clone())
            .serialize(File::create(&pkg_meta_package_file).unwrap())
            .unwrap();
        let pkg_url = RelativePackageUrl::from(pkg_name);

        assert_eq!(
            manifest.to_subpackages().unwrap(),
            vec![(pkg_url, pkg_hash, pkg_package_manifest_file)]
        );
    }

    #[test]
    fn test_merkle_file_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_name = PackageName::try_from("pkg".to_string()).unwrap();
        let pkg_url = RelativePackageUrl::from(pkg_name);
        let pkg_package_manifest_file = dir.join("package_manifest.json");
        let pkg_meta_far_file = dir.join("meta.far");

        let subpackages_build_manifest_path = dir.join("subpackages-build-manifest");
        serde_json::to_writer(
            File::create(&subpackages_build_manifest_path).unwrap(),
            &json!([
                {
                    "name": pkg_url.to_string(),
                    "package_manifest_file": pkg_package_manifest_file.to_string(),
                },
            ]),
        )
        .unwrap();

        let manifest = SubpackagesBuildManifest::deserialize(io::BufReader::new(
            File::open(&subpackages_build_manifest_path).unwrap(),
        ))
        .unwrap();

        // We should error out if the package manifest file doesn't exist.
        assert_matches!(
            manifest.to_subpackages(),
            Err(err) if err.downcast_ref::<io::Error>().unwrap().kind() == io::ErrorKind::NotFound
        );

        // It should work once we write the files.
        let mut builder = PackageBuilder::new("pkg", FAKE_ABI_REVISION);
        builder.manifest_path(&pkg_package_manifest_file);
        let package_manifest = builder.build(dir, pkg_meta_far_file).unwrap();
        let pkg_hash = package_manifest.blobs().iter().find(|b| b.path == "meta/").unwrap().merkle;

        assert_eq!(
            manifest.to_subpackages().unwrap(),
            vec![(pkg_url, pkg_hash, pkg_package_manifest_file)]
        );
    }

    #[test]
    fn test_serialize() {
        let entries = vec![
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::Empty,
                "package-manifest-path-0".into(),
            ),
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::Url("subpackage-name".parse().unwrap()),
                "package-manifest-path-0".into(),
            ),
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::MetaPackageFile("file-path".into()),
                "package-manifest-path-1".into(),
            ),
        ];
        let manifest = SubpackagesBuildManifest::from(entries);

        let mut bytes = vec![];
        let () = manifest.serialize(&mut bytes).unwrap();
        let actual_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            actual_json,
            json!([
                {
                    "package_manifest_file": "package-manifest-path-0"
                },
                {
                    "name": "subpackage-name",
                    "package_manifest_file": "package-manifest-path-0"
                },
                {
                    "meta_package_file": "file-path",
                    "package_manifest_file": "package-manifest-path-1"
                },
            ])
        );
    }

    #[test]
    fn test_serialize_deserialize() {
        let entries = vec![
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::Empty,
                "package-manifest-path-0".into(),
            ),
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::Url("subpackage-name".parse().unwrap()),
                "package-manifest-path-0".into(),
            ),
            SubpackagesBuildManifestEntry::new(
                SubpackagesBuildManifestEntryKind::MetaPackageFile("file-path".into()),
                "package-manifest-path-1".into(),
            ),
        ];
        let manifest = SubpackagesBuildManifest::from(entries);

        let mut bytes = vec![];
        let () = manifest.serialize(&mut bytes).unwrap();
        let deserialized =
            SubpackagesBuildManifest::deserialize(io::BufReader::new(bytes.as_slice())).unwrap();

        assert_eq!(deserialized, manifest);
    }
}

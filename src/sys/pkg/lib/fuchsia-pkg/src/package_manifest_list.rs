// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::RelativeTo;
use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::fs::{File, create_dir_all};
use std::{slice, vec};
use utf8_path::{path_relative_from_file, resolve_path_from_file};

/// [PackageManifestList] is a construct that points at a path that contains a
/// package manifest list. This will be used by the packaging tooling to
/// understand when packages have changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PackageManifestList(VersionedPackageManifestList);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "version", content = "content", deny_unknown_fields)]
enum VersionedPackageManifestList {
    #[serde(rename = "1")]
    Version1(PackageManifestListV1),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct PackageManifestListV1 {
    /// Are the blob source_paths relative to the working dir (default, as made
    /// by 'pm') or the file containing the serialized manifest (new, portable,
    /// behavior)
    #[serde(default, skip_serializing_if = "RelativeTo::is_default")]
    paths_relative: RelativeTo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    manifests: Vec<Utf8PathBuf>,
}

impl PackageManifestList {
    /// Construct a new [PackageManifestList].
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
            manifests: vec![],
            paths_relative: RelativeTo::default(),
        }))
    }

    /// Push a package manifest path to the end of the [PackageManifestList].
    pub fn push(&mut self, package_manifest_path: Utf8PathBuf) {
        match &mut self.0 {
            VersionedPackageManifestList::Version1(package_manifest_list_v1) => {
                package_manifest_list_v1.manifests.push(package_manifest_path)
            }
        }
    }

    /// Returns an iterator over the package manifest path entries.
    pub fn iter(&self) -> Iter<'_> {
        match &self.0 {
            VersionedPackageManifestList::Version1(package_manifest_list_v1) => {
                Iter(package_manifest_list_v1.manifests.iter())
            }
        }
    }

    pub fn from_reader(
        manifest_list_path: &Utf8Path,
        mut reader: impl std::io::Read,
    ) -> anyhow::Result<Self> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;

        let versioned_list = match serde_json::from_slice(bytes.as_slice())? {
            VersionedPackageManifestList::Version1(manifest_list) => {
                VersionedPackageManifestList::Version1(
                    manifest_list.resolve_source_paths(manifest_list_path)?,
                )
            }
        };

        Ok(Self(versioned_list))
    }

    pub fn write_with_relative_paths(self, path: &Utf8Path) -> anyhow::Result<Self> {
        let versioned = match self.0 {
            VersionedPackageManifestList::Version1(package_manifest_list) => {
                VersionedPackageManifestList::Version1(
                    package_manifest_list.write_with_relative_paths(path)?,
                )
            }
        };

        Ok(PackageManifestList(versioned))
    }

    /// Write the package list manifest to this path.
    pub fn to_writer(&self, writer: impl std::io::Write) -> Result<(), std::io::Error> {
        serde_json::to_writer(writer, &self.0).unwrap();
        Ok(())
    }
}

impl PackageManifestListV1 {
    pub fn write_with_relative_paths(self, manifest_list_path: &Utf8Path) -> anyhow::Result<Self> {
        // If necessary, adjust manifest files to relative paths
        let manifest_list = if let RelativeTo::WorkingDir = &self.paths_relative {
            let manifests = self
                .manifests
                .into_iter()
                .map(|manifest_path| {
                    path_relative_from_file(manifest_path, manifest_list_path).unwrap()
                })
                .collect::<Vec<_>>();
            Self { manifests, paths_relative: RelativeTo::File }
        } else {
            self
        };

        let versioned_manifest = VersionedPackageManifestList::Version1(manifest_list.clone());

        create_dir_all(manifest_list_path.parent().unwrap())?;
        let file = File::create(manifest_list_path)?;
        serde_json::to_writer(file, &versioned_manifest)?;

        Ok(manifest_list)
    }

    pub fn resolve_source_paths(self, manifest_list_path: &Utf8Path) -> anyhow::Result<Self> {
        if let RelativeTo::File = self.paths_relative {
            let manifests = self
                .manifests
                .into_iter()
                .map(|manifest_path| {
                    resolve_path_from_file(manifest_path, manifest_list_path).unwrap()
                })
                .collect::<Vec<_>>();
            Ok(Self { manifests, ..self })
        } else {
            Ok(self)
        }
    }
}

impl IntoIterator for PackageManifestList {
    type Item = Utf8PathBuf;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        match self.0 {
            VersionedPackageManifestList::Version1(package_manifest_list_v1) => {
                IntoIter(package_manifest_list_v1.manifests.into_iter())
            }
        }
    }
}

impl From<Vec<Utf8PathBuf>> for PackageManifestList {
    fn from(package_manifest_list: Vec<Utf8PathBuf>) -> Self {
        Self(VersionedPackageManifestList::Version1(PackageManifestListV1 {
            manifests: package_manifest_list,
            paths_relative: RelativeTo::default(),
        }))
    }
}

impl From<PackageManifestList> for Vec<Utf8PathBuf> {
    fn from(package_manifest_list: PackageManifestList) -> Self {
        match package_manifest_list.0 {
            VersionedPackageManifestList::Version1(package_manifest_list_v1) => {
                package_manifest_list_v1.manifests
            }
        }
    }
}

impl FromIterator<Utf8PathBuf> for PackageManifestList {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Utf8PathBuf>,
    {
        Self(VersionedPackageManifestList::Version1(PackageManifestListV1 {
            manifests: iter.into_iter().collect(),
            paths_relative: RelativeTo::default(),
        }))
    }
}

/// Immutable iterator over the package manifest paths.
pub struct Iter<'a>(slice::Iter<'a, Utf8PathBuf>);

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Utf8PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// An iterator that moves out of the [PackageManifestList].
pub struct IntoIter(vec::IntoIter<Utf8PathBuf>);

impl Iterator for IntoIter {
    type Item = Utf8PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_serialize() {
        let package_manifest_list = PackageManifestList::from(vec![
            "obj/build/images/config-data/package_manifest.json".into(),
            "obj/build/images/shell-commands/package_manifest.json".into(),
            "obj/src/sys/component_index/component_index/package_manifest.json".into(),
        ]);

        assert_eq!(
            serde_json::to_value(package_manifest_list).unwrap(),
            json!(
                {
                    "content": {
                        "manifests": [
                            "obj/build/images/config-data/package_manifest.json",
                            "obj/build/images/shell-commands/package_manifest.json",
                            "obj/src/sys/component_index/component_index/package_manifest.json",
                        ]
                    },
                    "version": "1"
                }
            ),
        );
    }

    #[test]
    fn test_iter_from_reader_json_v1() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let manifest_dir = temp_dir.join("manifest_dir");
        let manifest_path = manifest_dir.join("package_manifest_list.json");
        std::fs::create_dir_all(&manifest_dir).unwrap();

        // RelativeTo::WorkingDir
        let raw_package_manifest_list_json_v1 = r#"{
            "version": "1",
            "content": {
                "paths_relative": "working_dir",
                "manifests": [
                    "obj/build/images/config-data/package_manifest.json",
                    "obj/build/images/shell-commands/package_manifest.json",
                    "obj/src/sys/component_index/component_index/package_manifest.json"
                ]
            }
        }"#;
        std::fs::write(&manifest_path, raw_package_manifest_list_json_v1).unwrap();

        let package_manifest_list =
            PackageManifestList::from_reader(&manifest_path, File::open(&manifest_path).unwrap())
                .unwrap();
        assert_eq!(
            PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
                paths_relative: RelativeTo::WorkingDir,
                manifests: vec![
                    "obj/build/images/config-data/package_manifest.json".into(),
                    "obj/build/images/shell-commands/package_manifest.json".into(),
                    "obj/src/sys/component_index/component_index/package_manifest.json".into(),
                ]
            })),
            package_manifest_list
        );

        assert_eq!(
            package_manifest_list.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                "obj/build/images/config-data/package_manifest.json",
                "obj/build/images/shell-commands/package_manifest.json",
                "obj/src/sys/component_index/component_index/package_manifest.json",
            ]
        );

        // RelativeTo::File
        let raw_package_manifest_list_json_v1 = r#"{
            "version": "1",
            "content": {
                "paths_relative": "file",
                "manifests": [
                    "obj/build/images/config-data/package_manifest.json",
                    "obj/build/images/shell-commands/package_manifest.json",
                    "obj/src/sys/component_index/component_index/package_manifest.json"
                ]
            }
        }"#;
        std::fs::write(&manifest_path, raw_package_manifest_list_json_v1).unwrap();

        let package_manifest_list =
            PackageManifestList::from_reader(&manifest_path, File::open(&manifest_path).unwrap())
                .unwrap();
        assert_eq!(
            PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
                paths_relative: RelativeTo::File,
                manifests: vec![
                    manifest_dir.join("obj/build/images/config-data/package_manifest.json"),
                    manifest_dir.join("obj/build/images/shell-commands/package_manifest.json"),
                    manifest_dir
                        .join("obj/src/sys/component_index/component_index/package_manifest.json"),
                ]
            })),
            package_manifest_list
        );

        assert_eq!(
            package_manifest_list.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                manifest_dir.join("obj/build/images/config-data/package_manifest.json"),
                manifest_dir.join("obj/build/images/shell-commands/package_manifest.json"),
                manifest_dir
                    .join("obj/src/sys/component_index/component_index/package_manifest.json"),
            ]
        );
    }

    #[test]
    fn test_to_writer() {
        let package_manifest_list = PackageManifestList::from(vec![
            "obj/build/images/config-data/package_manifest.json".into(),
            "obj/build/images/shell-commands/package_manifest.json".into(),
            "obj/src/sys/component_index/component_index/package_manifest.json".into(),
        ]);

        let mut out = vec![];
        package_manifest_list.to_writer(&mut out).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\
              \"version\":\"1\",\
              \"content\":{\
                  \"manifests\":[\
                      \"obj/build/images/config-data/package_manifest.json\",\
                      \"obj/build/images/shell-commands/package_manifest.json\",\
                      \"obj/src/sys/component_index/component_index/package_manifest.json\"\
                  ]\
              }\
          }"
        );
    }

    #[test]
    fn test_write_package_manifest_list_making_paths_relative() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let other_dir = temp_dir.join("other_dir");
        let manifest_dir = temp_dir.join("manifest_dir");
        let manifest_path = manifest_dir.join("package_manifest_list.json");

        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();

        let package_manifest_list =
            PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
                paths_relative: RelativeTo::WorkingDir,
                manifests: vec![
                    other_dir.join("foo.package_manifest.json"),
                    other_dir.join("bar.package_manifest.json"),
                ],
            }));

        // manifests will now have relative paths
        let result_manifest_list =
            package_manifest_list.write_with_relative_paths(&manifest_path).unwrap();

        // TODO(https://fxbug.dev/42077054): Add "re-read" test once writer is functioning.
        let path_relative_package_manifest_list =
            PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
                paths_relative: RelativeTo::File,
                manifests: vec![
                    "../other_dir/foo.package_manifest.json".into(),
                    "../other_dir/bar.package_manifest.json".into(),
                ],
            }));

        assert_eq!(result_manifest_list, path_relative_package_manifest_list);
    }

    #[test]
    fn test_write_package_manifest_list_already_relative() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let manifest_dir = temp_dir.join("manifest_dir");
        let manifest_path = manifest_dir.join("package_manifest_list.json");

        std::fs::create_dir_all(&manifest_dir).unwrap();

        let package_manifest_list =
            PackageManifestList(VersionedPackageManifestList::Version1(PackageManifestListV1 {
                paths_relative: RelativeTo::File,
                manifests: vec![
                    "../other_dir/foo.package_manifest.json".into(),
                    "../other_dir/bar.package_manifest.json".into(),
                ],
            }));

        // manifests will be untouched, as paths are already relative.
        let result_manifest_list =
            package_manifest_list.clone().write_with_relative_paths(&manifest_path).unwrap();

        // TODO(https://fxbug.dev/42077054): Add "re-read" test once writer is functioning.
        assert_eq!(result_manifest_list, package_manifest_list);
    }

    #[test]
    fn test_iter() {
        let package_manifest_list = PackageManifestList::from(vec![
            "obj/build/images/config-data/package_manifest.json".into(),
            "obj/build/images/shell-commands/package_manifest.json".into(),
            "obj/src/sys/component_index/component_index/package_manifest.json".into(),
        ]);

        assert_eq!(
            package_manifest_list.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                "obj/build/images/config-data/package_manifest.json",
                "obj/build/images/shell-commands/package_manifest.json",
                "obj/src/sys/component_index/component_index/package_manifest.json",
            ]
        );
    }

    #[test]
    fn test_into_iter() {
        let entries = vec![
            "obj/build/images/config-data/package_manifest.json".into(),
            "obj/build/images/shell-commands/package_manifest.json".into(),
            "obj/src/sys/component_index/component_index/package_manifest.json".into(),
        ];
        let package_manifest_list = PackageManifestList::from(entries.clone());

        assert_eq!(package_manifest_list.into_iter().collect::<Vec<_>>(), entries,);
    }
}

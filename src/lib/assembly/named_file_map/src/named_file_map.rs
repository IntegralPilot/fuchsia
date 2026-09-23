// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use assembly_constants::{BootfsDestination, Destination, FileEntry};
use assembly_util::{BTreeMapDuplicateKeyError, InsertUniqueExt, MapEntry, NamedMap};
use camino::Utf8PathBuf;
use fuchsia_merkle::Hash;
use fuchsia_pkg::BlobInfo;
use serde::Serialize;
use utf8_path::path_relative_from_current_dir;

/// A paired source and optional merkle for a file.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SourceMerklePair {
    /// The source path.
    pub source: Utf8PathBuf,
    /// The merkle of the file at the source path.
    pub merkle: Option<Hash>,
}

impl From<BlobInfo> for SourceMerklePair {
    fn from(blob: BlobInfo) -> Self {
        Self { source: blob.source_path.into(), merkle: Some(blob.merkle) }
    }
}

impl<D: Destination> From<FileEntry<D>> for SourceMerklePair {
    fn from(entry: FileEntry<D>) -> Self {
        Self { source: entry.source, merkle: None }
    }
}

fn relativize_entry<D: Destination>(entry: FileEntry<D>) -> Result<FileEntry<D>> {
    let source = path_relative_from_current_dir(&entry.source)
        .with_context(|| format!("relativizing path {}", entry.source))?;
    Ok(FileEntry { source, destination: entry.destination })
}

/// A named set of file entries, keyed by file destination.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NamedFileMap<D: Destination> {
    /// Map from destination path to source path and merkle.
    pub map: NamedMap<D, SourceMerklePair>,
}

impl<D: Destination> NamedFileMap<D> {
    /// Construct a NamedFileMap.
    pub fn new(name: &str) -> Self {
        NamedFileMap { map: NamedMap::new(name) }
    }

    /// Add a single FileEntry to the map, if the 'destination' path is a
    /// duplicate, return an error, otherwise add the entry.
    pub fn add_entry(&mut self, entry: FileEntry<D>) -> Result<()> {
        let entry = relativize_entry(entry)?;
        self.map
            .try_insert_unique(entry.destination.clone(), entry.into())
            .with_context(|| format!("Adding entry to set: {}", self.map.name))
    }

    /// Return the contents of the NamedFileMap as a Vec<FileEntry>.
    pub fn into_file_entries(self) -> Vec<FileEntry<D>> {
        self.map
            .entries
            .into_iter()
            .map(|(destination, SourceMerklePair { source, .. })| FileEntry { destination, source })
            .collect()
    }
}

impl NamedFileMap<BootfsDestination> {
    /// Add a single blob to the map. If the 'destination' path is a
    /// duplicate but the merkle is the same, do nothing. If the
    /// `destination' path is a duplicate and the merkle is different,
    /// return an error. Otherwise add the entry.
    pub fn add_blob(&mut self, destination: BootfsDestination, blob: BlobInfo) -> Result<()> {
        let result = self.map.entries.try_insert_unique(MapEntry(destination, blob.into()));
        match result {
            Ok(_) => Ok(()),
            Err(BTreeMapDuplicateKeyError { existing_entry, new_value }) => {
                let previous_value = existing_entry.get();
                if let (Some(previous_merkle), Some(new_merkle)) =
                    (previous_value.merkle, new_value.merkle)
                {
                    if previous_merkle == new_merkle {
                        return Ok(());
                    }
                }
                return Err(anyhow!(
                    r#"File has the same destination but a different merkle
            destination     = {}
            previous_merkle = {:?}
            merkle          = {:?}"#,
                    existing_entry.key(),
                    previous_value.merkle,
                    new_value.merkle
                ));
            }
        }
    }
}

impl<D: Destination> std::ops::Deref for NamedFileMap<D> {
    type Target = NamedMap<D, SourceMerklePair>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<D: Destination> std::ops::DerefMut for NamedFileMap<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_util::NamedMapKey;
    use serde::{Deserialize, Serialize};

    /// Destinations that can be used for testing.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum TestDestination {
        /// Test with "foo" path.
        Foo,
        /// Test with "bar" path.
        Bar,
    }

    impl NamedMapKey for TestDestination {}
    impl Destination for TestDestination {}

    impl std::fmt::Display for TestDestination {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let value = serde_json::to_value(self).expect("serialize enum");
            write!(f, "{}", value.as_str().expect("enum is str"))
        }
    }

    #[test]
    fn entries_diff_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_entry(FileEntry { source: "src1".into(), destination: TestDestination::Foo })
            .unwrap();
        map.add_entry(FileEntry { source: "src2".into(), destination: TestDestination::Bar })
            .unwrap();

        assert_eq!(
            vec![
                FileEntry { source: "src1".into(), destination: TestDestination::Foo },
                FileEntry { source: "src2".into(), destination: TestDestination::Bar },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn entries_same_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_entry(FileEntry { source: "src1".into(), destination: TestDestination::Foo })
            .unwrap();
        map.add_entry(FileEntry { source: "src1".into(), destination: TestDestination::Bar })
            .unwrap();

        assert_eq!(
            vec![
                FileEntry { source: "src1".into(), destination: TestDestination::Foo },
                FileEntry { source: "src1".into(), destination: TestDestination::Bar },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn entries_same_src_same_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_entry(FileEntry { source: "src1".into(), destination: TestDestination::Foo })
            .unwrap();
        let res =
            map.add_entry(FileEntry { source: "src1".into(), destination: TestDestination::Foo });
        assert!(res.is_err());

        assert_eq!(
            vec![FileEntry { source: "src1".into(), destination: TestDestination::Foo }],
            map.into_file_entries()
        );
    }

    #[test]
    fn blobs_diff_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        map.add_blob(
            BootfsDestination::FromAIB("dest2".into()),
            BlobInfo {
                source_path: "src2".into(),
                path: "dest2".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();

        assert_eq!(
            vec![
                FileEntry {
                    source: "src1".into(),
                    destination: BootfsDestination::FromAIB("dest1".into()),
                },
                FileEntry {
                    source: "src2".into(),
                    destination: BootfsDestination::FromAIB("dest2".into()),
                },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn blobs_same_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        map.add_blob(
            BootfsDestination::FromAIB("dest2".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest2".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();

        assert_eq!(
            vec![
                FileEntry {
                    source: "src1".into(),
                    destination: BootfsDestination::FromAIB("dest1".into()),
                },
                FileEntry {
                    source: "src1".into(),
                    destination: BootfsDestination::FromAIB("dest2".into()),
                },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn blobs_same_src_same_dest_same_merkle() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();

        assert_eq!(
            vec![FileEntry {
                source: "src1".into(),
                destination: BootfsDestination::FromAIB("dest1".into()),
            },],
            map.into_file_entries()
        );
    }

    #[test]
    fn blobs_same_src_same_dest_diff_merkle() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        let res = map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "1111111111111111111111111111111111111111111111111111111111111111"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        );
        assert!(res.is_err());

        assert_eq!(
            vec![FileEntry {
                source: "src1".into(),
                destination: BootfsDestination::FromAIB("dest1".into()),
            },],
            map.into_file_entries()
        );
    }

    #[test]
    fn blob_and_entry_diff_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        map.add_entry(FileEntry { source: "src2".into(), destination: BootfsDestination::ForTest })
            .unwrap();

        assert_eq!(
            vec![
                FileEntry {
                    source: "src1".into(),
                    destination: BootfsDestination::FromAIB("dest1".into()),
                },
                FileEntry { source: "src2".into(), destination: BootfsDestination::ForTest },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn blob_and_entry_same_src_diff_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        map.add_entry(FileEntry { source: "src1".into(), destination: BootfsDestination::ForTest })
            .unwrap();

        assert_eq!(
            vec![
                FileEntry {
                    source: "src1".into(),
                    destination: BootfsDestination::FromAIB("dest1".into()),
                },
                FileEntry { source: "src1".into(), destination: BootfsDestination::ForTest },
            ],
            map.into_file_entries()
        );
    }

    #[test]
    fn blob_and_entry_same_src_same_dest() {
        let mut map = NamedFileMap::new("test");
        map.add_blob(
            BootfsDestination::FromAIB("dest1".into()),
            BlobInfo {
                source_path: "src1".into(),
                path: "dest1".into(),
                merkle: "0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap(),
                size: 0,
            },
        )
        .unwrap();
        let res = map.add_entry(FileEntry {
            source: "src1".into(),
            destination: BootfsDestination::FromAIB("dest1".into()),
        });
        assert!(res.is_err());

        assert_eq!(
            vec![FileEntry {
                source: "src1".into(),
                destination: BootfsDestination::FromAIB("dest1".into()),
            }],
            map.into_file_entries()
        );
    }
}

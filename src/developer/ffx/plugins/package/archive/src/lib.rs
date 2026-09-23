// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use fuchsia_archive::{Error, Utf8Entry, Utf8Reader};
use fuchsia_hash::Hash;
use fuchsia_pkg::MetaContents;
use mockall::automock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Cursor;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq, Serialize, Hash)]
pub struct ArchiveEntry {
    pub name: String,
    pub path: String,
    pub length: Option<u64>,
}

impl From<Utf8Entry<'_>> for ArchiveEntry {
    fn from(entry: Utf8Entry<'_>) -> Self {
        ArchiveEntry {
            name: entry.path().to_string(),
            path: entry.path().to_string(),
            length: Some(entry.length()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FarCatResult {
    pub data_base64: String,
}

pub fn to_far_cat_result(data: &[u8]) -> FarCatResult {
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(data);
    FarCatResult { data_base64 }
}

/// Trait for listing the contents of the package archive. This
/// enables mocking of the reader for testing.
#[automock]
pub trait FarListReader {
    fn list_contents(&self) -> Result<Vec<ArchiveEntry>>;
    fn list_meta_contents(&mut self) -> Result<(Vec<ArchiveEntry>, MetaContents)>;
    fn read_entry(&mut self, entry: &ArchiveEntry) -> Result<Vec<u8>>;
}

/// Struct to implement the FarListReader by using the fuchsia_archive library.
pub struct FarArchiveReader {
    archive: Utf8Reader<File>,
}

impl FarArchiveReader {
    pub fn new(archive_name: &PathBuf) -> Result<FarArchiveReader> {
        Ok(FarArchiveReader {
            archive: fuchsia_archive::Utf8Reader::new(File::open(archive_name)?)?,
        })
    }

    fn read_from_meta(&mut self, file_name: &str) -> Result<Vec<u8>> {
        let meta_far_blob = self.archive.read_file("meta.far")?;
        let meta_cursor = Cursor::new(meta_far_blob);
        let mut meta_archive = fuchsia_archive::Utf8Reader::new(meta_cursor)?;
        meta_archive.read_file(file_name).map_err(|e| anyhow!("{}", e))
    }
}

impl FarListReader for FarArchiveReader {
    fn list_contents(&self) -> Result<Vec<ArchiveEntry>> {
        return Ok(self.archive.list().map(|e| e.into()).collect());
    }

    fn list_meta_contents(&mut self) -> Result<(Vec<ArchiveEntry>, MetaContents)> {
        let (meta_entries, meta_contents) = match self.archive.read_file("meta.far") {
            Ok(meta_far_blob) => {
                let meta_cursor = Cursor::new(meta_far_blob);
                let mut meta_archive = fuchsia_archive::Utf8Reader::new(meta_cursor)?;
                let meta_entries = meta_archive.list().map(|e| e.into()).collect();

                let contents_blob = meta_archive.read_file(MetaContents::PATH)?;
                let meta_contents = MetaContents::deserialize(contents_blob.as_slice())?;
                (meta_entries, meta_contents)
            }
            Err(Error::PathNotPresent(_)) => (vec![], MetaContents::new()),
            Err(e) => bail!("Error reading meta.far: {:#}", e),
        };
        Ok((meta_entries, meta_contents))
    }

    fn read_entry(&mut self, entry: &ArchiveEntry) -> Result<Vec<u8>> {
        let contents = match self.archive.read_file(&entry.path) {
            Ok(data) => data,
            Err(Error::PathNotPresent(p)) => {
                // Check if path starts with meta
                if entry.path.starts_with("meta/") {
                    self.read_from_meta(&entry.path)?
                } else {
                    bail!(Error::PathNotPresent(p));
                }
            }
            Err(e) => bail!("{}", e),
        };
        Ok(contents)
    }
}

pub fn read_file_entries(reader: &mut dyn FarListReader) -> Result<Vec<ArchiveEntry>> {
    // Create a map of hash to entry. This will be matched against
    // the file names in meta/contents.
    let archive_entries: HashMap<_, _> =
        reader.list_contents()?.into_iter().map(|e| (e.path.clone(), e)).collect();

    let (mut entries, meta_contents) = reader.list_meta_contents()?;

    // Match the hash of the file from the contents list to a
    // blob entry in the archive. If it is missing, mark the length
    // and offset as zero.
    let mut used_archive_entries = HashSet::new();
    for (name, hash) in &meta_contents {
        if let Some(mut blob) = archive_entries.get(&hash.to_string()).cloned() {
            used_archive_entries.insert(hash.to_string());
            blob.name = name.to_string();
            entries.push(blob);
        } else {
            entries.push(ArchiveEntry {
                name: name.to_string(),
                path: hash.to_string(),
                length: None,
            });
        }
    }

    // After processing meta/contents, or if there is no meta.far in this archive,
    // there will be unreferenced blob entries listed, so add them to the
    // output.
    for (path, entry) in archive_entries {
        if !used_archive_entries.contains(&path) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

pub mod test_utils {
    use super::*;
    use std::str::FromStr;

    pub static BLOB1: &str = "1f487b576253664f9de1a940ad3a350ca47316b5cdb65254fbf267367fd77c62";
    pub static RUN_ME_BLOB: &str = BLOB1;
    pub static RUN_ME_PATH: &str = "run_me";
    pub static BLOB2: &str = "892d655f2c841030d1b5556f9f124a753b5e32948471be76e72d330c6b6ba1db";
    pub static LIB_RUN_SO_BLOB: &str = BLOB2;
    pub static LIB_RUN_SO_PATH: &str = "lib/run.so";
    pub static BLOB3: &str = "4ef082296b26108697e851e0b40f8d8d31f96f934d7076f3bad37d5103be172c";
    pub static DATA_SOME_FILE_BLOB: &str = BLOB3;
    pub static DATA_SOME_FILE_PATH: &str = "data/some_file";
    pub static MISSING_BLOB: &str =
        "acfe18f46d86a6d0848ce02320acb455b17f2df9fe5806dc52465b3d74cf2fd9";
    pub static MISSING_BLOB_PATH: &str = "data/missing_blob";

    pub fn create_mockreader() -> MockFarListReader {
        let mut mockreader = MockFarListReader::new();
        mockreader.expect_list_contents().returning(|| {
            Ok(vec![
                ArchiveEntry {
                    name: BLOB1.to_string(),
                    path: BLOB1.to_string(),
                    length: Some(1024 * 4),
                },
                ArchiveEntry {
                    name: BLOB2.to_string(),
                    path: BLOB2.to_string(),
                    length: Some(1024 * 4),
                },
                ArchiveEntry {
                    name: BLOB3.to_string(),
                    path: BLOB3.to_string(),
                    length: Some(300000),
                },
                ArchiveEntry {
                    name: "meta.far".to_string(),
                    path: "meta.far".to_string(),
                    length: Some(1024 * 16),
                },
            ])
        });
        mockreader.expect_list_meta_contents().returning(|| {
            Ok((
                vec![
                    ArchiveEntry {
                        name: "meta/the_component.cm".to_string(),
                        path: "meta/the_component.cm".to_string(),
                        length: Some(100),
                    },
                    ArchiveEntry {
                        name: "meta/package".to_string(),
                        path: "meta/package".to_string(),
                        length: Some(25),
                    },
                    ArchiveEntry {
                        name: "meta/contents".to_string(),
                        path: "meta/contents".to_string(),
                        length: Some(55),
                    },
                ],
                MetaContents::from([
                    (RUN_ME_PATH, Hash::from_str(RUN_ME_BLOB)?),
                    (LIB_RUN_SO_PATH, Hash::from_str(LIB_RUN_SO_BLOB)?),
                    (DATA_SOME_FILE_PATH, Hash::from_str(DATA_SOME_FILE_BLOB)?),
                    (MISSING_BLOB_PATH, Hash::from_str(MISSING_BLOB)?),
                ]),
            ))
        });

        mockreader.expect_read_entry().returning(|entry| {
            let ret = test_contents(&entry.path);
            match ret.len() {
                0 => bail!("Zero length content for {:?}", entry),
                _ => Ok(ret),
            }
        });
        mockreader
    }

    pub fn test_contents(value: &str) -> Vec<u8> {
        format!("Contents for {}", value).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::test_utils::BLOB1;
    use super::*;

    #[test]
    fn read_file_entries_handles_duplicate_content_blobs() {
        let mut mockreader = MockFarListReader::new();
        mockreader.expect_list_contents().returning(|| {
            Ok(vec![ArchiveEntry { name: BLOB1.into(), path: BLOB1.into(), length: Some(1) }])
        });
        mockreader.expect_list_meta_contents().returning(|| {
            Ok((
                vec![],
                MetaContents::from([
                    ("first-copy", BLOB1.parse().unwrap()),
                    ("second-copy", BLOB1.parse().unwrap()),
                ]),
            ))
        });

        let mut entries = read_file_entries(&mut mockreader).unwrap();
        entries.sort_unstable();

        // Both package entries that are backed by BLOB1 will be present and have the correct size.
        assert_eq!(
            entries,
            vec![
                ArchiveEntry { name: "first-copy".into(), path: BLOB1.into(), length: Some(1) },
                ArchiveEntry { name: "second-copy".into(), path: BLOB1.into(), length: Some(1) }
            ]
        );
    }

    #[test]
    fn read_file_entries_missing_content_blob() {
        let mut mockreader = MockFarListReader::new();
        mockreader.expect_list_contents().returning(|| Ok(vec![]));
        mockreader.expect_list_meta_contents().returning(|| {
            Ok((vec![], MetaContents::from([("missing-blob", BLOB1.parse().unwrap())])))
        });

        let entries = read_file_entries(&mut mockreader).unwrap();

        assert_eq!(
            entries,
            vec![ArchiveEntry { name: "missing-blob".into(), path: BLOB1.into(), length: None },]
        );
    }
}

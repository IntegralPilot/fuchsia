// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::io::ReadSeek;
use anyhow::{Context, Result, anyhow};
use delivery_blob::DeliveryBlobType;
use log::warn;
use pathdiff::diff_paths;
use std::collections::HashSet;
use std::fs::{self};
use std::path::{Path, PathBuf};

/// Interface for fetching raw bytes by file path.
pub trait ArtifactReader: Send + Sync {
    /// Open the file located at `path`.
    fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>>;

    /// Read the raw bytes stored in filesystem location `path`.
    fn read_bytes(&mut self, path: &Path) -> Result<Vec<u8>>;

    /// Get the accumulated set of filesystem locations that have been read by
    /// this reader.
    fn get_deps(&self) -> HashSet<PathBuf>;
}

/// An artifact reader that consults a sequence of delegate readers, returning
/// the first non-error result, or else an error describing all error results.
/// The dependencies tracked by this implementation is the union of all
/// delegates' dependencies.
pub struct CompoundArtifactReader {
    delegates: Vec<Box<dyn ArtifactReader>>,
}

impl CompoundArtifactReader {
    pub fn new(delegates: Vec<Box<dyn ArtifactReader>>) -> Self {
        Self { delegates }
    }
}

impl ArtifactReader for CompoundArtifactReader {
    fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        let mut errs = vec![];
        for delegate in self.delegates.iter_mut() {
            match delegate.open(path) {
                Ok(rs) => return Ok(rs),
                Err(err) => errs.push(err),
            }
        }
        let mut compound_err = anyhow!("Compound artifact read failed");
        for err in errs.into_iter() {
            compound_err = compound_err.context("Read failure");
            for ctx in err.chain() {
                compound_err = compound_err.context(ctx.to_string());
            }
        }
        Err(compound_err)
    }

    fn read_bytes(&mut self, path: &Path) -> Result<Vec<u8>> {
        let mut errs = vec![];
        for delegate in self.delegates.iter_mut() {
            match delegate.read_bytes(path) {
                Ok(data) => {
                    return Ok(data);
                }
                Err(err) => {
                    errs.push(err);
                }
            }
        }
        let mut compound_err = anyhow!("Compound artifact read failed");
        for err in errs.into_iter() {
            compound_err = compound_err.context("Read failure");
            for ctx in err.chain() {
                compound_err = compound_err.context(ctx.to_string());
            }
        }
        Err(compound_err)
    }

    fn get_deps(&self) -> HashSet<PathBuf> {
        let mut deps = HashSet::new();
        for delegate in self.delegates.iter() {
            deps.extend(delegate.get_deps().into_iter());
        }
        deps
    }
}

/// An `ArtifactReader` implementation that reads paths relative to a particular
/// directory.
#[derive(Clone)]
pub struct FileArtifactReader {
    build_path: PathBuf,
    artifact_path: PathBuf,
    delivery_blob_type: DeliveryBlobType,
    deps: HashSet<PathBuf>,
}

impl FileArtifactReader {
    /// Construct a new artifact reader that tracks dependencies relative to
    /// `build_path` and reads artifacts relative to `artifact_path`.
    pub fn new(
        build_path: &Path,
        artifact_path: &Path,
        delivery_blob_type: DeliveryBlobType,
    ) -> Self {
        let build_path = match build_path.canonicalize() {
            Ok(path) => path,
            Err(err) => {
                warn!(
                    "File artifact reader failed to canonicalize build path: {:?}: {}",
                    build_path, err
                );
                build_path.to_path_buf()
            }
        };
        let artifact_path = match artifact_path.canonicalize() {
            Ok(path) => path,
            Err(err) => {
                warn!(
                    "File artifact reader failed to canonicalize artifact path: {:?}: {}",
                    artifact_path, err
                );
                artifact_path.to_path_buf()
            }
        };
        Self { build_path, artifact_path, delivery_blob_type, deps: HashSet::new() }
    }
}

impl ArtifactReader for FileArtifactReader {
    fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        let absolute_path_string =
            absolute_from_absolute_or_artifact_relative(&self.artifact_path, path)
                .context("Absolute path conversion failure during read")?;
        let dep_path_string = dep_from_absolute(&self.build_path, &absolute_path_string)
            .context("Dep path conversion failed during read")?;
        self.deps.insert(dep_path_string);

        Ok(match self.delivery_blob_type {
            // Read-in and decompress delivery blobs
            DeliveryBlobType::Type1 => {
                let raw_blob_contents = fs::read(&absolute_path_string).map_err(|err| {
                    anyhow!("Artifact read failed ({}): {}", absolute_path_string, err)
                })?;
                let decompressed_contents = delivery_blob::decompress(&raw_blob_contents)?;
                Box::new(std::io::Cursor::new(decompressed_contents))
            }

            // Directly open non-delivery blobs as-is
            _ => Box::new(
                fs::File::open(absolute_path_string)
                    .context("<FileArtifactReader as ArtifactReader>::open")?,
            ),
        })
    }

    fn read_bytes(&mut self, path: &Path) -> Result<Vec<u8>> {
        let absolute_path_string =
            absolute_from_absolute_or_artifact_relative(&self.artifact_path, path)
                .context("Absolute path conversion failure during read")?;
        let dep_path_string = dep_from_absolute(&self.build_path, &absolute_path_string)
            .context("Dep path conversion failed during read")?;
        self.deps.insert(dep_path_string);

        // First read in the blob
        let raw_blob_contents = fs::read(&absolute_path_string)
            .map_err(|err| anyhow!("Artifact read failed ({}): {}", absolute_path_string, err))?;

        Ok(match self.delivery_blob_type {
            // Decompress delivery blobs
            DeliveryBlobType::Type1 => delivery_blob::decompress(raw_blob_contents.as_slice())?,

            // Directly return non-delivery blobs as-is
            _ => raw_blob_contents,
        })
    }

    fn get_deps(&self) -> HashSet<PathBuf> {
        self.deps.clone()
    }
}

fn absolute_from_absolute_or_artifact_relative<P1: AsRef<Path>, P2: AsRef<Path>>(
    artifact_path: P1,
    path: P2,
) -> Result<String> {
    let artifact_path_ref = artifact_path.as_ref();
    let path_ref = path.as_ref();
    let artifact_relative_path_buf = if path_ref.is_absolute() {
        diff_paths(path_ref, &artifact_path).ok_or_else(|| {
            anyhow!(
                "Absolute artifact path {:?} cannot be rebased to base artifact path {:?}",
                path_ref,
                artifact_path_ref,
            )
        })?
    } else {
        path_ref.to_path_buf()
    };
    let absolute_path_buf = artifact_path_ref.join(&artifact_relative_path_buf);
    let absolute_path_buf = absolute_path_buf.canonicalize().map_err(|err| {
        anyhow!("Failed to canonicalize computed path: {:?}: {}", absolute_path_buf, err)
    })?;

    if absolute_path_buf.is_relative() {
        return Err(anyhow!(
            "Computed artifact path is relative: computed {:?} from path {:?} and artifact base path {:?}",
            absolute_path_buf,
            path_ref,
            artifact_path_ref,
        ));
    }
    if absolute_path_buf.is_dir() {
        return Err(anyhow!(
            "Computed artifact path is directory: computed {:?} from path {:?} and artifact base path {:?}",
            absolute_path_buf,
            path_ref,
            artifact_path_ref,
        ));
    }

    let absolute_path_str = absolute_path_buf.to_str();
    if absolute_path_str.is_none() {
        return Err(anyhow!(
            "Computed absolute artifact path {:?} could not be converted to string",
            absolute_path_buf
        ));
    };
    Ok(absolute_path_str.unwrap().to_string())
}

fn dep_from_absolute<P1: AsRef<Path>, P2: AsRef<Path>>(
    build_path: P1,
    path: P2,
) -> Result<PathBuf> {
    let build_path_ref = build_path.as_ref();
    let path_ref = path.as_ref();
    let canonical_path_buf = path_ref.canonicalize().map_err(|err| {
        anyhow!("Failed to canonicalize absolute path: {:?}: {:?}", path_ref, err.to_string())
    })?;
    if canonical_path_buf.is_absolute() {
        diff_paths(&canonical_path_buf, &build_path).ok_or_else(|| {
            anyhow!(
                "Artifact path {:?} (from {:?}) cannot be formatted relative to build path {:?}",
                canonical_path_buf,
                path_ref,
                build_path_ref,
            )
        })
    } else {
        Err(anyhow!(
            "Canonicalized form of {:?} is {:?}, which is not an absolute path",
            path_ref,
            canonical_path_buf,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactReader, FileArtifactReader};
    use delivery_blob::DeliveryBlobType;
    use maplit::hashset;
    use std::fs::{File, create_dir};
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_basic() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        let mut loader = FileArtifactReader::new(&dir, &dir, DeliveryBlobType::Reserved);
        let mut file = File::create(dir.join("foo")).unwrap();
        file.write_all(b"test_data").unwrap();
        file.sync_all().unwrap();
        let result = loader.read_bytes(&Path::new("foo"));
        assert_eq!(result.is_ok(), true);
        let data = result.unwrap();
        assert_eq!(data, b"test_data");
    }

    #[test]
    fn test_compressed() {
        let temp = tempdir().unwrap();
        let dir = temp.path();

        // Write out a delivery blob
        let mut file = File::create(dir.join("foo")).unwrap();
        delivery_blob::generate_to(DeliveryBlobType::Type1, b"test_data", &mut file).unwrap();
        file.sync_all().unwrap();

        // Load it using the matching DeliveryBlobType FileArtifactReader
        let mut loader = FileArtifactReader::new(&dir, &dir, DeliveryBlobType::Type1);
        let result = loader.read_bytes(&Path::new("foo"));

        assert_eq!(result.is_ok(), true);
        let data = result.unwrap();
        assert_eq!(data, b"test_data");
    }

    #[test]
    fn test_deps() {
        let temp = tempdir().unwrap();
        let build_path = temp.path();
        let artifact_path_buf = build_path.join("artifacts");
        let artifact_path = artifact_path_buf.as_path();
        create_dir(&artifact_path).unwrap();
        let mut loader =
            FileArtifactReader::new(&build_path, artifact_path, DeliveryBlobType::Reserved);

        let mut file = File::create(&artifact_path.join("foo")).unwrap();
        file.write_all(b"test_data").unwrap();
        file.sync_all().unwrap();

        let mut file = File::create(&artifact_path.join("bar")).unwrap();
        file.write_all(b"test_data").unwrap();
        file.sync_all().unwrap();

        assert_eq!(loader.read_bytes(&Path::new("foo")).is_ok(), true);
        assert_eq!(loader.read_bytes(&Path::new("bar")).is_ok(), true);
        assert_eq!(loader.read_bytes(&Path::new("foo")).is_ok(), true);
        let deps = loader.get_deps();
        assert_eq!(
            deps,
            hashset! {"artifacts/foo".to_string().into(), "artifacts/bar".to_string().into()}
        );
    }
}

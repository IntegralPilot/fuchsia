// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    BlobEntry, MetaContents, MetaPackage, MetaPackageError, MetaSubpackages, PackageArchiveBuilder,
    PackageManifestError, PackageName, PackagePath, PackageVariant,
};
use anyhow::{Context, Result};
use camino::Utf8Path;
use delivery_blob::DeliveryBlobType;
use fuchsia_archive::Utf8Reader;
use fuchsia_hash::Hash;
use fuchsia_merkle::root_from_slice;
use fuchsia_url::RepositoryUrl;
use fuchsia_url::fuchsia_pkg::UnpinnedAbsolutePackageUrl;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, create_dir_all};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::str;
use tempfile_ext::NamedTempFileExt as _;
use utf8_path::{path_relative_from_file, resolve_path_from_file};
use version_history::AbiRevision;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PackageManifest(VersionedPackageManifest);

impl PackageManifest {
    /// Blob path used in package manifests to indicate the `meta.far`.
    pub const META_FAR_BLOB_PATH: &'static str = "meta/";

    /// Return a reference vector of blobs in this PackageManifest.
    ///
    /// NB: Does not include blobs referenced by possible subpackages.
    pub fn blobs(&self) -> &[BlobInfo] {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => &manifest.blobs,
        }
    }

    /// Returns a reference vector of SubpackageInfo in this PackageManifest.
    pub fn subpackages(&self) -> &[SubpackageInfo] {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => &manifest.subpackages,
        }
    }

    /// Returns a vector of the blobs in the current PackageManifest.
    pub fn into_blobs(self) -> Vec<BlobInfo> {
        match self.0 {
            VersionedPackageManifest::Version1(manifest) => manifest.blobs,
        }
    }

    /// Returns a tuple of the current PackageManifest's blobs and subpackages.
    /// `blobs` does not include blobs referenced by the subpackages.
    pub fn into_blobs_and_subpackages(self) -> (Vec<BlobInfo>, Vec<SubpackageInfo>) {
        match self.0 {
            VersionedPackageManifest::Version1(manifest) => (manifest.blobs, manifest.subpackages),
        }
    }

    /// Returns the name from the PackageMetadata.
    pub fn name(&self) -> &PackageName {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => &manifest.package.name,
        }
    }

    /// Sets the name of the package.
    pub fn set_name(&mut self, name: PackageName) {
        match &mut self.0 {
            VersionedPackageManifest::Version1(manifest) => {
                manifest.package.name = name;
            }
        }
    }

    /// Write a package archive into the `out` file. The source files are relative to the `root_dir`
    /// directory.
    pub async fn archive(
        self,
        root_dir: impl AsRef<Path>,
        out: impl Write,
    ) -> Result<(), PackageManifestError> {
        let root_dir = root_dir.as_ref();

        let (meta_far_blob_info, all_blobs) = Self::package_and_subpackage_blobs(self)?;

        let source_path = root_dir.join(&meta_far_blob_info.source_path);
        let mut meta_far_blob = File::open(&source_path).map_err(|err| {
            PackageManifestError::IoErrorWithPath { cause: err, path: source_path }
        })?;
        meta_far_blob.seek(SeekFrom::Start(0))?;
        let mut archive_builder = PackageArchiveBuilder::with_meta_far(
            meta_far_blob.metadata()?.len(),
            Box::new(meta_far_blob),
        );

        for blob_info in all_blobs.values() {
            let source_path = root_dir.join(&blob_info.source_path);

            let blob_file = File::open(&source_path).map_err(|err| {
                PackageManifestError::IoErrorWithPath { cause: err, path: source_path }
            })?;
            archive_builder.add_blob(
                blob_info.merkle,
                blob_file.metadata()?.len(),
                Box::new(blob_file),
            );
        }

        archive_builder.build(out)?;
        Ok(())
    }

    /// Returns a `PackagePath` formatted from the metadata of the PackageManifest.
    pub fn package_path(&self) -> PackagePath {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => PackagePath::from_name_and_variant(
                manifest.package.name.to_owned(),
                manifest.package.version.to_owned(),
            ),
        }
    }

    pub fn repository(&self) -> Option<&str> {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => manifest.repository.as_deref(),
        }
    }

    pub fn set_repository(&mut self, repository: Option<String>) {
        match &mut self.0 {
            VersionedPackageManifest::Version1(manifest) => {
                manifest.repository = repository;
            }
        }
    }

    pub fn package_url(&self) -> Result<Option<UnpinnedAbsolutePackageUrl>> {
        if let Some(url) = self.repository() {
            let repo = RepositoryUrl::parse_host(url.to_string())?;
            return Ok(Some(UnpinnedAbsolutePackageUrl::new(repo, self.name().clone(), None)));
        };
        Ok(None)
    }

    /// Returns the merkle root of the meta.far.
    ///
    /// # Panics
    ///
    /// Panics if the PackageManifest is missing a "meta/" entry
    pub fn hash(&self) -> Hash {
        self.blobs().iter().find(|blob| blob.path == Self::META_FAR_BLOB_PATH).unwrap().merkle
    }

    pub fn abi_revision(&self) -> Option<AbiRevision> {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => manifest.abi_revision,
        }
    }

    pub fn delivery_blob_type(&self) -> Option<DeliveryBlobType> {
        match &self.0 {
            VersionedPackageManifest::Version1(manifest) => manifest.delivery_blob_type,
        }
    }

    /// Create a `PackageManifest` and populate a manifest directory given a blobs directory and the
    /// top level meta.far hash.
    ///
    /// The `blobs_dir_root` directory must contain all the package blobs either uncompressed in
    /// root, or delivery blobs in a sub directory.
    ///
    /// The `out_manifest_dir` will be a flat file populated with JSON representations of
    /// PackageManifests corresponding to the subpackages.
    pub fn from_blobs_dir(
        blobs_dir_root: &Path,
        delivery_blob_type: Option<DeliveryBlobType>,
        meta_far_hash: Hash,
        out_manifest_dir: &Path,
    ) -> Result<Self, PackageManifestError> {
        let blobs_dir = if let Some(delivery_blob_type) = delivery_blob_type {
            blobs_dir_root.join(u32::from(delivery_blob_type).to_string())
        } else {
            blobs_dir_root.to_path_buf()
        };
        let meta_far_path = blobs_dir.join(meta_far_hash.to_string());
        let (meta_far_blob, meta_far_size) = if delivery_blob_type.is_some() {
            let meta_far_delivery_blob = std::fs::read(&meta_far_path).map_err(|e| {
                PackageManifestError::IoErrorWithPath { cause: e, path: meta_far_path.clone() }
            })?;
            let meta_far_blob =
                delivery_blob::decompress(&meta_far_delivery_blob).map_err(|e| {
                    PackageManifestError::DecompressDeliveryBlob {
                        cause: e,
                        path: meta_far_path.clone(),
                    }
                })?;
            let meta_far_size = meta_far_blob.len().try_into().expect("meta.far size fits in u64");
            (meta_far_blob, meta_far_size)
        } else {
            let mut meta_far_file = File::open(&meta_far_path).map_err(|e| {
                PackageManifestError::IoErrorWithPath { cause: e, path: meta_far_path.clone() }
            })?;

            let mut meta_far_blob = vec![];
            meta_far_file.read_to_end(&mut meta_far_blob)?;
            (meta_far_blob, meta_far_file.metadata()?.len())
        };
        let mut meta_far = fuchsia_archive::Utf8Reader::new(std::io::Cursor::new(meta_far_blob))?;

        let meta_contents = meta_far.read_file(MetaContents::PATH)?;
        let meta_contents = MetaContents::deserialize(meta_contents.as_slice())?.into_contents();

        let meta_package = meta_far.read_file(MetaPackage::PATH)?;
        let meta_package = MetaPackage::deserialize(meta_package.as_slice())?;

        let abi_revision = match meta_far.read_file(AbiRevision::PATH) {
            Ok(bytes) => Some(AbiRevision::from_bytes(
                bytes.as_slice().try_into().map_err(crate::errors::AbiRevisionError::from)?,
            )),
            Err(fuchsia_archive::Error::PathNotPresent(_)) => {
                return Err(PackageManifestError::AbiRevision(
                    crate::errors::AbiRevisionError::Missing,
                ));
            }
            Err(e) => return Err(e.into()),
        };

        let meta_subpackages = match meta_far.read_file(MetaSubpackages::PATH) {
            Ok(meta_subpackages) => {
                let meta_subpackages =
                    MetaSubpackages::deserialize(meta_subpackages.as_slice())?.into_subpackages();

                // The meta subpackages are unordered, so sort them to keep things consistent.
                meta_subpackages.into_iter().collect::<BTreeMap<_, _>>()
            }
            Err(fuchsia_archive::Error::PathNotPresent(_)) => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };

        let mut sub_packages = vec![];
        for (name, hash) in meta_subpackages {
            let sub_package_manifest =
                Self::from_blobs_dir(blobs_dir_root, delivery_blob_type, hash, out_manifest_dir)?;

            let source_pathbuf = out_manifest_dir.join(format!("{}_package_manifest.json", hash));
            let source_path = source_pathbuf.as_path();

            let relative_path = Utf8Path::from_path(source_path).unwrap();

            let _ = sub_package_manifest
                .write_with_relative_paths(relative_path)
                .map_err(PackageManifestError::RelativeWrite)?;
            sub_packages.push((name, hash, source_path.to_owned()));
        }

        // Build the PackageManifest of this package.
        let mut builder = PackageManifestBuilder::new(meta_package)
            .delivery_blob_type(delivery_blob_type)
            .abi_revision(abi_revision);

        // Add the meta.far blob. We add this first since some scripts assume the first entry is the
        // meta.far entry.
        builder = builder.add_blob(BlobInfo {
            source_path: meta_far_path.into_os_string().into_string().map_err(|source_path| {
                PackageManifestError::InvalidBlobPath {
                    merkle: meta_far_hash,
                    source_path: source_path.into(),
                }
            })?,
            path: Self::META_FAR_BLOB_PATH.into(),
            merkle: meta_far_hash,
            size: meta_far_size,
        });

        for (blob_path, merkle) in &meta_contents {
            let source_path = blobs_dir.join(merkle.to_string());

            if !source_path.exists() {
                return Err(PackageManifestError::IoErrorWithPath {
                    cause: io::ErrorKind::NotFound.into(),
                    path: source_path,
                });
            }

            let size = if delivery_blob_type.is_some() {
                let file = File::open(&source_path)?;
                delivery_blob::decompressed_size_from_reader(file).map_err(|e| {
                    PackageManifestError::DecompressDeliveryBlob {
                        cause: e,
                        path: source_path.clone(),
                    }
                })?
            } else {
                fs::metadata(&source_path)?.len()
            };

            builder = builder.add_blob(BlobInfo {
                source_path: source_path.into_os_string().into_string().map_err(|source_path| {
                    PackageManifestError::InvalidBlobPath {
                        merkle: *merkle,
                        source_path: source_path.into(),
                    }
                })?,
                path: blob_path.to_string(),
                merkle: *merkle,
                size,
            });
        }

        for (name, merkle, path) in sub_packages {
            builder = builder.add_subpackage(SubpackageInfo {
                manifest_path: path.to_str().expect("better work").to_string(),
                name: name.to_string(),
                merkle,
            });
        }

        Ok(builder.build())
    }

    /// Extract the package blobs from `archive_path` into the `blobs_dir` directory and
    /// extracts all the JSON representations of the subpackages' PackageManifests and
    /// top level PackageManifest into `out_manifest_dir`.
    ///
    /// Returns an in-memory `PackageManifest` for these files.
    pub fn from_archive(
        archive_path: &Path,
        blobs_dir: &Path,
        out_manifest_dir: &Path,
    ) -> Result<Self, PackageManifestError> {
        let archive_file = File::open(archive_path)?;
        let mut archive_reader = Utf8Reader::new(&archive_file)?;

        let far_paths =
            archive_reader.list().map(|entry| entry.path().to_owned()).collect::<Vec<_>>();

        for path in far_paths {
            let blob_path = blobs_dir.join(&path);

            if &path != "meta.far" && !blob_path.as_path().exists() {
                let contents = archive_reader.read_file(&path)?;
                let mut tmp = tempfile::NamedTempFile::new_in(blobs_dir)?;
                tmp.write_all(&contents)?;
                tmp.persist_if_changed(&blob_path)
                    .map_err(|err| PackageManifestError::Persist { cause: err, path: blob_path })?;
            }
        }

        let meta_far = archive_reader.read_file("meta.far")?;
        let meta_far_hash = root_from_slice(&meta_far);

        let meta_far_path = blobs_dir.join(meta_far_hash.to_string());
        let mut tmp = tempfile::NamedTempFile::new_in(blobs_dir)?;
        tmp.write_all(&meta_far)?;
        tmp.persist_if_changed(&meta_far_path)
            .map_err(|err| PackageManifestError::Persist { cause: err, path: meta_far_path })?;

        PackageManifest::from_blobs_dir(blobs_dir, None, meta_far_hash, out_manifest_dir)
    }

    /// Verify that all blob and subpackage paths are valid and return the PackageManifest.
    pub(crate) fn from_parts(
        meta_package: MetaPackage,
        repository: Option<String>,
        mut package_blobs: BTreeMap<String, BlobEntry>,
        package_subpackages: Vec<crate::SubpackageEntry>,
        abi_revision: AbiRevision,
    ) -> Result<Self, PackageManifestError> {
        let mut blobs = Vec::with_capacity(package_blobs.len());

        let mut push_blob = |blob_path, blob_entry: BlobEntry| {
            let source_path = blob_entry.source_path;

            blobs.push(BlobInfo {
                source_path: source_path.into_os_string().into_string().map_err(|source_path| {
                    PackageManifestError::InvalidBlobPath {
                        merkle: blob_entry.hash,
                        source_path: source_path.into(),
                    }
                })?,
                path: blob_path,
                merkle: blob_entry.hash,
                size: blob_entry.size,
            });

            Ok::<(), PackageManifestError>(())
        };

        // Add the meta.far blob. We add this first since some scripts assume the first entry is the
        // meta.far entry.
        if let Some((blob_path, blob_entry)) = package_blobs.remove_entry(Self::META_FAR_BLOB_PATH)
        {
            push_blob(blob_path, blob_entry)?;
        }

        for (blob_path, blob_entry) in package_blobs {
            push_blob(blob_path, blob_entry)?;
        }

        let mut subpackages = Vec::with_capacity(package_subpackages.len());

        for subpackage in package_subpackages {
            subpackages.push(SubpackageInfo {
                manifest_path: subpackage
                    .package_manifest_path
                    .into_os_string()
                    .into_string()
                    .map_err(|package_manifest_path| {
                        PackageManifestError::InvalidSubpackagePath {
                            merkle: subpackage.merkle,
                            path: package_manifest_path.into(),
                        }
                    })?,
                name: subpackage.name.to_string(),
                merkle: subpackage.merkle,
            });
        }

        let manifest_v1 = PackageManifestV1 {
            package: PackageMetadata {
                name: meta_package.name().to_owned(),
                version: meta_package.variant().to_owned(),
            },
            blobs,
            repository,
            blob_sources_relative: Default::default(),
            subpackages,
            delivery_blob_type: None,
            abi_revision: Some(abi_revision),
        };
        Ok(PackageManifest(VersionedPackageManifest::Version1(manifest_v1)))
    }

    pub fn try_load_from(manifest_path: impl AsRef<Utf8Path>) -> anyhow::Result<Self> {
        fn inner(manifest_path: &Utf8Path) -> anyhow::Result<PackageManifest> {
            let file = File::open(manifest_path)
                .with_context(|| format!("Opening package manifest: {manifest_path}"))?;

            PackageManifest::from_reader(manifest_path, BufReader::new(file))
        }
        inner(manifest_path.as_ref())
    }

    pub fn from_reader(
        manifest_path: impl AsRef<Utf8Path>,
        reader: impl std::io::Read,
    ) -> anyhow::Result<Self> {
        fn inner(
            manifest_path: &Utf8Path,
            reader: impl std::io::Read,
        ) -> anyhow::Result<PackageManifest> {
            let versioned: VersionedPackageManifest = serde_json::from_reader(reader)?;

            let versioned = match versioned {
                VersionedPackageManifest::Version1(manifest) => VersionedPackageManifest::Version1(
                    manifest.resolve_source_paths(manifest_path)?,
                ),
            };

            Ok(PackageManifest(versioned))
        }
        inner(manifest_path.as_ref(), reader)
    }

    fn package_and_subpackage_blobs_impl(
        contents: &mut HashMap<Hash, BlobInfo>,
        visited_subpackages: &mut HashSet<Hash>,
        package_manifest: Self,
    ) -> Result<(), PackageManifestError> {
        let (blobs, subpackages) = package_manifest.into_blobs_and_subpackages();
        for blob in blobs {
            contents.insert(blob.merkle, blob);
        }

        for sp in subpackages {
            let key = sp.merkle;

            if visited_subpackages.insert(key) {
                let package_manifest = Self::try_load_from(&sp.manifest_path).map_err(|_| {
                    PackageManifestError::InvalidSubpackagePath {
                        merkle: sp.merkle,
                        path: sp.manifest_path.into(),
                    }
                })?;

                Self::package_and_subpackage_blobs_impl(
                    contents,
                    visited_subpackages,
                    package_manifest,
                )?;
            }
        }
        Ok(())
    }

    /// Returns a tuple of a BlobInfo corresponding to the top level meta.far blob and a HashMap
    /// containing all of the blobs (both content blobs and meta.fars) from the transitive closure
    /// of the subpackages. The HashMap will not contain the BlobInfo of the top level meta.far.
    pub fn package_and_subpackage_blobs(
        self,
    ) -> Result<(BlobInfo, HashMap<Hash, BlobInfo>), PackageManifestError> {
        let mut contents = HashMap::new();
        let mut visited_subpackages = HashSet::new();

        Self::package_and_subpackage_blobs_impl(
            &mut contents,
            &mut visited_subpackages,
            self.clone(),
        )?;

        let blobs = self.into_blobs();
        for blob in blobs {
            if blob.path == Self::META_FAR_BLOB_PATH && contents.remove(&blob.merkle).is_some() {
                return Ok((blob, contents));
            }
        }
        Err(PackageManifestError::MetaPackage(MetaPackageError::MetaPackageMissing))
    }

    pub fn write_with_relative_paths(self, path: impl AsRef<Utf8Path>) -> anyhow::Result<Self> {
        fn inner(this: PackageManifest, path: &Utf8Path) -> anyhow::Result<PackageManifest> {
            let manifest = match this.0 {
                VersionedPackageManifest::Version1(manifest) => PackageManifest(
                    VersionedPackageManifest::Version1(manifest.with_relative_paths(path)?),
                ),
            };
            let () = manifest.write(path)?;
            Ok(manifest)
        }
        inner(self, path.as_ref())
    }

    pub fn write(&self, path: impl AsRef<Utf8Path>) -> anyhow::Result<()> {
        fn inner(this: &PackageManifest, path: &Utf8Path) -> anyhow::Result<()> {
            let mut tmp = if let Some(parent) = path.parent() {
                create_dir_all(parent)?;
                tempfile::NamedTempFile::new_in(parent)?
            } else {
                tempfile::NamedTempFile::new()?
            };

            serde_json::to_writer_pretty(BufWriter::new(&mut tmp), this)?;
            tmp.persist_if_changed(path)
                .with_context(|| format!("failed to persist package manifest: {path}"))?;

            Ok(())
        }
        inner(self, path.as_ref())
    }
}

pub struct PackageManifestBuilder {
    manifest: PackageManifestV1,
}

impl PackageManifestBuilder {
    pub fn new(meta_package: MetaPackage) -> Self {
        Self {
            manifest: PackageManifestV1 {
                package: PackageMetadata {
                    name: meta_package.name().to_owned(),
                    version: meta_package.variant().to_owned(),
                },
                blobs: vec![],
                repository: None,
                blob_sources_relative: Default::default(),
                subpackages: vec![],
                delivery_blob_type: None,
                abi_revision: None,
            },
        }
    }

    pub fn repository(mut self, repository: impl Into<String>) -> Self {
        self.manifest.repository = Some(repository.into());
        self
    }

    pub fn delivery_blob_type(mut self, delivery_blob_type: Option<DeliveryBlobType>) -> Self {
        self.manifest.delivery_blob_type = delivery_blob_type;
        self
    }

    pub fn add_blob(mut self, info: BlobInfo) -> Self {
        self.manifest.blobs.push(info);
        self
    }

    pub fn add_subpackage(mut self, info: SubpackageInfo) -> Self {
        self.manifest.subpackages.push(info);
        self
    }

    pub fn abi_revision(mut self, abi_revision: Option<AbiRevision>) -> Self {
        self.manifest.abi_revision = abi_revision;
        self
    }

    pub fn build(self) -> PackageManifest {
        PackageManifest(VersionedPackageManifest::Version1(self.manifest))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "version")]
enum VersionedPackageManifest {
    #[serde(rename = "1")]
    Version1(PackageManifestV1),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct PackageManifestV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    package: PackageMetadata,
    blobs: Vec<BlobInfo>,

    /// Are the blob source_paths relative to the working dir (default, as made
    /// by 'pm') or the file containing the serialized manifest (new, portable,
    /// behavior)
    #[serde(default, skip_serializing_if = "RelativeTo::is_default")]
    // TODO(https://fxbug.dev/42066050): rename this to `paths_relative` since it applies
    // to both blobs and subpackages. (I'd change it now, but it's encoded in
    // JSON files so we may need a soft transition to support both at first.)
    blob_sources_relative: RelativeTo,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subpackages: Vec<SubpackageInfo>,
    /// If not None, the `source_path` of the `blobs` are delivery blobs of the given type instead of
    /// uncompressed blobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_blob_type: Option<DeliveryBlobType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abi_revision: Option<AbiRevision>,
}

impl PackageManifestV1 {
    pub fn with_relative_paths(
        self,
        manifest_path: impl AsRef<Utf8Path>,
    ) -> anyhow::Result<PackageManifestV1> {
        fn inner(
            this: PackageManifestV1,
            manifest_path: &Utf8Path,
        ) -> anyhow::Result<PackageManifestV1> {
            let manifest = if let RelativeTo::WorkingDir = &this.blob_sources_relative {
                // manifest contains working-dir relative source paths, make
                // them relative to the file, instead.
                let blobs = this
                    .blobs
                    .into_iter()
                    .map(|blob| relativize_blob_source_path(blob, manifest_path))
                    .collect::<anyhow::Result<_>>()?;
                let subpackages = this
                    .subpackages
                    .into_iter()
                    .map(|subpackage| {
                        relativize_subpackage_manifest_path(subpackage, manifest_path)
                    })
                    .collect::<anyhow::Result<_>>()?;
                PackageManifestV1 {
                    blobs,
                    subpackages,
                    blob_sources_relative: RelativeTo::File,
                    ..this
                }
            } else {
                this
            };

            Ok(manifest)
        }
        inner(self, manifest_path.as_ref())
    }

    pub fn resolve_source_paths(self, manifest_path: impl AsRef<Utf8Path>) -> anyhow::Result<Self> {
        fn inner(
            this: PackageManifestV1,
            manifest_path: &Utf8Path,
        ) -> anyhow::Result<PackageManifestV1> {
            if let RelativeTo::File = &this.blob_sources_relative {
                let blobs = this
                    .blobs
                    .into_iter()
                    .map(|blob| resolve_blob_source_path(blob, manifest_path))
                    .collect::<anyhow::Result<_>>()?;
                let subpackages = this
                    .subpackages
                    .into_iter()
                    .map(|subpackage| resolve_subpackage_manifest_path(subpackage, manifest_path))
                    .collect::<anyhow::Result<_>>()?;
                let blob_sources_relative = RelativeTo::WorkingDir;
                Ok(PackageManifestV1 { blobs, subpackages, blob_sources_relative, ..this })
            } else {
                Ok(this)
            }
        }
        inner(self, manifest_path.as_ref())
    }
}

/// If the path is a relative path, what is it relative from?
///
/// If 'RelativeTo::WorkingDir', then the path is assumed to be relative to the
/// working dir, and can be used directly as a path.
///
/// If 'RelativeTo::File', then the path is relative to the file that contained
/// the path. To use the path, it must be resolved against the path of the
/// file.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum RelativeTo {
    #[serde(rename = "working_dir")]
    #[default]
    WorkingDir,
    #[serde(rename = "file")]
    File,
}

impl RelativeTo {
    pub(crate) fn is_default(&self) -> bool {
        matches!(self, RelativeTo::WorkingDir)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct PackageMetadata {
    name: PackageName,
    version: PackageVariant,
}

#[derive(Clone, Debug, Eq, Deserialize, Serialize, PartialOrd, Ord)]
pub struct BlobInfo {
    /// Path to the blob file, could be a delivery blob or uncompressed blob depending on
    /// `delivery_blob_type` in the manifest.
    pub source_path: String,
    /// The virtual path of the blob in the package.
    pub path: String,
    pub merkle: fuchsia_merkle::Hash,
    /// Uncompressed size of the blob.
    pub size: u64,
}

// Write a custom PartialEq so that we ignore `source_path`.
// Blobs are identical if their destination path and merkle are the same.
impl PartialEq for BlobInfo {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.merkle == other.merkle
    }
}

#[derive(Clone, Debug, Eq, Deserialize, Serialize)]
pub struct SubpackageInfo {
    /// Path to a PackageManifest for the subpackage.
    pub manifest_path: String,

    /// The package-relative name of this declared subpackage.
    pub name: String,

    /// The package hash (meta.far merkle) of the subpackage.
    pub merkle: fuchsia_merkle::Hash,
}

// Write a custom PartialEq so that we ignore `manifest_path`.
// Subpackages are identical if their name and merkle are the same.
impl PartialEq for SubpackageInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.merkle == other.merkle
    }
}

fn relativize_blob_source_path(
    blob: BlobInfo,
    manifest_path: &Utf8Path,
) -> anyhow::Result<BlobInfo> {
    let source_path = path_relative_from_file(blob.source_path, manifest_path)?;
    let source_path = source_path.into_string();

    Ok(BlobInfo { source_path, ..blob })
}

fn resolve_blob_source_path(blob: BlobInfo, manifest_path: &Utf8Path) -> anyhow::Result<BlobInfo> {
    let source_path = resolve_path_from_file(&blob.source_path, manifest_path)
        .with_context(|| format!("Resolving blob path: {}", blob.source_path))?
        .into_string();
    Ok(BlobInfo { source_path, ..blob })
}

fn relativize_subpackage_manifest_path(
    subpackage: SubpackageInfo,
    manifest_path: &Utf8Path,
) -> anyhow::Result<SubpackageInfo> {
    let manifest_path = path_relative_from_file(subpackage.manifest_path, manifest_path)?;
    let manifest_path = manifest_path.into_string();

    Ok(SubpackageInfo { manifest_path, ..subpackage })
}

fn resolve_subpackage_manifest_path(
    subpackage: SubpackageInfo,
    manifest_path: &Utf8Path,
) -> anyhow::Result<SubpackageInfo> {
    let manifest_path = resolve_path_from_file(&subpackage.manifest_path, manifest_path)
        .with_context(|| {
            format!("Resolving subpackage manifest path: {}", subpackage.manifest_path)
        })?
        .into_string();
    Ok(SubpackageInfo { manifest_path, ..subpackage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_to_string::PathToStringExt;
    use crate::{BlobEntry, MetaPackage, PackageBuilder};
    use assert_matches::assert_matches;
    use camino::Utf8PathBuf;
    use fuchsia_url::RelativePackageUrl;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};
    use tempfile::{NamedTempFile, TempDir};

    const FAKE_ABI_REVISION: version_history::AbiRevision =
        version_history::AbiRevision::from_u64(0x323dd69d73d957a7);

    const HASH_0: Hash = Hash::from_array([0; fuchsia_hash::HASH_SIZE]);
    const HASH_1: Hash = Hash::from_array([1; fuchsia_hash::HASH_SIZE]);
    const HASH_2: Hash = Hash::from_array([2; fuchsia_hash::HASH_SIZE]);
    const HASH_3: Hash = Hash::from_array([3; fuchsia_hash::HASH_SIZE]);
    const HASH_4: Hash = Hash::from_array([4; fuchsia_hash::HASH_SIZE]);

    pub struct TestEnv {
        pub _temp: TempDir,
        pub dir_path: Utf8PathBuf,
        pub manifest_path: Utf8PathBuf,
        pub subpackage_path: Utf8PathBuf,
        pub data_dir: Utf8PathBuf,
    }

    impl TestEnv {
        pub fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let dir_path = Utf8Path::from_path(temp.path()).unwrap().to_path_buf();

            let manifest_dir = dir_path.join("manifest_dir");
            std::fs::create_dir_all(&manifest_dir).unwrap();

            let subpackage_dir = dir_path.join("subpackage_manifests");
            std::fs::create_dir_all(&subpackage_dir).unwrap();

            let data_dir = dir_path.join("data_source");
            std::fs::create_dir_all(&data_dir).unwrap();

            TestEnv {
                _temp: temp,
                dir_path,
                manifest_path: manifest_dir.join("package_manifest.json"),
                subpackage_path: subpackage_dir.join(HASH_0.to_string()),
                data_dir,
            }
        }
    }

    #[test]
    fn test_version1_serialization() {
        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../p1".into(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![],
            repository: None,
            blob_sources_relative: Default::default(),
            delivery_blob_type: None,
            abi_revision: None,
        }));

        assert_eq!(
            serde_json::to_value(manifest).unwrap(),
            json!(
                {
                    "version": "1",
                    "package": {
                        "name": "example",
                        "version": "0"
                    },
                    "blobs": [
                        {
                            "source_path": "../p1",
                            "path": "data/p1",
                            "merkle": "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 1
                        },
                    ]
                }
            )
        );

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../p1".into(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![],
            repository: Some("testrepository.org".into()),
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: Some(FAKE_ABI_REVISION),
        }));

        assert_eq!(
            serde_json::to_value(manifest).unwrap(),
            json!(
                {
                    "version": "1",
                    "repository": "testrepository.org",
                    "package": {
                        "name": "example",
                        "version": "0"
                    },
                    "blobs": [
                        {
                            "source_path": "../p1",
                            "path": "data/p1",
                            "merkle": HASH_0,
                            "size": 1
                        },
                    ],
                    "blob_sources_relative": "file",
                    "abi_revision": "0x323dd69d73d957a7",
                }
            )
        );
    }

    #[test]
    fn test_version1_deserialization() {
        let manifest = serde_json::from_value::<VersionedPackageManifest>(json!(
            {
                "version": "1",
                "repository": "testrepository.org",
                "package": {
                    "name": "example",
                    "version": "0"
                },
                "blobs": [
                    {
                        "source_path": "../p1",
                        "path": "data/p1",
                        "merkle": HASH_0,
                        "size": 1
                    },
                ]
            }
        ))
        .expect("valid json");

        assert_eq!(
            manifest,
            VersionedPackageManifest::Version1(PackageManifestV1 {
                package: PackageMetadata {
                    name: "example".parse().unwrap(),
                    version: "0".parse().unwrap(),
                },
                blobs: vec![BlobInfo {
                    source_path: "../p1".into(),
                    path: "data/p1".into(),
                    merkle: HASH_0,
                    size: 1,
                }],
                subpackages: vec![],
                repository: Some("testrepository.org".into()),
                blob_sources_relative: Default::default(),
                delivery_blob_type: None,
                abi_revision: None,
            })
        );

        let manifest = serde_json::from_value::<VersionedPackageManifest>(json!(
            {
                "version": "1",
                "package": {
                    "name": "example",
                    "version": "0"
                },
                "blobs": [
                    {
                        "source_path": "../p1",
                        "path": "data/p1",
                        "merkle": HASH_0,
                        "size": 1
                    },
                ],
                "blob_sources_relative": "file",
                "abi_revision": "0x323dd69d73d957a7",
            }
        ))
        .expect("valid json");

        assert_eq!(
            manifest,
            VersionedPackageManifest::Version1(PackageManifestV1 {
                package: PackageMetadata {
                    name: "example".parse().unwrap(),
                    version: "0".parse().unwrap(),
                },
                blobs: vec![BlobInfo {
                    source_path: "../p1".into(),
                    path: "data/p1".into(),
                    merkle: HASH_0,
                    size: 1,
                }],
                subpackages: vec![],
                repository: None,
                blob_sources_relative: RelativeTo::File,
                delivery_blob_type: None,
                abi_revision: Some(FAKE_ABI_REVISION),
            })
        )
    }

    #[test]
    fn test_create_package_manifest_from_parts() {
        let meta_package = MetaPackage::from_name_and_variant_zero("package-name".parse().unwrap());
        let mut blobs = BTreeMap::new();
        blobs.insert(
            "bin/my_prog".to_string(),
            BlobEntry { source_path: "src/bin/my_prog".into(), hash: HASH_0, size: 1 },
        );
        let package_manifest =
            PackageManifest::from_parts(meta_package, None, blobs, vec![], FAKE_ABI_REVISION)
                .unwrap();

        assert_eq!(&"package-name".parse::<PackageName>().unwrap(), package_manifest.name());
        assert_eq!(None, package_manifest.repository());
        assert_eq!(Some(FAKE_ABI_REVISION), package_manifest.abi_revision());
    }

    #[test]
    fn test_from_blobs_dir() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let gen_dir = temp_dir.join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();

        let blobs_dir = temp_dir.join("blobs/1");
        std::fs::create_dir_all(&blobs_dir).unwrap();

        let manifests_dir = temp_dir.join("manifests");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        // Helper to write some content into a delivery blob.
        let write_blob = |contents| {
            let hash = fuchsia_merkle::root_from_slice(contents);

            let path = blobs_dir.join(hash.to_string());

            let blob_file = File::create(&path).unwrap();
            delivery_blob::generate_to(DeliveryBlobType::Type1, contents, &blob_file).unwrap();

            (path, hash)
        };

        // Create a package.
        let mut package_builder = PackageBuilder::new("package", FAKE_ABI_REVISION);
        let (file1_path, file1_hash) = write_blob(b"file 1");
        package_builder.add_contents_as_blob("file-1", b"file 1", &gen_dir).unwrap();
        let (file2_path, file2_hash) = write_blob(b"file 2");
        package_builder.add_contents_as_blob("file-2", b"file 2", &gen_dir).unwrap();

        let gen_meta_far_path = temp_dir.join("meta.far");
        let _package_manifest = package_builder.build(&gen_dir, &gen_meta_far_path).unwrap();

        // Compute the meta.far hash, and generate a delivery blob in the blobs/1/ directory.
        let meta_far_bytes = std::fs::read(&gen_meta_far_path).unwrap();
        let (meta_far_path, meta_far_hash) = write_blob(&meta_far_bytes);

        // We should be able to create a manifest from the blob directory that matches the one
        // created by the builder.
        assert_eq!(
            PackageManifest::from_blobs_dir(
                blobs_dir.as_std_path().parent().unwrap(),
                Some(DeliveryBlobType::Type1),
                meta_far_hash,
                manifests_dir.as_std_path()
            )
            .unwrap(),
            PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
                package: PackageMetadata {
                    name: "package".parse().unwrap(),
                    version: PackageVariant::zero(),
                },
                blobs: vec![
                    BlobInfo {
                        source_path: meta_far_path.to_string(),
                        path: PackageManifest::META_FAR_BLOB_PATH.into(),
                        merkle: meta_far_hash,
                        size: 16384,
                    },
                    BlobInfo {
                        source_path: file1_path.to_string(),
                        path: "file-1".into(),
                        merkle: file1_hash,
                        size: 6,
                    },
                    BlobInfo {
                        source_path: file2_path.to_string(),
                        path: "file-2".into(),
                        merkle: file2_hash,
                        size: 6,
                    },
                ],
                subpackages: vec![],
                repository: None,
                blob_sources_relative: RelativeTo::WorkingDir,
                delivery_blob_type: Some(DeliveryBlobType::Type1),
                abi_revision: Some(FAKE_ABI_REVISION),
            }))
        );
    }

    #[test]
    fn test_load_from_simple() {
        let env = TestEnv::new();

        let expected_blob_source_path = &env.data_dir.join("p1").to_string();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: expected_blob_source_path.clone(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![SubpackageInfo {
                manifest_path: env.subpackage_path.to_string(),
                name: "subpackage0".into(),
                merkle: HASH_0,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::WorkingDir,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let manifest_file = File::create(&env.manifest_path).unwrap();
        serde_json::to_writer(manifest_file, &manifest).unwrap();

        let loaded_manifest = PackageManifest::try_load_from(&env.manifest_path).unwrap();
        assert_eq!(loaded_manifest.name(), &"example".parse::<PackageName>().unwrap());

        let (blobs, subpackages) = loaded_manifest.into_blobs_and_subpackages();

        assert_eq!(blobs.len(), 1);
        let blob = blobs.first().unwrap();
        assert_eq!(blob.path, "data/p1");

        assert_eq!(&blob.source_path, expected_blob_source_path);

        assert_eq!(subpackages.len(), 1);
        let subpackage = subpackages.first().unwrap();
        assert_eq!(subpackage.name, "subpackage0");
        assert_eq!(&subpackage.manifest_path, &env.subpackage_path.to_string());
    }

    #[test]
    fn test_load_from_resolves_source_paths() {
        let env = TestEnv::new();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../data_source/p1".into(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![SubpackageInfo {
                manifest_path: "../subpackage_manifests/0000000000000000000000000000000000000000000000000000000000000000".into(),
                name: "subpackage0".into(),
                merkle: HASH_0,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let manifest_file = File::create(&env.manifest_path).unwrap();
        serde_json::to_writer(manifest_file, &manifest).unwrap();

        let loaded_manifest = PackageManifest::try_load_from(&env.manifest_path).unwrap();
        assert_eq!(
            loaded_manifest,
            PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
                package: PackageMetadata {
                    name: "example".parse::<PackageName>().unwrap(),
                    version: "0".parse().unwrap(),
                },
                blobs: vec![BlobInfo {
                    source_path: env.data_dir.join("p1").to_string(),
                    path: "data/p1".into(),
                    merkle: HASH_0,
                    size: 1,
                }],
                subpackages: vec![SubpackageInfo {
                    manifest_path: env.subpackage_path.to_string(),
                    name: "subpackage0".into(),
                    merkle: HASH_0,
                }],
                repository: None,
                blob_sources_relative: RelativeTo::WorkingDir,
                delivery_blob_type: None,
                abi_revision: None,
            }))
        );
    }

    #[test]
    fn test_package_and_subpackage_blobs_meta_far_error() {
        let env = TestEnv::new();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../data_source/p1".into(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![SubpackageInfo {
                manifest_path: format!("../subpackage_manifests/{HASH_0}"),
                name: "subpackage0".into(),
                merkle: HASH_0,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let manifest_file = File::create(&env.manifest_path).unwrap();
        serde_json::to_writer(manifest_file, &manifest).unwrap();

        let sub_manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "sub_manifest".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../data_source/p2".into(),
                path: "data/p2".into(),
                merkle: HASH_1,
                size: 1,
            }],
            subpackages: vec![],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let sub_manifest_file = File::create(&env.subpackage_path).unwrap();
        serde_json::to_writer(sub_manifest_file, &sub_manifest).unwrap();

        let loaded_manifest = PackageManifest::try_load_from(&env.manifest_path).unwrap();

        let result = loaded_manifest.package_and_subpackage_blobs();
        assert_matches!(
            result,
            Err(PackageManifestError::MetaPackage(MetaPackageError::MetaPackageMissing))
        );
    }

    #[test]
    fn test_package_and_subpackage_blobs() {
        let env = TestEnv::new();
        let subsubpackage_dir = &env.dir_path.join("subsubpackage_manifests");

        let expected_subsubpackage_manifest_path =
            subsubpackage_dir.join(HASH_0.to_string()).to_string();

        std::fs::create_dir_all(subsubpackage_dir).unwrap();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![
                BlobInfo {
                    source_path: "../data_source/p0".into(),
                    path: "meta/".into(),
                    merkle: HASH_0,
                    size: 1,
                },
                BlobInfo {
                    source_path: "../data_source/p1".into(),
                    path: "data/p1".into(),
                    merkle: HASH_1,
                    size: 1,
                },
            ],
            subpackages: vec![SubpackageInfo {
                manifest_path: format!("../subpackage_manifests/{HASH_0}"),
                name: "subpackage0".into(),
                merkle: HASH_2,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let manifest_file = File::create(&env.manifest_path).unwrap();
        serde_json::to_writer(manifest_file, &manifest).unwrap();

        let sub_manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "sub_manifest".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![
                BlobInfo {
                    source_path: "../data_source/p2".into(),
                    path: "meta/".into(),
                    merkle: HASH_2,
                    size: 1,
                },
                BlobInfo {
                    source_path: "../data_source/p3".into(),
                    path: "data/p3".into(),
                    merkle: HASH_3,
                    size: 1,
                },
            ],
            subpackages: vec![SubpackageInfo {
                manifest_path: format!("../subsubpackage_manifests/{HASH_0}"),
                name: "subsubpackage0".into(),
                merkle: HASH_4,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let sub_manifest_file = File::create(&env.subpackage_path).unwrap();
        serde_json::to_writer(sub_manifest_file, &sub_manifest).unwrap();

        let sub_sub_manifest =
            PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
                package: PackageMetadata {
                    name: "sub_sub_manifest".parse().unwrap(),
                    version: "0".parse().unwrap(),
                },
                blobs: vec![BlobInfo {
                    source_path: "../data_source/p4".into(),
                    path: "meta/".into(),
                    merkle: HASH_4,
                    size: 1,
                }],
                subpackages: vec![],
                repository: None,
                blob_sources_relative: RelativeTo::File,
                delivery_blob_type: None,
                abi_revision: None,
            }));

        let sub_sub_manifest_file = File::create(expected_subsubpackage_manifest_path).unwrap();
        serde_json::to_writer(sub_sub_manifest_file, &sub_sub_manifest).unwrap();

        let loaded_manifest = PackageManifest::try_load_from(&env.manifest_path).unwrap();

        let (meta_far, contents) = loaded_manifest.package_and_subpackage_blobs().unwrap();
        assert_eq!(
            meta_far,
            BlobInfo {
                source_path: env.data_dir.join("p0").to_string(),
                path: "meta/".into(),
                merkle: HASH_0,
                size: 1,
            }
        );

        // Does not contain top level meta.far
        assert_eq!(
            contents,
            HashMap::from([
                (
                    HASH_1,
                    BlobInfo {
                        source_path: env.data_dir.join("p1").to_string(),
                        path: "data/p1".into(),
                        merkle: HASH_1,
                        size: 1,
                    },
                ),
                (
                    HASH_2,
                    BlobInfo {
                        source_path: env.data_dir.join("p2").to_string(),
                        path: "meta/".into(),
                        merkle: HASH_2,
                        size: 1,
                    },
                ),
                (
                    HASH_3,
                    BlobInfo {
                        source_path: env.data_dir.join("p3").to_string(),
                        path: "data/p3".into(),
                        merkle: HASH_3,
                        size: 1,
                    },
                ),
                (
                    HASH_4,
                    BlobInfo {
                        source_path: env.data_dir.join("p4").to_string(),
                        path: "meta/".into(),
                        merkle: HASH_4,
                        size: 1,
                    },
                ),
            ]),
        );
    }

    #[test]
    fn test_package_and_subpackage_blobs_deduped() {
        let env = TestEnv::new();

        let expected_meta_far_source_path = env.data_dir.join("p0").to_string();
        let expected_blob_source_path_1 = env.data_dir.join("p1").to_string();
        let expected_blob_source_path_2 = env.data_dir.join("p2").to_string();
        let expected_blob_source_path_3 = env.data_dir.join("p3").to_string();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![
                BlobInfo {
                    source_path: "../data_source/p0".into(),
                    path: "meta/".into(),
                    merkle: HASH_0,
                    size: 1,
                },
                BlobInfo {
                    source_path: "../data_source/p1".into(),
                    path: "data/p1".into(),
                    merkle: HASH_1,
                    size: 1,
                },
            ],
            // Note that we're intentionally duplicating the subpackages with
            // separate names.
            subpackages: vec![
                SubpackageInfo {
                    manifest_path: format!("../subpackage_manifests/{HASH_0}"),
                    name: "subpackage0".into(),
                    merkle: HASH_2,
                },
                SubpackageInfo {
                    manifest_path: format!("../subpackage_manifests/{HASH_0}"),
                    name: "subpackage1".into(),
                    merkle: HASH_2,
                },
            ],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let manifest_file = File::create(&env.manifest_path).unwrap();
        serde_json::to_writer(manifest_file, &manifest).unwrap();

        let sub_manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "sub_manifest".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![
                BlobInfo {
                    source_path: "../data_source/p2".into(),
                    path: "meta/".into(),
                    merkle: HASH_2,
                    size: 1,
                },
                BlobInfo {
                    source_path: "../data_source/p3".into(),
                    path: "data/p3".into(),
                    merkle: HASH_3,
                    size: 1,
                },
            ],
            subpackages: vec![],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        serde_json::to_writer(File::create(&env.subpackage_path).unwrap(), &sub_manifest).unwrap();

        let loaded_manifest = PackageManifest::try_load_from(&env.manifest_path).unwrap();

        let (meta_far, contents) = loaded_manifest.package_and_subpackage_blobs().unwrap();
        assert_eq!(
            meta_far,
            BlobInfo {
                source_path: expected_meta_far_source_path,
                path: "meta/".into(),
                merkle: HASH_0,
                size: 1,
            }
        );

        // Does not contain meta.far
        assert_eq!(
            contents,
            HashMap::from([
                (
                    HASH_1,
                    BlobInfo {
                        source_path: expected_blob_source_path_1,
                        path: "data/p1".into(),
                        merkle: HASH_1,
                        size: 1,
                    }
                ),
                (
                    HASH_2,
                    BlobInfo {
                        source_path: expected_blob_source_path_2,
                        path: "meta/".into(),
                        merkle: HASH_2,
                        size: 1,
                    }
                ),
                (
                    HASH_3,
                    BlobInfo {
                        source_path: expected_blob_source_path_3,
                        path: "data/p3".into(),
                        merkle: HASH_3,
                        size: 1,
                    }
                ),
            ])
        );
    }

    #[test]
    fn test_from_package_archive_bogus() {
        let temp_blobs = TempDir::new().unwrap();
        let temp_manifest = TempDir::new().unwrap();
        let temp_archive = TempDir::new().unwrap();

        let result = PackageManifest::from_archive(
            temp_archive.path(),
            temp_blobs.path(),
            temp_manifest.path(),
        );
        assert!(result.is_err())
    }

    #[fuchsia::test]
    async fn test_from_package_manifest_archive_manifest() {
        let outdir = TempDir::new().unwrap();

        let sub_outdir = outdir.path().join("subpackage_manifests");
        std::fs::create_dir(&sub_outdir).unwrap();

        // Create a file to write to the sub package metafar
        let sub_far_source_file_path = NamedTempFile::new_in(&sub_outdir).unwrap();
        std::fs::write(&sub_far_source_file_path, "some data for sub far").unwrap();

        // Create a file to include as a blob
        let sub_blob_source_file_path = sub_outdir.as_path().join("sub_blob_a");
        let blob_contents = "sub some data for blob";
        std::fs::write(&sub_blob_source_file_path, blob_contents).unwrap();

        // Create a file to include as a blob
        let sub_blob_source_file_path2 = sub_outdir.as_path().join("sub_blob_b");
        let blob_contents = "sub some data for blob2";
        std::fs::write(&sub_blob_source_file_path2, blob_contents).unwrap();

        // Create the sub builder
        let mut sub_builder = PackageBuilder::new("some_pkg_name", FAKE_ABI_REVISION);
        sub_builder
            .add_file_as_blob(
                "sub_blob_a",
                sub_blob_source_file_path.as_path().path_to_string().unwrap(),
            )
            .unwrap();
        sub_builder
            .add_file_as_blob(
                "sub_blob_b",
                sub_blob_source_file_path2.as_path().path_to_string().unwrap(),
            )
            .unwrap();
        sub_builder
            .add_file_to_far(
                "meta/some/file",
                sub_far_source_file_path.path().path_to_string().unwrap(),
            )
            .unwrap();

        let sub_metafar_path = sub_outdir.as_path().join("meta.far");
        let sub_manifest = sub_builder.build(&sub_outdir, &sub_metafar_path).unwrap();

        let manifest_tempdir = TempDir::new().unwrap();
        let manifest_outdir = manifest_tempdir.path();
        let subpackage_manifest_path =
            manifest_outdir.join(format!("{}_package_manifest.json", sub_manifest.hash()));

        serde_json::to_writer(
            std::fs::File::create(&subpackage_manifest_path).unwrap(),
            &sub_manifest,
        )
        .unwrap();

        let subpackage_url = "subpackage_manifests".parse::<RelativePackageUrl>().unwrap();

        let metafar_path = outdir.path().join("meta.far");

        // Create a file to write to the package metafar
        let far_source_file_path = NamedTempFile::new_in(&outdir).unwrap();
        std::fs::write(&far_source_file_path, "some data for far").unwrap();

        // Create a file to include as a blob
        let blob_source_file_path = outdir.path().join("blob_c");
        let blob_contents = "some data for blob";
        std::fs::write(&blob_source_file_path, blob_contents).unwrap();

        // Create a file to include as a blob
        let blob_source_file_path2 = outdir.path().join("blob_d");
        let blob_contents = "some data for blob2";
        std::fs::write(&blob_source_file_path2, blob_contents).unwrap();

        // Create the builder
        let mut builder = PackageBuilder::new("some_pkg_name", FAKE_ABI_REVISION);
        builder
            .add_file_as_blob("blob_c", blob_source_file_path.as_path().path_to_string().unwrap())
            .unwrap();
        builder
            .add_file_as_blob("blob_d", blob_source_file_path2.as_path().path_to_string().unwrap())
            .unwrap();
        builder
            .add_file_to_far(
                "meta/some/file",
                far_source_file_path.path().path_to_string().unwrap(),
            )
            .unwrap();
        builder
            .add_subpackage(&subpackage_url, sub_manifest.hash(), subpackage_manifest_path)
            .unwrap();

        // Build the package
        let manifest = builder.build(&outdir, &metafar_path).unwrap();

        let archive_outdir = TempDir::new().unwrap();
        let archive_path = archive_outdir.path().join("test.far");
        let archive_file = File::create(archive_path.clone()).unwrap();
        manifest.clone().archive(&outdir, &archive_file).await.unwrap();

        let blobs_tempdir = TempDir::new().unwrap();
        let blobs_outdir = blobs_tempdir.path();

        let manifest_2 =
            PackageManifest::from_archive(&archive_path, blobs_outdir, manifest_outdir).unwrap();
        assert_eq!(manifest_2.package_path(), manifest.package_path());

        let (_blob1_info, all_blobs_1) = manifest.package_and_subpackage_blobs().unwrap();
        let (_blob2_info, mut all_blobs_2) = manifest_2.package_and_subpackage_blobs().unwrap();

        for (merkle, blob1) in all_blobs_1 {
            let blob2 = all_blobs_2.remove_entry(&merkle).unwrap().1;
            assert_eq!(
                std::fs::read(&blob1.source_path).unwrap(),
                std::fs::read(&blob2.source_path).unwrap(),
            );
        }

        assert!(all_blobs_2.is_empty());
    }

    #[test]
    fn test_write_package_manifest_already_relative() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let data_dir = temp_dir.join("data_source");
        let subpackage_dir = temp_dir.join("subpackage_manifests");
        let manifest_dir = temp_dir.join("manifest_dir");
        let manifest_path = manifest_dir.join("package_manifest.json");

        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&subpackage_dir).unwrap();
        std::fs::create_dir_all(&manifest_dir).unwrap();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: "../data_source/p1".into(),
                path: "data/p1".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![SubpackageInfo {
                manifest_path: format!("../subpackage_manifests/{HASH_0}"),
                name: "subpackage0".into(),
                merkle: HASH_0,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::File,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let result_manifest = manifest.clone().write_with_relative_paths(&manifest_path).unwrap();

        // The manifest should not have been changed in this case.
        assert_eq!(result_manifest, manifest);

        let parsed_manifest: Value =
            serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();
        let object = parsed_manifest.as_object().unwrap();
        let version = object.get("version").unwrap();

        let blobs_value = object.get("blobs").unwrap();
        let blobs = blobs_value.as_array().unwrap();
        let blob_value = blobs.first().unwrap();
        let blob = blob_value.as_object().unwrap();
        let source_path_value = blob.get("source_path").unwrap();
        let source_path = source_path_value.as_str().unwrap();

        let subpackages_value = object.get("subpackages").unwrap();
        let subpackages = subpackages_value.as_array().unwrap();
        let subpackage_value = subpackages.first().unwrap();
        let subpackage = subpackage_value.as_object().unwrap();
        let subpackage_manifest_path_value = subpackage.get("manifest_path").unwrap();
        let subpackage_manifest_path = subpackage_manifest_path_value.as_str().unwrap();

        assert_eq!(version, "1");
        assert_eq!(source_path, "../data_source/p1");
        assert_eq!(subpackage_manifest_path, format!("../subpackage_manifests/{HASH_0}"));
    }

    #[test]
    fn test_write_package_manifest_making_paths_relative() {
        let temp = TempDir::new().unwrap();
        let temp_dir = Utf8Path::from_path(temp.path()).unwrap();

        let data_dir = temp_dir.join("data_source");
        let subpackage_dir = temp_dir.join("subpackage_manifests");
        let manifest_dir = temp_dir.join("manifest_dir");
        let manifest_path = manifest_dir.join("package_manifest.json");
        let manifest_dir_2 = temp_dir.join("subdir").join("other_manifest_dir");
        let manifest_path_2 = manifest_dir_2.join("package_manifest.json");
        let blob_source_path = data_dir.join("p2").to_string();
        let subpackage_manifest_path = subpackage_dir.join(HASH_1.to_string()).to_string();

        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&subpackage_dir).unwrap();
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::create_dir_all(&manifest_dir_2).unwrap();

        let manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "example".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![BlobInfo {
                source_path: blob_source_path,
                path: "data/p2".into(),
                merkle: HASH_0,
                size: 1,
            }],
            subpackages: vec![SubpackageInfo {
                manifest_path: subpackage_manifest_path,
                name: "subpackage1".into(),
                merkle: HASH_1,
            }],
            repository: None,
            blob_sources_relative: RelativeTo::WorkingDir,
            delivery_blob_type: None,
            abi_revision: None,
        }));

        let result_manifest = manifest.clone().write_with_relative_paths(&manifest_path).unwrap();
        let result_manifest_2 =
            manifest.clone().write_with_relative_paths(&manifest_path_2).unwrap();

        // The manifests should be equivalent, even if the paths have been relativized to
        // directories at different depths (which results in different manifest_paths).
        assert_eq!(result_manifest, result_manifest_2);
        assert_ne!(
            result_manifest.subpackages()[0].manifest_path,
            result_manifest_2.subpackages()[0].manifest_path
        );

        let blob = result_manifest.blobs().first().unwrap();
        assert_eq!(blob.source_path, "../data_source/p2");
        let subpackage = result_manifest.subpackages().first().unwrap();
        assert_eq!(subpackage.manifest_path, format!("../subpackage_manifests/{HASH_1}"));

        let parsed_manifest: serde_json::Value =
            serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();

        let object = parsed_manifest.as_object().unwrap();

        let blobs_value = object.get("blobs").unwrap();
        let blobs = blobs_value.as_array().unwrap();
        let blob_value = blobs.first().unwrap();
        let blob = blob_value.as_object().unwrap();
        let source_path_value = blob.get("source_path").unwrap();
        let source_path = source_path_value.as_str().unwrap();

        let subpackages_value = object.get("subpackages").unwrap();
        let subpackages = subpackages_value.as_array().unwrap();
        let subpackage_value = subpackages.first().unwrap();
        let subpackage = subpackage_value.as_object().unwrap();
        let subpackage_manifest_path_value = subpackage.get("manifest_path").unwrap();
        let subpackage_manifest_path = subpackage_manifest_path_value.as_str().unwrap();

        assert_eq!(source_path, "../data_source/p2");
        assert_eq!(subpackage_manifest_path, format!("../subpackage_manifests/{HASH_1}"));
    }

    #[test]
    fn test_set_name() {
        let mut manifest = PackageManifest(VersionedPackageManifest::Version1(PackageManifestV1 {
            package: PackageMetadata {
                name: "original-name".parse().unwrap(),
                version: "0".parse().unwrap(),
            },
            blobs: vec![],
            subpackages: vec![],
            repository: None,
            blob_sources_relative: Default::default(),
            delivery_blob_type: None,
            abi_revision: None,
        }));

        assert_eq!(manifest.name(), &"original-name".parse::<PackageName>().unwrap());

        let new_name = "new-name".parse().unwrap();
        manifest.set_name(new_name);

        assert_eq!(manifest.name(), &"new-name".parse::<PackageName>().unwrap());
    }
}

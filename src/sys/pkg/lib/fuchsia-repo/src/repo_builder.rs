// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::repo_keys::RepoKeys;
use crate::repository::RepoStorageProvider;
use anyhow::{Context, Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Duration, Utc};
use delivery_blob::DeliveryBlobType;
use fuchsia_merkle::Hash;
use fuchsia_pkg::{BlobInfo, PackageManifest, PackageManifestList, PackagePath, SubpackageInfo};
use futures::stream::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map};
use std::fs::{self, File};
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::pin::Pin;
use tempfile::TempDir;
use tuf::Database;
use tuf::crypto::HashAlgorithm;
use tuf::metadata::TargetPath;
use tuf::pouf::Pouf1;
use tuf::repo_builder::RepoBuilder as TufRepoBuilder;

#[cfg(not(target_os = "fuchsia"))]
use crate::repo_client::RepoClient;

/// Number of days from now before the root metadata is expired.
const DEFAULT_ROOT_EXPIRATION: i64 = 365;

/// Number of days from now before the targets metadata is expired.
const DEFAULT_TARGETS_EXPIRATION: i64 = 90;

/// Number of days from now before the snapshot metadata is expired.
const DEFAULT_SNAPSHOT_EXPIRATION: i64 = 30;

/// Number of days from now before the timestamp metadata is expired.
const DEFAULT_TIMESTAMP_EXPIRATION: i64 = 30;

#[derive(Debug)]
struct ToBeStagedPackage {
    manifest_path: Option<Utf8PathBuf>,
    kind: ToBeStagedPackageKind,
}

#[derive(Debug)]
enum ToBeStagedPackageKind {
    Manifest { manifest: PackageManifest },
    Archive { _archive_out: TempDir, manifest: PackageManifest },
}

impl ToBeStagedPackage {
    fn manifest(&self) -> &PackageManifest {
        match &self.kind {
            ToBeStagedPackageKind::Manifest { manifest } => manifest,
            ToBeStagedPackageKind::Archive { manifest, .. } => manifest,
        }
    }
}

#[derive(Debug)]
struct StagedBlob {
    info: BlobInfo,
    delivery_blob_type: Option<DeliveryBlobType>,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "blob {hash} at {path:?} is {file_size} bytes in size, \
     but the package manifest indicates it should be {manifest_size} bytes in size"
)]
struct BlobSizeMismatchError {
    hash: Hash,
    path: String,
    manifest_size: u64,
    file_size: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("Blobs {hash1} and {hash2} refer to the same hardlinked file")]
struct MerkleHardLinkMismatchError {
    hash1: Hash,
    hash2: Hash,
}

/// RepoBuilder can create and manipulate package repositories.
#[derive(Debug)]
pub struct RepoBuilder<'a, R: RepoStorageProvider> {
    signing_repo_keys: Option<&'a RepoKeys>,
    trusted_repo_keys: &'a RepoKeys,
    database: Option<&'a Database<Pouf1>>,
    repo: R,
    ignore_missing_packages: bool,
    current_time: DateTime<Utc>,
    time_versioning: bool,
    refresh_metadata: bool,
    refresh_non_root_metadata: bool,
    inherit_from_trusted_targets: bool,
    named_packages: HashMap<PackagePath, Hash>,
    staged_packages: HashMap<Hash, ToBeStagedPackage>,
    staged_blobs: HashMap<Hash, StagedBlob>,
    deps: HashSet<Utf8PathBuf>,
}

#[cfg(not(target_os = "fuchsia"))]
impl<'a, R> RepoBuilder<'a, &'a R>
where
    R: RepoStorageProvider,
{
    pub fn from_client(
        client: &'a RepoClient<R>,
        repo_keys: &'a RepoKeys,
    ) -> RepoBuilder<'a, &'a R> {
        Self::from_database(client.remote_repo(), repo_keys, client.database())
    }
}

impl<'a, R> RepoBuilder<'a, R>
where
    R: RepoStorageProvider,
{
    pub fn create(repo: R, repo_keys: &'a RepoKeys) -> RepoBuilder<'a, R> {
        Self::new(repo, repo_keys, None)
    }

    pub fn from_database(
        repo: R,
        repo_keys: &'a RepoKeys,
        database: &'a Database<Pouf1>,
    ) -> RepoBuilder<'a, R> {
        Self::new(repo, repo_keys, Some(database))
    }

    fn new(
        repo: R,
        trusted_repo_keys: &'a RepoKeys,
        database: Option<&'a Database<Pouf1>>,
    ) -> RepoBuilder<'a, R> {
        RepoBuilder {
            repo,
            signing_repo_keys: None,
            trusted_repo_keys,
            database,
            ignore_missing_packages: false,
            current_time: Utc::now(),
            time_versioning: false,
            refresh_metadata: false,
            refresh_non_root_metadata: false,
            inherit_from_trusted_targets: true,
            named_packages: HashMap::new(),
            staged_packages: HashMap::new(),
            staged_blobs: HashMap::new(),
            deps: HashSet::new(),
        }
    }

    pub fn signing_repo_keys(mut self, signing_repo_keys: &'a RepoKeys) -> Self {
        self.signing_repo_keys = Some(signing_repo_keys);
        self
    }

    pub fn current_time(mut self, current_time: DateTime<Utc>) -> Self {
        self.current_time = current_time;
        self
    }

    pub fn time_versioning(mut self, time_versioning: bool) -> Self {
        self.time_versioning = time_versioning;
        self
    }

    /// Always generate new root, targets, snapshot, and timestamp metadata, even if unchanged and
    /// not expired.
    pub fn refresh_metadata(mut self, refresh_metadata: bool) -> Self {
        self.refresh_metadata = refresh_metadata;
        self
    }

    /// Generate a new targets, snapshot, and timestamp metadata, even if unchanged and not expired.
    pub fn refresh_non_root_metadata(mut self, refresh_non_root_metadata: bool) -> Self {
        self.refresh_non_root_metadata = refresh_non_root_metadata;
        self
    }

    /// Whether or not the new targets metadata inherits targets and delegations from the trusted
    /// targets metadata.
    ///
    /// Default is `true`.
    pub fn inherit_from_trusted_targets(mut self, inherit_from_trusted_targets: bool) -> Self {
        self.inherit_from_trusted_targets = inherit_from_trusted_targets;
        self
    }

    /// Whether or not to raise an error if package does not exist.
    pub fn ignore_missing_packages(mut self, ignore_missing_packages: bool) -> Self {
        self.ignore_missing_packages = ignore_missing_packages;
        self
    }

    /// Stage a package manifest from the `path` to be published.
    pub async fn add_package(self, path: Utf8PathBuf) -> Result<RepoBuilder<'a, R>> {
        match fs::read(path.as_std_path()) {
            Ok(contents) => {
                let package = PackageManifest::from_reader(&path, &contents[..])
                    .with_context(|| format!("reading package manifest {path}"))?;

                self.add_package_manifest(Some(path), package).await
            }
            Err(err) => {
                if self.ignore_missing_packages && err.kind() == std::io::ErrorKind::NotFound {
                    Ok(self)
                } else {
                    Err(err).with_context(|| format!("reading package manifest {path}"))
                }
            }
        }
    }

    /// Stage the package manifests from the iterator of paths to be published.
    pub async fn add_packages(
        mut self,
        paths: impl Iterator<Item = Utf8PathBuf>,
    ) -> Result<RepoBuilder<'a, R>> {
        for path in paths {
            self = self.add_package(path).await?;
        }
        Ok(self)
    }

    /// Stage a package manifest, which was optionally loaded from `path`, to be published.
    pub async fn add_package_manifest(
        self,
        path: Option<Utf8PathBuf>,
        manifest: PackageManifest,
    ) -> Result<RepoBuilder<'a, R>> {
        self.stage_named_package(
            manifest.package_path(),
            ToBeStagedPackage {
                manifest_path: path,
                kind: ToBeStagedPackageKind::Manifest { manifest },
            },
        )
        .await
    }

    /// Stage all the top-level package manifests from `iter` to be published.
    pub async fn add_package_manifests(
        mut self,
        iter: impl Iterator<Item = (Option<Utf8PathBuf>, PackageManifest)>,
    ) -> Result<RepoBuilder<'a, R>> {
        for (path, package) in iter {
            self = self.add_package_manifest(path, package).await?;
        }
        Ok(self)
    }

    /// Stage a package archive from the `path` to be published.
    pub async fn add_package_archive(self, path: Utf8PathBuf) -> Result<RepoBuilder<'a, R>> {
        if self.ignore_missing_packages && !path.exists() {
            return Ok(self);
        }

        let blobs_out = TempDir::new().unwrap();
        let manifest_out = TempDir::new().unwrap();
        let manifest = PackageManifest::from_archive(
            path.as_std_path(),
            blobs_out.path(),
            manifest_out.path(),
        )
        .with_context(|| format!("reading package archive {path}"))
        .expect("archive to manifest");

        self.stage_named_package(
            manifest.package_path(),
            ToBeStagedPackage {
                manifest_path: Some(path),
                kind: ToBeStagedPackageKind::Archive { _archive_out: blobs_out, manifest },
            },
        )
        .await
    }

    /// Stage the package archives from the iterator of paths to be published.
    pub async fn add_package_archives(
        mut self,
        paths: impl Iterator<Item = Utf8PathBuf>,
    ) -> Result<RepoBuilder<'a, R>> {
        for path in paths {
            self = self.add_package_archive(path).await?;
        }
        Ok(self)
    }

    /// Stage a top-level package manifest described by `package` from the
    /// `path` to be published. Duplicates are ignored unless registering two
    /// packages with the same package path and different package hashes.
    async fn stage_named_package(
        mut self,
        package_path: PackagePath,
        package: ToBeStagedPackage,
    ) -> Result<RepoBuilder<'a, R>> {
        let package_hash = package.manifest().hash();

        // We don't want to stage any blobs if we have `ignore_missing_files ==
        // true`, and a package or subpackage is missing, so we'll track that
        // separately before we decide to commit to stage this package.
        let mut staged_blobs = HashMap::new();
        let mut staged_packages = HashMap::new();
        let mut deps = HashSet::new();

        let did_stage_package = self
            .stage_package(package, &mut staged_packages, &mut staged_blobs, &mut deps)
            .await
            .with_context(|| format!("staging package path '{package_path}'"))?;

        // Exit early if we did not stage the package.
        if !did_stage_package {
            return Ok(self);
        }

        // Since we successfully processed this package, merge in all the
        // package hashes and blobs.
        self.staged_packages.extend(staged_packages);
        self.staged_blobs.extend(staged_blobs);
        self.deps.extend(deps);

        match self.named_packages.entry(package_path) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(package_hash);
            }
            hash_map::Entry::Occupied(entry) => {
                let old_package = self.staged_packages.get(entry.get()).unwrap();
                let new_package = self.staged_packages.get(&package_hash).unwrap();

                check_manifests_are_equivalent(old_package, new_package)
                    .with_context(|| format!("staging package path '{}'", entry.key()))?;
            }
        }

        Ok(self)
    }

    /// Try to stage a package and the blobs inside of it for publishing to the
    /// package repository.
    ///
    /// The behavior of this function depends on the `ignore_missing_packages`
    /// setting. If it is true, this will exit early and not stage any files. If
    /// false, this will return an error.
    ///
    /// This returns `true` if the package was staged, or `false` if any of the
    /// package contents were missing.
    async fn stage_package(
        &self,
        package: ToBeStagedPackage,
        to_be_staged_packages: &mut HashMap<Hash, ToBeStagedPackage>,
        to_be_staged_blobs: &mut HashMap<Hash, StagedBlob>,
        deps: &mut HashSet<Utf8PathBuf>,
    ) -> Result<bool> {
        if let Some(path) = &package.manifest_path {
            deps.insert(Utf8PathBuf::from(path));
        }

        let package_hash = package.manifest().hash();

        // We'll only add the package manifest if we haven't already staged a manifest for this hash.
        if self.staged_packages.contains_key(&package_hash)
            || to_be_staged_packages.contains_key(&package_hash)
        {
            return Ok(true);
        }

        // Determine the set of blobs in the package that are unique.
        let mut unique_package_blobs = vec![];
        let package_blobs = package.manifest().blobs();
        let mut seen_paths = HashSet::new();
        let mut seen_hashes = HashSet::new();
        for blob in package_blobs {
            if seen_paths.insert(&blob.source_path) && seen_hashes.insert(&blob.merkle) {
                unique_package_blobs.push(blob.to_owned());
            }
        }

        // Rust doesn't know we've fully processed the stream, so we need to
        // explicitly drop it so it knows we're done borrowing values.
        let blobs = {
            let to_be_staged_blobs = &*to_be_staged_blobs;

            // Iterate over the blobs in parallel and check if they exist, ignoring
            // any that we've already staged.

            let stream = futures::stream::iter(unique_package_blobs.iter())
                .filter_map(|blob| async move {
                    if self.staged_blobs.contains_key(&blob.merkle)
                        || to_be_staged_blobs.contains_key(&blob.merkle)
                    {
                        None
                    } else {
                        Some(async move {
                            let result = fs::metadata(&blob.source_path);
                            (blob, result)
                        })
                    }
                })
                .buffer_unordered(std::thread::available_parallelism()?.get());

            // Gather up the results. If `ignore_missing_files` is true and any of
            // the files are missing, exit early. Otherwise error out.
            let mut blobs = vec![];
            let mut seen_links = HashMap::new();

            let mut stream = std::pin::pin!(stream);

            while let Some((blob, result)) = stream.next().await {
                match result {
                    Ok(metadata) => {
                        // Skip over any hardlinks we've already seen.
                        if let Some(first_merkle) =
                            seen_links.insert((metadata.dev(), metadata.ino()), blob.merkle)
                        {
                            if first_merkle == blob.merkle {
                                continue;
                            } else {
                                return Err(anyhow!(MerkleHardLinkMismatchError {
                                    hash1: first_merkle,
                                    hash2: blob.merkle
                                }));
                            }
                        }

                        if package.manifest().delivery_blob_type().is_none()
                            && blob.size != metadata.len()
                        {
                            if self.ignore_missing_packages {
                                return Ok(false);
                            } else {
                                return Err(anyhow!(BlobSizeMismatchError {
                                    hash: blob.merkle,
                                    path: blob.source_path.clone(),
                                    manifest_size: blob.size,
                                    file_size: metadata.len(),
                                }));
                            }
                        }

                        blobs.push(blob);
                    }
                    Err(err) => {
                        if self.ignore_missing_packages
                            && err.kind() == std::io::ErrorKind::NotFound
                        {
                            return Ok(false);
                        } else {
                            return Err(err).with_context(|| {
                                format!("checking if {} exists", blob.source_path)
                            });
                        }
                    }
                }
            }

            blobs
        };

        // Merge our blobs in to be staged. We can't do the merge above because
        // the stream has an immutable reference to `to_be_staged_blobs` so it
        // can filter our already staged blobs.
        for blob in blobs {
            deps.insert(Utf8PathBuf::from(&blob.source_path));
            to_be_staged_blobs.insert(
                blob.merkle,
                StagedBlob {
                    info: blob.clone(),
                    delivery_blob_type: package.manifest().delivery_blob_type(),
                },
            );
        }

        // Stage all subpackages.
        for subpackage in package.manifest().subpackages() {
            // We only need to stage the subpackage if we haven't staged this merkle.
            if !self.staged_packages.contains_key(&subpackage.merkle)
                && !to_be_staged_packages.contains_key(&subpackage.merkle)
            {
                let manifest_path = Utf8PathBuf::from(&subpackage.manifest_path);

                // Don't stage the package if any subpackages are missing.
                if !self
                    .stage_subpackage(
                        manifest_path,
                        to_be_staged_packages,
                        to_be_staged_blobs,
                        deps,
                    )
                    .await?
                {
                    return Ok(false);
                }
            }
        }

        to_be_staged_packages.insert(package_hash, package);

        Ok(true)
    }

    /// Stage a subpackage's package manifest from the `path` to be published.
    async fn stage_subpackage(
        &self,
        path: Utf8PathBuf,
        to_be_staged_packages: &mut HashMap<Hash, ToBeStagedPackage>,
        to_be_staged_blobs: &mut HashMap<Hash, StagedBlob>,
        deps: &mut HashSet<Utf8PathBuf>,
    ) -> Result<bool> {
        let contents = match fs::read(path.as_std_path()) {
            Ok(contents) => contents,
            Err(err) => {
                if self.ignore_missing_packages && err.kind() == std::io::ErrorKind::NotFound {
                    return Ok(false);
                }

                return Err(err).with_context(|| format!("reading package manifest {path}"));
            }
        };

        let manifest = PackageManifest::from_reader(&path, &contents[..])
            .with_context(|| format!("parsing package manifest {path}"))?;

        // Stage the subpackage. We will use `stage_package_contents` since we don't need to include
        // the package in the TUF metadata. We're recursing, so we need to box our future.
        let fut: Pin<Box<dyn Future<Output = _>>> = Box::pin(self.stage_package(
            ToBeStagedPackage {
                manifest_path: Some(path),
                kind: ToBeStagedPackageKind::Manifest { manifest },
            },
            to_be_staged_packages,
            to_be_staged_blobs,
            deps,
        ));

        fut.await
    }

    /// Stage all the packages pointed to by the package list to be published.
    /// Paths in the package list file are relative to the directory that contains the package list.
    pub async fn add_package_list(mut self, path: Utf8PathBuf) -> Result<RepoBuilder<'a, R>> {
        let contents = fs::read(path.as_std_path())
            .with_context(|| format!("reading package manifest list {path}"))?;

        let package_list_manifest = PackageManifestList::from_reader(&path, &contents[..])
            .with_context(|| format!("reading package manifest list {path}"))?;

        self.deps.insert(path.clone());

        self.add_packages(package_list_manifest.into_iter()).await
    }

    /// Stage all the packages pointed to by the iterator of package lists to be published.
    pub async fn add_package_lists(
        mut self,
        paths: impl Iterator<Item = Utf8PathBuf>,
    ) -> Result<RepoBuilder<'a, R>> {
        for path in paths {
            self = self.add_package_list(path).await?;
        }

        Ok(self)
    }

    /// Read all remaining subpackages, and then commit the changes to the
    /// repository.
    ///
    /// Returns the list of the files that were read and the staged blobs.
    pub async fn commit(self) -> Result<(HashSet<Utf8PathBuf>, BTreeSet<BlobInfo>)> {
        let repo_builder = if let Some(database) = self.database.as_ref() {
            TufRepoBuilder::from_database(&self.repo, database)
        } else {
            TufRepoBuilder::create(&self.repo)
        };

        // Create a repo builder for the metadata, and initialize it with our repository keys.
        let mut repo_builder = repo_builder
            .current_time(self.current_time)
            .time_versioning(self.time_versioning)
            .root_expiration_duration(Duration::days(DEFAULT_ROOT_EXPIRATION))
            .targets_expiration_duration(Duration::days(DEFAULT_TARGETS_EXPIRATION))
            .snapshot_expiration_duration(Duration::days(DEFAULT_SNAPSHOT_EXPIRATION))
            .timestamp_expiration_duration(Duration::days(DEFAULT_TIMESTAMP_EXPIRATION));

        if let Some(signing_repo_keys) = self.signing_repo_keys {
            for key in signing_repo_keys.root_keys() {
                repo_builder = repo_builder.signing_root_keys(&[&**key]);
            }

            for key in signing_repo_keys.targets_keys() {
                repo_builder = repo_builder.signing_targets_keys(&[&**key]);
            }

            for key in signing_repo_keys.snapshot_keys() {
                repo_builder = repo_builder.signing_snapshot_keys(&[&**key]);
            }

            for key in signing_repo_keys.timestamp_keys() {
                repo_builder = repo_builder.signing_timestamp_keys(&[&**key]);
            }
        }

        for key in self.trusted_repo_keys.root_keys() {
            repo_builder = repo_builder.trusted_root_keys(&[&**key]);
        }

        for key in self.trusted_repo_keys.targets_keys() {
            repo_builder = repo_builder.trusted_targets_keys(&[&**key]);
        }

        for key in self.trusted_repo_keys.snapshot_keys() {
            repo_builder = repo_builder.trusted_snapshot_keys(&[&**key]);
        }

        for key in self.trusted_repo_keys.timestamp_keys() {
            repo_builder = repo_builder.trusted_timestamp_keys(&[&**key]);
        }

        // We can't generate a new root if we don't have any root keys.
        let mut repo_builder = if self.trusted_repo_keys.root_keys().is_empty() {
            repo_builder.skip_root()
        } else if self.refresh_metadata {
            repo_builder.stage_root()?
        } else {
            repo_builder.stage_root_if_necessary()?
        };

        repo_builder = repo_builder
            .inherit_from_trusted_targets(self.inherit_from_trusted_targets)
            .target_hash_algorithms(&[HashAlgorithm::Sha512]);

        let mut package_meta_fars = HashMap::new();
        for (package_path, package_hash) in &self.named_packages {
            let meta_far_blob = self.staged_blobs.get(package_hash).unwrap();
            package_meta_fars.insert(package_path, meta_far_blob);
        }

        // Stage the metadata blobs.
        for (package_path, meta_far_blob) in package_meta_fars {
            let target_path = TargetPath::new(package_path.to_string())?;
            let mut custom = HashMap::new();

            custom.insert("merkle".into(), serde_json::to_value(meta_far_blob.info.merkle)?);
            custom.insert("size".into(), serde_json::to_value(meta_far_blob.info.size)?);

            match meta_far_blob.delivery_blob_type {
                Some(_) => {
                    let delivery_blob = std::fs::read(&meta_far_blob.info.source_path)?;
                    let meta_far_blob =
                        delivery_blob::decompress(&delivery_blob).with_context(|| {
                            format!(
                                "decompressing delivery blob {}",
                                meta_far_blob.info.source_path
                            )
                        })?;

                    repo_builder = repo_builder
                        .add_target_with_custom(
                            target_path,
                            futures::io::Cursor::new(meta_far_blob),
                            custom,
                        )
                        .await?;
                }
                None => {
                    let f = File::open(&meta_far_blob.info.source_path)?;

                    repo_builder = repo_builder
                        .add_target_with_custom(
                            target_path,
                            futures::io::AllowStdIo::new(f),
                            custom,
                        )
                        .await?;
                }
            }
        }

        // Stage the targets metadata. If we're forcing a metadata refresh, force a new targets,
        // snapshot, and timestamp, even if nothing changed in the contents.
        let repo_builder = if self.refresh_metadata || self.refresh_non_root_metadata {
            repo_builder.stage_targets()?
        } else {
            repo_builder.stage_targets_if_necessary()?
        };

        let repo_builder = repo_builder
            .snapshot_includes_length(true)
            .snapshot_includes_hashes(&[HashAlgorithm::Sha512]);

        let repo_builder = if self.refresh_metadata || self.refresh_non_root_metadata {
            repo_builder.stage_snapshot()?
        } else {
            repo_builder.stage_snapshot_if_necessary()?
        };

        let repo_builder = repo_builder
            .timestamp_includes_length(true)
            .timestamp_includes_hashes(&[HashAlgorithm::Sha512]);

        let repo_builder = if self.refresh_metadata || self.refresh_non_root_metadata {
            repo_builder.stage_timestamp()?
        } else {
            repo_builder.stage_timestamp_if_necessary()?
        };

        repo_builder.commit().await.context("publishing metadata")?;

        // Stage the blobs.
        let () = futures::stream::iter(&self.staged_blobs)
            .map(Ok)
            .try_for_each_concurrent(
                std::thread::available_parallelism()?.get(),
                |(blob_hash, blob)| match blob.delivery_blob_type {
                    Some(delivery_blob_type) => self.repo.store_delivery_blob(
                        blob_hash,
                        Utf8Path::new(&blob.info.source_path),
                        delivery_blob_type,
                    ),
                    None => self.repo.store_blob(
                        blob_hash,
                        blob.info.size,
                        Utf8Path::new(&blob.info.source_path),
                    ),
                },
            )
            .await?;

        Ok((self.deps, self.staged_blobs.into_values().map(|blob| blob.info).collect()))
    }
}

fn check_manifests_are_equivalent(
    old_package: &ToBeStagedPackage,
    new_package: &ToBeStagedPackage,
) -> Result<()> {
    // Check if the packages conflict.
    if old_package.manifest().hash() == new_package.manifest().hash() {
        return Ok(());
    }

    // Create a message that tries to explain why we have a conflict.
    let old_manifest_path =
        old_package.manifest_path.as_ref().map(|path| path.as_str()).unwrap_or("<generated>");
    let new_manifest_path =
        new_package.manifest_path.as_ref().map(|path| path.as_str()).unwrap_or("<generated>");

    let mut msg = vec![format!(
        "conflict between package manifests\
        \n  manifest paths:\
        \n  - {old_manifest_path}\
        \n  - {new_manifest_path}\
        \n  differences:",
    )];

    #[derive(PartialEq, Eq)]
    enum BlobEntry {
        Contents(Vec<u8>),
        Blob(BlobInfo),
    }

    // Helper to read in all the package contents so we can compare entries.
    fn manifest_contents(manifest: &PackageManifest) -> Result<BTreeMap<String, BlobEntry>> {
        let mut entries = BTreeMap::new();

        for blob in manifest.blobs() {
            if blob.path == "meta/" {
                let file = std::fs::File::open(&blob.source_path)
                    .with_context(|| format!("reading {}", blob.path))?;
                let mut far = fuchsia_archive::Utf8Reader::new(file)?;

                let far_entries =
                    far.list().map(|entry| entry.path().to_owned()).collect::<Vec<_>>();
                for path in far_entries {
                    let contents = far.read_file(&path)?;
                    entries.insert(path, BlobEntry::Contents(contents));
                }
            }

            entries.insert(blob.path.clone(), BlobEntry::Blob(blob.clone()));
        }

        Ok(entries)
    }

    // Compare the contents and report any differences.
    if let (Ok(old_contents), Ok(mut new_contents)) =
        (manifest_contents(old_package.manifest()), manifest_contents(new_package.manifest()))
    {
        for (path, old_entry) in old_contents {
            if let Some(new_entry) = new_contents.remove(&path) {
                match (old_entry, new_entry) {
                    (BlobEntry::Blob(old_blob), BlobEntry::Blob(new_blob)) => {
                        if old_blob.merkle != new_blob.merkle {
                            msg.push(format!(
                                "  - {}: different contents found in:\n    - {}\n    - {}",
                                path, old_blob.source_path, new_blob.source_path
                            ));
                        }
                    }
                    (old_entry, new_entry) => {
                        if old_entry != new_entry {
                            msg.push(format!("  - {path}: different contents"));
                        }
                    }
                }
            } else {
                msg.push(format!("  - {path}: missing from manifest {new_manifest_path}"));
            }
        }

        for path in new_contents.into_keys() {
            msg.push(format!("  - {path}: missing from manifest {old_manifest_path}"));
        }
    }

    // Helper to read in all the subpackages so we can compare entries.
    fn manifest_subpackages(
        manifest: &PackageManifest,
    ) -> Result<BTreeMap<String, SubpackageInfo>> {
        let mut entries = BTreeMap::new();
        for subpackage in manifest.subpackages() {
            entries.insert(subpackage.name.clone(), subpackage.clone());
        }
        Ok(entries)
    }

    // Compare the subpackages and report any differences.
    if let (Ok(old_subpackages), Ok(mut new_subpackages)) =
        (manifest_subpackages(old_package.manifest()), manifest_subpackages(new_package.manifest()))
    {
        for (name, old_subpackage) in old_subpackages {
            if let Some(new_subpackage) = new_subpackages.remove(&name) {
                if old_subpackage.merkle != new_subpackage.merkle {
                    msg.push(format!(
                        "  - {}: different subpackages found in:\n    - {}\n    - {}",
                        name, old_subpackage.manifest_path, new_subpackage.manifest_path
                    ));
                }
            } else {
                msg.push(format!(
                    "  - {name}: subpackage missing from manifest {new_manifest_path}"
                ));
            }
        }

        for name in new_subpackages.into_keys() {
            msg.push(format!("  - {name}: subpackage missing from manifest {old_manifest_path}"));
        }
    }

    Err(anyhow!(msg.join("\n")))
}

#[cfg(not(target_os = "fuchsia"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::{FileSystemRepository, PmRepository};
    use crate::test_utils;
    use assert_matches::assert_matches;
    use fuchsia_pkg::PackageBuilder;
    use pretty_assertions::{assert_eq, assert_ne};
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use tuf::crypto::Ed25519PrivateKey;
    use tuf::metadata::{Metadata as _, MetadataPath};
    use walkdir::WalkDir;

    pub(crate) fn read_dir(dir: &Utf8Path) -> BTreeMap<String, Vec<u8>> {
        let mut entries = BTreeMap::new();
        for entry in WalkDir::new(dir) {
            let entry = entry.unwrap();
            if entry.metadata().unwrap().is_file() {
                let path = entry.path().strip_prefix(dir).unwrap().to_str().unwrap().to_string();
                let contents = std::fs::read(entry.path()).unwrap();

                entries.insert(path, contents);
            }
        }

        entries
    }

    #[fuchsia::test]
    async fn test_create() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let repo = PmRepository::new(dir.to_path_buf());
        let repo_keys = test_utils::make_repo_keys();

        RepoBuilder::create(&repo, &repo_keys).commit().await.unwrap();

        // Make sure we can update a client from this metadata.
        let mut repo_client = RepoClient::from_trusted_remote(repo).await.unwrap();
        assert_matches!(repo_client.update().await, Ok(true));
    }

    #[fuchsia::test]
    async fn test_create_and_update_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let (pkg1_meta_far_path, pkg1_manifest) =
            test_utils::make_package_manifest("package1", pkg1_dir.as_std_path(), Vec::new());
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();
        let pkg1_meta_far_contents = std::fs::read(&pkg1_meta_far_path).unwrap();

        RepoBuilder::create(&repo, &repo_keys)
            .add_package(pkg1_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // Make sure we wrote all the blobs from package1.
        assert_eq!(
            read_dir(&blob_repo_path),
            [
                (test_utils::PKG1_HASH, pkg1_meta_far_contents.clone()),
                (test_utils::PKG1_BIN_HASH, b"binary package1".to_vec()),
                (test_utils::PKG1_LIB_HASH, b"lib package1".to_vec()),
            ]
            .into_iter()
            .map(|(hash, content)| (
                format!("1/{hash}"),
                delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &content)
            ),)
            .collect()
        );

        // Make sure we can update a client from this metadata.
        let mut repo_client = RepoClient::from_trusted_remote(repo).await.unwrap();
        assert_matches!(repo_client.update().await, Ok(true));

        assert_eq!(repo_client.database().trusted_root().version(), 1);
        assert_eq!(repo_client.database().trusted_targets().map(|m| m.version()), Some(1));
        assert_eq!(repo_client.database().trusted_snapshot().map(|m| m.version()), Some(1));
        assert_eq!(repo_client.database().trusted_timestamp().map(|m| m.version()), Some(1));

        // Create the next version of the metadata and add a new package to it.
        let pkg2_dir = dir.join("package2");
        let (pkg2_meta_far_path, pkg2_manifest) =
            test_utils::make_package_manifest("package2", pkg2_dir.as_std_path(), Vec::new());
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();
        let pkg2_meta_far_contents = std::fs::read(&pkg2_meta_far_path).unwrap();

        let archive_outdir = TempDir::new().unwrap();

        let archive_path = archive_outdir.path().join("p2.far");
        let archive_file = fs::File::create(archive_path.clone()).unwrap();
        pkg2_manifest.archive(&pkg2_dir, &archive_file).await.unwrap();

        RepoBuilder::from_client(&repo_client, &repo_keys)
            .add_package_archive(Utf8PathBuf::from_path_buf(archive_path).unwrap())
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // Make sure we wrote all the blobs from package1 and package2.
        assert_eq!(
            read_dir(&blob_repo_path),
            [
                (test_utils::PKG1_HASH, pkg1_meta_far_contents.clone()),
                (test_utils::PKG1_BIN_HASH, b"binary package1".to_vec()),
                (test_utils::PKG1_LIB_HASH, b"lib package1".to_vec()),
                (test_utils::PKG2_HASH, pkg2_meta_far_contents.clone()),
                (test_utils::PKG2_BIN_HASH, b"binary package2".to_vec()),
                (test_utils::PKG2_LIB_HASH, b"lib package2".to_vec()),
            ]
            .into_iter()
            .map(|(hash, content)| (
                format!("1/{hash}"),
                delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &content)
            ))
            .collect()
        );

        // Make sure we can resolve the new metadata.
        assert_matches!(repo_client.update().await, Ok(true));
        assert_eq!(repo_client.database().trusted_root().version(), 1);
        assert_eq!(repo_client.database().trusted_targets().map(|m| m.version()), Some(2));
        assert_eq!(repo_client.database().trusted_snapshot().map(|m| m.version()), Some(2));
        assert_eq!(repo_client.database().trusted_timestamp().map(|m| m.version()), Some(2));

        // Make sure the timestamp and snapshot metadata was generated with the snapshot and targets
        // length and hashes.
        let snapshot_description = repo_client.database().trusted_timestamp().unwrap().snapshot();
        assert!(snapshot_description.length().is_some());
        assert!(!snapshot_description.hashes().is_empty());

        let trusted_snapshot = repo_client.database().trusted_snapshot().unwrap();
        let targets_description = trusted_snapshot.meta().get(&MetadataPath::targets()).unwrap();
        assert!(targets_description.length().is_some());
        assert!(!targets_description.hashes().is_empty());
    }

    #[fuchsia::test]
    async fn test_create_and_update_repo_with_subpackages() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        const ANONYMOUS_SUBPACKAGE: &str = "anonymous_subpackage";
        const NAMED_SUBPACKAGE: &str = "named_subpackage";
        const NAMED_PACKAGE: &str = "named_package";
        const SUPERPACKAGE: &str = "superpackage";

        // Create an anonymous subpackage (a subpackage that is not directly
        // added to the RepoBuilder, but is added indirectly because it is
        // referenced as a subpackage of another package added to the repo).
        let anonsubpkg_dir = dir.join(ANONYMOUS_SUBPACKAGE);
        let (anonsubpkg_meta_far_path, anonsubpkg_manifest) = test_utils::make_package_manifest(
            ANONYMOUS_SUBPACKAGE,
            anonsubpkg_dir.as_std_path(),
            Vec::new(),
        );
        let anonsubpkg_manifest_path = anonsubpkg_dir.join("anonymous_subpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&anonsubpkg_manifest_path).unwrap(),
            &anonsubpkg_manifest,
        )
        .unwrap();
        let anonsubpkg_meta_far_contents = std::fs::read(&anonsubpkg_meta_far_path).unwrap();

        // Create a named package (named_subpackage), which will also be a
        // subpackage of "superpackage". This named_subpackage will include the
        // anonymous_subpackage.
        let namedsubpkg_dir = dir.join(NAMED_SUBPACKAGE);
        let (namedsubpkg_meta_far_path, namedsubpkg_manifest) = test_utils::make_package_manifest(
            NAMED_SUBPACKAGE,
            namedsubpkg_dir.as_std_path(),
            vec![(
                "anon_subpackage_of_namedsubpkg".parse().unwrap(),
                anonsubpkg_manifest.hash(),
                anonsubpkg_manifest_path.clone().into(),
            )],
        );
        let namedsubpkg_manifest_path = namedsubpkg_dir.join("named_subpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&namedsubpkg_manifest_path).unwrap(),
            &namedsubpkg_manifest,
        )
        .unwrap();
        let namedsubpkg_meta_far_contents = std::fs::read(&namedsubpkg_meta_far_path).unwrap();

        // Make a package that's a duplicate of `named_subpackage` but with different files. This
        // will be added after `named_subpackage`, so we shouldn't try to read any of these files.
        let namedpkg_dir = dir.join(NAMED_PACKAGE);
        let (_, namedpkg_manifest) = test_utils::make_package_manifest(
            NAMED_SUBPACKAGE,
            namedpkg_dir.as_std_path(),
            vec![(
                "anon_subpackage_of_namedsubpkg".parse().unwrap(),
                anonsubpkg_manifest.hash(),
                anonsubpkg_manifest_path.clone().into(),
            )],
        );
        let namedpkg_manifest_path = namedpkg_dir.join("named.manifest");
        serde_json::to_writer(
            std::fs::File::create(&namedpkg_manifest_path).unwrap(),
            &namedpkg_manifest,
        )
        .unwrap();

        // Create a named package ("superpackage"), which will also be a superpackage
        // of both named_subpackage and anonymous_subpackage. Note that
        // named_subpackage is ALSO a superpackage of anonymous_subpackage, so
        // anonymous_subpackage is referenced twice. It will only exist once in
        // the repo.
        let superpkg_dir = dir.join(SUPERPACKAGE);
        let (superpkg_meta_far_path, superpkg_manifest) = test_utils::make_package_manifest(
            SUPERPACKAGE,
            superpkg_dir.as_std_path(),
            vec![
                (
                    NAMED_SUBPACKAGE.parse().unwrap(),
                    namedsubpkg_manifest.hash(),
                    namedsubpkg_manifest_path.clone().into(),
                ),
                (
                    "anon_subpackage_of_superpkg".parse().unwrap(),
                    anonsubpkg_manifest.hash(),
                    anonsubpkg_manifest_path.clone().into(),
                ),
            ],
        );
        let superpkg_manifest_path = superpkg_dir.join("superpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&superpkg_manifest_path).unwrap(),
            &superpkg_manifest,
        )
        .unwrap();
        let superpkg_meta_far_contents = std::fs::read(&superpkg_meta_far_path).unwrap();

        // Add the two named packages. The anonymous subpackage will be added
        // automatically.
        let (actual_deps, _) = RepoBuilder::create(&repo, &repo_keys)
            .add_package(superpkg_manifest_path.clone())
            .await
            .unwrap()
            .add_package(namedsubpkg_manifest_path.clone())
            .await
            .unwrap()
            .add_package(namedpkg_manifest_path.clone())
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        let mut expected_deps = BTreeSet::new();
        expected_deps.insert(anonsubpkg_manifest_path);
        expected_deps.extend(
            anonsubpkg_manifest.blobs().iter().map(|blob| Utf8PathBuf::from(&blob.source_path)),
        );
        expected_deps.insert(namedsubpkg_manifest_path);
        expected_deps.extend(
            namedsubpkg_manifest.blobs().iter().map(|blob| Utf8PathBuf::from(&blob.source_path)),
        );
        expected_deps.insert(superpkg_manifest_path);
        expected_deps.extend(
            superpkg_manifest.blobs().iter().map(|blob| Utf8PathBuf::from(&blob.source_path)),
        );
        // We should only read from the `named_package` manifest, but none of the files that are in
        // the manifest.
        expected_deps.insert(namedpkg_manifest_path);

        assert_eq!(actual_deps.into_iter().collect::<BTreeSet<_>>(), expected_deps);

        let repo_blobs = read_dir(&blob_repo_path);

        assert_eq!(
            repo_blobs.keys().map(|k| k.to_owned()).collect::<BTreeSet<String>>(),
            [
                test_utils::ANONSUBPKG_HASH,
                test_utils::ANONSUBPKG_BIN_HASH,
                test_utils::ANONSUBPKG_LIB_HASH,
                test_utils::NAMEDSUBPKG_HASH,
                test_utils::NAMEDSUBPKG_BIN_HASH,
                test_utils::NAMEDSUBPKG_LIB_HASH,
                test_utils::SUPERPKG_HASH,
                test_utils::SUPERPKG_BIN_HASH,
                test_utils::SUPERPKG_LIB_HASH,
            ]
            .into_iter()
            .map(|hash| format!("1/{hash}"))
            .collect()
        );

        // Make sure we wrote all the blobs from package1.
        assert_eq!(
            read_dir(&blob_repo_path),
            [
                (test_utils::ANONSUBPKG_HASH, anonsubpkg_meta_far_contents.clone()),
                (test_utils::ANONSUBPKG_BIN_HASH, b"binary anonymous_subpackage".to_vec()),
                (test_utils::ANONSUBPKG_LIB_HASH, b"lib anonymous_subpackage".to_vec()),
                (test_utils::NAMEDSUBPKG_HASH, namedsubpkg_meta_far_contents.clone()),
                (test_utils::NAMEDSUBPKG_BIN_HASH, b"binary named_subpackage".to_vec()),
                (test_utils::NAMEDSUBPKG_LIB_HASH, b"lib named_subpackage".to_vec()),
                (test_utils::SUPERPKG_HASH, superpkg_meta_far_contents.clone()),
                (test_utils::SUPERPKG_BIN_HASH, b"binary superpackage".to_vec()),
                (test_utils::SUPERPKG_LIB_HASH, b"lib superpackage".to_vec()),
            ]
            .into_iter()
            .map(|(hash, content)| (
                format!("1/{hash}"),
                delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &content)
            ))
            .collect()
        );

        // Make sure we can update a client from this metadata.
        let mut repo_client = RepoClient::from_trusted_remote(repo).await.unwrap();
        assert_matches!(repo_client.update().await, Ok(true));

        assert_eq!(repo_client.database().trusted_root().version(), 1);
        assert_eq!(repo_client.database().trusted_targets().map(|m| m.version()), Some(1));
        assert_eq!(repo_client.database().trusted_snapshot().map(|m| m.version()), Some(1));
        assert_eq!(repo_client.database().trusted_timestamp().map(|m| m.version()), Some(1));

        // Make sure we have targets for the named packages only.
        let trusted_targets = repo_client.database().trusted_targets().unwrap();
        assert!(
            trusted_targets
                .targets()
                .get(&TargetPath::new(format!("{SUPERPACKAGE}/0")).unwrap())
                .is_some()
        );
        assert!(
            trusted_targets
                .targets()
                .get(&TargetPath::new(format!("{NAMED_SUBPACKAGE}/0")).unwrap())
                .is_some()
        );
        assert!(
            trusted_targets
                .targets()
                .get(&TargetPath::new(format!("{ANONYMOUS_SUBPACKAGE}/0")).unwrap())
                .is_none()
        );
    }

    #[fuchsia::test]
    async fn test_error_if_package_manifest_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Add the superpackage. This should fail because we haven't set `ignore_missing_files`.
        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        // Try to stage the superpackage, which should error out because the
        // subpackage doesn't exist.
        assert_matches!(
            RepoBuilder::create(&repo, &repo_keys).add_package(dir.join("does-not-exist")).await,
            Err(_)
        );
    }

    #[fuchsia::test]
    async fn test_do_not_stage_blobs_if_ignore_missing_files_and_package_files_are_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let (_, pkg_manifest) =
            test_utils::make_package_manifest("package", dir.as_std_path(), vec![]);

        // Delete a subpackage file.
        std::fs::remove_file(dir.join("package").join("meta.far")).unwrap();

        // Commit the supackage to the repository. This should succeed because
        // `ignore_missing_files` is true.
        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        let (actual_deps, committed_blobs) = RepoBuilder::create(&repo, &repo_keys)
            .ignore_missing_packages(true)
            .add_package_manifest(None, pkg_manifest.clone())
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // However we shouldn't have tried to write anything to the repository.
        assert_eq!(actual_deps, HashSet::new());
        assert_eq!(committed_blobs, BTreeSet::new());
    }

    #[fuchsia::test]
    async fn test_error_if_subpackage_manifest_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Make a subpackage.
        let subpkg_dir = dir.join("subpackage");
        let (_, subpkg_manifest) =
            test_utils::make_package_manifest("subpackage", subpkg_dir.as_std_path(), Vec::new());

        let subpkg_manifest_path = subpkg_dir.join("subpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&subpkg_manifest_path).unwrap(),
            &subpkg_manifest,
        )
        .unwrap();

        // Make a superpackage that uses the subpackage.
        let superpkg_dir = dir.join("superpackage");
        let (_, superpkg_manifest) = test_utils::make_package_manifest(
            "superpackage",
            superpkg_dir.as_std_path(),
            vec![(
                "subpackage".parse().unwrap(),
                subpkg_manifest.hash(),
                subpkg_manifest_path.clone().into(),
            )],
        );

        // Delete the subpackage manifest.
        std::fs::remove_file(&subpkg_manifest_path).unwrap();

        // Add the superpackage. This should fail because we haven't set `ignore_missing_files`.
        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        // Try to stage the superpackage, which should error out because the
        // subpackage doesn't exist.
        assert_matches!(
            RepoBuilder::create(&repo, &repo_keys)
                .add_package_manifest(None, superpkg_manifest)
                .await,
            Err(_)
        );
    }

    #[fuchsia::test]
    async fn test_do_not_stage_blobs_if_ignore_missing_files_and_subpackage_manifest_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Create the subpackage.
        let subpkg_dir = dir.join("subpackage");
        let (_, subpkg_manifest) =
            test_utils::make_package_manifest("subpackage", subpkg_dir.as_std_path(), Vec::new());

        let subpkg_manifest_path = subpkg_dir.join("subpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&subpkg_manifest_path).unwrap(),
            &subpkg_manifest,
        )
        .unwrap();

        // Create the superpackage that uses the subpackage.
        let superpkg_dir = dir.join("superpackage");
        let (_, superpkg_manifest) = test_utils::make_package_manifest(
            "superpackage",
            superpkg_dir.as_std_path(),
            vec![(
                "subpackage".parse().unwrap(),
                subpkg_manifest.hash(),
                subpkg_manifest_path.clone().into(),
            )],
        );

        // Delete the subpackage manifest.
        std::fs::remove_file(&subpkg_manifest_path).unwrap();

        // Commit the supackage to the repository. This should succeed because
        // `ignore_missing_files` is true.
        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        let (actual_deps, committed_blobs) = RepoBuilder::create(&repo, &repo_keys)
            .ignore_missing_packages(true)
            .add_package_manifest(None, superpkg_manifest.clone())
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // However we shouldn't have tried to write anything to the repository.
        assert_eq!(actual_deps, HashSet::new());
        assert_eq!(committed_blobs, BTreeSet::new());
    }

    #[fuchsia::test]
    async fn test_do_not_stage_blobs_if_ignore_missing_files_and_subpackage_files_are_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Create the subpackage.
        let subpkg_dir = dir.join("subpackage");
        let (_, subpkg_manifest) =
            test_utils::make_package_manifest("subpackage", subpkg_dir.as_std_path(), Vec::new());

        let subpkg_manifest_path = subpkg_dir.join("subpackage.manifest");
        serde_json::to_writer(
            std::fs::File::create(&subpkg_manifest_path).unwrap(),
            &subpkg_manifest,
        )
        .unwrap();

        // Create the superpackage that uses the subpackage.
        let superpkg_dir = dir.join("superpackage");
        let (_, superpkg_manifest) = test_utils::make_package_manifest(
            "superpackage",
            superpkg_dir.as_std_path(),
            vec![(
                "subpackage".parse().unwrap(),
                subpkg_manifest.hash(),
                subpkg_manifest_path.clone().into(),
            )],
        );

        // Delete a subpackage file.
        std::fs::remove_file(subpkg_dir.join("subpackage").join("meta.far")).unwrap();

        // Commit the supackage to the repository. This should succeed because
        // `ignore_missing_files` is true.
        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        let (actual_deps, committed_blobs) = RepoBuilder::create(&repo, &repo_keys)
            .ignore_missing_packages(true)
            .add_package_manifest(None, superpkg_manifest.clone())
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // However we shouldn't have tried to write anything to the repository.
        assert_eq!(actual_deps, HashSet::new());
        assert_eq!(committed_blobs, BTreeSet::new());
    }

    #[fuchsia::test]
    async fn test_refresh_metadata_with_all_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Load up the test metadata, which was created some time ago, and has a different
        // expiration date.
        let repo = test_utils::make_pm_repository(dir).await;

        // Download the older metadata before we refresh it.
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        repo_client.update().await.unwrap();

        let root1 = (*repo_client.database().trusted_root()).clone();
        let targets1 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot1 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp1 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // Update the metadata expiration.
        let repo_keys = RepoKeys::from_dir(&dir.join("keys").into_std_path_buf()).unwrap();
        RepoBuilder::from_database(repo_client.remote_repo(), &repo_keys, repo_client.database())
            .refresh_metadata(true)
            .commit()
            .await
            .unwrap();

        // Finally, make sure the metadata has changed.
        assert_matches!(repo_client.update().await, Ok(true));

        let root2 = (*repo_client.database().trusted_root()).clone();
        let targets2 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot2 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp2 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // Make sure we generated new metadata.
        assert_ne!(root1, root2);
        assert_ne!(targets1, targets2);
        assert_ne!(snapshot1, snapshot2);
        assert_ne!(timestamp1, timestamp2);

        // We should have kept our old snapshot entries (except the target should have changed).
        assert_eq!(
            snapshot1
                .meta()
                .iter()
                .filter(|(k, _)| **k != MetadataPath::targets())
                .collect::<HashMap<_, _>>(),
            snapshot2
                .meta()
                .iter()
                .filter(|(k, _)| **k != MetadataPath::targets())
                .collect::<HashMap<_, _>>(),
        );

        // We should have kept our targets and delegations.
        assert_eq!(targets1.targets(), targets2.targets());
        assert_eq!(targets1.delegations(), targets2.delegations());
    }

    #[fuchsia::test]
    async fn test_refresh_metadata_with_some_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Load the repo.
        let repo = test_utils::make_pm_repository(dir).await;

        // Download the older metadata before we refresh it.
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        repo_client.update().await.unwrap();

        let root1 = (*repo_client.database().trusted_root()).clone();
        let targets1 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot1 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp1 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // Load the repo, but delete the root private key file.
        let keys_dir = dir.join("keys");
        std::fs::remove_file(keys_dir.join("root.json")).unwrap();

        // Update the metadata expiration.
        let repo_keys = RepoKeys::from_dir(&dir.join("keys").into_std_path_buf()).unwrap();

        // Update the metadata expiration should succeed.
        RepoBuilder::from_database(repo_client.remote_repo(), &repo_keys, repo_client.database())
            .refresh_metadata(true)
            .commit()
            .await
            .unwrap();

        // Make sure the metadata has changed.
        assert_matches!(repo_client.update().await, Ok(true));

        let root2 = (*repo_client.database().trusted_root()).clone();
        let targets2 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot2 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp2 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // Make sure we generated new metadata, except for the root metadata.
        assert_eq!(root1, root2);
        assert_ne!(targets1, targets2);
        assert_ne!(snapshot1, snapshot2);
        assert_ne!(timestamp1, timestamp2);

        // We should have kept our old snapshot entries (except the target should have changed).
        assert_eq!(
            snapshot1
                .meta()
                .iter()
                .filter(|(k, _)| **k != MetadataPath::targets())
                .collect::<HashMap<_, _>>(),
            snapshot2
                .meta()
                .iter()
                .filter(|(k, _)| **k != MetadataPath::targets())
                .collect::<HashMap<_, _>>(),
        );

        // We should have kept our targets and delegations.
        assert_eq!(targets1.targets(), targets2.targets());
        assert_eq!(targets1.delegations(), targets2.delegations());
    }

    #[fuchsia::test]
    async fn test_refresh_metadata_with_no_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Load the repo.
        let repo = test_utils::make_pm_repository(dir).await;

        // Download the older metadata before we refresh it.
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        repo_client.update().await.unwrap();

        let root1 = (*repo_client.database().trusted_root()).clone();
        let targets1 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot1 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp1 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // Try to refresh the metadata with an empty key set, which should error out.
        let repo_keys = RepoKeys::builder().build();
        let res = RepoBuilder::from_database(
            repo_client.remote_repo(),
            &repo_keys,
            repo_client.database(),
        )
        .refresh_metadata(true)
        .commit()
        .await;
        assert_matches!(res, Err(_));

        // Updating the client should return that there were no changes.
        assert_matches!(repo_client.update().await, Ok(false));

        let root2 = (*repo_client.database().trusted_root()).clone();
        let targets2 = repo_client.database().trusted_targets().cloned().unwrap();
        let snapshot2 = repo_client.database().trusted_snapshot().cloned().unwrap();
        let timestamp2 = repo_client.database().trusted_timestamp().cloned().unwrap();

        // We should not have changed the metadata.
        assert_eq!(root1, root2);
        assert_eq!(targets1, targets2);
        assert_eq!(snapshot1, snapshot2);
        assert_eq!(timestamp1, timestamp2);
    }

    #[fuchsia::test]
    async fn test_refresh_metadata_with_root_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        // First create a repository.
        let full_repo_path = root.join("full");
        let full_metadata_repo_path = full_repo_path.join("repository");
        test_utils::make_pm_repo_dir(full_repo_path.as_std_path()).await;

        // Then create a repository, which only has the root metadata in it.
        let test_repo_path = root.join("test");
        let test_metadata_repo_path = test_repo_path.join("repository");
        std::fs::create_dir_all(&test_metadata_repo_path).unwrap();

        std::fs::copy(
            full_metadata_repo_path.join("root.json"),
            test_metadata_repo_path.join("1.root.json"),
        )
        .unwrap();

        // Create a repo client and download the root metadata. Update should fail with missint TUF
        // metadata since we don't have any other metadata.
        let repo = PmRepository::new(test_repo_path);
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        assert_matches!(
            repo_client.update().await,
            Err(crate::repository::Error::Tuf(tuf::Error::MetadataNotFound { path, .. }))
            if path == tuf::metadata::MetadataPath::timestamp()
        );

        assert!(repo_client.database().trusted_targets().is_none());
        assert!(repo_client.database().trusted_snapshot().is_none());
        assert!(repo_client.database().trusted_timestamp().is_none());

        // Update the metadata expiration. We'll use the keys from the full pm directory.
        let repo_keys =
            RepoKeys::from_dir(&full_repo_path.join("keys").into_std_path_buf()).unwrap();
        RepoBuilder::from_database(repo_client.remote_repo(), &repo_keys, repo_client.database())
            .refresh_metadata(true)
            .commit()
            .await
            .unwrap();

        // Updating the client should succeed since we created the missing metadata.
        assert_matches!(repo_client.update().await, Ok(true));

        assert!(repo_client.database().trusted_targets().is_some());
        assert!(repo_client.database().trusted_snapshot().is_some());
        assert!(repo_client.database().trusted_timestamp().is_some());
    }

    #[fuchsia::test]
    async fn test_inherit_from_trusted_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let repo_dir = root.join("repo");

        // Load the repo, which already contains package1 and package2.
        let repo = test_utils::make_pm_repository(repo_dir).await;
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        repo_client.update().await.unwrap();

        // Publish package3 to the repository.
        let pkg3_dir = root.join("pkg3");
        let (_, pkg3_manifest) =
            test_utils::make_package_manifest("package3", pkg3_dir.as_std_path(), Vec::new());
        let pkg3_manifest_path = pkg3_dir.join("package3.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg3_manifest_path).unwrap(), &pkg3_manifest)
            .unwrap();

        let repo_keys = test_utils::make_repo_keys();
        RepoBuilder::from_database(repo_client.remote_repo(), &repo_keys, repo_client.database())
            .add_package(pkg3_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // Make sure we have metadata for package1, package2, and package3.
        assert_matches!(repo_client.update().await, Ok(true));
        let trusted_targets = repo_client.database().trusted_targets().unwrap();
        assert!(trusted_targets.targets().get("package1/0").is_some());
        assert!(trusted_targets.targets().get("package2/0").is_some());
        assert!(trusted_targets.targets().get("package3/0").is_some());

        // Now do another commit, but this time not inheriting the old packages.
        let pkg4_dir = root.join("pkg4");
        let (_, pkg4_manifest) =
            test_utils::make_package_manifest("package4", pkg4_dir.as_std_path(), Vec::new());
        let pkg4_manifest_path = pkg4_dir.join("package4.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg4_manifest_path).unwrap(), &pkg4_manifest)
            .unwrap();

        RepoBuilder::from_database(repo_client.remote_repo(), &repo_keys, repo_client.database())
            .inherit_from_trusted_targets(false)
            .add_package(pkg4_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // We should only have metadata for package4.
        assert_matches!(repo_client.update().await, Ok(true));
        let trusted_targets = repo_client.database().trusted_targets().unwrap();
        assert!(trusted_targets.targets().get("package1/0").is_none());
        assert!(trusted_targets.targets().get("package2/0").is_none());
        assert!(trusted_targets.targets().get("package3/0").is_none());
        assert!(trusted_targets.targets().get("package4/0").is_some());
    }

    fn generate_ed25519_private_key() -> Ed25519PrivateKey {
        Ed25519PrivateKey::from_pkcs8(&Ed25519PrivateKey::pkcs8().unwrap()).unwrap()
    }

    #[fuchsia::test]
    async fn test_key_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let repo_dir = root.join("repo");

        // First, make a repository.
        let repo = test_utils::make_pm_repository(repo_dir).await;
        let mut repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();
        repo_client.update().await.unwrap();

        // Then make a new RepoKeys with unique keys.
        let repo_trusted_keys = RepoKeys::builder()
            .add_root_key(Box::new(generate_ed25519_private_key()))
            .add_targets_key(Box::new(generate_ed25519_private_key()))
            .add_snapshot_key(Box::new(generate_ed25519_private_key()))
            .add_timestamp_key(Box::new(generate_ed25519_private_key()))
            .build();

        // Generate new metadata that trusts the new keys, but signs it with the old keys.
        let repo_signing_keys = repo.repo_keys().unwrap();
        RepoBuilder::from_database(
            repo_client.remote_repo(),
            &repo_trusted_keys,
            repo_client.database(),
        )
        .signing_repo_keys(&repo_signing_keys)
        .commit()
        .await
        .unwrap();

        // Make sure we can update.
        assert_matches!(repo_client.update().await, Ok(true));
        assert_eq!(repo_client.database().trusted_root().version(), 2);
        assert_eq!(repo_client.database().trusted_snapshot().unwrap().version(), 2);
        assert_eq!(repo_client.database().trusted_targets().unwrap().version(), 2);
        assert_eq!(repo_client.database().trusted_timestamp().unwrap().version(), 2);

        // Make sure we only trust the new keys.
        let trusted_root = repo_client.database().trusted_root();
        assert_eq!(
            trusted_root.root_keys().collect::<Vec<_>>(),
            repo_trusted_keys.root_keys().iter().map(|k| k.public()).collect::<Vec<_>>(),
        );

        assert_eq!(
            trusted_root.targets_keys().collect::<Vec<_>>(),
            repo_trusted_keys.targets_keys().iter().map(|k| k.public()).collect::<Vec<_>>(),
        );

        assert_eq!(
            trusted_root.snapshot_keys().collect::<Vec<_>>(),
            repo_trusted_keys.snapshot_keys().iter().map(|k| k.public()).collect::<Vec<_>>(),
        );

        assert_eq!(
            trusted_root.timestamp_keys().collect::<Vec<_>>(),
            repo_trusted_keys.timestamp_keys().iter().map(|k| k.public()).collect::<Vec<_>>(),
        );
    }

    const FAKE_ABI_REVISION: version_history::AbiRevision =
        version_history::AbiRevision::from_u64(0x7171611eb2b7b74a);

    #[fuchsia::test]
    async fn test_conflicting_package_manifests_errors_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path);
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let (_, pkg1_manifest) =
            test_utils::make_package_manifest("package1", pkg1_dir.as_std_path(), Vec::new());
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        // Whoops, we created a package with the same package name but with different contents.
        let pkg2_dir = dir.join("package2");
        let pkg2_meta_far_path = pkg2_dir.join("meta.far");
        let pkg2_manifest = PackageBuilder::new("package1", FAKE_ABI_REVISION)
            .build(&pkg2_dir, &pkg2_meta_far_path)
            .unwrap();
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();

        assert!(
            RepoBuilder::create(&repo, &repo_keys)
                .add_package(pkg1_manifest_path)
                .await
                .unwrap()
                .add_package(pkg2_manifest_path)
                .await
                .is_err()
        );
    }

    #[fuchsia::test]
    async fn test_stage_package_overwrites_corrupted_blob_in_blob_store() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        // Create 2 packages with a shared blob between them
        let contents = b"shared blob";
        let hash = fuchsia_merkle::root_from_slice(contents);

        let pkg1_dir = dir.join("package1");
        let pkg1_meta_far_path = pkg1_dir.join("meta.far");
        let pkg1_manifest = {
            let mut builder = PackageBuilder::new("package1", FAKE_ABI_REVISION);
            builder.add_contents_as_blob("bin/shared", contents, &pkg1_dir).unwrap();
            builder.build(&pkg1_dir, &pkg1_meta_far_path).unwrap()
        };
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        let pkg2_dir = dir.join("package2");
        let pkg2_meta_far_path = pkg2_dir.join("meta.far");
        let pkg2_manifest = {
            let mut builder = PackageBuilder::new("package2", FAKE_ABI_REVISION);
            builder.add_contents_as_blob("bin/shared", contents, &pkg2_dir).unwrap();
            builder.build(&pkg2_dir, &pkg2_meta_far_path).unwrap()
        };
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();

        // Create the repo and publish the first package.
        RepoBuilder::create(&repo, &repo_keys)
            .add_package(pkg1_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // Corrupt the contents of the blob, and change it's size.
        let shared_blob_path = blob_repo_path.join(format!("1/{hash}"));
        let mut perms = fs::metadata(&shared_blob_path).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o664);
        fs::set_permissions(&shared_blob_path, perms).unwrap();
        // It's no longer a delivery blob after this write
        fs::write(&shared_blob_path, b"corrupted contents").unwrap();

        // Publish the second package, which should succeed and overwrite the corrupted blob.
        RepoBuilder::create(&repo, &repo_keys)
            .add_package(pkg2_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        let delivery_blob = std::fs::read(&shared_blob_path).unwrap();
        let actual: Vec<u8> = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(actual, contents);
    }

    #[fuchsia::test]
    async fn test_stage_package_does_not_overwrite_corrupted_blob_in_blob_store_if_lengths_match() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path.clone());
        let repo_keys = test_utils::make_repo_keys();

        // Create 2 packages with a shared blob between them
        let contents = b"shared blob";
        let contents2 = b"corruptblob";
        let hash = fuchsia_merkle::root_from_slice(contents);

        let pkg1_dir = dir.join("package1");
        let pkg1_meta_far_path = pkg1_dir.join("meta.far");
        let pkg1_manifest = {
            let mut builder = PackageBuilder::new("package1", FAKE_ABI_REVISION);
            builder.add_contents_as_blob("bin/shared", contents, &pkg1_dir).unwrap();
            builder.build(&pkg1_dir, &pkg1_meta_far_path).unwrap()
        };
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        let pkg2_dir = dir.join("package2");
        let pkg2_meta_far_path = pkg2_dir.join("meta.far");
        let pkg2_manifest = {
            let mut builder = PackageBuilder::new("package2", FAKE_ABI_REVISION);
            builder.add_contents_as_blob("bin/shared", contents, &pkg2_dir).unwrap();
            builder.build(&pkg2_dir, &pkg2_meta_far_path).unwrap()
        };
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();

        // Create the repo and publish the first package.
        RepoBuilder::create(&repo, &repo_keys)
            .add_package(pkg1_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // Corrupt the contents of the blob, but keep it's length the same.
        let shared_blob_path = blob_repo_path.join(format!("1/{hash}"));
        let mut perms = fs::metadata(&shared_blob_path).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o664);
        fs::set_permissions(&shared_blob_path, perms).unwrap();
        fs::write(&shared_blob_path, delivery_blob::generate(DeliveryBlobType::Type1, contents2))
            .unwrap();

        // Publish the second package, which should succeed and overwrite the corrupted blob.
        RepoBuilder::create(&repo, &repo_keys)
            .add_package(pkg2_manifest_path)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();

        // The blob still contains the corrupted contents.
        let delivery_blob = std::fs::read(&shared_blob_path).unwrap();
        let actual: Vec<u8> = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(actual, contents2);
    }

    #[fuchsia::test]
    async fn test_conflicting_package_archives_errors_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path);
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let (_, pkg1_manifest) =
            test_utils::make_package_manifest("package1", pkg1_dir.as_std_path(), Vec::new());
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        // Whoops, we created a package with the same package name but with different contents.
        let pkg2_dir = dir.join("package2");
        let pkg2_meta_far_path = pkg2_dir.join("meta.far");
        let pkg2_manifest = PackageBuilder::new("package1", FAKE_ABI_REVISION)
            .build(&pkg2_dir, &pkg2_meta_far_path)
            .unwrap();
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();

        let archive_outdir = TempDir::new().unwrap();

        let archive_path1 = archive_outdir.path().join("p1.far");
        let archive_file1 = fs::File::create(archive_path1.clone()).unwrap();
        pkg1_manifest.archive(&pkg1_dir, &archive_file1).await.unwrap();

        let archive_path2 = archive_outdir.path().join("p2.far");
        let archive_file2 = fs::File::create(archive_path2.clone()).unwrap();
        pkg2_manifest.archive(&pkg2_dir, &archive_file2).await.unwrap();

        assert!(
            RepoBuilder::create(&repo, &repo_keys)
                .add_package_archive(Utf8PathBuf::from_path_buf(archive_path1).unwrap())
                .await
                .unwrap()
                .add_package_archive(Utf8PathBuf::from_path_buf(archive_path2).unwrap())
                .await
                .is_err()
        );
    }

    #[fuchsia::test]
    async fn test_conflicting_package_archive_and_manifest_errors_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path);
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let (_, pkg1_manifest) =
            test_utils::make_package_manifest("package1", pkg1_dir.as_std_path(), Vec::new());
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        // Whoops, we created a package with the same package name but with different contents.
        let pkg2_dir = dir.join("package2");
        let pkg2_meta_far_path = pkg2_dir.join("meta.far");
        let pkg2_manifest = PackageBuilder::new("package1", FAKE_ABI_REVISION)
            .build(&pkg2_dir, &pkg2_meta_far_path)
            .unwrap();
        let pkg2_manifest_path = pkg2_dir.join("package2.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg2_manifest_path).unwrap(), &pkg2_manifest)
            .unwrap();

        let archive_outdir = TempDir::new().unwrap();

        let archive_path1 = archive_outdir.path().join("p1.far");
        let archive_file1 = fs::File::create(archive_path1.clone()).unwrap();
        pkg1_manifest.archive(&pkg1_dir, &archive_file1).await.unwrap();

        assert!(
            RepoBuilder::create(&repo, &repo_keys)
                .add_package_archive(Utf8PathBuf::from_path_buf(archive_path1).unwrap())
                .await
                .unwrap()
                .add_package(pkg2_manifest_path)
                .await
                .is_err()
        );
    }

    #[fuchsia::test]
    async fn test_blob_size_differs_from_manifest_errors_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path);
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let (_pkg1_meta_far_path, pkg1_manifest) =
            test_utils::make_package_manifest("package1", pkg1_dir.as_std_path(), Vec::new());
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        // Oh no, the blob now disagrees with metadata in the manifest.
        let pkg1_blob_path = pkg1_dir.join("package1").join("bin").join("package1");
        let mut blob = fs::File::options().append(true).open(pkg1_blob_path).unwrap();
        blob.write_all(b"more bytes the manifest doesn't know about.").unwrap();
        blob.write_all(b"also, my hash is now wrong").unwrap();
        drop(blob);

        let err = RepoBuilder::create(&repo, &repo_keys)
            .add_package_manifest(Some(pkg1_dir), pkg1_manifest)
            .await
            .unwrap_err();
        assert_matches!(err.downcast_ref::<BlobSizeMismatchError>(), Some(_));
    }

    #[fuchsia::test]
    async fn test_rejects_distinct_merkles_referring_to_the_same_inode() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        let repo = FileSystemRepository::new(metadata_repo_path, blob_repo_path);
        let repo_keys = test_utils::make_repo_keys();

        let pkg1_dir = dir.join("package1");
        let pkg1_meta_far_path = pkg1_dir.join("meta.far");
        let pkg1_manifest = {
            let mut builder = PackageBuilder::new("package1", FAKE_ABI_REVISION);
            builder.add_contents_as_blob("bin/blob1", b"blob1", &pkg1_dir).unwrap();
            builder.add_contents_as_blob("bin/blob2", b"blob2", &pkg1_dir).unwrap();
            builder.build(&pkg1_dir, &pkg1_meta_far_path).unwrap()
        };
        let pkg1_manifest_path = pkg1_dir.join("package1.manifest");
        serde_json::to_writer(std::fs::File::create(&pkg1_manifest_path).unwrap(), &pkg1_manifest)
            .unwrap();

        // Make blob2 a hardlink of blob1, but the manifest has 2 distinct merkles for it
        let pkg1_blob1_path = pkg1_dir.join("bin").join("blob1");
        let pkg1_blob2_path = pkg1_dir.join("bin").join("blob2");
        fs::remove_file(&pkg1_blob2_path).unwrap();
        fs::hard_link(&pkg1_blob1_path, &pkg1_blob2_path).unwrap();

        let err = RepoBuilder::create(&repo, &repo_keys)
            .add_package_manifest(Some(pkg1_dir), pkg1_manifest)
            .await
            .unwrap_err();
        assert_matches!(err.downcast_ref::<MerkleHardLinkMismatchError>(), Some(_));
    }
}

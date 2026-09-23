// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::range::Range;
use crate::repository::{Error, RepoProvider, RepositorySpec};
use crate::resource::Resource;
use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use fidl_fuchsia_pkg_ext::{
    MirrorConfigBuilder, RepositoryConfig, RepositoryConfigBuilder, RepositoryKey,
    RepositoryStorageType,
};
use fuchsia_fs::file::Adapter;
use fuchsia_hash::Hash;
use fuchsia_pkg::{MetaContents, MetaSubpackages};
use fuchsia_url::RepositoryUrl;
use futures::future::{BoxFuture, Shared};
use futures::io::Cursor;
use futures::stream::{self, BoxStream};
use futures::{AsyncRead, AsyncReadExt as _, FutureExt as _, StreamExt as _, TryStreamExt as _};
use std::collections::BTreeSet;
use std::fmt::{self, Debug};
use std::time::SystemTime;
use tuf::Database;
use tuf::client::{Client as TufClient, Config};
use tuf::crypto::KeyType;
use tuf::metadata::{
    Metadata as _, MetadataPath, MetadataVersion, RawSignedMetadata, RootMetadata,
    TargetDescription, TargetPath,
};
use tuf::pouf::Pouf1;
use tuf::repository::{
    EphemeralRepository, RepositoryProvider, RepositoryProvider as TufRepositoryProvider,
    RepositoryStorage as TufRepositoryStorage,
};

const LIST_PACKAGE_CONCURRENCY: usize = 5;

pub struct RepoClient<R>
where
    R: RepoProvider,
{
    /// _tx_on_drop is a channel that will emit a `Cancelled` message to `rx_on_drop` when this
    /// repository is dropped. This is a convenient way to notify any downstream users to clean up
    /// any side tables that associate a repository to some other data.
    _tx_on_drop: futures::channel::oneshot::Sender<()>,

    /// Channel Receiver that receives a `Cancelled` signal when this repository is dropped.
    rx_on_drop: futures::future::Shared<futures::channel::oneshot::Receiver<()>>,

    /// The TUF client for this repository
    tuf_client: TufClient<Pouf1, EphemeralRepository<Pouf1>, R>,
}

impl<R> RepoClient<R>
where
    R: RepoProvider,
{
    /// Creates a [RepoClient] that establishes trust with an initial trusted root metadata.
    pub async fn from_trusted_root(
        trusted_root: &RawSignedMetadata<Pouf1, RootMetadata>,
        remote: R,
    ) -> Result<Self, Error> {
        let local = EphemeralRepository::<Pouf1>::new();
        let tuf_client =
            TufClient::with_trusted_root(Config::default(), trusted_root, local, remote).await?;
        Ok(Self::new(tuf_client))
    }

    /// Creates a [RepoClient] that downloads the initial TUF root metadata from the remote
    /// [RepoProvider].
    pub async fn from_trusted_remote(backend: R) -> Result<Self, Error> {
        let tuf_client = get_tuf_client(backend).await?;
        Ok(Self::new(tuf_client))
    }

    /// Creates a [RepoClient] that communicates with the remote [RepoProvider] with a trusted TUF
    /// [Database].
    pub fn from_database(database: Database<Pouf1>, remote: R) -> Self {
        let local = EphemeralRepository::new();
        let tuf_client = TufClient::from_database(Config::default(), database, local, remote);

        Self::new(tuf_client)
    }

    fn new(tuf_client: TufClient<Pouf1, EphemeralRepository<Pouf1>, R>) -> Self {
        let (tx_on_drop, rx_on_drop) = futures::channel::oneshot::channel();
        let rx_on_drop = rx_on_drop.shared();

        Self { tuf_client, _tx_on_drop: tx_on_drop, rx_on_drop }
    }

    /// Returns a receiver that will receive a `Canceled` signal when the repository is dropped.
    pub fn on_dropped_signal(&self) -> Shared<futures::channel::oneshot::Receiver<()>> {
        self.rx_on_drop.clone()
    }

    /// Returns the client's tuf [Database].
    pub fn database(&self) -> &tuf::Database<Pouf1> {
        self.tuf_client.database()
    }

    /// Returns the client's remote repository [RepoProvider].
    pub fn remote_repo(&self) -> &R {
        self.tuf_client.remote_repo()
    }

    /// Get a [RepositorySpec] for this [Repository].
    #[cfg(not(target_os = "fuchsia"))]
    pub fn spec(&self) -> RepositorySpec {
        self.tuf_client.remote_repo().spec()
    }

    /// Returns if the repository supports watching for timestamp changes.
    pub fn supports_watch(&self) -> bool {
        self.tuf_client.remote_repo().supports_watch()
    }

    /// Return a stream that yields whenever the repository's timestamp changes.
    pub fn watch(&self) -> anyhow::Result<BoxStream<'static, ()>> {
        self.tuf_client.remote_repo().watch()
    }

    /// Update client to the latest available metadata.
    pub async fn update(&mut self) -> Result<bool, Error> {
        self.update_with_start_time(&Utc::now()).await
    }

    /// Update client to the latest available metadata relative to the specified update start time.
    pub async fn update_with_start_time(
        &mut self,
        start_time: &DateTime<Utc>,
    ) -> Result<bool, Error> {
        Ok(self.tuf_client.update_with_start_time(start_time).await?)
    }

    /// Return a stream of bytes for the metadata resource.
    pub async fn fetch_metadata(&self, path: &str) -> Result<Resource, Error> {
        self.fetch_metadata_range(path, Range::Full).await
    }

    /// Return a stream of bytes for the metadata resource in given range.
    pub async fn fetch_metadata_range(&self, path: &str, range: Range) -> Result<Resource, Error> {
        self.tuf_client.remote_repo().fetch_metadata_range(path, range).await
    }

    /// Return a stream of bytes for the blob resource.
    pub async fn fetch_blob(&self, path: &str) -> Result<Resource, Error> {
        self.fetch_blob_range(path, Range::Full).await
    }

    /// Return a stream of bytes for the blob resource in given range.
    pub async fn fetch_blob_range(&self, path: &str, range: Range) -> Result<Resource, Error> {
        self.tuf_client.remote_repo().fetch_blob_range(path, range).await
    }

    pub fn delivery_blob_path(&self, hash: &Hash) -> String {
        format!("{}/{hash}", u32::from(self.blob_type()))
    }

    /// Return a Vec of the blob decompressed.
    pub async fn read_blob_decompressed(&self, hash: &Hash) -> Result<Vec<u8>> {
        let path = self.delivery_blob_path(hash);
        let mut delivery_blob = vec![];
        self.fetch_blob(&path)
            .await
            .with_context(|| format!("fetching blob {path}"))?
            .read_to_end(&mut delivery_blob)
            .await
            .with_context(|| format!("reading blob {path}"))?;
        delivery_blob::decompress(&delivery_blob)
            .with_context(|| format!("decompressing blob {path}"))
    }

    async fn blob_decompressed_size(&self, hash: &Hash) -> Result<u64> {
        let path = self.delivery_blob_path(hash);
        let mut delivery_blob = vec![];
        let mut blob_resource =
            self.fetch_blob(&path).await.with_context(|| format!("fetching blob {path}"))?;

        while let Some(chunk) = blob_resource.stream.try_next().await? {
            delivery_blob.extend_from_slice(&chunk);
            match delivery_blob::decompressed_size(&delivery_blob) {
                Ok(len) => {
                    return Ok(len);
                }
                Err(delivery_blob::DecompressError::NeedMoreData) => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("parsing delivery blob {path}"));
                }
            }
        }
        Err(anyhow!("blob {path} too small"))
    }

    async fn blob_modification_time_secs(&self, hash: &Hash) -> Result<Option<u64>> {
        let path = self.delivery_blob_path(hash);
        self.tuf_client
            .remote_repo()
            .blob_modification_time(&path)
            .await
            .with_context(|| format!("could not get modtime for {path}"))?
            .map(|x| -> anyhow::Result<u64> {
                Ok(x.duration_since(SystemTime::UNIX_EPOCH)?.as_secs())
            })
            .transpose()
    }

    /// Return the target description for a TUF target path.
    pub async fn get_target_description(
        &self,
        path: &str,
    ) -> Result<Option<TargetDescription>, Error> {
        match self.tuf_client.database().trusted_targets() {
            Some(trusted_targets) => Ok(trusted_targets
                .targets()
                .get(&TargetPath::new(path).map_err(|e| anyhow::anyhow!(e))?)
                .cloned()),
            None => Ok(None),
        }
    }

    pub fn get_config(
        &self,
        repo_url: RepositoryUrl,
        mirror_url: http::Uri,
        repo_storage_type: Option<RepositoryStorageType>,
    ) -> Result<RepositoryConfig, Error> {
        let trusted_root = self.tuf_client.database().trusted_root();

        let mut repo_config_builder = RepositoryConfigBuilder::new(repo_url)
            .root_version(trusted_root.version())
            .root_threshold(trusted_root.root().threshold())
            .add_mirror(
                MirrorConfigBuilder::new(mirror_url)?
                    .subscribe(self.tuf_client.remote_repo().supports_watch())
                    .build(),
            );

        if let Some(repo_storage_type) = repo_storage_type {
            repo_config_builder = repo_config_builder.repo_storage_type(repo_storage_type);
        }

        for root_key in trusted_root.root_keys().filter(|k| *k.typ() == KeyType::Ed25519) {
            repo_config_builder = repo_config_builder
                .add_root_key(RepositoryKey::Ed25519(root_key.as_bytes().to_vec()));
        }

        let repo_config = repo_config_builder.build();

        Ok(repo_config)
    }

    pub async fn list_packages(&self) -> Result<Vec<RepositoryPackage>, Error> {
        let trusted_targets =
            self.tuf_client.database().trusted_targets().context("missing target information")?;

        let mut packages = vec![];
        for (package_name, package_description) in trusted_targets.targets() {
            let Some(meta_far_hash_str) = package_description.custom().get("merkle") else {
                continue;
            };

            let meta_far_hash_str = meta_far_hash_str.as_str().ok_or_else(|| {
                anyhow!(
                    "package {:?} hash should be a string, not {:?}",
                    package_name,
                    meta_far_hash_str
                )
            })?;

            let meta_far_hash = Hash::try_from(meta_far_hash_str)
                .context("failed hash from {meta_far_hash_str}")?;

            packages.push(RepositoryPackage {
                name: package_name.to_string(),
                hash: meta_far_hash,
                modified: self
                    .blob_modification_time_secs(&meta_far_hash)
                    .await
                    .context("getting blob_modification_time_secs")?,
            });
        }

        Ok(packages)
    }

    pub async fn show_package(
        &self,
        package_name: &str,
        include_subpackages: bool,
    ) -> Result<Option<Vec<PackageEntry>>> {
        let trusted_targets =
            self.tuf_client.database().trusted_targets().context("expected targets information")?;

        let target_path = TargetPath::new(package_name)?;
        let target = if let Some(target) = trusted_targets.targets().get(&target_path) {
            target
        } else {
            return Ok(None);
        };

        let hash_str = target
            .custom()
            .get("merkle")
            .ok_or_else(|| anyhow!("package {:?} is missing the `merkle` field", package_name))?;

        let hash_str: &str = hash_str.as_str().ok_or_else(|| {
            anyhow!("package {:?} hash should be a string, not {:?}", package_name, hash_str)
        })?;

        let hash = Hash::try_from(hash_str)?;

        Ok(Some(self.walk_meta_package(&hash, package_name, &None, include_subpackages).await?))
    }

    fn walk_meta_package<'a>(
        &'a self,
        hash: &'a Hash,
        package_name: &'a str,
        subpackage: &'a Option<String>,
        include_subpackages: bool,
    ) -> BoxFuture<'a, Result<Vec<PackageEntry>>> {
        async move {
            // Read the meta.far.
            let meta_far_bytes =
                self.read_blob_decompressed(hash).await.context("reading meta.far")?;
            let size: u64 = meta_far_bytes.len().try_into()?;

            let mut archive =
                fuchsia_archive::AsyncUtf8Reader::new(Adapter::new(Cursor::new(meta_far_bytes)))
                    .await?;

            let modified = self.blob_modification_time_secs(hash).await?;

            // Add entry for meta.far
            let mut entries = vec![PackageEntry {
                subpackage: subpackage.clone(),
                path: "meta.far".to_string(),
                hash: Some(*hash),
                size: Some(size),
                modified,
            }];

            entries.extend(archive.list().map(|item| PackageEntry {
                subpackage: subpackage.clone(),
                path: item.path().to_string(),
                hash: None,
                size: Some(item.length()),
                modified,
            }));

            match archive.read_file(MetaContents::PATH).await {
                Ok(c) => {
                    // Concurrently fetch the package blob sizes.
                    // FIXME(https://fxbug.dev/42179393): Use work queue so we can globally control the
                    // concurrency here, rather than limiting fetches per call.
                    let contents = MetaContents::deserialize(c.as_slice())?;
                    let futures: Vec<_> = contents
                        .contents()
                        .iter()
                        .map(|(name, hash)| async move {
                            let blob_size =
                                self.blob_decompressed_size(hash).await.with_context(|| {
                                    format!("getting decompressed size of blob {hash}")
                                })?;
                            let modified = self.blob_modification_time_secs(hash).await?;
                            Ok::<_, anyhow::Error>(PackageEntry {
                                subpackage: subpackage.clone(),
                                path: name.to_owned(),
                                hash: Some(*hash),
                                size: Some(blob_size),
                                modified,
                            })
                        })
                        .collect();
                    let mut tasks =
                        stream::iter(futures).buffer_unordered(LIST_PACKAGE_CONCURRENCY);
                    while let Some(entry) = tasks.try_next().await? {
                        entries.push(entry);
                    }
                }
                Err(e) => {
                    log::warn!("failed to read meta/contents for package {}: {}", package_name, e);
                }
            }

            if include_subpackages {
                match archive.read_file(MetaSubpackages::PATH).await {
                    Ok(c) => {
                        let subpackages = MetaSubpackages::deserialize(c.as_slice())?;
                        for (relative_url, hash) in subpackages.subpackages() {
                            let subpackage = match subpackage {
                                None => Some(relative_url.to_string()),
                                Some(p) => Some(format!("{p}/{relative_url}")),
                            };
                            entries.extend(
                                self.walk_meta_package(
                                    hash,
                                    package_name,
                                    &subpackage,
                                    include_subpackages,
                                )
                                .await?,
                            );
                        }
                    }
                    Err(_) => {
                        // Skip if package does not contain subpackages
                    }
                }
            }

            Ok(entries)
        }
        .boxed()
    }
}

impl<R> Debug for RepoClient<R>
where
    R: RepoProvider,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Repository").field("tuf_client", &self.tuf_client).finish_non_exhaustive()
    }
}

impl<R> TufRepositoryProvider<Pouf1> for RepoClient<R>
where
    R: RepoProvider,
{
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_client.remote_repo().fetch_metadata(meta_path, version)
    }

    fn fetch_target<'a>(
        &'a self,
        target_path: &TargetPath,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_client.remote_repo().fetch_target(target_path)
    }
}

impl<R> TufRepositoryStorage<Pouf1> for RepoClient<R>
where
    R: RepoProvider + TufRepositoryStorage<Pouf1>,
{
    fn store_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
        metadata: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.tuf_client.remote_repo().store_metadata(meta_path, version, metadata)
    }

    fn store_target<'a>(
        &'a self,
        target_path: &TargetPath,
        target: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.tuf_client.remote_repo().store_target(target_path, target)
    }
}

impl<R> RepoProvider for RepoClient<R>
where
    R: RepoProvider,
{
    fn spec(&self) -> RepositorySpec {
        self.tuf_client.remote_repo().spec()
    }

    fn aliases(&self) -> &BTreeSet<String> {
        self.tuf_client.remote_repo().aliases()
    }

    fn fetch_metadata_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.tuf_client.remote_repo().fetch_metadata_range(resource_path, range)
    }

    fn fetch_blob_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.tuf_client.remote_repo().fetch_blob_range(resource_path, range)
    }

    fn supports_watch(&self) -> bool {
        self.tuf_client.remote_repo().supports_watch()
    }

    fn watch(&self) -> Result<BoxStream<'static, ()>> {
        self.tuf_client.remote_repo().watch()
    }

    fn blob_modification_time<'a>(
        &'a self,
        path: &str,
    ) -> BoxFuture<'a, Result<Option<SystemTime>>> {
        self.tuf_client.remote_repo().blob_modification_time(path)
    }
}

pub(crate) async fn get_tuf_client<R>(
    tuf_repo: R,
) -> Result<TufClient<Pouf1, EphemeralRepository<Pouf1>, R>, Error>
where
    R: RepositoryProvider<Pouf1> + Sync,
{
    let metadata_repo = EphemeralRepository::<Pouf1>::new();

    let raw_signed_meta = {
        // FIXME(https://fxbug.dev/42173766) we really should be initializing trust, rather than just
        // trusting 1.root.json.
        let root = tuf_repo.fetch_metadata(&MetadataPath::root(), MetadataVersion::Number(1)).await;

        // If we couldn't find 1.root.json, see if root.json exists and try to initialize trust with it.
        let mut root = match root {
            Err(tuf::Error::MetadataNotFound { .. }) => {
                tuf_repo.fetch_metadata(&MetadataPath::root(), MetadataVersion::None).await?
            }
            Err(err) => return Err(err.into()),
            Ok(root) => root,
        };

        let mut buf = Vec::new();
        root.read_to_end(&mut buf).await.map_err(Error::Io)?;

        RawSignedMetadata::<Pouf1, _>::new(buf)
    };

    let client =
        TufClient::with_trusted_root(Config::default(), &raw_signed_meta, metadata_repo, tuf_repo)
            .await?;

    Ok(client)
}

/// This describes the metadata about a package.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct RepositoryPackage {
    /// The package name.
    pub name: String,

    /// The package merkle hash (the hash of the `meta.far`).
    pub hash: Hash,

    /// The last modification timestamp (seconds since UNIX epoch) if known.
    pub modified: Option<u64>,
}

/// This describes the metadata about a blob in a package.
#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct PackageEntry {
    /// The subpackage hierarchy this entry belongs to relative to the top level package.
    pub subpackage: Option<String>,

    /// The path inside the package namespace.
    pub path: String,

    /// The merkle hash of the file. If `None`, the file is in the `meta.far`.
    pub hash: Option<Hash>,

    /// The size of the blob, if known.
    pub size: Option<u64>,

    /// The last modification timestamp (seconds since UNIX epoch) if known.
    pub modified: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_builder::RepoBuilder;
    use crate::repository::PmRepository;
    use crate::test_utils::{
        PKG1_BIN_HASH, PKG1_HASH, PKG1_LIB_HASH, PKG2_HASH, make_pm_repository,
        make_readonly_empty_repository, repo_key, repo_private_key,
    };
    use assert_matches::assert_matches;
    use camino::{Utf8Path, Utf8PathBuf};
    use pretty_assertions::assert_eq;
    use std::fs::{self, create_dir_all};
    use tuf::repo_builder::RepoBuilder as TufRepoBuilder;
    use tuf::repository::FileSystemRepositoryBuilder;

    fn get_modtime(path: Utf8PathBuf) -> u64 {
        std::fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[fuchsia::test]
    async fn test_from_trusted_root() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Set up a simple repository.
        let repo = make_pm_repository(dir).await;
        let repo_keys = repo.repo_keys().unwrap();
        let repo_client = RepoClient::from_trusted_remote(&repo).await.unwrap();

        // Refresh all the metadata.
        RepoBuilder::from_client(&repo_client, &repo_keys)
            .refresh_metadata(true)
            .commit()
            .await
            .unwrap();

        // Make sure we can update a client with 2.root.json metadata.
        let buf = fs::read(dir.join("repository").join("2.root.json")).unwrap();
        let trusted_root = RawSignedMetadata::new(buf);

        let mut repo_client = RepoClient::from_trusted_root(&trusted_root, repo).await.unwrap();
        assert_matches!(repo_client.update().await, Ok(true));
        assert_eq!(repo_client.database().trusted_root().version(), 2);
    }

    #[fuchsia::test]
    async fn test_get_config() {
        let repo = make_readonly_empty_repository().await.unwrap();

        let repo_url: RepositoryUrl = "fuchsia-pkg://fake-repo".parse().unwrap();
        let mirror_url: http::Uri = "http://some-url:1234".parse().unwrap();

        assert_eq!(
            repo.get_config(repo_url.clone(), mirror_url.clone(), None).unwrap(),
            RepositoryConfigBuilder::new(repo_url.clone())
                .add_root_key(repo_key())
                .add_mirror(
                    MirrorConfigBuilder::new(mirror_url.clone()).unwrap().subscribe(true).build()
                )
                .build()
        );

        assert_eq!(
            repo.get_config(
                repo_url.clone(),
                mirror_url.clone(),
                Some(RepositoryStorageType::Persistent)
            )
            .unwrap(),
            RepositoryConfigBuilder::new(repo_url)
                .add_root_key(repo_key())
                .add_mirror(MirrorConfigBuilder::new(mirror_url).unwrap().subscribe(true).build())
                .repo_storage_type(RepositoryStorageType::Persistent)
                .build()
        );
    }

    #[fuchsia::test]
    async fn test_list_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let mut repo = RepoClient::from_trusted_remote(Box::new(make_pm_repository(&dir).await))
            .await
            .unwrap();
        repo.update().await.unwrap();

        // Look up the timestamp for the meta.far for the modified setting.
        let pkg1_modified = get_modtime(dir.join("repository/blobs/1").join(PKG1_HASH));
        let pkg2_modified = get_modtime(dir.join("repository/blobs/1").join(PKG2_HASH));

        let mut packages = repo.list_packages().await.unwrap();

        // list_packages returns the contents out of order. Sort the entries so they are consistent.
        packages.sort_unstable_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

        assert_eq!(
            packages,
            vec![
                RepositoryPackage {
                    name: "package1/0".into(),
                    hash: PKG1_HASH.try_into().unwrap(),
                    modified: Some(pkg1_modified),
                },
                RepositoryPackage {
                    name: "package2/0".into(),
                    hash: PKG2_HASH.try_into().unwrap(),
                    modified: Some(pkg2_modified),
                },
            ],
        );
    }

    #[fuchsia::test]
    async fn test_get_tuf_client() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let metadata_dir = dir.join("repository");
        create_dir_all(&metadata_dir).unwrap();

        let mut repo = FileSystemRepositoryBuilder::<Pouf1>::new(metadata_dir.clone())
            .targets_prefix("targets")
            .build();

        let key = repo_private_key();
        let metadata = TufRepoBuilder::create(&mut repo)
            .trusted_root_keys(&[&key])
            .trusted_targets_keys(&[&key])
            .trusted_snapshot_keys(&[&key])
            .trusted_timestamp_keys(&[&key])
            .commit()
            .await
            .unwrap();

        let database = Database::from_trusted_metadata(&metadata).unwrap();

        TufRepoBuilder::from_database(&mut repo, &database)
            .trusted_root_keys(&[&key])
            .trusted_targets_keys(&[&key])
            .trusted_snapshot_keys(&[&key])
            .trusted_timestamp_keys(&[&key])
            .stage_root()
            .unwrap()
            .commit()
            .await
            .unwrap();

        let backend = PmRepository::new(dir.to_path_buf());
        let _repo = RepoClient::from_trusted_remote(backend).await.unwrap();

        std::fs::remove_file(dir.join("repository").join("1.root.json")).unwrap();

        let backend = PmRepository::new(dir.to_path_buf());
        let _repo = RepoClient::from_trusted_remote(backend).await.unwrap();
    }

    #[fuchsia::test]
    async fn test_show_package() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let backend = make_pm_repository(&dir).await;
        let mut repo = RepoClient::from_trusted_remote(backend).await.unwrap();
        repo.update().await.unwrap();

        // Look up the timestamps for the blobs.
        let blob_dir = dir.join("repository/blobs/1");
        let meta_far_modified = get_modtime(blob_dir.join(PKG1_HASH));

        let bin_modified = get_modtime(blob_dir.join(PKG1_BIN_HASH));
        let lib_modified = get_modtime(blob_dir.join(PKG1_LIB_HASH));

        let mut entries = repo.show_package("package1/0", true).await.unwrap().unwrap();

        // show_packages returns contents out of order. Sort the entries so they are consistent.
        entries.sort_unstable_by(|lhs, rhs| lhs.path.cmp(&rhs.path));

        assert_eq!(
            entries,
            vec![
                PackageEntry {
                    subpackage: None,
                    path: "bin/package1".into(),
                    hash: Some(PKG1_BIN_HASH.try_into().unwrap()),
                    size: Some(15),
                    modified: Some(bin_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "lib/package1".into(),
                    hash: Some(PKG1_LIB_HASH.try_into().unwrap()),
                    size: Some(12),
                    modified: Some(lib_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta.far".into(),
                    hash: Some(PKG1_HASH.try_into().unwrap()),
                    size: Some(24576),
                    modified: Some(meta_far_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta/contents".into(),
                    hash: None,
                    size: Some(156),
                    modified: Some(meta_far_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta/fuchsia.abi/abi-revision".into(),
                    hash: None,
                    size: Some(8),
                    modified: Some(meta_far_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta/package".into(),
                    hash: None,
                    size: Some(33),
                    modified: Some(meta_far_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta/package1.cm".into(),
                    hash: None,
                    size: Some(11),
                    modified: Some(meta_far_modified),
                },
                PackageEntry {
                    subpackage: None,
                    path: "meta/package1.cmx".into(),
                    hash: None,
                    size: Some(12),
                    modified: Some(meta_far_modified),
                },
            ]
        );
    }
}

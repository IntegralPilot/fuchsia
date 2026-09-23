// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::range::Range;
use crate::repo_keys;
use crate::repository::{
    CopyMode, Error, FileSystemRepository, FileSystemRepositoryBuilder, RepoProvider, RepoStorage,
};
use crate::resource::Resource;
use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use delivery_blob::DeliveryBlobType;
use fuchsia_merkle::Hash;
use futures::AsyncRead;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::time::SystemTime;
use tuf::metadata::{MetadataPath, MetadataVersion, TargetPath};
use tuf::pouf::Pouf1;
use tuf::repository::{
    RepositoryProvider as TufRepositoryProvider, RepositoryStorage as TufRepositoryStorage,
};

#[cfg(not(target_os = "fuchsia"))]
use crate::repository::RepositorySpec;

pub struct PmRepositoryBuilder {
    pm_repo_path: Utf8PathBuf,
    builder: FileSystemRepositoryBuilder,
}

impl PmRepositoryBuilder {
    pub fn new(pm_repo_path: Utf8PathBuf) -> Self {
        let metadata_repo_path = pm_repo_path.join("repository");
        let blob_repo_path = metadata_repo_path.join("blobs");

        PmRepositoryBuilder {
            pm_repo_path,
            builder: FileSystemRepositoryBuilder::new(metadata_repo_path, blob_repo_path),
        }
    }

    /// Select which [CopyMode] to use when copying files into the repository.
    pub fn copy_mode(mut self, copy_mode: CopyMode) -> Self {
        self.builder = self.builder.copy_mode(copy_mode);
        self
    }

    /// Alias this repository to this name when this repository is registered on a target.
    pub fn alias(mut self, alias: String) -> Self {
        self.builder = self.builder.alias(alias);
        self
    }

    /// alias this repository to these names when this repository is registered on a target.
    pub fn aliases(mut self, aliases: impl IntoIterator<Item = String>) -> Self {
        self.builder = self.builder.aliases(aliases);
        self
    }

    /// Set the type of delivery blob to generate when copying blobs into the repository.
    pub fn delivery_blob_type(mut self, delivery_blob_type: DeliveryBlobType) -> Self {
        self.builder = self.builder.delivery_blob_type(delivery_blob_type);
        self
    }

    /// Set the path to the blob repo.
    pub fn blob_repo_path(mut self, blob_repo_path: Utf8PathBuf) -> Self {
        self.builder = self.builder.blob_repo_path(blob_repo_path);
        self
    }

    pub fn build(self) -> PmRepository {
        PmRepository { pm_repo_path: self.pm_repo_path, repo: self.builder.build() }
    }
}

/// Serve a repository from a local pm repository.
#[derive(Debug)]
pub struct PmRepository {
    pm_repo_path: Utf8PathBuf,
    repo: FileSystemRepository,
}

impl PmRepository {
    pub fn builder(pm_repo_path: Utf8PathBuf) -> PmRepositoryBuilder {
        PmRepositoryBuilder::new(pm_repo_path)
    }

    /// Construct a [PmRepository].
    pub fn new(pm_repo_path: Utf8PathBuf) -> Self {
        Self::builder(pm_repo_path).build()
    }

    /// Tries to return the pm repository [RepoKeys]. Returns an empty [RepoKeys] if the `keys/`
    /// directory does not exist in the repository.
    pub fn repo_keys(&self) -> Result<repo_keys::RepoKeys, repo_keys::ParseError> {
        let keys_path = self.pm_repo_path.join("keys");
        if keys_path.exists() {
            repo_keys::RepoKeys::from_dir(keys_path.as_std_path())
        } else {
            Ok(repo_keys::RepoKeys::builder().build())
        }
    }
}

impl RepoProvider for PmRepository {
    #[cfg(not(target_os = "fuchsia"))]
    fn spec(&self) -> RepositorySpec {
        RepositorySpec::Pm { path: self.pm_repo_path.clone(), aliases: self.repo.aliases().clone() }
    }

    fn aliases(&self) -> &BTreeSet<String> {
        self.repo.aliases()
    }

    fn fetch_metadata_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.repo.fetch_metadata_range(resource_path, range)
    }

    fn fetch_blob_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.repo.fetch_blob_range(resource_path, range)
    }

    fn supports_watch(&self) -> bool {
        self.repo.supports_watch()
    }

    fn watch(&self) -> Result<BoxStream<'static, ()>> {
        self.repo.watch()
    }

    fn blob_modification_time<'a>(
        &'a self,
        path: &str,
    ) -> BoxFuture<'a, Result<Option<SystemTime>>> {
        self.repo.blob_modification_time(path)
    }

    fn blob_type(&self) -> DeliveryBlobType {
        self.repo.blob_type()
    }
}

impl TufRepositoryProvider<Pouf1> for PmRepository {
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.repo.fetch_metadata(meta_path, version)
    }

    fn fetch_target<'a>(
        &'a self,
        target_path: &TargetPath,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.repo.fetch_target(target_path)
    }
}

impl TufRepositoryStorage<Pouf1> for PmRepository {
    fn store_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
        metadata: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.repo.store_metadata(meta_path, version, metadata)
    }

    fn store_target<'a>(
        &'a self,
        target_path: &TargetPath,
        target: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.repo.store_target(target_path, target)
    }
}

impl RepoStorage for PmRepository {
    fn store_blob<'a>(
        &'a self,
        hash: &Hash,
        len: u64,
        path: &Utf8Path,
    ) -> BoxFuture<'a, Result<()>> {
        self.repo.store_blob(hash, len, path)
    }
    fn store_delivery_blob<'a>(
        &'a self,
        hash: &Hash,
        path: &Utf8Path,
        delivery_blob_type: DeliveryBlobType,
    ) -> BoxFuture<'a, Result<()>> {
        self.repo.store_delivery_blob(hash, path, delivery_blob_type)
    }
}

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test tools for building and serving TUF repositories containing Fuchsia packages.

use crate::package::Package;
use crate::serve::ServedRepositoryBuilder;
use anyhow::{Context as _, Error, format_err};
use delivery_blob::DeliveryBlobType;
use fidl_fuchsia_pkg_ext::{
    MirrorConfig, RepositoryConfig, RepositoryConfigBuilder, RepositoryKey,
};
use fuchsia_merkle::Hash;
use fuchsia_repo::repo_builder::RepoBuilder;
use fuchsia_repo::repo_keys::RepoKeys;
use fuchsia_repo::repository::PmRepository;
use fuchsia_url::RepositoryUrl;
use maybe_owned::MaybeOwned;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use walkdir::WalkDir;

/// A builder to simplify construction of TUF repositories containing Fuchsia packages.
#[derive(Debug)]
pub struct RepositoryBuilder<'a> {
    packages: Vec<MaybeOwned<'a, Package>>,
    repodir: Option<PathBuf>,
    delivery_blob_type: DeliveryBlobType,
}

impl Default for RepositoryBuilder<'_> {
    fn default() -> Self {
        Self { packages: vec![], repodir: None, delivery_blob_type: DeliveryBlobType::Type1 }
    }
}

impl<'a> RepositoryBuilder<'a> {
    /// Creates a new `RepositoryBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `RepositoryBuilder` from a template TUF repository dir.
    pub fn from_template_dir(path: impl Into<PathBuf>) -> Self {
        Self { repodir: Some(path.into()), ..Self::default() }
    }

    /// Adds a package (or a reference to one) to the repository.
    pub fn add_package(mut self, package: impl Into<MaybeOwned<'a, Package>>) -> Self {
        self.packages.push(package.into());
        self
    }

    /// Set the type of delivery blob, the blobs will be exposed at "/blobs/{type}/".
    pub fn delivery_blob_type(mut self, delivery_blob_type: DeliveryBlobType) -> Self {
        self.delivery_blob_type = delivery_blob_type;
        self
    }

    /// Builds the repository.
    pub async fn build(self) -> Result<Repository, Error> {
        let repodir = tempfile::tempdir().context("create /repo")?;

        // If configured to use a template repository directory, first copy it into the repo dir.
        let keys = if let Some(templatedir) = self.repodir {
            for entry in WalkDir::new(&templatedir) {
                let entry = entry?;
                if entry.path() == templatedir {
                    continue;
                }
                let relative_entry_path = entry.path().strip_prefix(&templatedir)?;
                let target_path = repodir.path().join(relative_entry_path);
                if entry.file_type().is_dir() {
                    fs::create_dir(target_path)?;
                } else {
                    fs::copy(entry.path(), target_path)?;
                }
            }

            RepoKeys::from_dir(repodir.path().join("keys").as_path()).unwrap()
        } else {
            // Otherwise, generate a new empty repo and keys.
            let keys = RepoKeys::generate(repodir.path()).unwrap();

            RepoBuilder::create(
                PmRepository::builder(repodir.path().to_owned().try_into()?)
                    .delivery_blob_type(self.delivery_blob_type)
                    .build(),
                &keys,
            )
            .commit()
            .await
            .unwrap();

            keys
        };

        // Open the repo for an update.
        let pm_repo = PmRepository::builder(repodir.path().to_owned().try_into()?)
            .delivery_blob_type(self.delivery_blob_type)
            .build();
        let client = {
            let local = tuf::repository::EphemeralRepository::<tuf::pouf::Pouf1>::new();

            let mut client = tuf::client::Client::with_trusted_root_keys(
                tuf::client::Config::default(),
                tuf::metadata::MetadataVersion::None,
                keys.root_keys().len() as u32,
                keys.root_keys().iter().map(|key| key.public()),
                local,
                &pm_repo,
            )
            .await
            .unwrap();
            client.update().await.unwrap();

            client
        };
        let database = client.database();
        let mut repo = RepoBuilder::from_database(&pm_repo, &keys, database);

        repo = repo
            .add_packages(
                self.packages.iter().map(|package| package.artifacts().join("manifest.json")),
            )
            .await
            .unwrap();
        repo.commit().await.unwrap();

        Ok(Repository { dir: repodir, delivery_blob_type: self.delivery_blob_type })
    }
}

/// Metadata for a package contained within a [`Repository`].
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq)]
pub struct PackageEntry {
    path: String,
    meta_far_merkle: Hash,
    meta_far_size: usize,
}

pub(crate) fn iter_packages(
    reader: impl Read,
) -> Result<impl Iterator<Item = Result<PackageEntry, Error>>, Error> {
    // TODO when metadata is compatible, use rust-tuf instead.
    #[derive(Debug, Deserialize)]
    struct TargetsJson {
        signed: Targets,
    }
    #[derive(Debug, Deserialize)]
    struct Targets {
        targets: BTreeMap<String, Target>,
    }
    #[derive(Debug, Deserialize)]
    struct Target {
        custom: TargetCustom,
    }
    #[derive(Debug, Deserialize)]
    struct TargetCustom {
        merkle: String,
        size: usize,
    }

    let targets_json: TargetsJson = serde_json::from_reader(reader)?;

    Ok(targets_json.signed.targets.into_iter().map(|(path, target)| {
        Ok(PackageEntry {
            path,
            meta_far_merkle: target.custom.merkle.parse()?,
            meta_far_size: target.custom.size,
        })
    }))
}

/// A TUF repository generated by a [`RepositoryBuilder`].
#[derive(Debug)]
pub struct Repository {
    dir: TempDir,
    delivery_blob_type: DeliveryBlobType,
}

impl Repository {
    fn blobs_dir(&self) -> PathBuf {
        match self.delivery_blob_type {
            DeliveryBlobType::Type1 => self.dir.path().join("repository/blobs/1"),
            t => panic!("Unsupported delivery blob type: {:?}", t),
        }
    }

    /// Returns an iterator over all blobs contained in this repository.
    pub fn iter_blobs(&self) -> Result<impl Iterator<Item = Result<Hash, Error>>, Error> {
        Ok(fs::read_dir(self.blobs_dir())?
            .filter(|entry| entry.as_ref().map(|e| !e.path().is_dir()).unwrap_or(true))
            .map(|entry| {
                Ok(entry?
                    .file_name()
                    .to_str()
                    .ok_or_else(|| format_err!("non-utf8 file path"))?
                    .parse()?)
            }))
    }

    /// Returns a set of all blobs contained in this repository.
    pub fn list_blobs(&self) -> Result<BTreeSet<Hash>, Error> {
        self.iter_blobs()?.collect()
    }

    /// Removes the specified from the repository.
    pub fn purge_blobs(&self, blobs: impl Iterator<Item = Hash>) {
        for blob in blobs {
            fs::remove_file(self.blobs_dir().join(format!("{blob}"))).unwrap();
        }
    }

    /// Reads the contents of requested blob from the repository.
    pub fn read_blob(&self, merkle_root: &Hash) -> Result<Vec<u8>, Error> {
        let raw_blob = fs::read(self.blobs_dir().join(format!("{merkle_root}")))?;
        Ok(delivery_blob::decompress(&raw_blob)?)
    }

    /// Reads the contents of requested delivery blob from the repository.
    pub fn read_delivery_blob(
        &self,
        delivery_blob_type: u32,
        merkle_root: &Hash,
    ) -> Result<Vec<u8>, io::Error> {
        fs::read(
            self.dir.path().join(format!("repository/blobs/{delivery_blob_type}/{merkle_root}")),
        )
    }

    /// Writes a blob with the given merkle and data (which may not match), into the repository's
    /// blobs directory, without any compression.
    pub fn write_blob(&self, merkle_root: &Hash, blob: &[u8]) -> Result<usize, Error> {
        let blob_data_to_write = match self.delivery_blob_type {
            DeliveryBlobType::Type1 => {
                delivery_blob::Type1Blob::generate(blob, delivery_blob::CompressionMode::Never)
            }
            t => panic!("Unsupported delivery blob type: {:?}", t),
        };
        let () = fs::write(self.blobs_dir().join(format!("{merkle_root}")), &blob_data_to_write)
            .with_context(|| format!("writing blob: {merkle_root}"))?;
        Ok(blob_data_to_write.len())
    }

    /// Overwrites the delivery blob to uncompressed version from an uncompressed blob that is
    /// already in the repository, returns the size of the delivery blob.
    pub fn overwrite_uncompressed_delivery_blob(&self, merkle_root: &Hash) -> Result<usize, Error> {
        let blob =
            self.read_blob(merkle_root).with_context(|| format!("reading blob: {merkle_root}"))?;
        let delivery_blob =
            delivery_blob::Type1Blob::generate(&blob, delivery_blob::CompressionMode::Never);
        let () = fs::write(self.blobs_dir().join(format!("{merkle_root}")), &delivery_blob)?;
        Ok(delivery_blob.len())
    }

    /// Returns the path of the base of the repository.
    pub fn path(&self) -> PathBuf {
        self.dir.path().join("repository")
    }

    /// Returns an iterator over all packages contained in this repository.
    pub fn iter_packages(
        &self,
    ) -> Result<impl Iterator<Item = Result<PackageEntry, Error>>, Error> {
        iter_packages(io::BufReader::new(File::open(
            self.dir.path().join("repository/targets.json"),
        )?))
    }

    /// Returns a sorted vector of all packages contained in this repository.
    pub fn list_packages(&self) -> Result<Vec<PackageEntry>, Error> {
        let mut packages = self.iter_packages()?.collect::<Result<Vec<_>, _>>()?;
        packages.sort_unstable();
        Ok(packages)
    }

    /// Generate a [`RepositoryConfigBuilder`] suitable for configuring a
    /// package resolver to use this repository when it is served at the given
    /// URL.
    pub fn make_repo_config_builder(&self, url: RepositoryUrl) -> RepositoryConfigBuilder {
        let mut builder = RepositoryConfigBuilder::new(url);

        for key in self.root_keys() {
            builder = builder.add_root_key(key);
        }

        builder
    }

    /// Generate a [`RepositoryConfig`] suitable for configuring a package resolver to use this
    /// repository when it is served at the given URL.
    pub fn make_repo_config(
        &self,
        url: RepositoryUrl,
        mirror_config: Option<MirrorConfig>,
        use_local_mirror: bool,
    ) -> RepositoryConfig {
        let mut builder = self.make_repo_config_builder(url);

        if let Some(mirror_config) = mirror_config {
            builder = builder.add_mirror(mirror_config)
        }

        builder.use_local_mirror(use_local_mirror).build()
    }

    /// Get the root keys used by this repository.
    pub fn root_keys(&self) -> BTreeSet<RepositoryKey> {
        // TODO when metadata is compatible, use rust-tuf instead.
        #[derive(Debug, Deserialize)]
        struct RootJson {
            signed: Root,
        }
        #[derive(Debug, Deserialize)]
        struct Root {
            roles: BTreeMap<String, Role>,
            keys: BTreeMap<String, Key>,
        }
        #[derive(Debug, Deserialize)]
        struct Role {
            keyids: Vec<String>,
        }
        #[derive(Debug, Deserialize)]
        struct Key {
            keyval: KeyVal,
        }
        #[derive(Debug, Deserialize)]
        struct KeyVal {
            public: String,
        }

        let root_json: RootJson = serde_json::from_reader(io::BufReader::new(
            File::open(self.dir.path().join("repository/root.json")).unwrap(),
        ))
        .unwrap();
        let root = root_json.signed;

        root.roles["root"]
            .keyids
            .iter()
            .map(|keyid| {
                RepositoryKey::Ed25519(hex::decode(root.keys[keyid].keyval.public.clone()).unwrap())
            })
            .collect()
    }

    /// Serves the repository over HTTP using hyper.
    pub fn server(self: Arc<Self>) -> ServedRepositoryBuilder {
        ServedRepositoryBuilder::new(self)
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::package::PackageBuilder;

    #[fuchsia::test]
    async fn test_repo_builder() {
        let same_contents = b"same contents";
        let repo = RepositoryBuilder::new()
            .delivery_blob_type(DeliveryBlobType::Type1)
            .add_package(
                PackageBuilder::new("rolldice")
                    .add_resource_at("bin/rolldice", "#!/boot/bin/sh\necho 4\n".as_bytes())
                    .add_resource_at(
                        "meta/rolldice.cml",
                        r#"{"program":{"binary":"bin/rolldice"}}"#.as_bytes(),
                    )
                    .add_resource_at("data/duplicate_a", "same contents".as_bytes())
                    .build()
                    .await
                    .unwrap(),
            )
            .add_package(
                PackageBuilder::new("fortune")
                    .add_resource_at(
                        "bin/fortune",
                        "#!/boot/bin/sh\necho ask again later\n".as_bytes(),
                    )
                    .add_resource_at(
                        "meta/fortune.cml",
                        r#"{"program":{"binary":"bin/fortune"}}"#.as_bytes(),
                    )
                    .add_resource_at("data/duplicate_b", &same_contents[..])
                    .add_resource_at("data/duplicate_c", &same_contents[..])
                    .build()
                    .await
                    .unwrap(),
            )
            .build()
            .await
            .unwrap();

        let mut blobs = repo.list_blobs().unwrap();
        // 2 meta FARs, 2 binaries, and 1 duplicated resource
        assert_eq!(blobs.len(), 5);

        // Spot check the contents of a blob in the repo.
        let same_contents_merkle = fuchsia_merkle::root_from_slice(same_contents);
        assert_eq!(repo.read_blob(&same_contents_merkle).unwrap(), same_contents);
        assert_eq!(
            repo.read_delivery_blob(1, &same_contents_merkle).unwrap(),
            delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, same_contents)
        );

        let packages = repo.list_packages().unwrap();
        assert_eq!(
            packages.into_iter().map(|pkg| pkg.path).collect::<Vec<_>>(),
            vec!["fortune/0".to_owned(), "rolldice/0".to_owned()]
        );

        // Ensure purge_blobs purges blobs
        let cutpoint = blobs.iter().nth(2).unwrap().to_owned();
        let removed = blobs.split_off(&cutpoint);
        repo.purge_blobs(removed.into_iter());
        assert_eq!(repo.list_blobs().unwrap(), blobs);
    }

    #[fuchsia::test]
    async fn test_repo_builder_template() -> Result<(), Error> {
        let repodir = tempfile::tempdir().context("create tempdir")?;

        // Populate repodir with a freshly created repository.
        let keys_dir = repodir.path().join("keys");
        fs::create_dir(&keys_dir).unwrap();
        let repo_keys = RepoKeys::generate(&keys_dir).unwrap();
        RepoBuilder::create(
            PmRepository::builder(repodir.path().to_owned().try_into()?).build(),
            &repo_keys,
        )
        .commit()
        .await
        .unwrap();

        // Build a repo from the template.
        let repo = RepositoryBuilder::from_template_dir(repodir.path())
            .add_package(PackageBuilder::new("test").build().await?)
            .build()
            .await?;

        // Ensure the repository used the generated keys.
        for path in &["root.json", "snapshot.json", "timestamp.json", "targets.json"] {
            assert_eq!(
                fs::read(repodir.path().join("keys").join(path))?,
                fs::read(repo.dir.path().join("keys").join(path))?,
            );
        }

        Ok(())
    }
}

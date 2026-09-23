// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fuchsia_inspect as finspect;
use fuchsia_merkle::Hash;
use fuchsia_url::fuchsia_pkg::UnpinnedAbsolutePackageUrl;
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use log::warn;
use sorted_vec_map::SortedVecMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A forest of packages and the blobs they require (including subpackages).
#[derive(Debug)]
pub struct FrozenIndex<Marker> {
    /// The meta.fars of the packages (including subpackages).
    packages: HashSet<Hash>,
    /// The meta.fars and content blobs of the packages (including subpackages).
    /// Equivalently, the contents of `packages` plus their corresponding content blobs.
    blobs: Vec<Hash>,
    /// The package urls and hashes of the root packages (i.e. not including subpackages).
    root_package_urls_and_hashes: SortedVecMap<UnpinnedAbsolutePackageUrl, Hash>,

    phantom: std::marker::PhantomData<Marker>,
}

/// Marker type for BaseIndex.
#[derive(Debug)]
pub struct Base;
/// The system_image package, the packages in the static packages manifest, and the transitive
/// closure of their subpackages, or empty if the system does not have a system_image package.
pub type BaseIndex = FrozenIndex<Base>;

/// Marker type for CacheIndex.
#[derive(Debug)]
pub struct Cache;
/// The packages in the cache packages manifest and the transitive closure of their subpackages,
/// or empty if the system does not have a cache packages manifest.
/// To avoid breaking the system because of a failure in a non-critical package, packages and blobs
/// are loaded best-effort from blobfs. Packages with missing meta.fars or missing subpackage
/// meta.fars will be dropped from the index, resulting in:
///   * is_package returning false for the package
///   * list_blobs not returning any of the package's blobs (unless the blobs happen to be
///     referenced by another package)
///
/// root_package_urls_and_hashes will still return the URLs and hashes of dropped packages.
pub type CacheIndex = FrozenIndex<Cache>;

#[derive(Debug, Clone, Copy)]
enum OnPackageLoadError {
    Fail,
    Log,
}

impl FrozenIndex<Base> {
    pub async fn new(
        blobfs: &blobfs::Client,
        system_image: &system_image::SystemImage,
    ) -> Result<Self, anyhow::Error> {
        let base_repo = fuchsia_url::RepositoryUrl::parse_host("fuchsia.com".into())
            .expect("valid repository hostname");
        let root_package_urls_and_hashes = system_image
            .static_packages()
            .await
            .context("failed to determine static packages")?
            .into_contents()
            .into_iter()
            .chain([(system_image::SystemImage::package_path(), *system_image.hash())])
            .map(|(path, hash)| {
                let (name, variant) = path.into_name_and_variant();
                // TODO(https://fxbug.dev/42131375) Remove variant checks when variant concept is deleted.
                if !variant.is_zero() {
                    panic!("base package variants must be zero: {name} {variant}");
                }
                (UnpinnedAbsolutePackageUrl::new(base_repo.clone(), name, None), hash)
            })
            .collect::<SortedVecMap<_, _>>();
        Self::from_urls(blobfs, root_package_urls_and_hashes, OnPackageLoadError::Fail).await
    }
}

impl FrozenIndex<Cache> {
    pub async fn new(
        blobfs: &blobfs::Client,
        cache_packages: &system_image::CachePackages,
    ) -> Result<Self, anyhow::Error> {
        let root_package_urls_and_hashes = cache_packages
            .contents()
            .cloned()
            .map(|url| {
                let (mut url, hash) = url.into_unpinned_and_hash();
                // TODO(https://fxbug.dev/42131375) Remove variant checks when variant concept is deleted.
                if matches!(url.variant(), Some(v) if !v.is_zero()) {
                    panic!("cache package variants must be zero: {url}");
                }
                url.clear_variant();
                (url, hash)
            })
            .collect();
        Self::from_urls(blobfs, root_package_urls_and_hashes, OnPackageLoadError::Log).await
    }
}

impl<Marker: Send + Sync + 'static> FrozenIndex<Marker> {
    /// Create a `FrozenIndex` from a mapping of root package URLs to their hashes.
    /// Determines the content and subpackage blobs by reading the meta.fars from `blobfs`.
    async fn from_urls(
        blobfs: &blobfs::Client,
        root_package_urls_and_hashes: SortedVecMap<UnpinnedAbsolutePackageUrl, Hash>,
        on_package_load_error: OnPackageLoadError,
    ) -> Result<Self, anyhow::Error> {
        let (packages, blobs) = Self::load_packages_and_blobs(
            blobfs,
            root_package_urls_and_hashes.values().copied(),
            on_package_load_error,
        )
        .await
        .context("Error determining blobs")?;
        Ok(Self {
            packages,
            blobs: blobs.into_iter().collect(),
            root_package_urls_and_hashes,
            phantom: std::marker::PhantomData,
        })
    }

    /// Takes `root_packages`, the hashes of the root packages.
    /// Returns the hashes of all packages including subpackages and the hashes of all blobs
    /// including subpackages.
    async fn load_packages_and_blobs(
        blobfs: &blobfs::Client,
        root_packages: impl Iterator<Item = Hash>,
        on_package_load_error: OnPackageLoadError,
    ) -> Result<(HashSet<Hash>, HashSet<Hash>), anyhow::Error> {
        let memoized_packages = async_lock::RwLock::new(HashMap::new());
        let mut futures = futures::stream::iter(
            root_packages.map(|p| Self::package_blobs(blobfs, p, &memoized_packages)),
        )
        .buffer_unordered(1000);

        let mut blobs = HashSet::new();
        while let Some(res) = futures.next().await {
            use OnPackageLoadError::*;
            match (res, on_package_load_error) {
                (Ok(p), _) => blobs.extend(p),
                (Err(e), Fail) => Err(e)?,
                (Err(e), Log) => warn!("failed to load cache package: {:#}", anyhow!(e)),
            }
        }
        drop(futures);

        Ok((memoized_packages.into_inner().into_keys().collect(), blobs.into_iter().collect()))
    }

    // Returns all blobs of `package`: the meta.far, the content blobs, and the transitive
    // closure of subpackage blobs.
    async fn package_blobs(
        blobfs: &blobfs::Client,
        package: Hash,
        memoized_packages: &async_lock::RwLock<HashMap<Hash, HashSet<Hash>>>,
    ) -> Result<impl Iterator<Item = Hash>, anyhow::Error> {
        Ok(std::iter::once(package).chain(
            crate::required_blobs::find_required_blobs_recursive(
                blobfs,
                &package,
                memoized_packages,
                crate::required_blobs::ErrorStrategy::PropagateFailure,
            )
            .await
            .with_context(|| format!("determining required blobs for package {package}"))?,
        ))
    }

    /// Create an empty `FrozenIndex`, i.e. a `FrozenIndex` that does not have any packages (and
    /// therefore does not have any blobs). Useful for when there is no system_image package.
    pub fn empty() -> Self {
        Self {
            packages: HashSet::new(),
            blobs: vec![],
            root_package_urls_and_hashes: SortedVecMap::new(),
            phantom: std::marker::PhantomData,
        }
    }

    /// The meta.fars and content blobs of the packages (including subpackages).
    pub fn list_blobs(&self) -> &[Hash] {
        &self.blobs
    }

    /// Returns `true` iff `pkg` is the hash of a package (including subpackages).
    pub fn is_package(&self, pkg: Hash) -> bool {
        self.packages.contains(&pkg)
    }

    /// Hashmap mapping the root (i.e not including subpackages) package urls to hashes.
    pub fn root_package_urls_and_hashes(&self) -> &SortedVecMap<UnpinnedAbsolutePackageUrl, Hash> {
        &self.root_package_urls_and_hashes
    }

    /// Obtain the hash of the package indicated by the URL if it exists in the index, else None.
    pub fn url_to_hash(&self, url: &UnpinnedAbsolutePackageUrl) -> Option<&Hash> {
        self.root_package_urls_and_hashes.get(url)
    }

    /// Returns a callback to be given to `finspect::Node::record_lazy_child`.
    /// Records the URLs and hashes the FrozenIndex was initialized with.
    pub fn record_lazy_inspect(
        self: &Arc<Self>,
    ) -> impl Fn() -> BoxFuture<'static, Result<finspect::Inspector, anyhow::Error>>
    + Send
    + Sync
    + 'static {
        let this = Arc::downgrade(self);
        move || {
            let this = this.clone();
            async move {
                let inspector = finspect::Inspector::default();
                if let Some(this) = this.upgrade() {
                    let root = inspector.root();
                    let () = this.root_package_urls_and_hashes.iter().for_each(|(path, hash)| {
                        // Packages are encoded as nodes instead of string properties because the
                        // privacy allowlist prefers to wildcard nodes instead of properties.
                        root.record_child(path.to_string(), |n| {
                            n.record_string("hash", hash.to_string())
                        })
                    });
                }
                Ok(inspector)
            }
            .boxed()
        }
    }

    /// Test-only constructor to allow testing with this type without constructing a blobfs.
    /// `packages` isn't populated, so `is_package` will always return false.
    #[cfg(test)]
    pub(crate) fn new_test_only(
        blobs: HashSet<Hash>,
        root_package_urls_and_hashes: impl IntoIterator<Item = (UnpinnedAbsolutePackageUrl, Hash)>,
    ) -> Self {
        Self {
            packages: HashSet::new(),
            blobs: blobs.into_iter().collect(),
            root_package_urls_and_hashes: root_package_urls_and_hashes.into_iter().collect(),
            phantom: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_pkg_testing::{PackageBuilder, SystemImageBuilder};
    use std::iter::FromIterator as _;

    struct TestEnv {
        _blobfs: blobfs_ramdisk::BlobfsRamdisk,
        system_image: fuchsia_pkg_testing::Package,
        inspector: finspect::types::Inspector,
    }

    impl TestEnv {
        async fn new_with_subpackages(
            static_packages: &[&fuchsia_pkg_testing::Package],
            subpackages: &[&fuchsia_pkg_testing::Package],
        ) -> (Self, FrozenIndex<Base>) {
            let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
            let blobfs_client = blobfs.client();

            for p in static_packages.iter().chain(subpackages) {
                p.write_to_blobfs(&blobfs).await;
            }

            let system_image = fuchsia_pkg_testing::SystemImageBuilder::new()
                .static_packages(static_packages)
                .build()
                .await;
            system_image.write_to_blobfs(&blobfs).await;

            let inspector = finspect::Inspector::default();

            let base_packages = FrozenIndex::<Base>::new(
                &blobfs_client,
                &system_image::SystemImage::from_root_dir(
                    package_directory::RootDir::new(blobfs_client.clone(), *system_image.hash())
                        .await
                        .unwrap(),
                ),
            )
            .await
            .unwrap();

            (Self { _blobfs: blobfs, system_image, inspector }, base_packages)
        }

        async fn new(
            static_packages: &[&fuchsia_pkg_testing::Package],
        ) -> (Self, FrozenIndex<Base>) {
            Self::new_with_subpackages(static_packages, &[]).await
        }
    }

    #[fuchsia::test]
    async fn identifies_all_blobs() {
        let base_subpackage = PackageBuilder::new("base-subpackage")
            .add_resource_at("base-subpackage-blob", &b"base-subpackage-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let a_base_package = PackageBuilder::new("a-base-package")
            .add_resource_at("a-base-blob", &b"a-base-blob-contents"[..])
            .add_subpackage("my-subpackage", &base_subpackage)
            .build()
            .await
            .unwrap();
        let (env, base_packages) =
            TestEnv::new_with_subpackages(&[&a_base_package], &[&base_subpackage]).await;

        let expected_blobs = env
            .system_image
            .list_blobs()
            .into_iter()
            .chain(a_base_package.list_blobs())
            .chain(base_subpackage.list_blobs())
            .collect::<HashSet<_>>();
        assert_eq!(
            base_packages.list_blobs().iter().copied().collect::<HashSet<_>>(),
            expected_blobs
        );
        // Six expected blobs:
        //   system_image meta.far
        //   system_image content blob "data/static_packages"
        //   base-subpackage meta.far
        //   base-subpackage content blob "base-subpackage-blob"
        //   a-base-package meta.far
        //   a-base-package content blob "a-base-blob"
        assert_eq!(base_packages.list_blobs().len(), 6);
    }

    #[fuchsia::test]
    async fn correct_blob_count_shared_blob() {
        let a_base_package0 = PackageBuilder::new("a-base-package0")
            .add_resource_at("a-base-blob0", &b"duplicate-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let a_base_package1 = PackageBuilder::new("a-base-package1")
            .add_resource_at("a-base-blob1", &b"duplicate-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let (_env, base_packages) = TestEnv::new(&[&a_base_package0, &a_base_package1]).await;

        // Expect 5 blobs:
        //   * system_image meta.far
        //   * system_image data/static_packages
        //   * a-base-package0 meta.far
        //   * a-base-package0 a-base-blob0
        //   * a-base-package1 meta.far -> differs with a-base-package0 meta.far because
        //       meta/package and meta/contents differ
        //   * a-base-package1 a-base-blob1 -> duplicate of a-base-package0 a-base-blob0
        assert_eq!(base_packages.list_blobs().len(), 5);
    }

    #[fuchsia::test]
    async fn inspect_base_packages() {
        let base_subpackage = PackageBuilder::new("base-subpackage").build().await.unwrap();
        let a_base_package = PackageBuilder::new("a-base-package")
            .add_subpackage("my-subpackage", &base_subpackage)
            .build()
            .await
            .unwrap();
        let (env, base_packages) =
            TestEnv::new_with_subpackages(&[&a_base_package], &[&base_subpackage]).await;
        let base_packages = Arc::new(base_packages);

        env.inspector
            .root()
            .record_lazy_child("base-packages", base_packages.record_lazy_inspect());

        // Note base-subpackage is not present.
        assert_data_tree!(env.inspector, root: {
            "base-packages": {
                "fuchsia-pkg://fuchsia.com/a-base-package": {
                    "hash": a_base_package.hash().to_string(),
                },
                "fuchsia-pkg://fuchsia.com/system_image": {
                    "hash": env.system_image.hash().to_string(),
                }
            }
        });
    }

    #[fuchsia::test]
    async fn paths_and_hashes_includes_system_image() {
        let a_base_package = PackageBuilder::new("a-base-package")
            .add_resource_at("a-base-blob", &b"a-base-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let a_base_package_hash = *a_base_package.hash();
        let (env, base_packages) = TestEnv::new(&[&a_base_package]).await;

        assert_eq!(
            base_packages
                .root_package_urls_and_hashes()
                .iter()
                .map(|(p, h)| (p.clone(), *h))
                .collect::<HashSet<_>>(),
            HashSet::from_iter([
                (
                    "fuchsia-pkg://fuchsia.com/system_image".parse().unwrap(),
                    *env.system_image.hash()
                ),
                ("fuchsia-pkg://fuchsia.com/a-base-package".parse().unwrap(), a_base_package_hash),
            ])
        );
    }

    #[fuchsia::test]
    async fn paths_and_hashes_includes_system_image_even_if_no_static_packages() {
        let (env, base_packages) = TestEnv::new(&[]).await;

        assert_eq!(
            base_packages
                .root_package_urls_and_hashes()
                .iter()
                .map(|(p, h)| (p.clone(), *h))
                .collect::<HashSet<_>>(),
            HashSet::from_iter([(
                "fuchsia-pkg://fuchsia.com/system_image".parse().unwrap(),
                *env.system_image.hash()
            ),])
        );
    }

    #[fuchsia::test]
    async fn is_base_package_root_package() {
        let (env, base_packages) = TestEnv::new(&[]).await;
        let system_image = *env.system_image.hash();
        let mut not_system_image = Into::<[u8; 32]>::into(system_image);
        not_system_image[0] = !not_system_image[0];
        let not_system_image = Hash::from(not_system_image);

        assert!(base_packages.is_package(system_image));
        assert!(!base_packages.is_package(not_system_image));
    }

    #[fuchsia::test]
    async fn is_base_package_subpackage() {
        let subpackage = PackageBuilder::new("base-subpackage").build().await.unwrap();
        let superpackage = PackageBuilder::new("base-superpackage")
            .add_subpackage("my-subpackage", &subpackage)
            .build()
            .await
            .unwrap();
        let (_env, base_packages) =
            TestEnv::new_with_subpackages(&[&superpackage], &[&subpackage]).await;

        assert!(base_packages.is_package(*subpackage.hash()));
    }

    #[fuchsia::test]
    async fn base_packages_fails_when_reading_manifest_fails() {
        let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
        let blobfs_client = blobfs.client();
        // system_image package has no data/static_packages file
        let system_image = PackageBuilder::new("system_image").build().await.unwrap();
        system_image.write_to_blobfs(&blobfs).await;

        let base_packages_res = FrozenIndex::<Base>::new(
            &blobfs_client,
            &system_image::SystemImage::from_root_dir(
                package_directory::RootDir::new(blobfs_client.clone(), *system_image.hash())
                    .await
                    .unwrap(),
            ),
        )
        .await;

        assert_matches!(base_packages_res, Err(_))
    }

    #[fuchsia::test]
    async fn base_packages_fails_when_reading_package_fails() {
        let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
        let blobfs_client = blobfs.client();
        let present = PackageBuilder::new("present")
            .add_resource_at("present-blob", &b"present-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let absent = PackageBuilder::new("absent")
            .add_resource_at("absent-blob", &b"absent-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let system_image =
            SystemImageBuilder::new().static_packages(&[&present, &absent]).build().await;
        system_image.write_to_blobfs(&blobfs).await;
        present.write_to_blobfs(&blobfs).await;

        assert_matches!(
            FrozenIndex::<Base>::new(
                &blobfs_client,
                &system_image::SystemImage::from_root_dir(
                    package_directory::RootDir::new(blobfs_client.clone(), *system_image.hash())
                        .await
                        .unwrap(),
                ),
            )
            .await,
            Err(_)
        );
    }

    #[fuchsia::test]
    async fn cache_packages_ignores_failed_package_load() {
        let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
        let blobfs_client = blobfs.client();
        let present = PackageBuilder::new("present")
            .add_resource_at("present-blob", &b"present-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let absent = PackageBuilder::new("absent")
            .add_resource_at("absent-blob", &b"absent-blob-contents"[..])
            .build()
            .await
            .unwrap();
        let system_image =
            SystemImageBuilder::new().cache_packages(&[&present, &absent]).build().await;
        system_image.write_to_blobfs(&blobfs).await;
        present.write_to_blobfs(&blobfs).await;

        let cache_packages = FrozenIndex::<Cache>::new(
            &blobfs_client,
            &system_image::SystemImage::from_root_dir(
                package_directory::RootDir::new(blobfs_client.clone(), *system_image.hash())
                    .await
                    .unwrap(),
            )
            .cache_packages()
            .await
            .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(cache_packages.packages, HashSet::from([*present.hash()]));
        assert_eq!(cache_packages.blobs.len(), present.list_blobs().len());
        assert_eq!(
            cache_packages.blobs.iter().collect::<HashSet<_>>(),
            present.list_blobs().iter().collect::<HashSet<_>>(),
        );
        assert_eq!(
            cache_packages.root_package_urls_and_hashes(),
            &SortedVecMap::from_iter([
                ("fuchsia-pkg://fuchsia.com/present".parse().unwrap(), *present.hash()),
                ("fuchsia-pkg://fuchsia.com/absent".parse().unwrap(), *absent.hash())
            ])
        );
    }
}

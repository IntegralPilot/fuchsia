// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hash::Hash;
use futures::future::FutureExt as _;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ErrorStrategy {
    /// If any required blobs can be determined, return those blobs. Otherwise, return the error.
    BestEffort,

    /// Failure anywhere in the (possibly recursive) computation fails the entire computation.
    PropagateFailure,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum FindRequiredBlobsError {
    #[error("failed to create a RootDir for meta.far {meta_far}")]
    CreateRootDir { source: package_directory::Error, meta_far: Hash },

    #[error("reading the subpackages manifest of {meta_far}")]
    InvalidSubpackagesManifest { source: package_directory::SubpackagesError, meta_far: Hash },
}

/// Returns all required blobs (the content blobs and the transitive closure of all subpackage
/// meta.fars and content blobs) of package `meta_hash`.
/// Caches intermediate results in `memoized_packages` to de-duplicate work in case there are
/// shared subpackages.
/// `memoized_packages` maps meta.fars to their required blobs and on success will contain entries
/// for `meta_hash` and all its (transitive) subpackages.
/// On error:
///   If `error_strategy` is `BestEffort`:
///     If some required blobs could be determined, return those blobs, otherwise return the error.
///   If `error_strategy` is `PropagateFailure`
///     returns Error
/// TODO(https://fxbug.dev/42064108) Replace with an iterative implementation to avoid stack overflows.
pub(crate) fn find_required_blobs_recursive<'a>(
    blobfs: &'a blobfs::Client,
    meta_hash: &'a Hash,
    memoized_packages: &'a async_lock::RwLock<HashMap<Hash, HashSet<Hash>>>,
    error_strategy: ErrorStrategy,
) -> futures::future::BoxFuture<'a, Result<HashSet<Hash>, FindRequiredBlobsError>> {
    use ErrorStrategy::*;
    async move {
        if let Some(required_blobs) = memoized_packages.read().await.get(meta_hash) {
            return Ok(required_blobs.clone());
        }

        let root_dir = match package_directory::RootDir::new(blobfs.clone(), *meta_hash).await {
            Ok(root_dir) => root_dir,
            Err(e) => {
                return Err(FindRequiredBlobsError::CreateRootDir {
                    source: e,
                    meta_far: *meta_hash,
                });
            }
        };

        let mut required_blobs = root_dir.external_file_hashes().copied().collect::<HashSet<_>>();

        let subpackages = match root_dir.subpackages().await {
            Ok(subpackages) => subpackages.into_subpackages(),
            Err(e) => match error_strategy {
                BestEffort => {
                    memoized_packages.write().await.insert(*meta_hash, required_blobs.clone());
                    return Ok(required_blobs);
                }
                PropagateFailure => {
                    return Err(FindRequiredBlobsError::InvalidSubpackagesManifest {
                        source: e,
                        meta_far: *meta_hash,
                    });
                }
            },
        };
        for subpackage in subpackages.values() {
            required_blobs.insert(*subpackage);
            match find_required_blobs_recursive(
                blobfs,
                subpackage,
                memoized_packages,
                error_strategy,
            )
            .await
            {
                Ok(subpackage_required_blobs) => {
                    required_blobs.extend(subpackage_required_blobs);
                }
                Err(e) => match error_strategy {
                    BestEffort => continue,
                    PropagateFailure => return Err(e),
                },
            }
        }

        memoized_packages.write().await.insert(*meta_hash, required_blobs.clone());
        required_blobs.shrink_to_fit();
        Ok(required_blobs)
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::add_meta_far_to_blobfs;
    use assert_matches::assert_matches;
    use async_lock::RwLock;

    fn hash(n: u8) -> Hash {
        Hash::from([n; 32])
    }

    #[fuchsia::test]
    async fn success_no_subpackages() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [hash(1)], []);

        for strategy in [ErrorStrategy::BestEffort, ErrorStrategy::PropagateFailure] {
            let memoized_packages = RwLock::new(HashMap::new());
            assert_eq!(
                find_required_blobs_recursive(&blobfs, &hash(0), &memoized_packages, strategy)
                    .await
                    .unwrap(),
                HashSet::from_iter([hash(1)]),
                "strategy {strategy:?}"
            );
            assert_eq!(
                *memoized_packages.read().await,
                HashMap::from_iter([(hash(0), HashSet::from_iter([hash(1)]))]),
                "strategy {strategy:?}"
            );
        }
    }

    #[fuchsia::test]
    async fn success_recursive_subpackages() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [], [hash(1), hash(2)]);
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(1), "pkg-1", [hash(3)], []);
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(2), "pkg-2", [], [hash(4)]);
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(4), "pkg-4", [hash(5)], []);

        for strategy in [ErrorStrategy::BestEffort, ErrorStrategy::PropagateFailure] {
            let memoized_packages = RwLock::new(HashMap::new());
            assert_eq!(
                find_required_blobs_recursive(&blobfs, &hash(0), &memoized_packages, strategy)
                    .await
                    .unwrap(),
                HashSet::from_iter([hash(1), hash(2), hash(3), hash(4), hash(5)]),
                "strategy {strategy:?}"
            );
            assert_eq!(
                *memoized_packages.read().await,
                HashMap::from_iter([
                    (hash(0), HashSet::from_iter([hash(1), hash(2), hash(3), hash(4), hash(5)])),
                    (hash(1), HashSet::from_iter([hash(3)])),
                    (hash(2), HashSet::from_iter([hash(4), hash(5)])),
                    (hash(4), HashSet::from_iter([hash(5)])),
                ]),
                "strategy {strategy:?}"
            );
        }
    }

    #[fuchsia::test]
    async fn missing_meta_far() {
        let (_blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        for strategy in [ErrorStrategy::BestEffort, ErrorStrategy::PropagateFailure] {
            let memoized_packages = RwLock::new(HashMap::new());

            assert_matches!(
                find_required_blobs_recursive(&blobfs, &hash(0), &memoized_packages, strategy)
                    .await,
                Err(FindRequiredBlobsError::CreateRootDir { .. })
            );
            assert!(memoized_packages.read().await.is_empty());
        }
    }

    #[fuchsia::test]
    async fn best_effort_missing_subpackage_meta_far() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [hash(1)], [hash(2)]);

        let memoized_packages = RwLock::new(HashMap::new());
        assert_eq!(
            find_required_blobs_recursive(
                &blobfs,
                &hash(0),
                &memoized_packages,
                ErrorStrategy::BestEffort
            )
            .await
            .unwrap(),
            HashSet::from_iter([hash(1), hash(2)]),
        );
        assert_eq!(
            *memoized_packages.read().await,
            HashMap::from_iter([(hash(0), HashSet::from_iter([hash(1), hash(2)]))]),
        );
    }

    #[fuchsia::test]
    async fn propagate_failure_missing_subpackage_meta_far() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();
        let () = add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [hash(1)], [hash(2)]);

        let memoized_packages = RwLock::new(HashMap::new());
        assert_matches!(
            find_required_blobs_recursive(
                &blobfs,
                &hash(0),
                &memoized_packages,
                ErrorStrategy::PropagateFailure
            )
            .await,
            Err(FindRequiredBlobsError::CreateRootDir { .. })
        );
        assert!(memoized_packages.read().await.is_empty());
    }
}

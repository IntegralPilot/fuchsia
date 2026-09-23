// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::retained::{RetainedBlobs, RetainedIndex};
use super::writing::{CleanupGuard, WritingIndex};
use fuchsia_hash::Hash;
use futures::future::{BoxFuture, FutureExt as _};
use std::collections::HashSet;
use std::sync::Arc;
use {fidl_fuchsia_pkg as fpkg, fuchsia_inspect as finspect};

/// The index of packages known to pkg-cache.
#[derive(Clone, Debug)]
pub struct PackageIndex {
    retained: RetainedIndex,
    retained_blobs: RetainedBlobs,
    writing: WritingIndex,
}

#[derive(thiserror::Error, Debug)]
pub enum AddBlobsError {
    #[error("adding blobs to the writing index")]
    Writing(#[source] super::writing::AddBlobsError),
}

impl PackageIndex {
    /// Creates an empty PackageIndex.
    pub fn new() -> Self {
        Self {
            retained: RetainedIndex::new(),
            retained_blobs: RetainedBlobs::default(),
            writing: WritingIndex::new(),
        }
    }

    /// Notifies the appropriate indices that the package with the given hash is going to be
    /// written, ensuring the meta.far blob is protected by the index.
    /// The returned `CleanupGuard` must be given back to `Self::stop_writing`.
    pub fn start_writing(
        &mut self,
        pkg: Hash,
        gc_protection: fpkg::GcProtection,
    ) -> Option<CleanupGuard> {
        match gc_protection {
            // The purpose of Retained protection Get's is to not automatically protect the package
            // and instead to rely entirely on the client (the system-updater) to protect the
            // package using the Retained index. This allows the system-updater to meet the GC
            // requirements of the OTA process.
            fpkg::GcProtection::Retained => None,
            fpkg::GcProtection::OpenPackageTracking => Some(self.writing.start(pkg)),
        }
    }

    /// Associate additional blobs (e.g. subpackage meta.fars and content blobs) with a package
    /// that is being cached.
    pub fn add_blobs(
        &mut self,
        package_hash: Hash,
        additional_blobs: HashSet<Hash>,
        gc_protection: fpkg::GcProtection,
    ) -> Result<(), AddBlobsError> {
        // Always give the retained index information about dependent blobs to improve OTA forward
        // progress.
        let _: bool = self.retained.add_blobs(&package_hash, &additional_blobs);
        match gc_protection {
            fpkg::GcProtection::Retained => Ok(()),
            fpkg::GcProtection::OpenPackageTracking => self
                .writing
                .add_blobs(&package_hash, &additional_blobs)
                .map_err(AddBlobsError::Writing),
        }
    }

    /// Notifies the appropriate indices that the package corresponding to `guard` that was
    /// obtained from an earlier call to `Self::start_writing` is no longer being written.
    pub fn stop_writing(
        &mut self,
        guard: Option<super::writing::CleanupGuard>,
    ) -> Result<(), super::writing::StopError> {
        if let Some(guard) = guard {
            let () = self.writing.stop(guard)?;
        }
        Ok(())
    }

    fn set_retained_index(&mut self, mut index: RetainedIndex) {
        // Populate the new retained index with the relevant blobs from the writing index.
        let retained_packages = index.retained_packages().copied().collect::<Vec<_>>();
        for hash in retained_packages {
            if let Some(blobs) = self.writing.get_required_blobs(&hash) {
                // TODO(https://fxbug.dev/42064103) Consider replacing this panic with an error, or
                // e.g. adding a method to the retained index that makes both unnecessary.
                assert!(index.add_blobs(&hash, blobs));
            }
        }

        // Replace our retained index with the new one, which will populate content blobs from the
        // old retained index.
        self.retained.replace(index);
    }

    fn set_retained_blobs(&mut self, blobs: RetainedBlobs) {
        self.retained_blobs = blobs;
    }

    /// Returns all blobs protected by this index.
    pub fn all_blobs(&self) -> HashSet<Hash> {
        let mut all = self.retained.all_blobs();
        all.extend(self.retained_blobs.all_blobs());
        all.extend(self.writing.all_blobs());
        all
    }

    /// Returns a callback to be given to `finspect::Node::record_lazy_child`.
    /// The callback holds a weak reference to the PackageIndex.
    pub fn record_lazy_inspect(
        this: &Arc<async_lock::RwLock<Self>>,
    ) -> impl Fn() -> BoxFuture<'static, Result<finspect::Inspector, anyhow::Error>>
    + Send
    + Sync
    + 'static {
        let this = Arc::downgrade(this);
        move || {
            let this = this.clone();
            async move {
                let inspector = finspect::Inspector::default();
                if let Some(this) = this.upgrade() {
                    let index: Self = (*this.read().await).clone();
                    let root = inspector.root();
                    let () = root.record_child("retained", |n| index.retained.record_inspect(n));
                    let () = root
                        .record_child("retained-blobs", |n| index.retained_blobs.record_inspect(n));
                    let () = root.record_child("writing", |n| index.writing.record_inspect(n));
                } else {
                    inspector.root().record_string("error", "the package index was dropped");
                }
                Ok(inspector)
            }
            .boxed()
        }
    }
}

/// Replaces the retained blobs with one that tracks the given blob hashes.
pub async fn set_retained_blobs(
    index: &Arc<async_lock::RwLock<PackageIndex>>,
    blobs: HashSet<Hash>,
) {
    index.write().await.set_retained_blobs(RetainedBlobs::new(blobs));
}

/// Replaces the retained index with one that tracks the given meta far hashes.
pub async fn set_retained_index(
    index: &Arc<async_lock::RwLock<PackageIndex>>,
    blobfs: &blobfs::Client,
    meta_hashes: HashSet<Hash>,
) {
    // To avoid having to hold a lock while reading/parsing meta fars, first produce a fresh
    // retained index from meta fars available in blobfs.
    let new_retained = crate::index::retained::populate_retained_index(blobfs, meta_hashes).await;

    // Then atomically merge in available data from the retained indices and swap in the new
    // retained index.
    index.write().await.set_retained_index(new_retained);
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use maplit::hashmap;

    fn hash(n: u8) -> Hash {
        Hash::from([n; 32])
    }

    #[test]
    fn set_retained_index_hashes_are_extended_with_writing_index_hashes() {
        let mut index = PackageIndex::new();
        let guard = index.start_writing(hash(0), fpkg::GcProtection::OpenPackageTracking);
        index
            .add_blobs(
                hash(0),
                HashSet::from([hash(1), hash(2)]),
                fpkg::GcProtection::OpenPackageTracking,
            )
            .unwrap();

        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([hash(1)])),
        }));

        assert_eq!(
            index.retained.packages(),
            hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2)])),
            }
        );

        let () = index.stop_writing(guard).unwrap();
    }

    #[test]
    fn retained_index_is_not_informed_of_packages_it_does_not_track() {
        let mut index = PackageIndex::new();

        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([hash(10)])),
            hash(1) => None,
        }));

        // install a package not tracked by the retained index
        let guard = index.start_writing(hash(2), fpkg::GcProtection::OpenPackageTracking);
        index
            .add_blobs(hash(2), HashSet::from([hash(10)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();

        assert_eq!(
            index.retained.packages(),
            hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(10)])),
                hash(1) => None,
            }
        );
        assert_eq!(
            index.writing.packages(),
            hashmap! {
                hash(2) => HashSet::from_iter([hash(10)]),
            }
        );
        let () = index.stop_writing(guard).unwrap();
    }

    #[test]
    fn set_retained_index_to_self_is_nop() {
        let mut index = PackageIndex::new();
        let guard0 = index.start_writing(hash(3), fpkg::GcProtection::OpenPackageTracking);
        let guard1 = index.start_writing(hash(5), fpkg::GcProtection::OpenPackageTracking);

        index
            .add_blobs(hash(3), HashSet::from([hash(10)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();

        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([hash(11)])),
            hash(1) => None,
            hash(2) => None,
            hash(3) => None,
            hash(4) => None,
            hash(5) => None,
        }));

        index
            .add_blobs(hash(5), HashSet::from([hash(12)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();

        let retained_index = index.retained.clone();

        index.set_retained_index(retained_index.clone());
        assert_eq!(index.retained, retained_index);

        let () = index.stop_writing(guard0).unwrap();
        let () = index.stop_writing(guard1).unwrap();
    }

    #[test]
    fn add_blobs_with_open_package_tracking_protection_adds_to_retained_and_writing() {
        let mut index = PackageIndex::new();

        let guard = index.start_writing(hash(2), fpkg::GcProtection::OpenPackageTracking);
        index
            .add_blobs(hash(2), HashSet::from([hash(10)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();
        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([hash(10)])),
        }));

        index
            .add_blobs(hash(2), HashSet::from([hash(11)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();

        assert_eq!(
            index.retained.packages(),
            hashmap! {
                hash(2) => Some(HashSet::from([hash(10), hash(11)]))
            }
        );
        assert_eq!(
            index.writing.packages(),
            hashmap! {
                hash(2) => HashSet::from([hash(10), hash(11)])
            }
        );
        let () = index.stop_writing(guard).unwrap();
    }

    #[test]
    fn add_blobs_with_retained_protection_adds_to_retained_index_only() {
        let mut index = PackageIndex::new();

        let guard = index.start_writing(hash(2), fpkg::GcProtection::OpenPackageTracking);
        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(2) => None
        }));

        index.add_blobs(hash(2), HashSet::from([hash(11)]), fpkg::GcProtection::Retained).unwrap();

        assert_eq!(
            index.retained.packages(),
            hashmap! {
                hash(2) => Some(HashSet::from([hash(11)]))
            }
        );
        assert_eq!(
            index.writing.packages(),
            hashmap! {
                hash(2) => HashSet::new()
            }
        );
        let () = index.stop_writing(guard).unwrap();
    }

    #[test]
    fn add_blobs_errors_if_writing_index_in_wrong_state() {
        let mut index = PackageIndex::new();

        assert_matches!(
            index.add_blobs(
                hash(2),
                HashSet::from([hash(11)]),
                fpkg::GcProtection::OpenPackageTracking
            ),
            Err(AddBlobsError::Writing(super::super::writing::AddBlobsError::UnknownPackage{
                pkg
            })) if pkg == hash(2)
        );
    }

    #[test]
    fn all_blobs_produces_union_of_retained_and_writing_all_blobs() {
        let mut index = PackageIndex::new();

        let guard0 = index.start_writing(hash(0), fpkg::GcProtection::OpenPackageTracking);

        index.set_retained_index(RetainedIndex::from_packages(hashmap! {
            hash(0) => None,
            hash(5) => None,
            hash(6) => Some(HashSet::from_iter([hash(60), hash(61)])),
        }));

        let guard1 = index.start_writing(hash(1), fpkg::GcProtection::OpenPackageTracking);

        index
            .add_blobs(hash(0), HashSet::from([hash(10)]), fpkg::GcProtection::OpenPackageTracking)
            .unwrap();
        index
            .add_blobs(
                hash(1),
                HashSet::from([hash(11), hash(61)]),
                fpkg::GcProtection::OpenPackageTracking,
            )
            .unwrap();

        assert_eq!(
            index.all_blobs(),
            HashSet::from([
                hash(0),
                hash(1),
                hash(5),
                hash(6),
                hash(10),
                hash(11),
                hash(60),
                hash(61)
            ])
        );
        let () = index.stop_writing(guard0).unwrap();
        let () = index.stop_writing(guard1).unwrap();
    }

    #[test]
    fn stop_writing_fowarded_to_writing_index() {
        let mut index = PackageIndex::new();

        let guard = index.start_writing(hash(0), fpkg::GcProtection::OpenPackageTracking);
        let () = index.stop_writing(guard).unwrap();
        assert_eq!(index.writing.packages(), hashmap! {})
    }
}

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::{self as finspect};
use fuchsia_merkle::Hash;
use std::collections::{HashMap, HashSet};

/// Protects packages from GC while they are being written. This index does not protect packages
/// from GC after they have been written, so, to guarantee continuous protection, clients should
/// arrange for protection from another source, e.g. open package tracking, before informing the
/// index that the write is complete.
///
/// Allows clients to concurrently protect the same package (implemented with reference counting);
/// packages will be protected until all clients say they are done writing.
/// This avoids race conditions where pkg-resolver retries a PackageCache.Get call before
/// pkg-cache tears down the original call.
#[derive(Debug, Clone)]
pub struct WritingIndex {
    pkg_to_refcount_and_blobs: HashMap<Hash, (u64, HashSet<Hash>)>,
}

impl WritingIndex {
    /// Creates an empty `WritingIndex`.
    pub fn new() -> Self {
        Self { pkg_to_refcount_and_blobs: HashMap::new() }
    }

    /// Starts to protect `pkg`. Supports multiple outstanding calls to `start` for the same `pkg`.
    /// `pkg` will be protected until all outstanding calls are stopped.
    /// `Self::stop` *must* be called with the returned guard, dropping the returned guard will
    /// panic.
    /// `Self::stop` cannot be called by the guard's `Drop` impl because in practice the
    /// `WritingIndex` is behind an async lock (async because it is held during GC, which reads
    /// meta.fars and deletes blobs from blobfs).
    pub fn start(&mut self, pkg: Hash) -> CleanupGuard {
        self.pkg_to_refcount_and_blobs.entry(pkg).or_insert_with(|| (0, HashSet::new())).0 += 1;
        CleanupGuard { pkg }
    }

    /// Adds `blobs` to the set of blobs protected by `pkg`.
    /// `Self::start` must already have been called with `pkg`.
    pub fn add_blobs(&mut self, pkg: &Hash, blobs: &HashSet<Hash>) -> Result<(), AddBlobsError> {
        self.pkg_to_refcount_and_blobs
            .get_mut(pkg)
            .ok_or(AddBlobsError::UnknownPackage { pkg: *pkg })?
            .1
            .extend(blobs);
        Ok(())
    }

    /// Returns all blobs protected by the index.
    pub fn all_blobs(&self) -> HashSet<Hash> {
        let mut ret = HashSet::new();
        self.pkg_to_refcount_and_blobs.iter().for_each(|(p, (_, bs))| {
            ret.insert(*p);
            ret.extend(bs);
        });
        ret
    }

    /// Stops protecting the package corresponding to `guard` if `guard` is the last outstanding
    /// guard for the package.
    pub fn stop(&mut self, guard: CleanupGuard) -> Result<(), StopError> {
        let pkg = guard.consume();
        use std::collections::hash_map::Entry::*;
        match self.pkg_to_refcount_and_blobs.entry(pkg) {
            Occupied(mut o) => {
                // This should be impossible for clients to trigger because the count is
                // initialized to 1 when the first guard is created, and the entry is removed when
                // `stop` is called and the count is 1.
                if o.get().0 == 0 {
                    // Remove to allow retries to recover.
                    o.remove();
                    return Err(StopError::InvalidRefCount { pkg });
                } else if o.get().0 == 1 {
                    o.remove();
                } else {
                    o.get_mut().0 -= 1;
                }
            }
            // This should be impossible for clients to trigger because guards are only created by
            // calls to `start` (guards are not Clone), so the entry should not be removed until
            // the final outstanding guard is returned to `stop`.
            Vacant(_) => {
                return Err(StopError::UnknownPackage { pkg });
            }
        }
        Ok(())
    }

    /// Records self to `node`.
    pub fn record_inspect(&self, node: &finspect::Node) {
        for (pkg, (count, _)) in &self.pkg_to_refcount_and_blobs {
            node.record_child(pkg.to_string(), |n| {
                n.record_uint("count", *count);
            })
        }
    }

    /// The blobs (content and subpackage) known to be required by `package`. Not guaranteed to be
    /// complete (more required blobs are discovered as the meta.far and subpackage meta.fars are
    /// written).
    pub fn get_required_blobs(&self, package: &Hash) -> Option<&HashSet<Hash>> {
        self.pkg_to_refcount_and_blobs.get(package).map(|(_, bs)| bs)
    }

    #[cfg(test)]
    pub fn packages(&self) -> HashMap<Hash, HashSet<Hash>> {
        self.pkg_to_refcount_and_blobs.iter().map(|(p, (_, bs))| (*p, bs.clone())).collect()
    }
}

#[derive(Debug)]
#[must_use]
/// Created by calls to `WritingIndex::start` and *must* be returned to `WritingIndex::stop` when
/// the package is written.
///
/// Dropping the guard will panic.
///
/// `Self::stop` cannot be called by the guard's `Drop` impl because in practice the `WritingIndex`
/// is behind an async lock (async because it is held during GC, which reads meta.fars and deletes
/// blobs from blobfs).
pub struct CleanupGuard {
    pkg: Hash,
}

impl CleanupGuard {
    fn consume(self) -> Hash {
        let ret = self.pkg;
        std::mem::forget(self);
        ret
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        panic!(
            "Dropped CleanupGuard for {}. Guard must be returned to WritingIndex::stop",
            self.pkg
        );
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AddBlobsError {
    #[error("WritingIndex cannot add blobs to an unknown package {pkg}")]
    UnknownPackage { pkg: Hash },
}

#[derive(thiserror::Error, Debug)]
pub enum StopError {
    #[error("WritingIndex cannot stop writing an unknown package {pkg}")]
    UnknownPackage { pkg: Hash },

    #[error("WritingIndex cannot stop writing package {pkg} with a refcount of zero")]
    InvalidRefCount { pkg: Hash },
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    fn hash(b: u8) -> Hash {
        [b; 32].into()
    }

    #[test]
    fn regular_flow() {
        let mut index = WritingIndex::new();
        assert_eq!(index.all_blobs(), HashSet::new());

        let guard = index.start(hash(0));
        assert_eq!(index.all_blobs(), HashSet::from_iter([hash(0)]));

        let () = index.add_blobs(&hash(0), &HashSet::from_iter([hash(1), hash(2)])).unwrap();
        assert_eq!(index.all_blobs(), HashSet::from_iter([hash(0), hash(1), hash(2)]));

        let () = index.stop(guard).unwrap();
        assert_eq!(index.all_blobs(), HashSet::new());
        assert_eq!(index.pkg_to_refcount_and_blobs, HashMap::new());
    }

    #[test]
    fn refcount_prevents_removal() {
        let mut index = WritingIndex::new();

        let guard0 = index.start(hash(0));
        let () = index.add_blobs(&hash(0), &HashSet::from_iter([hash(1), hash(2)])).unwrap();

        let guard1 = index.start(hash(0));
        assert_eq!(index.all_blobs(), HashSet::from_iter([hash(0), hash(1), hash(2)]));
        assert_eq!(index.pkg_to_refcount_and_blobs[&hash(0)].0, 2);

        let () = index.stop(guard0).unwrap();
        assert_eq!(index.all_blobs(), HashSet::from_iter([hash(0), hash(1), hash(2)]));
        assert_eq!(index.pkg_to_refcount_and_blobs[&hash(0)].0, 1);

        let () = index.stop(guard1).unwrap();
        assert_eq!(index.all_blobs(), HashSet::new());
        assert_eq!(index.pkg_to_refcount_and_blobs, HashMap::new());
    }

    #[test]
    fn add_blobs_to_unknown_package() {
        let mut index = WritingIndex::new();
        let guard = index.start(hash(0));

        assert_matches!(
            index.add_blobs(&hash(1), &HashSet::from_iter([hash(1), hash(2)])),
            Err(AddBlobsError::UnknownPackage{pkg}) if pkg == hash(1)
        );

        let () = index.stop(guard).unwrap();
    }

    #[fuchsia::test]
    async fn inspect() {
        let mut index = WritingIndex::new();
        let guard = index.start(hash(0));
        let () = index.add_blobs(&hash(0), &HashSet::from_iter([hash(1), hash(2)])).unwrap();

        let inspector = finspect::Inspector::default();
        inspector.root().record_child("writing-index", |n| index.record_inspect(n));

        diagnostics_assertions::assert_data_tree!(
            inspector,
            root: {
                "writing-index": {
                    "0000000000000000000000000000000000000000000000000000000000000000": {
                        "count": 1u64
                    }
                }
            }
        );

        let () = index.stop(guard).unwrap();
    }
}

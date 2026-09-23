// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hash::Hash;
use fuchsia_inspect::{self as finspect};
use std::collections::{HashMap, HashSet};

/// An index of packages considered to be part of a new system's base package set.
#[derive(Clone, Debug, Default)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct RetainedIndex {
    /// Map from package hash to content and subpackage blobs (meta.fars and content blobs of
    /// all subpackages, plus the subpackages' subpackage blobs, recursively), if known.
    /// If the package is not cached (or is in the process of being cached), the list of
    /// content and subpackage blobs may not be complete.
    /// Value will be None if no hashes have been added yet, i.e. if the package's meta.far was not
    /// available when the package was added to the index.
    /// Value will be Some if at least some of the required blobs are known.
    /// TODO(https://fxbug.dev/42063880) Explicitly model the intermediate state in which the meta.far is
    /// cached but not all subpackage meta.fars are cached. The blobs cached in the intermediate
    /// state are protected from GC (serve_needed_blobs gives MissingBlobs a callback that adds
    /// blobs to the retained index as they are encountered in the caching process), but this could
    /// be made more obvious with more specific types.
    packages: HashMap<Hash, Option<HashSet<Hash>>>,
}

impl RetainedIndex {
    /// Creates an empty instance of the RetainedIndex.
    pub fn new() -> Self {
        Default::default()
    }

    /// The packages protected by this index.
    pub fn retained_packages(&self) -> impl Iterator<Item = &Hash> {
        self.packages.keys()
    }

    #[cfg(test)]
    pub fn from_packages(packages: HashMap<Hash, Option<HashSet<Hash>>>) -> Self {
        Self { packages }
    }

    #[cfg(test)]
    pub fn packages(&self) -> HashMap<Hash, Option<HashSet<Hash>>> {
        self.packages.clone()
    }

    /// Associates blobs with the given package hash if that package is known to the retained
    /// index, protecting those blobs from garbage collection.
    /// Returns true iff the package is known to this index.
    pub fn add_blobs(&mut self, meta_hash: &Hash, blobs: &HashSet<Hash>) -> bool {
        match self.packages.get_mut(meta_hash) {
            Some(required_blobs) => {
                match required_blobs {
                    Some(required_blobs) => required_blobs.extend(blobs),
                    None => *required_blobs = Some(blobs.clone()),
                }
                true
            }
            None => false,
        }
    }

    /// Replaces this retained index instance with other, populating other's blob sets
    /// using data from `self` when possible.
    pub fn replace(&mut self, mut other: Self) {
        for (meta_hash, other_hashes) in other.packages.iter_mut() {
            if let Some(Some(this_hashes)) = self.packages.remove(meta_hash) {
                if let Some(other_hashes) = other_hashes {
                    other_hashes.extend(this_hashes)
                } else {
                    *other_hashes = Some(this_hashes);
                }
            }
        }
        self.packages = other.packages;
    }

    /// Returns the set of all blobs currently protected by the retained index.
    pub fn all_blobs(&self) -> HashSet<Hash> {
        self.packages
            .iter()
            .flat_map(|(meta_hash, retained_hashes)| {
                std::iter::once(meta_hash).chain(retained_hashes.iter().flatten())
            })
            .copied()
            .collect()
    }

    /// Records self to `node`.
    pub fn record_inspect(&self, node: &finspect::Node) {
        node.record_child("entries", |n| {
            for (hash, state) in &self.packages {
                n.record_child(hash.to_string(), |n| {
                    if let Some(blobs) = state {
                        n.record_string("state", "known");
                        n.record_uint("blobs-count", blobs.len() as u64);
                    } else {
                        n.record_string("state", "need-meta-far");
                    }
                })
            }
        })
    }
}

/// Constructs a new [`RetainedIndex`] from the given blobfs client and set of package meta.far
/// hashes, populating blob hashes for any packages with a meta.far present in blobfs.
/// Populated blob hashes are not guaranteed to be complete because a subpackage meta.far may
/// not be cached.
pub async fn populate_retained_index(
    blobfs: &blobfs::Client,
    meta_hashes: HashSet<Hash>,
) -> RetainedIndex {
    let mut packages = HashMap::with_capacity(meta_hashes.len());
    let memoized_packages = async_lock::RwLock::new(HashMap::new());
    for meta_hash in meta_hashes {
        let found = crate::required_blobs::find_required_blobs_recursive(
            blobfs,
            &meta_hash,
            &memoized_packages,
            crate::required_blobs::ErrorStrategy::BestEffort,
        )
        .await
        .ok();
        packages.insert(meta_hash, found);
    }
    RetainedIndex { packages }
}

/// An index of blobs considered to be part of a new system's base package set.
#[derive(Clone, Debug, Default)]
pub struct RetainedBlobs(HashSet<Hash>);

impl RetainedBlobs {
    pub fn new(blobs: HashSet<Hash>) -> Self {
        Self(blobs)
    }

    /// Returns the set of all blobs currently protected by the retained blobs index.
    pub fn all_blobs(&self) -> &HashSet<Hash> {
        &self.0
    }

    /// Records self to `node`.
    pub fn record_inspect(&self, node: &finspect::Node) {
        node.record_uint("blobs-count", self.0.len() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::add_meta_far_to_blobfs;
    use maplit::{hashmap, hashset};

    fn hash(n: u8) -> Hash {
        Hash::from([n; 32])
    }

    #[test]
    fn add_blobs_nops_if_content_blobs_are_already_known() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([hash(1)])),
        });

        assert!(index.add_blobs(&hash(0), &hashset! {hash(1)}));

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1)])),
            })
        );
    }

    #[test]
    fn add_blobs_ignores_unknown_packages() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::new()),
        });

        assert!(!index.add_blobs(&hash(1), &hashset! {}));

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::new()),
            })
        );
    }

    #[test]
    fn add_blobs_remembers_content_blobs_for_package() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => None,
        });

        assert!(index.add_blobs(&hash(0), &hashset! {hash(1), hash(2), hash(3)}));

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2), hash(3)])),
            })
        );
    }

    #[test]
    fn add_blobs_add_to_existing_blobs() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => None,
        });

        assert!(index.add_blobs(&hash(0), &hashset! {hash(1)}));
        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1)])),
            })
        );

        assert!(index.add_blobs(&hash(0), &hashset! {hash(2)}));
        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2)])),
            })
        );
    }

    #[test]
    fn replace_retains_keys_from_other_only() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => None,
            hash(1) => None,
            hash(2) => None,
        });
        let other = RetainedIndex::from_packages(hashmap! {
            hash(2) => None,
            hash(3) => None,
            hash(4) => None,
        });

        index.replace(other.clone());
        assert_eq!(index, other);
    }

    #[test]
    fn replace_retains_values_already_present_in_other() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => None,
            hash(1) => None,
            hash(2) => None,
        });
        let other = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
            hash(3) => Some(HashSet::from_iter([ hash(12), hash(13) ])),
            hash(4) => None,
        });

        index.replace(other.clone());
        assert_eq!(index, other);
    }

    #[test]
    fn replace_populates_known_values_from_self() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([ hash(123) ])),
            hash(1) => None,
            hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
        });
        let other = RetainedIndex::from_packages(hashmap! {
            hash(2) => None,
            hash(3) => Some(HashSet::from_iter([ hash(12), hash(13) ])),
            hash(4) => None,
        });
        let merged = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
            hash(3) => Some(HashSet::from_iter([ hash(12), hash(13) ])),
            hash(4) => None,
        });

        index.replace(other);
        assert_eq!(index, merged);
    }

    #[test]
    fn replace_allows_both_self_and_other_values_to_be_populated() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
        });
        let other = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
        });

        index.replace(other.clone());
        assert_eq!(index, other);
    }

    #[test]
    fn replace_extends_other_from_self() {
        let mut index = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(10)])),
        });
        let other = RetainedIndex::from_packages(hashmap! {
            hash(2) => Some(HashSet::from_iter([ hash(11) ])),
        });

        index.replace(other);
        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(2) => Some(HashSet::from_iter([ hash(10), hash(11) ])),
            })
        );
    }

    #[test]
    fn all_blobs_produces_union_of_meta_and_content_hashes() {
        let index = RetainedIndex::from_packages(hashmap! {
            hash(0) => Some(HashSet::from_iter([hash(1), hash(2), hash(3)])),
            hash(4) => None,
            hash(5) => Some(HashSet::from_iter([hash(0), hash(4)])),
        });

        let expected = (0..=5).map(hash).collect::<HashSet<Hash>>();
        assert_eq!(expected.len(), 6);

        assert_eq!(expected, index.all_blobs());
    }

    #[fuchsia::test]
    async fn populate_retained_index_maps_missing_packages_to_unknown_content_blobs() {
        let (_blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        let index = populate_retained_index(&blobfs, [hash(0), hash(1), hash(2)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => None,
                hash(1) => None,
                hash(2) => None,
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_maps_present_packages_to_all_content_blobs() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(
            &blobfs_fake,
            hash(0),
            "pkg-0",
            vec![hash(1), hash(2), hash(30)],
            [],
        );
        add_meta_far_to_blobfs(
            &blobfs_fake,
            hash(10),
            "pkg-1",
            vec![hash(11), hash(12), hash(30)],
            [],
        );

        let index = populate_retained_index(&blobfs, [hash(0), hash(10)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2), hash(30)])),
                hash(10) => Some(HashSet::from_iter([hash(11), hash(12), hash(30)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_maps_invalid_meta_far_to_unknown_content_blobs() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        blobfs_fake.add_blob(hash(0), b"invalid blob");
        add_meta_far_to_blobfs(
            &blobfs_fake,
            hash(10),
            "pkg-0",
            vec![hash(1), hash(2), hash(3)],
            [],
        );

        let index = populate_retained_index(&blobfs, [hash(0), hash(10)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => None,
                hash(10) => Some(HashSet::from_iter([hash(1), hash(2), hash(3)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_dedupes_content_blobs() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", vec![hash(1), hash(1)], []);

        let index = populate_retained_index(&blobfs, [hash(0)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_adds_subpackage() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [], [hash(1)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(1), "pkg-1", [hash(2)], []);

        let index = populate_retained_index(&blobfs, [hash(0)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_adds_subpackage_of_subpackage() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [], [hash(1)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(1), "pkg-1", [], [hash(2)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(2), "pkg-2", [hash(3)], []);

        let index = populate_retained_index(&blobfs, [hash(0)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2), hash(3)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_ignores_missing_subpackage() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [], [hash(1), hash(2)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(2), "pkg-2", [hash(3)], []);

        let index = populate_retained_index(&blobfs, [hash(0)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(1), hash(2), hash(3)])),
            })
        );
    }

    #[fuchsia::test]
    async fn populate_retained_index_duplicate_subpackage() {
        let (blobfs_fake, blobfs) = fuchsia_pkg_testing::blobfs::Fake::new();

        add_meta_far_to_blobfs(&blobfs_fake, hash(0), "pkg-0", [], [hash(2)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(1), "pkg-1", [], [hash(2)]);
        add_meta_far_to_blobfs(&blobfs_fake, hash(2), "pkg-2", [], [hash(3)]);

        let index = populate_retained_index(&blobfs, [hash(0), hash(1)].into()).await;

        assert_eq!(
            index,
            RetainedIndex::from_packages(hashmap! {
                hash(0) => Some(HashSet::from_iter([hash(2), hash(3)])),
                hash(1) => Some(HashSet::from_iter([hash(2), hash(3)])),
            })
        );
    }
}

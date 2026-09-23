// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Trait to add helper methods on map-like entry types.
pub trait EntryExt<'a, K: 'a, V: 'a> {
    fn or_insert_with_fallible<E, F: FnOnce() -> Result<V, E>>(
        self,
        default: F,
    ) -> Result<&'a mut V, E>;
}

impl<'a, K: Ord + 'a, V: 'a> EntryExt<'a, K, V> for std::collections::btree_map::Entry<'a, K, V> {
    fn or_insert_with_fallible<E, F: FnOnce() -> Result<V, E>>(
        self,
        default: F,
    ) -> Result<&'a mut V, E> {
        let r = match self {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => v.insert(default()?),
        };
        Ok(r)
    }
}

impl<'a, K: 'a, V: 'a> EntryExt<'a, K, V> for std::collections::hash_map::Entry<'a, K, V> {
    fn or_insert_with_fallible<E, F: FnOnce() -> Result<V, E>>(
        self,
        default: F,
    ) -> Result<&'a mut V, E> {
        let r = match self {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => v.insert(default()?),
        };
        Ok(r)
    }
}

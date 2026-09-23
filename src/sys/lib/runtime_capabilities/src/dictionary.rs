// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Capability, CapabilityBound};
use derivative::Derivative;
use fuchsia_sync::{Mutex, MutexGuard};
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

#[cfg(target_os = "fuchsia")]
use fuchsia_async as fasync;

pub type Key = cm_types::Name;
pub type BorrowedKey = cm_types::BorrowedName;

/// A capability that represents a dictionary of capabilities.
#[derive(Debug)]
pub struct Dictionary {
    inner: Mutex<DictInner>,
}

#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct DictInner {
    /// The contents of the [Dictionary].
    pub(crate) entries: HybridMap,

    /// When an external request tries to access a non-existent entry, this closure will be invoked
    /// with the name of the entry.
    #[derivative(Debug = "ignore")]
    // Currently this is only used on target, but it's compatible with host.
    #[allow(dead_code)]
    pub(crate) not_found: Option<Box<dyn Fn(&str) -> () + 'static + Send + Sync>>,

    /// Tasks that serve the Dictionary protocol. This is an `Option` because dictionaries are not
    /// necessarily used in async contexts but a Scope will fail construction if there is no
    /// async executor.
    #[cfg(target_os = "fuchsia")]
    #[derivative(Debug = "ignore")]
    task_group: Option<fasync::Scope>,

    /// Functions that will be invoked whenever the contents of this dictionary changes.
    #[derivative(Debug = "ignore")]
    update_notifiers: UpdateNotifiers,
}

impl CapabilityBound for Dictionary {
    fn debug_typename() -> &'static str {
        "Dictionary"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        scope: vfs::execution_scope::ExecutionScope,
        token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        self.try_into_directory_entry_inner(scope, token)
    }
}

impl Drop for DictInner {
    fn drop(&mut self) {
        // When this dictionary doesn't exist anymore, then neither do its entries (or at least
        // these references of them). Notify anyone listening that all of the entries have been
        // removed.
        let entries = std::mem::replace(&mut self.entries, HybridMap::default());
        for (key, _) in entries.into_iter() {
            self.update_notifiers.update(EntryUpdate::Remove(&key))
        }
    }
}

/// Represents a change to a dictionary, where an entry is either added or removed.
#[derive(Debug, Copy, Clone)]
pub enum EntryUpdate<'a> {
    Add(&'a BorrowedKey, &'a Capability),
    Remove(&'a BorrowedKey),
    Idle,
}

/// Represents whether an update notifier should be retained and thus continue to be called for
/// future updates, or if it should be dropped and no longer be called.
pub enum UpdateNotifierRetention {
    Retain,
    Drop_,
}

/// A function that will be called when the contents of a dictionary changes. Note that this
/// function will be called while the internal dictionary structure is locked, so it must not
/// interact with the dictionary on which it is registered. It shouldn't even hold a strong
/// reference to the dictionary it's registered on, as that'll create a cycle and make LSAN sad.
pub type UpdateNotifierFn =
    Box<dyn for<'a> FnMut(EntryUpdate<'a>) -> UpdateNotifierRetention + Sync + Send>;

impl Default for Dictionary {
    fn default() -> Self {
        Self {
            inner: Mutex::new(DictInner {
                entries: HybridMap::default(),
                not_found: None,
                #[cfg(target_os = "fuchsia")]
                task_group: None,
                update_notifiers: UpdateNotifiers::default(),
            }),
        }
    }
}

impl Dictionary {
    /// Creates an empty dictionary.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Creates an empty dictionary. When an external request tries to access a non-existent entry,
    /// the name of the entry will be sent using `not_found`.
    pub fn new_with_not_found(not_found: impl Fn(&str) -> () + 'static + Send + Sync) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(DictInner {
                entries: HybridMap::default(),
                not_found: Some(Box::new(not_found)),
                #[cfg(target_os = "fuchsia")]
                task_group: None,
                update_notifiers: UpdateNotifiers::default(),
            }),
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, DictInner> {
        self.inner.lock()
    }

    /// Registers a new update notifier function with this dictionary. The function will be called
    /// whenever an entry is added to or removed from this dictionary. The function will be
    /// immediately called with an `EntryUpdate::Add` value for any entries already in this
    /// dictionary.
    ///
    /// Note that this function will be called while the internal dictionary structure is locked,
    /// so it must not interact with the dictionary on which it is registered. It shouldn't even
    /// hold a strong reference to the dictionary it's registered on, as that'll create a cycle and
    /// make LSAN sad.
    pub fn register_update_notifier(&self, mut notifier_fn: UpdateNotifierFn) {
        let mut guard = self.lock();
        for (key, value) in guard.entries.iter() {
            if let UpdateNotifierRetention::Drop_ = (notifier_fn)(EntryUpdate::Add(key, value)) {
                // The notifier has signaled that it doesn't want to process any more updates.
                return;
            }
        }
        if let UpdateNotifierRetention::Retain = (notifier_fn)(EntryUpdate::Idle) {
            guard.update_notifiers.0.push(notifier_fn);
        }
    }

    /// Inserts an entry, mapping `key` to `capability`. If an entry already exists at `key`, the
    /// old value will be returned.
    pub fn insert(&self, key: Key, capability: Capability) -> Option<Capability> {
        let DictInner { entries, update_notifiers, .. } = &mut *self.lock();
        entries.insert(key, capability, update_notifiers)
    }

    pub fn append(&self, other: &Dictionary) -> Result<(), ()> {
        let DictInner { entries, update_notifiers, .. } = &mut *self.lock();
        let other = other.lock();
        entries.append(&other.entries, update_notifiers)
    }

    /// Returns a clone of the capability associated with `key`. If there is no entry for `key`,
    /// `None` is returned.
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<Capability>
    where
        Key: Borrow<Q> + Ord,
        Q: Ord,
    {
        self.lock().entries.get(key)
    }

    /// Returns a clone of the capability associated with `key`, or populates it with `default` if
    /// it is not present.
    pub fn get_or_insert(&self, key: &Key, default: impl FnOnce() -> Capability) -> Capability {
        let DictInner { entries, update_notifiers, .. } = &mut *self.lock();
        match entries.get(key) {
            Some(v) => v,
            None => {
                let v = (default)();
                entries.insert(key.clone(), v.clone(), update_notifiers);
                v
            }
        }
    }

    /// Removes `key` from the entries, returning the capability at `key` if the key was already in
    /// the entries.
    pub fn remove(&self, key: &BorrowedKey) -> Option<Capability> {
        let DictInner { entries, update_notifiers, .. } = &mut *self.lock();
        entries.remove(key, update_notifiers)
    }

    /// Returns an iterator over a clone of the entries, sorted by key.
    pub fn enumerate(&self) -> impl Iterator<Item = (Key, Capability)> + use<> {
        self.lock()
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Returns the keys.
    pub fn snapshot_keys_as_strings(&self) -> Vec<String> {
        let guard = self.lock();
        match &guard.entries {
            HybridMap::Vec(vec) => vec.iter().map(|(k, _)| k.to_string()).collect(),
            HybridMap::Map(map) => map.keys().map(|k| k.to_string()).collect(),
        }
    }

    /// Returns if entries is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().entries.is_empty()
    }

    /// Removes all entries from the Dictionary and returns them as an iterator.
    pub fn drain(&self) -> impl Iterator<Item = (Key, Capability)> + use<> {
        let DictInner { entries, update_notifiers, .. } = &mut *self.lock();
        let entries = std::mem::replace(entries, HybridMap::default());
        for (key, _) in entries.iter() {
            update_notifiers.update(EntryUpdate::Remove(&key))
        }
        entries.into_iter()
    }

    /// Creates a new Dictionary with entries cloned from this Dictionary.
    ///
    /// This is a shallow copy. Values are cloned, not copied, so are new references to the same
    /// underlying data.
    pub fn shallow_copy(&self) -> Arc<Self> {
        let copy = Self::new();
        {
            let DictInner { entries, .. } = &*self.lock();
            let mut copy = copy.lock();
            copy.entries = entries.shallow_copy();
        }
        copy
    }

    /// Sends the name of an entry to the not found handler.
    // Currently this is only used on target, but it's compatible with host.
    #[allow(dead_code)]
    pub(crate) fn not_found(&self, entry: &str) {
        if let Some(not_found_handler) = &self.lock().not_found {
            not_found_handler(entry);
        }
    }
}

impl DictInner {
    #[cfg(target_os = "fuchsia")]
    pub(crate) fn tasks(&mut self) -> &mut fasync::Scope {
        self.task_group.get_or_insert_with(|| fasync::Scope::new())
    }
}

pub(crate) const HYBRID_SWITCH_INSERTION_LEN: usize = 11;
pub(crate) const HYBRID_SWITCH_REMOVAL_LEN: usize = 5;

/// A map collection whose representation is a `Vec` for small `N` and a `BTreeMap` for larger `N`,
/// where the threshold is defined by the constant `HYBRID_SWITCH_INSERTION_LEN` for insertion and
/// `HYBRID_SWITCH_REMOVAL_LEN` for removal. This is a more space efficient representation for
/// small `N` than a `BTreeMap`, which has a big impact because component_manager creates lots of
/// `Dictionary` objects for component sandboxes.
///
/// Details: Rust's `BTreeMap` implementation uses a `B` value of 6, which means each node in the
/// `BTreeMap` reserves space for 11 entries. Inserting 1 entry will allocate a node with space for
/// 11 entries. Each entry is 48 bytes, 48 * 11 + some metadata = 544 bytes. Rounding up to the
/// nearest scudo bucket size means each node consumes 656 bytes of memory.
///
/// A BTreeMap with only 2 entries uses 656 bytes when 112 would be sufficient (48 * 2 = 96 fits in
/// the 112 byte scudo bucket).
#[derive(Debug)]
pub(crate) enum HybridMap {
    Vec(Vec<(Key, Capability)>),
    Map(BTreeMap<Key, Capability>),
}

impl Default for HybridMap {
    fn default() -> Self {
        Self::Vec(Vec::default())
    }
}

impl HybridMap {
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<Capability>
    where
        Key: Borrow<Q> + Ord,
        Q: Ord,
    {
        match self {
            Self::Vec(vec) => match Self::sorted_vec_index_of(vec, key) {
                Ok(index) => Some(vec[index].1.clone()),
                Err(_) => None,
            },
            Self::Map(map) => match map.get(key) {
                Some(capability) => Some(capability.clone()),
                None => None,
            },
        }
    }

    pub fn insert(
        &mut self,
        key: Key,
        mut capability: Capability,
        update_notifiers: &mut UpdateNotifiers,
    ) -> Option<Capability> {
        match self {
            Self::Vec(vec) => match Self::sorted_vec_index_of(vec, &key) {
                Ok(index) => {
                    update_notifiers.update(EntryUpdate::Remove(&key));
                    update_notifiers.update(EntryUpdate::Add(&key, &capability));
                    std::mem::swap(&mut capability, &mut vec[index].1);
                    Some(capability)
                }
                Err(index) => {
                    update_notifiers.update(EntryUpdate::Add(&key, &capability));
                    if vec.len() + 1 >= HYBRID_SWITCH_INSERTION_LEN {
                        self.switch_to_map();
                        let Self::Map(map) = self else { unreachable!() };
                        map.insert(key, capability);
                        None
                    } else {
                        vec.reserve_exact(1);
                        vec.insert(index, (key, capability));
                        None
                    }
                }
            },
            Self::Map(map) => match map.entry(key) {
                Entry::Occupied(mut occupied) => {
                    update_notifiers.update(EntryUpdate::Remove(occupied.key()));
                    update_notifiers.update(EntryUpdate::Add(occupied.key(), &capability));
                    Some(occupied.insert(capability))
                }
                Entry::Vacant(vacant) => {
                    update_notifiers.update(EntryUpdate::Add(vacant.key(), &capability));
                    vacant.insert(capability);
                    None
                }
            },
        }
    }

    pub fn remove(
        &mut self,
        key: &BorrowedKey,
        update_notifiers: &mut UpdateNotifiers,
    ) -> Option<Capability> {
        let result = match self {
            Self::Vec(vec) => match Self::sorted_vec_index_of(vec, key) {
                Ok(index) => {
                    update_notifiers.update(EntryUpdate::Remove(key));
                    Some(vec.remove(index).1)
                }
                Err(_) => None,
            },
            Self::Map(map) => {
                let result = map.remove(key);
                if result.is_some() {
                    update_notifiers.update(EntryUpdate::Remove(key));
                    if self.len() <= HYBRID_SWITCH_REMOVAL_LEN {
                        self.switch_to_vec();
                    }
                }
                result
            }
        };
        result
    }

    pub fn append(
        &mut self,
        other: &Self,
        update_notifiers: &mut UpdateNotifiers,
    ) -> Result<(), ()> {
        if other.is_empty() {
            // If other is empty then return early.
            return Ok(());
        }

        // If any values would be overwritten, throw an error early and don't modify the Dictionary
        for (k, _) in other.iter() {
            let contains_key = match self {
                Self::Vec(vec) => matches!(Self::sorted_vec_index_of(vec, k), Ok(_)),
                Self::Map(map) => map.contains_key(k),
            };
            if contains_key {
                return Err(());
            }
        }

        if self.len() + other.len() >= HYBRID_SWITCH_INSERTION_LEN {
            // If at some point we will need to switch to a map then do it now.
            self.switch_to_map();
        } else if let Self::Vec(vec) = self {
            // We're currently a Vec and won't need to convert to a Map so grow the Vec to the
            // final size now.
            vec.reserve(other.len());
        }
        for (k, v) in other.iter() {
            self.insert(k.clone(), v.clone(), update_notifiers);
        }
        Ok(())
    }

    pub fn shallow_copy(&self) -> Self {
        match self {
            Self::Vec(vec) => {
                let mut new_vec = Vec::with_capacity(vec.len());
                for (key, value) in vec.iter() {
                    new_vec.push((key.clone(), value.clone()));
                }
                HybridMap::Vec(new_vec)
            }
            Self::Map(map) => {
                let mut new_map = BTreeMap::new();
                for (key, value) in map.iter() {
                    new_map.insert(key.clone(), value.clone());
                }
                HybridMap::Map(new_map)
            }
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Map(map) => map.len(),
            Self::Vec(vec) => vec.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Map(map) => map.is_empty(),
            Self::Vec(vec) => vec.is_empty(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Capability)> {
        match self {
            Self::Vec(vec) => itertools::Either::Left(vec.iter().map(|kv| (&kv.0, &kv.1))),
            Self::Map(map) => itertools::Either::Right(map.iter()),
        }
    }

    pub fn into_iter(self) -> impl Iterator<Item = (Key, Capability)> {
        match self {
            Self::Vec(vec) => itertools::Either::Left(vec.into_iter()),
            Self::Map(map) => itertools::Either::Right(map.into_iter()),
        }
    }

    fn switch_to_map(&mut self) {
        match self {
            Self::Map(_) => {}
            Self::Vec(vec) => {
                let vec = std::mem::replace(vec, Vec::new());
                let map = BTreeMap::from_iter(vec.into_iter());
                *self = Self::Map(map);
            }
        }
    }

    fn switch_to_vec(&mut self) {
        match self {
            Self::Vec(_) => {}
            Self::Map(map) => {
                let map = std::mem::replace(map, Default::default());
                let vec = Vec::from_iter(map.into_iter());
                *self = Self::Vec(vec);
            }
        }
    }

    #[inline]
    fn sorted_vec_index_of<Q: ?Sized>(vec: &Vec<(Key, Capability)>, key: &Q) -> Result<usize, usize>
    where
        Key: Borrow<Q> + Ord,
        Q: Ord,
    {
        vec.binary_search_by(|probe| probe.0.borrow().cmp(&key))
    }
}

#[derive(Default)]
pub(crate) struct UpdateNotifiers(Vec<UpdateNotifierFn>);

impl UpdateNotifiers {
    fn update<'a>(&'a mut self, update: EntryUpdate<'a>) {
        self.0.retain_mut(|notifier_fn| match (notifier_fn)(update) {
            UpdateNotifierRetention::Retain => true,
            UpdateNotifierRetention::Drop_ => false,
        });
    }
}

// Tests are located in fidl/dictionary.rs.

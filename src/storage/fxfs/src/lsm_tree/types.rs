// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::drop_event::DropEvent;
use crate::object_handle::ReadObjectHandle;
use crate::serialized_types::serialized_key::SerializeKey;
use crate::serialized_types::{Version, Versioned, VersionedLatest};
use anyhow::Error;
use async_trait::async_trait;
use fprint::TypeFingerprint;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

pub use fxfs_macros::impl_fuzzy_hash;

// Force keys to be sorted first by a u64, so that they can be located approximately based on only
// that integer without the whole key.
pub trait SortByU64: Sized {
    // Return the u64 that is used as the first value when deciding on sort order of the key.
    fn get_leading_u64(&self) -> u64;
}

/// An extension to `std::hash::Hash` to support values which should be partitioned and hashed into
/// buckets, where nearby keys will have the same hash value.  This is used for existence filtering
/// in layer files (see `Layer::maybe_contains_key`).
///
/// For point-based keys, this can be the same as `std::hash::Hash`, but for range-based keys, the
/// hash can collapse nearby ranges into the same hash value.  Since a range-based key may span
/// several buckets, `FuzzyHash::fuzzy_hash` must be called to split the key up into each of the
/// possible values that it overlaps with.
pub trait FuzzyHash: Hash + Sized {
    /// To support range-based keys, multiple hash values may need to be checked for a given key.
    /// For example, an extent [0..1024) might return extents [0..512), [512..1024), each of which
    /// will have a unique return value for `Self::hash`.  For point-based keys, a single hash
    /// suffices, in which case None is returned and the hash value of `self` should be checked.
    /// Note that in general only a small number of partitions (e.g. 2) should be checked at once.
    /// Queries checking too many partitions will fall back to returning true from bloom filter
    /// checks to avoid degenerate performance.
    fn fuzzy_hash(&self) -> impl ExactSizeIterator<Item = u64>;

    /// Returns whether the type is a range-based key. Used to prevent use of range-based keys as
    /// a point query (see [`crate::lsm_tree::merge::Query::Point`]).
    fn is_range_key(&self) -> bool {
        false
    }
}

impl_fuzzy_hash!(u8);
impl_fuzzy_hash!(u32);
impl_fuzzy_hash!(u64);
impl_fuzzy_hash!(String);
impl_fuzzy_hash!(Vec<u8>);

/// Keys and values need to implement the following traits.  For merging, they need to implement
/// MergeableKey.  TODO: Use trait_alias when available.
pub trait Key:
    Clone
    + Debug
    + Hash
    + FuzzyHash
    + OrdUpperBound
    + Send
    + SortByU64
    + Sync
    + Versioned
    + VersionedLatest
    + SerializeKey
    + std::marker::Unpin
    + 'static
{
}

impl<K> Key for K where
    K: Clone
        + Debug
        + Hash
        + FuzzyHash
        + OrdUpperBound
        + Send
        + SortByU64
        + Sync
        + Versioned
        + VersionedLatest
        + SerializeKey
        + std::marker::Unpin
        + 'static
{
}

pub trait MergeableKey: Key + Eq + LayerKey + OrdLowerBound {}
impl<K> MergeableKey for K where K: Key + Eq + LayerKey + OrdLowerBound {}

/// Trait required for supporting Layer functionality.
pub trait LayerValue:
    Clone + Send + Sync + Versioned + VersionedLatest + Debug + std::marker::Unpin + 'static
{
}
impl<V> LayerValue for V where
    V: Clone + Send + Sync + Versioned + VersionedLatest + Debug + std::marker::Unpin + 'static
{
}

/// Superset of `LayerValue` to additionally support tree searching, requires comparison and an
/// `DELETED_MARKER` for indicating empty values used to indicate deletion in the `LSMTree`.
pub trait Value: PartialEq + LayerValue {
    /// Value used to represent that the entry is actually empty, and should be ignored.
    const DELETED_MARKER: Self;
}

/// ItemRef is a struct that contains references to key and value, which is useful since in many
/// cases since keys and values are stored separately so &Item is not possible.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ItemRef<'a, K, V> {
    pub key: &'a K,
    pub value: &'a V,
}

impl<K: Clone, V: Clone> ItemRef<'_, K, V> {
    pub fn cloned(&self) -> Item<K, V> {
        Item { key: self.key.clone(), value: self.value.clone() }
    }

    pub fn boxed(&self) -> BoxedItem<K, V> {
        Box::new(self.cloned())
    }
}

impl<'a, K, V> Clone for ItemRef<'a, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'a, K, V> Copy for ItemRef<'a, K, V> {}

/// Item is a struct that combines a key and a value.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct Item<K, V> {
    pub key: K,
    pub value: V,
}

#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyItem<K, V> {
    pub key: K,
    pub value: V,
    pub sequence: u64,
}

impl<K: TypeFingerprint, V: TypeFingerprint> TypeFingerprint for LegacyItem<K, V> {
    fn fingerprint() -> String {
        "struct {key:".to_owned()
            + &K::fingerprint()
            + ",value:"
            + &V::fingerprint()
            + ",sequence:u64}"
    }
}

pub type BoxedItem<K, V> = Box<Item<K, V>>;

// Nb: type-fprint doesn't support generics yet.
impl<K: TypeFingerprint, V: TypeFingerprint> TypeFingerprint for Item<K, V> {
    fn fingerprint() -> String {
        "struct {key:".to_owned() + &K::fingerprint() + ",value:" + &V::fingerprint() + "}"
    }
}

impl<K, V> Item<K, V> {
    pub fn new(key: K, value: V) -> Item<K, V> {
        Item { key, value }
    }

    pub fn as_item_ref(&self) -> ItemRef<'_, K, V> {
        self.into()
    }

    pub fn boxed(self) -> BoxedItem<K, V> {
        Box::new(self)
    }
}

impl<'a, K, V> From<&'a Item<K, V>> for ItemRef<'a, K, V> {
    fn from(item: &'a Item<K, V>) -> ItemRef<'a, K, V> {
        ItemRef { key: &item.key, value: &item.value }
    }
}

/// The find functions will return items with keys that are greater-than or equal to the search key,
/// so for keys that are like extents, the keys should sort (via OrdUpperBound) using the end
/// of their ranges, and you should set the search key accordingly.
///
/// For example, let's say the tree holds extents 100..200, 200..250 and you want to perform a read
/// for range 150..250, you should search for 0..151 which will first return the extent 100..200
/// (and then the iterator can be advanced to 200..250 after). When merging, keys can overlap, so
/// consider the case where we want to merge an extent with range 100..300 with an existing extent
/// of 200..250. In that case, we want to treat the extent with range 100..300 as lower than the key
/// 200..250 because we'll likely want to split the extents (e.g. perhaps we want 100..200,
/// 200..250, 250..300), so for merging, we need to use a different comparison function and we deal
/// with that using the OrdLowerBound trait.
///
/// If your keys don't have overlapping ranges that need to be merged, then these can be the same as
/// std::cmp::Ord (use the DefaultOrdUpperBound and DefaultOrdLowerBound traits).

/// Trait for ordering keys by their upper bound (e.g. `end` for range-based keys).
///
/// This ordering is used within layer files and for positioning iterators via `search_key()`.
pub trait OrdUpperBound {
    fn cmp_upper_bound(&self, other: &Self) -> std::cmp::Ordering;
}

pub trait DefaultOrdUpperBound: OrdUpperBound + Ord {}

impl<T: DefaultOrdUpperBound> OrdUpperBound for T {
    fn cmp_upper_bound(&self, other: &Self) -> std::cmp::Ordering {
        // Default to using cmp.
        self.cmp(other)
    }
}

/// Trait for ordering keys by their lower bound (e.g. `start` for range-based keys).
///
/// This ordering is used exclusively by the merger's min-heap to stream keys out in
/// left-to-right order (by `start`). It is distinct from `OrdUpperBound` and is not used
/// with search keys.
pub trait OrdLowerBound {
    fn cmp_lower_bound(&self, other: &Self) -> std::cmp::Ordering;
}

pub trait DefaultOrdLowerBound: OrdLowerBound + Ord {}

impl<T: DefaultOrdLowerBound> OrdLowerBound for T {
    fn cmp_lower_bound(&self, other: &Self) -> std::cmp::Ordering {
        // Default to using cmp.
        self.cmp(other)
    }
}

/// Result returned by `merge_type()` to determine how to properly merge values within a layerset.
#[derive(Clone, PartialEq)]
pub enum MergeType {
    /// Always includes every layer in the merger, when seeking or advancing. Always correct, but
    /// always as slow as possible.
    FullMerge,

    /// Stops seeking older layers when an exact key match is found in a newer one. Useful for keys
    /// that only replace data, or with `next_key()` implementations to decide on continued merging.
    OptimizedMerge,
}

/// Determines how to iterate forward from the current key, and how many older layers to include
/// when merging. See the different variants of `MergeKeyType` for more details.
pub trait LayerKey: Clone {
    /// Called to determine how to perform merge behaviours while advancing through a layer set.
    fn merge_type(&self) -> MergeType {
        // Defaults to full merge. The slowest, but most predictable in behaviour.
        MergeType::FullMerge
    }

    /// The next_key() call allows for an optimisation which allows the merger to avoid querying a
    /// layer if it knows it has found the next possible key.  It only makes sense for this to
    /// return Some() when merge_type() returns OptimizedMerge. Consider the following example
    /// showing two layers with range based keys.
    ///
    ///      +----------+------------+
    ///  0   |  0..100  |  100..200  |
    ///      +----------+------------+
    ///  1              |  100..200  |
    ///                 +------------+
    ///
    /// If you search and find the 0..100 key, then only layer 0 will be touched.  If you then want
    /// to advance to the 100..200 record, you can find it in layer 0 but unless you know that it
    /// immediately follows the 0..100 key i.e. that there is no possible key, K, such that 0..100 <
    /// K < 100..200, the merger has to consult all other layers to check.  `next_key` should return
    /// a search key for an extent that immediately follows.  In practice, for extents, this should
    /// be `end..end + 1`.
    ///
    /// This is purely an optimisation; the default None will be correct but not performant.
    fn next_key(&self) -> Option<Self> {
        None
    }
    /// Returns the search key (S) for this key (K), such that when searching for S in a layer
    /// file, it returns the earliest possible key that might be relevant to K.  Searching in a
    /// layer file is done using `cmp_upper_bound` and the iterator will be positioned on a key that
    /// is greater than or equal to S.  Returning `None` here is the right thing to do for
    /// non-ranged based keys, in which case K is used to search for the key.  In practice, the
    /// implementation should return `Some(start..start + 1)` for range based keys and `None`
    /// for everything else.  As an example, if the tree has extents 50..150 and 150..200 and we
    /// wish to search for 100..200, search_key would return 100..101 which would position the
    /// iterator on 50..150.  If this method is overridden, `is_search_key` below should also be
    /// overridden.
    fn search_key(&self) -> Option<Self> {
        None
    }

    /// If you override `search_key` you should override `is_search_key`.
    fn is_search_key(&self) -> bool {
        true
    }

    /// Returns true if the keys are overlapping or equal.
    fn overlaps(&self, other: &Self) -> bool;
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Existence {
    /// The key definitely exists.
    Exists,
    /// The key might exist.
    MaybeExists,
    /// The key definitely does not exist.
    Missing,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum MaybeContainsKey {
    /// The layer definitely does not contain records relevant to the key.
    False,
    /// The layer might contain records relevant to the key.
    Maybe,
    /// The range key was too large to check against the existence filter, so the check was skipped.
    RangeKeyTooLarge,
}

/// Layer is a trait that all layers need to implement (mutable and immutable).
#[async_trait]
pub trait Layer<K, V>: Send + Sync {
    /// If the layer is persistent, returns the handle to its contents.  Returns None for in-memory
    /// layers.
    fn handle(&self) -> Option<&dyn ReadObjectHandle> {
        None
    }

    /// Some layer implementations may choose to cache data in-memory.  Calling this function will
    /// request that the layer purges unused cached data.  This is intended to run on a timer.
    fn purge_cached_data(&self) {}

    /// Immediately clears any in-memory cached data for this layer.
    fn clear_cached_data(&self) {}

    /// Searches for a key. Bound::Excluded is not supported. Bound::Unbounded positions the
    /// iterator on the first item in the layer.
    async fn seek(&self, bound: std::ops::Bound<&K>)
    -> Result<BoxedLayerIterator<'_, K, V>, Error>;

    /// Returns the number of items in the layer file.
    fn len(&self) -> usize;

    /// Returns whether the layer *might* contain records relevant to `key`.  Note that this can
    /// return `MaybeContainsKey::Maybe` even if the layer has no records relevant to `key`, but it will
    /// never return `MaybeContainsKey::False` if there are such records.  (As such, always
    /// returning `MaybeContainsKey::Maybe` is a trivially correct implementation.)
    /// If the key has too many hash partitions to check against the existence filter, returns
    /// `MaybeContainsKey::RangeKeyTooLarge`.
    fn maybe_contains_key(&self, _key: &K) -> MaybeContainsKey {
        MaybeContainsKey::Maybe
    }

    /// This is similar to `maybe_contains_key` except that there *must* be a `key` and possible
    /// state where this will indicate the key is missing i.e. *always* returning
    /// `Existence::MaybeExists` is *not* a correct implementation.  If an implementation has a low
    /// cost way to determine if the key *might* exist, then it may use that, but if not, it must
    /// use a slower algorithm.  This method was introduced to allow for an efficient way of
    /// determining if a particular key is free to be used.  `maybe_contains_key` cannot be used for
    /// this purpose because implementations might *always* return `true` which would mean it would
    /// be impossible to ever find a key that is free to use.  It might not be appropriate to use
    /// this with range based keys: implementations should use `cmp_upper_bound` and test for
    /// equality, which might not give the desired results for range based keys.
    async fn key_exists(&self, key: &K) -> Result<Existence, Error>;

    /// Locks the layer preventing it from being closed. This will never block i.e. there can be
    /// many locks concurrently.  The lock is purely advisory: seek will still work even if lock has
    /// not been called; it merely causes close to wait until all locks are released.  Returns None
    /// if close has been called for the layer.
    fn lock(&self) -> Option<Arc<DropEvent>>;

    /// Waits for existing locks readers to finish and then returns.  Subsequent calls to lock will
    /// return None.
    async fn close(&self);

    /// Returns the version number used by structs in this layer
    fn get_version(&self) -> Version;

    /// Records inspect data for the layer into `node`.  Called lazily when inspect is queried.
    fn record_inspect_data(self: Arc<Self>, _node: &fuchsia_inspect::Node) {}
}

/// Something that implements LayerIterator is returned by the seek function.
pub trait LayerIterator<K, V>: Send + Sync {
    /// Advances the iterator (static dispatch, unboxed future).
    fn advance(&mut self) -> impl Future<Output = Result<(), Error>> + Send
    where
        Self: Sized;

    /// Advances the iterator for dynamic dispatch (trait objects).
    /// If the iterator advances synchronously, returns `Ok(None)`.
    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error>;

    /// Returns the current item. This will be None if called when the iterator is first crated i.e.
    /// before either seek or advance has been called, and None if the iterator has reached the end
    /// of the layer.
    fn get(&self) -> Option<ItemRef<'_, K, V>>;

    /// Creates an iterator that only yields items from the underlying iterator for which
    /// `predicate` returns `true`.
    fn filter<P>(
        self,
        predicate: P,
    ) -> impl Future<Output = Result<FilterLayerIterator<Self, P, K, V>, Error>> + Send
    where
        P: for<'b> Fn(ItemRef<'b, K, V>) -> bool + Send + Sync,
        Self: Sized,
        K: Send + Sync,
        V: Send + Sync,
    {
        FilterLayerIterator::new(self, predicate)
    }
}

pub type BoxedLayerIterator<'iter, K, V> = Box<dyn LayerIterator<K, V> + 'iter>;

impl<'iter, K, V> LayerIterator<K, V> for BoxedLayerIterator<'iter, K, V> {
    async fn advance(&mut self) -> Result<(), Error> {
        if let Some(fut) = self.as_mut().advance_dyn()? {
            fut.await?;
        }
        Ok(())
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        self.as_mut().advance_dyn()
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        self.as_ref().get()
    }
}

/// Mutable layers need an iterator that implements this in order to make merge_into work.
pub(super) trait LayerIteratorMut<K, V>: Sync {
    /// Advances the iterator.
    fn advance(&mut self);

    /// Returns the current item. This will be None if called when the iterator is first crated i.e.
    /// before either seek or advance has been called, and None if the iterator has reached the end
    /// of the layer.
    fn get(&self) -> Option<ItemRef<'_, K, V>>;

    /// Inserts the item before the item that the iterator is located at.  The insert won't be
    /// visible until the changes are committed (see `commit`).
    fn insert(&mut self, item: Item<K, V>);

    /// Erases the current item and positions the iterator on the next item, if any.  The change
    /// won't be visible until committed (see `commit`).
    fn erase(&mut self);

    /// Commits the changes.  This does not wait for existing readers to finish.
    fn commit(&mut self);
}

/// Trait for writing new layers.
pub trait LayerWriter<K, V>: Sized
where
    K: Debug + Send + Versioned + Sync,
    V: Debug + Send + Versioned + Sync,
{
    /// Writes the given item to this layer.
    fn write(&mut self, item: ItemRef<'_, K, V>) -> impl Future<Output = Result<(), Error>> + Send;

    /// Flushes any buffered items to the backing storage. The total number of bytes written is
    /// returned.
    fn complete(self) -> impl Future<Output = Result<u64, Error>> + Send;
}

/// A `LayerIterator`` that filters the items of another `LayerIterator`.
pub struct FilterLayerIterator<I, P, K, V> {
    iter: I,
    predicate: P,
    _key: PhantomData<K>,
    _value: PhantomData<V>,
}

impl<I, P, K, V> FilterLayerIterator<I, P, K, V>
where
    I: LayerIterator<K, V>,
    P: for<'b> Fn(ItemRef<'b, K, V>) -> bool + Send + Sync,
{
    async fn new(iter: I, predicate: P) -> Result<Self, Error> {
        let mut filter = Self { iter, predicate, _key: PhantomData, _value: PhantomData };
        filter.skip_filtered().await?;
        Ok(filter)
    }

    async fn skip_filtered(&mut self) -> Result<(), Error> {
        loop {
            match self.iter.get() {
                Some(item) if !(self.predicate)(item) => {}
                _ => return Ok(()),
            }
            self.iter.advance().await?;
        }
    }
}

impl<I, P, K, V> LayerIterator<K, V> for FilterLayerIterator<I, P, K, V>
where
    I: LayerIterator<K, V>,
    P: for<'b> Fn(ItemRef<'b, K, V>) -> bool + Send + Sync,
    K: Send + Sync,
    V: Send + Sync,
{
    async fn advance(&mut self) -> Result<(), Error> {
        self.iter.advance().await?;
        self.skip_filtered().await
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        Ok(Some(Box::pin(self.advance())))
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        self.iter.get()
    }
}

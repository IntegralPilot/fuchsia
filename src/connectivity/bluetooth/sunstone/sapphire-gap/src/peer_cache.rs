// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::cell::RefCell;
use rand_core::Rng;
use sapphire_collections::storage::StorageFamily;
use sapphire_collections::vec::Vec;
use sapphire_common::{DeviceAddress, PeerId};

/// Represents a remote peer with a `PeerId` and `DeviceAddress`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    id: PeerId,
    address: DeviceAddress,
}

impl Peer {
    /// Creates a new `Peer`.
    pub const fn new(id: PeerId, address: DeviceAddress) -> Self {
        Self { id, address }
    }

    /// Returns the peer's unique ID.
    pub const fn id(&self) -> PeerId {
        self.id
    }

    /// Returns the peer's device address.
    pub const fn address(&self) -> DeviceAddress {
        self.address
    }

    /// Sets the peer's device address.
    pub fn set_address(&mut self, address: DeviceAddress) {
        self.address = address;
    }
}

struct PeerCacheInner<S: StorageFamily> {
    /// LRU peer is at index 0, MRU is at the back.
    // TODO(https://fxbug.dev/543866677): Consider using a better data structure for peers.
    peers: Vec<Peer, S>,
}

/// A cache of remote peers backed by an LRU storage container.
pub struct PeerCache<S: StorageFamily> {
    inner: RefCell<PeerCacheInner<S>>,
    max_capacity: usize,
}

impl<S: StorageFamily> PeerCache<S> {
    /// Creates a new `PeerCache` with the given maximum capacity.
    pub fn new(max_capacity: usize) -> Self
    where
        Vec<Peer, S>: Default,
    {
        Self { inner: RefCell::new(PeerCacheInner { peers: Vec::new() }), max_capacity }
    }

    /// Creates a new `PeerCache` using the given storage allocator and maximum capacity.
    pub fn new_in(alloc: S::Storage<Peer>, max_capacity: usize) -> Self {
        Self { inner: RefCell::new(PeerCacheInner { peers: Vec::new_in(alloc) }), max_capacity }
    }

    /// Helper method that performs find-or-insert and LRU promotion logic using dynamic dispatch
    /// to avoid monomorphization code bloat across callsites.
    fn find_or_insert_erased(
        &self,
        address: DeviceAddress,
        rng: &mut dyn Rng,
        f: &mut dyn FnMut(&mut Peer),
    ) -> bool {
        if self.max_capacity == 0 {
            return false;
        }

        let mut inner = self.inner.borrow_mut();

        // 1. If peer with address already exists, promote to MRU (back).
        let last_index =
            if let Some(index) = inner.peers.iter().position(|p| p.address() == address) {
                let last_index = inner.peers.len() - 1;
                if index < last_index {
                    let removed = inner.peers.remove(index);
                    inner.peers.try_push(removed).expect("space was just cleared by remove");
                }
                last_index
            } else {
                // 2. Otherwise generate a unique random ID and insert.
                let id = loop {
                    let val = rng.next_u64();
                    if let Some(candidate) = PeerId::new(val) {
                        if !inner.peers.iter().any(|p| p.id() == candidate) {
                            break candidate;
                        }
                    }
                };

                if inner.peers.len() >= self.max_capacity {
                    inner.peers.remove(0);
                }

                let peer = Peer::new(id, address);
                if inner.peers.try_push(peer).is_err() {
                    return false;
                }
                inner.peers.len() - 1
            };

        f(&mut inner.peers[last_index]);
        true
    }

    /// Finds the peer for `address` or inserts a new `Peer` if it does not exist, and executes
    /// `f` with a mutable reference to the `Peer`.
    ///
    /// The peer is moved to the MRU position before `f` is executed.
    /// Returns `None` if `max_capacity` is 0.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    #[inline]
    pub fn find_or_insert_with<R>(
        &self,
        address: DeviceAddress,
        rng: &mut impl Rng,
        f: impl FnOnce(&mut Peer) -> R,
    ) -> Option<R> {
        let mut result = None;
        let mut f = Some(f);
        let found = self.find_or_insert_erased(address, rng, &mut |peer| {
            if let Some(f) = f.take() {
                result = Some(f(peer));
            }
        });
        if found { result } else { None }
    }

    /// Finds the `PeerId` for `address` or inserts a new `Peer` if it does not exist.
    ///
    /// If a peer with `address` already exists, updates its LRU position (moves it to the back)
    /// and returns its `PeerId`.
    ///
    /// If it does not exist, generates a random `PeerId` using `rng`, evicts the least recently
    /// used peer (at index 0) if at maximum capacity, and inserts the new peer.
    ///
    /// Returns `None` if `max_capacity` is 0.
    #[inline]
    pub fn find_or_insert(&self, address: DeviceAddress, rng: &mut impl Rng) -> Option<PeerId> {
        self.find_or_insert_with(address, rng, |p| p.id())
    }

    /// Helper method to find a peer by a predicate and move it to the back (MRU position)
    /// using dynamic dispatch to prevent monomorphization code bloat.
    fn with_peer_mut_by_erased(
        &self,
        predicate: &mut dyn FnMut(&Peer) -> bool,
        f: &mut dyn FnMut(&mut Peer),
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(index) = inner.peers.iter().position(predicate) else {
            return false;
        };

        // Only move element to back if it isn't already the most recently used (MRU) element.
        let last_index = inner.peers.len() - 1;
        if index < last_index {
            let removed = inner.peers.remove(index);
            inner.peers.try_push(removed).expect("space was just cleared by remove");
        }

        f(&mut inner.peers[last_index]);
        true
    }

    /// Helper method to find a peer by a predicate, move it to the back (MRU position),
    /// and invoke `f` with a mutable reference to the `Peer`.
    #[inline]
    fn with_peer_mut_by<R>(
        &self,
        mut predicate: impl FnMut(&Peer) -> bool,
        f: impl FnOnce(&mut Peer) -> R,
    ) -> Option<R> {
        let mut result = None;
        let mut f = Some(f);
        let found = self.with_peer_mut_by_erased(&mut predicate, &mut |peer| {
            if let Some(f) = f.take() {
                result = Some(f(peer));
            }
        });
        if found { result } else { None }
    }

    /// Finds a peer by its `PeerId`, updates its LRU position (moves it to the back),
    /// and executes `f` with a mutable reference to the `Peer`.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    pub fn with_peer_mut<R>(&self, id: PeerId, f: impl FnOnce(&mut Peer) -> R) -> Option<R> {
        self.with_peer_mut_by(|p| p.id() == id, f)
    }

    /// Finds a peer by its `PeerId`, updates its LRU position (moves it to the back),
    /// and executes `f` with an immutable reference to the `Peer`.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    pub fn with_peer<R>(&self, id: PeerId, f: impl FnOnce(&Peer) -> R) -> Option<R> {
        self.with_peer_mut(id, |p| f(p))
    }

    /// Finds a peer by its `DeviceAddress`, updates its LRU position (moves it to the back),
    /// and executes `f` with a mutable reference to the `Peer`.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    pub fn with_peer_by_address_mut<R>(
        &self,
        address: DeviceAddress,
        f: impl FnOnce(&mut Peer) -> R,
    ) -> Option<R> {
        self.with_peer_mut_by(|p| p.address() == address, f)
    }

    /// Finds a peer by its `DeviceAddress`, updates its LRU position (moves it to the back),
    /// and executes `f` with an immutable reference to the `Peer`.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    pub fn with_peer_by_address<R>(
        &self,
        address: DeviceAddress,
        f: impl FnOnce(&Peer) -> R,
    ) -> Option<R> {
        self.with_peer_by_address_mut(address, |p| f(p))
    }

    /// Executes `f` on each peer in the cache, from least recently used to most recently used.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to mutably access this `PeerCache` instance re-entrantly.
    pub fn for_each(&self, mut f: impl FnMut(&Peer)) {
        let inner = self.inner.borrow();
        for peer in inner.peers.iter() {
            f(peer);
        }
    }

    /// Executes `f` with a mutable reference on each peer in the cache, from least recently
    /// used to most recently used.
    ///
    /// # Panics
    ///
    /// Panics if `f` attempts to access this `PeerCache` instance re-entrantly.
    pub fn for_each_mut(&self, mut f: impl FnMut(&mut Peer)) {
        let mut inner = self.inner.borrow_mut();
        for peer in inner.peers.iter_mut() {
            f(peer);
        }
    }

    /// Returns the number of peers currently in the cache.
    pub fn len(&self) -> usize {
        self.inner.borrow().peers.len()
    }

    /// Returns `true` if the cache contains no peers.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the maximum capacity of the cache.
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_collections::storage::ArrayStorage;
    use sapphire_common::AddressType;

    type TestPeerCache = PeerCache<ArrayStorage<10>>;

    struct TestRng(u64);

    impl rand_core::TryRng for TestRng {
        type Error = core::convert::Infallible;

        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok(self.try_next_u64()? as u32)
        }

        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            self.0 += 1;
            Ok(self.0)
        }

        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
            rand_core::utils::fill_bytes_via_next_word(dest, || self.try_next_u64())
        }
    }

    const ADDR1: DeviceAddress =
        DeviceAddress::new(AddressType::Public, [1, 2, 3, 4, 5, 6]).unwrap();
    const ADDR2: DeviceAddress =
        DeviceAddress::new(AddressType::RandomStatic, [2, 3, 4, 5, 6, 0b1100_0000]).unwrap();
    const ADDR3: DeviceAddress =
        DeviceAddress::new(AddressType::ResolvablePrivate, [3, 4, 5, 6, 7, 0b0100_0000]).unwrap();
    const ADDR4: DeviceAddress =
        DeviceAddress::new(AddressType::NonResolvablePrivate, [4, 5, 6, 7, 8, 0b0000_0000])
            .unwrap();

    #[test]
    fn test_device_address_getters() {
        assert_eq!(ADDR1.kind(), AddressType::Public);
        assert_eq!(ADDR1.bytes(), &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_peer_getters() {
        let id = PeerId::new(10).unwrap();
        let peer = Peer::new(id, ADDR1);
        assert_eq!(peer.id(), id);
        assert_eq!(peer.address(), ADDR1);
    }

    #[test]
    fn test_find_or_insert_and_lookups() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.max_capacity(), 5);

        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).expect("insert ADDR1");
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());

        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).expect("insert ADDR2");
        assert_eq!(cache.len(), 2);
        assert_ne!(peer1_id, peer2_id);

        // Find by ID
        let found_addr = cache.with_peer(peer1_id, |p| p.address()).expect("find peer1");
        assert_eq!(found_addr, ADDR1);

        // Find by Address
        let found_id = cache.with_peer_by_address(ADDR2, |p| p.id()).expect("find ADDR2");
        assert_eq!(found_id, peer2_id);

        // Non-existent lookups
        let unknown_id = PeerId::new(999).unwrap();
        assert!(cache.with_peer(unknown_id, |_| ()).is_none());
        assert!(cache.with_peer_by_address(ADDR3, |_| ()).is_none());
    }

    #[test]
    fn test_find_or_insert_existing_address() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).expect("insert ADDR1");
        assert_eq!(cache.len(), 1);

        // Calling find_or_insert with same address returns existing PeerId without creating a new peer
        let same_id = cache.find_or_insert(ADDR1, &mut rng).expect("find ADDR1");
        assert_eq!(same_id, peer1_id);
        assert_eq!(cache.len(), 1);

        // Look up by address still returns original peer ID
        assert_eq!(cache.with_peer_by_address(ADDR1, |p| p.id()), Some(peer1_id));
    }

    #[test]
    fn test_with_peer_mut_mutation() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        let peer_id = cache.find_or_insert(ADDR1, &mut rng).expect("insert ADDR1");

        // Mutate peer address using with_peer_mut
        cache
            .with_peer_mut(peer_id, |peer| {
                peer.set_address(ADDR2);
            })
            .expect("peer exists");

        // Verify mutation persisted in the cache
        let addr = cache.with_peer(peer_id, |peer| peer.address());
        assert_eq!(addr, Some(ADDR2));
    }

    #[test]
    fn test_with_peer_by_address_mut() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        let peer_id = cache.find_or_insert(ADDR1, &mut rng).expect("insert ADDR1");

        // Mutate peer address using with_peer_by_address_mut
        let res = cache.with_peer_by_address_mut(ADDR1, |peer| {
            peer.set_address(ADDR2);
            peer.id()
        });
        assert_eq!(res, Some(peer_id));

        // Verify mutation persisted and old address is no longer found
        assert!(cache.with_peer_by_address(ADDR1, |_| ()).is_none());
        assert_eq!(cache.with_peer_by_address(ADDR2, |p| p.id()), Some(peer_id));
    }

    #[test]
    fn test_lru_eviction() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(2);
        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).expect("insert ADDR1");
        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).expect("insert ADDR2");
        assert_eq!(cache.len(), 2);

        // Access peer1 via find_or_insert to make it MRU (most recently used) -> order: [peer2, peer1]
        assert_eq!(cache.find_or_insert(ADDR1, &mut rng), Some(peer1_id));

        // Insert peer3 when cache is full -> peer2 (LRU) should be evicted
        let peer3_id = cache.find_or_insert(ADDR3, &mut rng).expect("insert ADDR3");
        assert_eq!(cache.len(), 2);

        // peer2 should be gone
        assert!(cache.with_peer(peer2_id, |_| ()).is_none());
        assert!(cache.with_peer_by_address(ADDR2, |_| ()).is_none());

        // peer1 and peer3 should exist
        assert!(cache.with_peer(peer1_id, |_| ()).is_some());
        assert!(cache.with_peer(peer3_id, |_| ()).is_some());

        // Access peer3 by address to make it MRU -> order: [peer1, peer3]
        assert_eq!(cache.find_or_insert(ADDR3, &mut rng), Some(peer3_id));

        // Insert peer4 -> peer1 (LRU) should be evicted
        let peer4_id = cache.find_or_insert(ADDR4, &mut rng).expect("insert ADDR4");
        assert_eq!(cache.len(), 2);

        assert!(cache.with_peer(peer1_id, |_| ()).is_none());
        assert!(cache.with_peer(peer3_id, |_| ()).is_some());
        assert!(cache.with_peer(peer4_id, |_| ()).is_some());
    }

    #[test]
    fn test_lru_promotion_via_lookups() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(2);
        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).unwrap();

        // Access peer1 via with_peer -> promotes peer1 to MRU, making peer2 LRU
        assert!(cache.with_peer(peer1_id, |_| ()).is_some());

        // Insert peer3 -> evicts peer2 (LRU)
        let peer3_id = cache.find_or_insert(ADDR3, &mut rng).unwrap();
        assert!(cache.with_peer(peer1_id, |_| ()).is_some());
        assert!(cache.with_peer(peer2_id, |_| ()).is_none());
        assert!(cache.with_peer(peer3_id, |_| ()).is_some());

        // Access peer1 via with_peer_by_address -> promotes peer1 to MRU, making peer3 LRU
        assert!(cache.with_peer_by_address(ADDR1, |_| ()).is_some());

        // Insert peer4 -> evicts peer3 (LRU)
        let peer4_id = cache.find_or_insert(ADDR4, &mut rng).unwrap();
        assert!(cache.with_peer(peer1_id, |_| ()).is_some());
        assert!(cache.with_peer(peer3_id, |_| ()).is_none());
        assert!(cache.with_peer(peer4_id, |_| ()).is_some());
    }

    #[test]
    fn test_shared_reference_concurrency() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(3);
        let ref1 = &cache;
        let ref2 = &cache;

        let peer1_id = ref1.find_or_insert(ADDR1, &mut rng).unwrap();
        let peer2_id = ref2.find_or_insert(ADDR2, &mut rng).unwrap();

        assert_eq!(ref1.len(), 2);
        assert_eq!(ref2.with_peer(peer1_id, |p| p.address()), Some(ADDR1));
        assert_eq!(ref1.with_peer_by_address(ADDR2, |p| p.id()), Some(peer2_id));
    }

    #[test]
    fn test_zero_capacity_cache() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(0);
        assert_eq!(cache.max_capacity(), 0);
        assert_eq!(cache.len(), 0);
        assert!(cache.find_or_insert(ADDR1, &mut rng).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_single_capacity_cache() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(1);
        assert_eq!(cache.max_capacity(), 1);

        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.with_peer(peer1_id, |p| p.address()), Some(ADDR1));

        // Finding ADDR1 again keeps it in cache
        assert_eq!(cache.find_or_insert(ADDR1, &mut rng), Some(peer1_id));
        assert_eq!(cache.len(), 1);

        // Inserting ADDR2 evicts peer1 immediately
        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).unwrap();
        assert_eq!(cache.len(), 1);
        assert!(cache.with_peer(peer1_id, |_| ()).is_none());
        assert_eq!(cache.with_peer(peer2_id, |p| p.address()), Some(ADDR2));
    }

    #[test]
    fn test_custom_storage_family() {
        let mut rng = TestRng(0);
        let cache = PeerCache::<ArrayStorage<5>>::new(5);
        let peer_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.with_peer(peer_id, |p| p.address()), Some(ADDR1));
    }

    #[test]
    fn test_new_in() {
        let alloc = <ArrayStorage<5> as StorageFamily>::Storage::new();
        let cache = PeerCache::<ArrayStorage<5>>::new_in(alloc, 5);
        assert_eq!(cache.max_capacity(), 5);
        assert_eq!(cache.len(), 0);
        let mut rng = TestRng(0);
        let peer_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.with_peer(peer_id, |p| p.address()), Some(ADDR1));
    }

    #[test]
    fn test_for_each() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).unwrap();
        let peer3_id = cache.find_or_insert(ADDR3, &mut rng).unwrap();

        let mut visited_ids = [None; 3];
        let mut count = 0;
        cache.for_each(|peer| {
            visited_ids[count] = Some(peer.id());
            count += 1;
        });

        // Iteration order should match LRU -> MRU (peer1, peer2, peer3)
        assert_eq!(visited_ids, [Some(peer1_id), Some(peer2_id), Some(peer3_id)]);
    }

    #[test]
    fn test_for_each_mut() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);
        let peer1_id = cache.find_or_insert(ADDR1, &mut rng).unwrap();
        let peer2_id = cache.find_or_insert(ADDR2, &mut rng).unwrap();

        // Mutate each peer in place
        cache.for_each_mut(|peer| {
            if peer.id() == peer1_id {
                peer.set_address(ADDR3);
            } else if peer.id() == peer2_id {
                peer.set_address(ADDR4);
            }
        });

        // Verify mutations persisted
        assert_eq!(cache.with_peer(peer1_id, |p| p.address()), Some(ADDR3));
        assert_eq!(cache.with_peer(peer2_id, |p| p.address()), Some(ADDR4));
    }

    #[test]
    fn test_find_or_insert_with() {
        let mut rng = TestRng(0);
        let cache = TestPeerCache::new(5);

        let mut callback_invocations = 0;
        let mut captured_state = 42;

        // 1. Insert a new peer and immediately mutate it in the closure
        let peer1_id = cache
            .find_or_insert_with(ADDR1, &mut rng, |peer| {
                callback_invocations += 1;
                captured_state += 1;
                peer.set_address(ADDR2);
                peer.id()
            })
            .expect("inserted peer");

        assert_eq!(callback_invocations, 1);
        assert_eq!(captured_state, 43);
        assert_eq!(cache.len(), 1);

        // Verify mutation persisted
        assert_eq!(cache.with_peer(peer1_id, |p| p.address()), Some(ADDR2));

        // 2. Find the existing peer by its new address and mutate again
        let found_id = cache
            .find_or_insert_with(ADDR2, &mut rng, |peer| {
                callback_invocations += 1;
                peer.set_address(ADDR3);
                peer.id()
            })
            .expect("found existing peer");

        assert_eq!(found_id, peer1_id);
        assert_eq!(callback_invocations, 2);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.with_peer(peer1_id, |p| p.address()), Some(ADDR3));

        // 3. Zero-capacity cache returns None and does not invoke closure
        let zero_cache = TestPeerCache::new(0);
        let mut invoked = false;
        assert!(zero_cache.find_or_insert_with(ADDR1, &mut rng, |_| invoked = true).is_none());
        assert!(!invoked);
    }
}

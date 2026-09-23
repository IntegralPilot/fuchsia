// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::cache_stats::{AtomicCacheStats, CacheStats, ShardedCacheStats};
use crate::sync::RwLock;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use core::arch::asm;
use starnix_logging::CATEGORY_STARNIX_SECURITY;
use std::hint::cold_path;
use std::mem::size_of;
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering, compiler_fence};

/// Lock-free N-way associative cache with static capacity.
///
/// Each bucket is composed of `4*WAYS4` entries ("ways", up to 16) that are part of the same
/// eviction set.
///
/// Reads and writes are wait-free and protected by a non-traditional seqlock: reads ignore ways
/// that are currently being written to, and only need to spin if a write completes during the
/// read (this allows readers to avoid being blocked on a writer that isn't making progress).
/// Writers can also write concurrently to different ways of the same bucket - if all ways in the
/// bucket are busy, the result is returned without caching.
///
/// The cache provides both inline storage (so metadata, key and value can be stored in the same/
/// nearby cache lines) and out-of-line storage when entries are large. The storage is divided in
/// a user-controlled way into atomics of the given size. This allows user to get the best possible
/// packing, while reading as-large-as-possible atomics: the storage strategy gets access to
/// the given number of `INLINE_UXXS` atomic integers inline, and `OUT_OF_LINE_U64S` out-of-line
/// AtomicU64s.
pub struct LockFreeQueryCache<
    S: StorageStrategy<INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S, OUT_OF_LINE_U64S>
        + Send
        + Sync,
    const WAYS4: usize,
    const INLINE_U64S: usize,
    const INLINE_U32S: usize,
    const INLINE_U16S: usize,
    const INLINE_U8S: usize,
    const OUT_OF_LINE_U64S: usize,
> {
    /// Compact array of buckets.
    buckets: Box<[Bucket<WAYS4, INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S>]>,
    /// Out-of-line storage.
    out_of_line_data: Box<[[AtomicU64; OUT_OF_LINE_U64S]]>,
    /// Reset lock.
    reset_lock: RwLock<()>,
    /// The stats are sharded and padded to reduce cache line contention.
    stats: ShardedCacheStats,
    /// The storage strategy may contain state (this is useful in tests).
    storage: S,
}

/// Defines a strategy for storing `Self::Key` and `Self::Value` in the access cache.
///
/// The actual storage provided consists of `INLINE_U64S` atomic u64s, `INLINE_U32S` atomic u32s,
/// etc, and `OUT_OF_LINE_U64S` atomic u64s stored out-of-line. The Storage trait implementation is
/// responsible for encoding and decoding these.
///
/// The data will be protected by seqlock: hence, relaxed loads and stores are sufficient. However,
/// the implementation must be safe in the presence of arbitrary torn reads.
pub trait StorageStrategy<
    const INLINE_U64S: usize,
    const INLINE_U32S: usize,
    const INLINE_U16S: usize,
    const INLINE_U8S: usize,
    const OUT_OF_LINE_U64S: usize,
>
{
    /// The type to be used as cache key.
    type Key;
    /// The type of values to be stored in the cache.
    type Value;

    /// Computes a hash of `key`.
    fn hash_key(&self, key: &Self::Key) -> u64;

    /// Checks that the stored values match `key`.
    fn check_key(
        &self,
        key: &Self::Key,
        inline_u64s: &[std::sync::atomic::AtomicU64; INLINE_U64S],
        inline_u32s: &[std::sync::atomic::AtomicU32; INLINE_U32S],
        inline_u16s: &[std::sync::atomic::AtomicU16; INLINE_U16S],
        inline_u8s: &[std::sync::atomic::AtomicU8; INLINE_U8S],
        out_of_line_u64s: &[std::sync::atomic::AtomicU64; OUT_OF_LINE_U64S],
    ) -> bool;

    /// Reads the value from storage.
    fn read_value(
        &self,
        inline_u64s: &[std::sync::atomic::AtomicU64; INLINE_U64S],
        inline_u32s: &[std::sync::atomic::AtomicU32; INLINE_U32S],
        inline_u16s: &[std::sync::atomic::AtomicU16; INLINE_U16S],
        inline_u8s: &[std::sync::atomic::AtomicU8; INLINE_U8S],
        out_of_line_u64s: &[std::sync::atomic::AtomicU64; OUT_OF_LINE_U64S],
    ) -> Self::Value;

    /// Writes a key/value pair to storage.
    fn write_key_value(
        &self,
        key: &Self::Key,
        value: &Self::Value,
        inline_u64s: &[std::sync::atomic::AtomicU64; INLINE_U64S],
        inline_u32s: &[std::sync::atomic::AtomicU32; INLINE_U32S],
        inline_u16s: &[std::sync::atomic::AtomicU16; INLINE_U16S],
        inline_u8s: &[std::sync::atomic::AtomicU8; INLINE_U8S],
        out_of_line_u64s: &[std::sync::atomic::AtomicU64; OUT_OF_LINE_U64S],
    );
}

/// A bucket in the `LockFreeQueryCache`, with 4*WAYS4 associativity, and each entry storing
/// inline data in the form of `INLINE_U64S` u64s, `INLINE_U32S` u32s, `INLINE_U16S` u16s, and
/// `INLINE_U8S` u8s.
#[repr(align(64))]
struct Bucket<
    const WAYS4: usize,
    const INLINE_U64S: usize,
    const INLINE_U32S: usize,
    const INLINE_U16S: usize,
    const INLINE_U8S: usize,
> {
    /// The seqlock state of this bucket.
    /// Bits 0..15: write_mask (1 if the way is currently being written to).
    /// Bits 16..63: seqlock counter.
    /// Writes always increase this, either by setting a bit of the write mask before writing, or
    /// by clearing a bit and incrementing the seqlock once a write is done. A read is valid if the
    /// way is unmasked both before and after, and the seqlock counter hasn't changed between the two
    /// reads.
    seqlock_state: AtomicU64,
    /// Clock eviction algorithm state.
    clock: ClockState,
    /// 8-bit hash tags for quick matching. A "0" tag means the entry is empty.
    tags: [AtomicU32; WAYS4],
    /// Inline data.
    inline_u8s: [[[AtomicU8; INLINE_U8S]; 4]; WAYS4],
    inline_u16s: [[[AtomicU16; INLINE_U16S]; 4]; WAYS4],
    inline_u32s: [[[AtomicU32; INLINE_U32S]; 4]; WAYS4],
    inline_u64s: [[[AtomicU64; INLINE_U64S]; 4]; WAYS4],
}

/// Eviction state for a up-to-16-way cache, using a CLOCK eviction strategy.
///
/// When evicting, we look at the entry pointed by the clock hand. If the entry is cold, it is
/// evicted and the hand moves to the next entry. If the entry is hot, we mark it as cold and
/// move to the next entry.
/// Accessed entries are marked as hot if not already hot. On insertion, entries are left cold,
/// but are far away from the hand: this ensures that rarely used entries do not pollute the cache.
pub struct ClockState {
    /// The CLOCK state, encoded as a 32-bit integer to allow a direct atomic load.
    /// - Bits 0..ways : "hot" bits for ways 0 to ways.
    /// - Bits 16..31: Clock hand pointer (values 0 to ways - 1).
    state: AtomicU32,
}

impl ClockState {
    /// Initializes a new clock state.
    pub const fn new() -> Self {
        Self { state: AtomicU32::new(0) }
    }

    /// Resets the clock to the initial state.
    pub fn reset(&self) {
        self.state.store(0, Ordering::Relaxed);
    }

    /// Records an access to the given way.
    #[inline(always)]
    pub fn record_access(&self, way_index: usize) {
        assert!(way_index < 16, "way_index must be less than 16");

        let way_bit = 1 << (way_index as u32);
        let old_state = self.state.load(Ordering::Relaxed);

        // If the entry is marked as "hot", return immediately.
        if (old_state & way_bit) != 0 {
            return;
        }
        cold_path();

        let new_state = old_state | way_bit;
        // If the compare-exchange fails, it means there's contention on the cache line, or
        // the CPU experienced an interrupt.
        // Avoid retrying: this limits cache line thrashing, at the cost of slightly less
        // precise eviction.
        let _ = self.state.compare_exchange_weak(
            old_state,
            new_state,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// Finds the next entry to be evicted.
    ///
    /// `write_mask` is the mask of ways that are currently being written to and cannot be evicted.
    /// `ways` is the number of ways in the bucket.
    ///
    /// Returns `None` in case of contention: the caller should reload the write mask as it is
    /// likely to have changed.
    #[inline(always)]
    pub fn find_eviction(&self, write_mask: u16, ways: usize) -> Option<usize> {
        assert!(ways <= 16, "ways must be less than or equal to 16");

        if write_mask as u32 & ((1u32 << ways) - 1) == (1u32 << ways) - 1 {
            // All ways are masked - we shouldn't have been called.
            return None;
        }
        let old_state = self.state.load(Ordering::Relaxed);

        let mut hotness = (old_state & 0xFFFF) as u16;
        let mut hand = (old_state >> 16) as usize;
        let evict_way;

        loop {
            let hand_mask = 1 << hand;
            if (write_mask & hand_mask) == 0 {
                if (hotness & hand_mask) == 0 {
                    // Current slot is cold, evict it.
                    evict_way = hand;
                    // Advance the hand for the next eviction.
                    hand = (hand + 1) % ways;
                    break;
                }
                // Current slot is hot, mark it as cold and advance the hand.
                hotness &= !hand_mask;
            }
            hand = (hand + 1) % ways;
        }

        // We do not mark the newly inserted entry as "hot" and instead wait for a second
        // access to confirm that the entry is frequently accessed.
        let new_state = ((hand as u32) << 16) | (hotness as u32);

        // Try committing the state.
        match self.state.compare_exchange_weak(
            old_state,
            new_state,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => Some(evict_way as usize),
            Err(_) => None,
        }
    }
}

impl<
    S: StorageStrategy<INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S, OUT_OF_LINE_U64S>
        + Send
        + Sync
        + Default,
    const WAYS4: usize,
    const INLINE_U64S: usize,
    const INLINE_U32S: usize,
    const INLINE_U16S: usize,
    const INLINE_U8S: usize,
    const OUT_OF_LINE_U64S: usize,
>
    LockFreeQueryCache<
        S,
        WAYS4,
        INLINE_U64S,
        INLINE_U32S,
        INLINE_U16S,
        INLINE_U8S,
        OUT_OF_LINE_U64S,
    >
{
    pub fn new(capacity: usize) -> Self {
        Self::new_with_storage(S::default(), capacity)
    }
}

impl<
    S: StorageStrategy<INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S, OUT_OF_LINE_U64S>
        + Send
        + Sync,
    const WAYS4: usize,
    const INLINE_U64S: usize,
    const INLINE_U32S: usize,
    const INLINE_U16S: usize,
    const INLINE_U8S: usize,
    const OUT_OF_LINE_U64S: usize,
>
    LockFreeQueryCache<
        S,
        WAYS4,
        INLINE_U64S,
        INLINE_U32S,
        INLINE_U16S,
        INLINE_U8S,
        OUT_OF_LINE_U64S,
    >
{
    /// Creates an access cache of capacity at least `capacity`.
    pub fn new_with_storage(storage: S, capacity: usize) -> Self {
        assert!(WAYS4 <= 4, "ways must be less than or equal to 16");
        assert!(WAYS4 > 0, "ways must be non-zero");

        let required_buckets = capacity.div_ceil(WAYS4 * 4);
        let num_buckets = required_buckets.next_power_of_two();
        let total_capacity = num_buckets * WAYS4 * 4;

        let mut buckets = Vec::with_capacity(num_buckets);
        for _ in 0..num_buckets {
            buckets.push(Bucket {
                seqlock_state: AtomicU64::new(0),
                clock: ClockState::new(),
                tags: std::array::from_fn(|_| AtomicU32::new(0)),
                inline_u8s: std::array::from_fn(|_| {
                    std::array::from_fn(|_| std::array::from_fn(|_| AtomicU8::new(0)))
                }),
                inline_u16s: std::array::from_fn(|_| {
                    std::array::from_fn(|_| std::array::from_fn(|_| AtomicU16::new(0)))
                }),
                inline_u32s: std::array::from_fn(|_| {
                    std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                }),
                inline_u64s: std::array::from_fn(|_| {
                    std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                }),
            });
        }

        let mut outline_data = Vec::with_capacity(total_capacity);
        for _ in 0..total_capacity {
            outline_data.push(std::array::from_fn(|_| AtomicU64::new(0)));
        }

        Self {
            buckets: buckets.into_boxed_slice(),
            out_of_line_data: outline_data.into_boxed_slice(),
            stats: ShardedCacheStats::new(),
            reset_lock: RwLock::new(()),
            storage,
        }
    }

    /// Returns the total capacity with which this instance was initialized.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.buckets.len() * WAYS4 * 4
    }

    /// Searches the cache and returns a result `R` matching the given `query_args`.
    ///
    /// If the result is not found in the cache, the `callback` is invoked to
    /// compute the result, which is then inserted into the cache.
    #[inline]
    pub fn get_or_insert(&self, key: &S::Key, callback: impl FnOnce() -> S::Value) -> S::Value {
        self.get_or_try_insert::<()>(key, || Ok(callback())).expect("infallible callback")
    }

    /// Searches the cache and returns a result `R` matching the given `query_args`.
    ///
    /// If the result is not found in the cache, the `callback` is invoked to
    /// compute the result, which is then inserted into the cache.
    ///
    /// # Errors
    ///
    /// If the `callback` returns an error, it is propagated to the caller.
    #[inline]
    pub fn get_or_try_insert<E>(
        &self,
        key: &S::Key,
        callback: impl FnOnce() -> Result<S::Value, E>,
    ) -> Result<S::Value, E> {
        // Prefetch the stats shard for the current thread. We know it will be accessed by the
        // time we return.
        let stats = self.stats.shard();
        // SAFETY: we're prefetching a valid AtomicCacheStats address.
        unsafe {
            prefetch_write_data(stats as *const AtomicCacheStats as *const u8);
        }

        let hash = self.storage.hash_key(key);

        let num_buckets = self.buckets.len();
        let bucket_idx = (hash as usize) & (num_buckets - 1); // num_buckets is a power of 2
        let bucket = &self.buckets[bucket_idx];
        // Prefetch the full bucket (up to 128 bytes) - we will access it right after tag matching.
        // This ensures that, when we have two cache lines, both get loaded at once instead of
        // sequentially.
        // SAFETY: we're prefetching a valid address containing bucket data.
        unsafe {
            prefetch_read_data(
                bucket as *const Bucket<WAYS4, INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S>
                    as *const u8,
            );
        }
        if size_of::<Bucket<WAYS4, INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S>>() > 64 {
            // SAFETY: this address is within the Bucket structure.
            unsafe {
                prefetch_read_data(
                    (bucket
                        as *const Bucket<WAYS4, INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S>
                        as *const u8)
                        .wrapping_add(64),
                );
            }
        }

        // A zero tag is used to mark an un-filled entry.
        let expected_tag = std::cmp::max(1, (hash >> 56) as u8);

        // Seqlock spin loop for reading. If we spin too many times we should probably
        // give up and re-compute the result.
        const MAX_READ_SPINS: usize = 3;
        for _ in 0..MAX_READ_SPINS {
            // Acquire the seqlock. Subsequent reads can be relaxed (although they may read garbage).
            let state1 = bucket.seqlock_state.load(Ordering::Acquire);
            let write_mask = (state1 & 0xFFFF) as u16;

            let mut found = None;

            let tag = expected_tag as u32;
            'search: for way4 in 0..WAYS4 {
                let mut tag_cluster = bucket.tags[way4].load(Ordering::Relaxed);

                'ways: for sub_way in 0..4 {
                    let cur_tag = tag_cluster & 0xFF;
                    tag_cluster = tag_cluster >> 8;

                    if cur_tag != tag {
                        // Hint to the compiler that the last tag will probably match. This helps if the loop is unrolled.
                        if way4 == WAYS4 - 1 && sub_way == 3 {
                            cold_path();
                        }
                        continue 'ways;
                    }
                    if way4 < WAYS4 - 1 || sub_way != 3 {
                        // Before the last tag, it's unlikely the tag will match.
                        cold_path();
                    }

                    let way = way4 * 4 + sub_way;
                    if (write_mask & 1 << way) != 0 {
                        // The way is being written to, skip it.
                        cold_path();
                        continue 'ways;
                    }

                    if !self.storage.check_key(
                        key,
                        &bucket.inline_u64s[way4][sub_way],
                        &bucket.inline_u32s[way4][sub_way],
                        &bucket.inline_u16s[way4][sub_way],
                        &bucket.inline_u8s[way4][sub_way],
                        &self.out_of_line_data[bucket_idx * WAYS4 * 4 + way],
                    ) {
                        cold_path();
                        continue 'ways;
                    }
                    found = Some((
                        self.storage.read_value(
                            &bucket.inline_u64s[way4][sub_way],
                            &bucket.inline_u32s[way4][sub_way],
                            &bucket.inline_u16s[way4][sub_way],
                            &bucket.inline_u8s[way4][sub_way],
                            &self.out_of_line_data[bucket_idx * WAYS4 * 4 + way],
                        ),
                        way,
                    ));

                    break 'search;
                }
            }

            if found.is_none() {
                cold_path();
                break;
            }
            let (value, way) = found.unwrap();
            // A compiler fence ensures preceding reads aren't delayed, and an Acquire
            // memory barrier ensures data reads are not reordered with the state validation.
            compiler_fence(Ordering::SeqCst);
            std::sync::atomic::fence(Ordering::Acquire);

            let state2 = bucket.seqlock_state.load(Ordering::Acquire);
            // Our read is valid if the seqlock counter wasn't incremented, and the way we read is
            // not being written to.
            let way_mask = !(0xFFFF & !(1 << way));
            if (state1 & way_mask) == (state2 & way_mask) {
                // Record the hit - hopefully we will still hold the cache line since our Acquire load.
                bucket.clock.record_access(way);
                stats.hits.fetch_add(1, Ordering::Relaxed);
                stats.lookups.fetch_add(1, Ordering::Relaxed);
                return Ok(value);
            } else {
                // We're unlikely to need to loop - we have 99% cache hits, and concurrent misses
                // are unlikely to be in our bucket.
                cold_path();
            }
        }

        // We expect >99% cache hits.
        cold_path();

        fuchsia_trace::duration!(CATEGORY_STARNIX_SECURITY, "selinux.access_cache.miss");
        stats.lookups.fetch_add(1, Ordering::Relaxed);
        stats.misses.fetch_add(1, Ordering::Relaxed);

        // When writing we need to acquire the reset lock for reading - otherwise we may write our
        // pre-reset result post-reset.
        let _guard = self.reset_lock.read();

        let result = callback()?;

        let mut way_to_write = None;
        const MAX_WRITE_SPINS: usize = 10;
        'select_victim: for _ in 0..MAX_WRITE_SPINS {
            let mut state = bucket.seqlock_state.load(Ordering::Acquire);
            let write_mask = (state & 0xFFFF) as u16;

            if write_mask == ((1_usize << (WAYS4 * 4)) - 1) as u16 {
                // All slots are currently being written to. It would be unreasonable to spin to
                // evict a just-inserted entry so we can just early-return.
                return Ok(result);
            }

            let Some(victim) = bucket.clock.find_eviction(write_mask, WAYS4 * 4) else {
                // The clock state has been modified under us. In half of the case, it's because a
                // slot was selected by someone else. Loop to reload the write mask.
                continue 'select_victim;
            };
            'write_flag: loop {
                // Set the write_mask bit for the targeted way.
                let new_state = state | (1 << victim);
                match bucket.seqlock_state.compare_exchange_weak(
                    state,
                    new_state,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        way_to_write = Some(victim);
                        break 'select_victim;
                    }
                    Err(changed_state) => {
                        state = changed_state;
                        if state & (1 << victim) == 0 {
                            // Another write flag was changed. Retry setting our flag.
                            continue 'write_flag;
                        } else {
                            // Our slot was stolen! Re-select the victim.
                            continue 'select_victim;
                        }
                    }
                }
            }
        }

        let Some(way_to_write) = way_to_write else {
            // We failed to find a victim after MAX_WRITE_SPINS iterations. Return the result
            // without writing.
            return Ok(result);
        };

        // Perform the write now we have the exclusive lock for this way.
        let tag_word_idx = way_to_write / 4;
        let tag_byte_idx = way_to_write % 4;
        let shift = tag_byte_idx * 8;
        let is_filled = (bucket.tags[tag_word_idx].load(Ordering::Relaxed) >> shift) & 0xFF != 0;
        if is_filled {
            stats.reclaims.fetch_add(1, Ordering::Relaxed);
        }
        stats.allocs.fetch_add(1, Ordering::Relaxed);

        let way4 = way_to_write / 4;
        let sub_way = way_to_write % 4;
        self.storage.write_key_value(
            key,
            &result,
            &bucket.inline_u64s[way4][sub_way],
            &bucket.inline_u32s[way4][sub_way],
            &bucket.inline_u16s[way4][sub_way],
            &bucket.inline_u8s[way4][sub_way],
            &self.out_of_line_data[bucket_idx * WAYS4 * 4 + way_to_write],
        );

        // Write the new tag
        let mut old_tag_cluster = bucket.tags[tag_word_idx].load(Ordering::Relaxed);
        loop {
            let new_tag_cluster =
                (old_tag_cluster & !(0xFF << shift)) | ((expected_tag as u32) << shift);
            match bucket.tags[tag_word_idx].compare_exchange_weak(
                old_tag_cluster,
                new_tag_cluster,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(real) => old_tag_cluster = real,
            }
        }

        // Unlock and increment the seqlock. We unset the bit and increment the counter at once.
        bucket.seqlock_state.fetch_add((1 << 16) - (1 << way_to_write) as u64, Ordering::Release);
        Ok(result)
    }

    #[cfg(test)]
    pub fn is_full(&self) -> bool {
        let mut count = 0;
        for bucket in self.buckets.iter() {
            for way4 in 0..WAYS4 {
                let tags = bucket.tags[way4].load(Ordering::Relaxed);
                for i in 0..4 {
                    if (tags >> (i * 8)) & 0xFF != 0 {
                        count += 1;
                    }
                }
            }
        }
        count >= self.capacity()
    }

    #[cfg(test)]
    pub fn bucket_size() -> usize {
        size_of::<Bucket<WAYS4, INLINE_U64S, INLINE_U32S, INLINE_U16S, INLINE_U8S>>()
    }

    /// Resets the cache.
    pub fn reset(&self) {
        // Exclude concurrent writers.
        let _guard = self.reset_lock.write();

        // We only have to invalidate the tags:
        //  - there are no current writers.
        //  - this does not invalidate the data, so we do not need to touch the seqlock.
        // We also reset the clock.
        for bucket in self.buckets.iter() {
            bucket.clock.reset();
            for way4 in 0..WAYS4 {
                bucket.tags[way4].store(0, Ordering::Relaxed);
            }
        }

        self.stats.reset();
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.stats.read()
    }
}

/// Prefetches data into the L1 cache in anticipation of a read. Safe if the pointer points to valid memory.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn prefetch_read_data(ptr: *const u8) {
    // SAFETY: ok if ptr points to valid memory.
    unsafe {
        asm!(
            "prefetcht0 [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

#[inline(always)]
#[cfg(target_arch = "aarch64")]
unsafe fn prefetch_read_data(ptr: *const u8) {
    // SAFETY: ok if ptr points to valid memory.
    unsafe {
        asm!(
            "prfm pldl1keep, [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline(always)]
unsafe fn prefetch_read_data(_ptr: *const u8) {}

/// Prefetches data into the L1 cache in anticipation of a write. Safe if the pointer points to valid memory.
#[inline(always)]
#[cfg(target_arch = "x86_64")]
unsafe fn prefetch_write_data(ptr: *const u8) {
    // SAFETY: ok if ptr points to valid memory.
    unsafe {
        asm!(
            "prefetchw [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn prefetch_write_data(ptr: *const u8) {
    // SAFETY: ok if ptr points to valid memory.
    unsafe {
        asm!(
            "prfm pstl1keep, [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline(always)]
unsafe fn prefetch_write_data(_ptr: *const u8) {}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_sync::{Condvar, Mutex};
    use std::sync::Arc;

    /// A storage implementation that hashes all keys to 0.
    #[derive(Default)]
    struct ZeroHashStorage;

    impl StorageStrategy<0, 2, 0, 0, 0> for ZeroHashStorage {
        type Key = u32;
        type Value = u32;

        fn hash_key(&self, _key: &Self::Key) -> u64 {
            0
        }

        fn check_key(
            &self,
            key: &Self::Key,
            _inline_u64s: &[AtomicU64; 0],
            inline_u32s: &[AtomicU32; 2],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) -> bool {
            inline_u32s[0].load(Ordering::Relaxed) == *key
        }

        fn read_value(
            &self,
            _inline_u64s: &[AtomicU64; 0],
            inline_u32s: &[AtomicU32; 2],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) -> Self::Value {
            inline_u32s[1].load(Ordering::Relaxed)
        }

        fn write_key_value(
            &self,
            key: &Self::Key,
            value: &Self::Value,
            _inline_u64s: &[AtomicU64; 0],
            inline_u32s: &[AtomicU32; 2],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) {
            inline_u32s[0].store(*key, Ordering::Relaxed);
            inline_u32s[1].store(*value, Ordering::Relaxed);
        }
    }

    type ZeroHashCache = LockFreeQueryCache<ZeroHashStorage, 1, 0, 2, 0, 0, 0>;

    #[test]
    fn test_basic_insertion_query() {
        let cache = ZeroHashCache::new(0); // 1 bucket, 1 way cluster (4 ways total)

        let val = cache.get_or_insert(&1, || 100);
        assert_eq!(val, 100);

        let val = cache.get_or_insert(&1, || 200); // Should be a hit
        assert_eq!(val, 100); // Still 100

        let val = cache.get_or_insert(&2, || 200);
        assert_eq!(val, 200);
    }

    #[test]
    fn test_bucket_capacity() {
        loop {
            let cache = ZeroHashCache::new(0);

            // Insert 4 elements. They should all fit into a single bucket.
            // Initial
            cache.get_or_insert(&1, || 10);
            cache.get_or_insert(&2, || 20);
            cache.get_or_insert(&3, || 30);
            cache.get_or_insert(&4, || 40);

            // Verify all fit. This also marks the elements as hot.
            assert_eq!(cache.get_or_insert(&1, || unreachable!()), 10);
            assert_eq!(cache.get_or_insert(&2, || unreachable!()), 20);
            assert_eq!(cache.get_or_insert(&3, || unreachable!()), 30);
            assert_eq!(cache.get_or_insert(&4, || unreachable!()), 40);

            // Insert 5th element. Element 1 should be evicted.
            cache.get_or_insert(&5, || 50);

            // If a compare_exchange_weak failed in `record_access`, the eviction target will be wrong
            // and Element 1 will still be there.
            // If so, retry the test from the beginning.
            if cache.get_or_try_insert::<()>(&1, || Err(())).is_ok() {
                continue;
            }

            assert_eq!(cache.get_or_insert(&2, || unreachable!()), 20);
            assert_eq!(cache.get_or_insert(&3, || unreachable!()), 30);
            assert_eq!(cache.get_or_insert(&4, || unreachable!()), 40);
            assert_eq!(cache.get_or_insert(&5, || unreachable!()), 50);
            assert_eq!(cache.get_or_insert(&1, || 0), 0);

            // Check cache statistics: 5 insertions, 1 eviction
            let stats = cache.cache_stats();
            assert_eq!(stats.lookups, 15);
            assert_eq!(stats.hits, 8);
            assert_eq!(stats.misses, 7);
            assert_eq!(stats.allocs, 6);
            assert_eq!(stats.reclaims, 2);
            break;
        }
    }

    #[test]
    fn test_eviction_lru_hot_bit() {
        loop {
            let cache = ZeroHashCache::new(0);

            // Insert 4 elements
            cache.get_or_insert(&1, || 10);
            cache.get_or_insert(&2, || 20);
            cache.get_or_insert(&3, || 30);
            cache.get_or_insert(&4, || 40);

            // Hit 1, 2, 3 again to make them "hot"
            cache.get_or_insert(&1, || unreachable!());
            cache.get_or_insert(&2, || unreachable!());
            cache.get_or_insert(&3, || unreachable!());

            // Insert a new element.
            cache.get_or_insert(&5, || 50);

            // 1, 2, 3 are still there, 4 is evicted.
            // If a compare_exchange_weak failed in `record_access`, the eviction target will be wrong,
            // and Element 4 will still be there.
            // If so, retry the test from the beginning.
            if cache.get_or_try_insert::<()>(&4, || Err(())).is_ok() {
                continue;
            }

            // Verify the rest of the expected state.
            assert_eq!(cache.get_or_insert(&1, || unreachable!()), 10);
            assert_eq!(cache.get_or_insert(&2, || unreachable!()), 20);
            assert_eq!(cache.get_or_insert(&3, || unreachable!()), 30);
            assert_eq!(cache.get_or_insert(&4, || 0), 0);
            break;
        }
    }

    #[test]
    fn test_heavy_contention() {
        let cache = ZeroHashCache::new(0);
        let cache = Arc::new(cache);

        let mut threads = vec![];
        for i in 0..16 {
            let cache_clone = cache.clone();
            threads.push(std::thread::spawn(move || {
                for j in 0..100 {
                    let key = (i * 100 + j) % 10; // Share keys to force contention!
                    assert_eq!(cache_clone.get_or_insert(&key, || key * 10), key * 10);
                }
            }));
        }

        for t in threads {
            t.join().unwrap();
        }
    }

    #[derive(Default)]
    struct BlockingStorageState {
        // List of key-value pairs for writes that are blocked.
        blocked_writes: Vec<(u64, u64)>,
        // List of keys for reads that are blocked.
        blocked_reads: Vec<u64>,
    }

    /// A storage implementation that hashes all keys to 0, and blocks on reads and writes.
    struct BlockingStorage {
        state: Mutex<BlockingStorageState>,
        condvar: Condvar,
    }

    impl Default for BlockingStorage {
        fn default() -> Self {
            Self { state: Mutex::new(BlockingStorageState::default()), condvar: Condvar::new() }
        }
    }

    impl StorageStrategy<2, 0, 0, 0, 0> for BlockingStorage {
        type Key = u64;
        type Value = u64;

        fn hash_key(&self, _key: &Self::Key) -> u64 {
            0
        }

        fn check_key(
            &self,
            key: &Self::Key,
            inline_u64s: &[AtomicU64; 2],
            _inline_u32s: &[AtomicU32; 0],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) -> bool {
            *key == inline_u64s[0].load(Ordering::Relaxed)
        }

        fn read_value(
            &self,
            inline_u64s: &[AtomicU64; 2],
            _inline_u32s: &[AtomicU32; 0],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) -> Self::Value {
            let key = inline_u64s[0].load(Ordering::Relaxed);
            let mut state = self.state.lock();
            state.blocked_reads.push(key);
            self.condvar.notify_all();
            self.condvar.wait_while(&mut state, |state| state.blocked_reads.contains(&key));
            inline_u64s[1].load(Ordering::Relaxed)
        }

        fn write_key_value(
            &self,
            key: &Self::Key,
            value: &Self::Value,
            inline_u64s: &[AtomicU64; 2],
            _inline_u32s: &[AtomicU32; 0],
            _inline_u16s: &[AtomicU16; 0],
            _inline_u8s: &[AtomicU8; 0],
            _out_of_line_u64s: &[AtomicU64; 0],
        ) {
            let mut state = self.state.lock();
            state.blocked_writes.push((*key, *value));
            self.condvar.notify_all();
            self.condvar
                .wait_while(&mut state, |state| state.blocked_writes.contains(&(*key, *value)));
            inline_u64s[0].store(*key, Ordering::Relaxed);
            inline_u64s[1].store(*value, Ordering::Relaxed)
        }
    }

    type CacheWithBlockingStorage = LockFreeQueryCache<BlockingStorage, 1, 2, 0, 0, 0, 0>;

    #[test]
    fn test_skip_cache_when_all_ways_writing() {
        let cache = Arc::new(CacheWithBlockingStorage::new(0));

        // Spawn 4 writers thread. They will block in "write".
        let mut threads = vec![];
        for i in 0..4 {
            let cache = cache.clone();
            threads.push(std::thread::spawn(move || {
                cache.get_or_insert(&i, || i * 10);
            }));
        }

        // Wait until all writers are blocked in the write phase.
        {
            let mut state = cache.storage.state.lock();
            cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 4);
        }

        // All ways are being written to. Readers will call the backend, and their result will
        // not be cached.
        assert_eq!(cache.get_or_insert(&4, || 40), 40);
        // The result is not cached, so we get a different value.
        assert_eq!(cache.get_or_insert(&4, || 50), 50);

        // Unblock writers.
        cache.storage.state.lock().blocked_writes.clear();
        cache.storage.condvar.notify_all();
        for t in threads {
            t.join().unwrap();
        }
    }

    /// Tests that writes starting during a read invalidate the read.
    #[test]
    fn test_read_retries_on_concurrent_write_start() {
        let cache = Arc::new(CacheWithBlockingStorage::new(0));

        // Start 3 writes to make 3 ways busy.
        let mut busy_threads = vec![];
        for i in 1..4 {
            let cache_clone = cache.clone();
            busy_threads.push(std::thread::spawn(move || {
                cache_clone.get_or_insert(&i, || i * 10);
            }));
        }
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 3);
        drop(state);

        // Insert key 0. It must go to the single free way.
        let cache_clone = cache.clone();
        let t = std::thread::spawn(move || cache_clone.get_or_insert(&0, || 100));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 4);
        state.blocked_writes.retain(|&(k, _)| k != 0);
        cache.storage.condvar.notify_all();
        drop(state);
        t.join().unwrap();

        // Start a read for 0. It will hit and block in read_value.
        let cache_clone = cache.clone();
        let read_thread = std::thread::spawn(move || cache_clone.get_or_insert(&0, || 0));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| !state.blocked_reads.contains(&0));
        drop(state);

        // Start a write for 4. It must pick the way holding key 0, as others are being written to.
        let cache_clone = cache.clone();
        let writer_4_thread = std::thread::spawn(move || cache_clone.get_or_insert(&4, || 40));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 4);

        // Unblock the reader. It should fail and retry because the way is being written to.
        state.blocked_reads.retain(|&x| x != 0);
        cache.storage.condvar.notify_all();
        drop(state);
        let res = read_thread.join().unwrap();
        assert_eq!(res, 0, "Reader should miss and return 0 (callback result)");

        // Unblock all writers (so writer 4 finishes).
        let mut state = cache.storage.state.lock();
        state.blocked_writes.clear();
        cache.storage.condvar.notify_all();
        drop(state);

        writer_4_thread.join().unwrap();
        for t in busy_threads {
            t.join().unwrap();
        }
    }

    /// Tests that write fully completing during a read invalidate the read.
    #[test]
    fn test_read_retries_on_concurrent_write_complete() {
        let cache = Arc::new(CacheWithBlockingStorage::new(0));

        // Start 3 writes to make 3 ways busy.
        let mut busy_threads = vec![];
        for i in 1..4 {
            let cache_clone = cache.clone();
            busy_threads.push(std::thread::spawn(move || {
                cache_clone.get_or_insert(&i, || i * 10);
            }));
        }
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 3);
        drop(state);

        // Insert key 0, which will go to the only free way.
        let cache_clone = cache.clone();
        let t = std::thread::spawn(move || cache_clone.get_or_insert(&0, || 100));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 4);
        state.blocked_writes.retain(|&(k, _)| k != 0);
        cache.storage.condvar.notify_all();
        drop(state);
        t.join().unwrap();

        // Start a read for key 0.
        let cache_clone = cache.clone();
        let read_thread = std::thread::spawn(move || cache_clone.get_or_insert(&0, || 0));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| !state.blocked_reads.contains(&0));
        drop(state);

        // Start a write. This will necessarily write the last free way, which is occupied by key 0.
        let cache_clone = cache.clone();
        let writer_4_thread = std::thread::spawn(move || cache_clone.get_or_insert(&4, || 40));
        let mut state = cache.storage.state.lock();
        cache.storage.condvar.wait_while(&mut state, |state| state.blocked_writes.len() < 4);
        drop(state);

        // Let the write fully complete.
        let mut state = cache.storage.state.lock();
        state.blocked_writes.retain(|&(k, _)| k != 4);
        cache.storage.condvar.notify_all();
        drop(state);
        writer_4_thread.join().unwrap();

        // Unblock the read. It will fail the seqlock check and retry.
        let mut state = cache.storage.state.lock();
        state.blocked_reads.retain(|&x| x != 0);
        cache.storage.condvar.notify_all();
        drop(state);

        // The retry misses because the way was overwritten with key 4. It will block on a write.
        let mut state = cache.storage.state.lock();
        cache
            .storage
            .condvar
            .wait_while(&mut state, |state| !state.blocked_writes.iter().any(|&(k, _)| k == 0));
        state.blocked_writes.retain(|&(k, _)| k != 0);
        cache.storage.condvar.notify_all();
        drop(state);
        assert_eq!(read_thread.join().unwrap(), 0);

        // Unblock other writers.
        let mut state = cache.storage.state.lock();
        state.blocked_writes.clear();
        cache.storage.condvar.notify_all();
        drop(state);
        for t in busy_threads {
            t.join().unwrap();
        }
    }
}

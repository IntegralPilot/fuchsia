// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crossbeam_utils::CachePadded;
use std::sync::atomic::{AtomicU64, Ordering};

/// Describes the performance statistics of a cache implementation.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct CacheStats {
    /// Cumulative count of lookups performed on the cache.
    pub lookups: u64,
    /// Cumulative count of lookups that returned data from an existing cache entry.
    pub hits: u64,
    /// Cumulative count of lookups that did not match any existing cache entry.
    pub misses: u64,
    /// Cumulative count of insertions into the cache.
    pub allocs: u64,
    /// Cumulative count of evictions from the cache, to make space for a new insertion.
    pub reclaims: u64,
    /// Cumulative count of evictions from the cache due to no longer being deemed relevant.
    /// This is not used in our current implementation.
    pub frees: u64,
}

impl std::ops::Add for &CacheStats {
    type Output = CacheStats;

    fn add(self, other: &CacheStats) -> CacheStats {
        CacheStats {
            lookups: self.lookups + other.lookups,
            hits: self.hits + other.hits,
            misses: self.misses + other.misses,
            allocs: self.allocs + other.allocs,
            reclaims: self.reclaims + other.reclaims,
            frees: self.frees + other.frees,
        }
    }
}

#[derive(Default, Debug)]
pub struct AtomicCacheStats {
    pub lookups: AtomicU64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub allocs: AtomicU64,
    pub reclaims: AtomicU64,
    pub frees: AtomicU64,
}

impl AtomicCacheStats {
    pub fn snapshot(&self) -> CacheStats {
        CacheStats {
            lookups: self.lookups.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            allocs: self.allocs.load(Ordering::Relaxed),
            reclaims: self.reclaims.load(Ordering::Relaxed),
            frees: self.frees.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.lookups.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.allocs.store(0, Ordering::Relaxed);
        self.reclaims.store(0, Ordering::Relaxed);
        self.frees.store(0, Ordering::Relaxed);
    }
}

/// The number of shards to use for the cache stats.
// TODO: https://fxbug.dev/483629131 - Do per-CPU sharding using rseq.
fn num_shards() -> usize {
    8
}

unsafe extern "C" {
    fn thrd_current() -> std::ffi::c_ulong;
}

/// A sharded accumulator for cache statistics.
pub struct ShardedCacheStats(Vec<CachePadded<AtomicCacheStats>>);

impl ShardedCacheStats {
    pub fn new() -> ShardedCacheStats {
        ShardedCacheStats(
            (0..num_shards()).map(|_| CachePadded::new(AtomicCacheStats::default())).collect(),
        )
    }

    pub fn shard(&self) -> &AtomicCacheStats {
        // SAFETY: there's nothing unsafe about this, we're just calling a C function.
        let index = (rapidhash::rapidhash(&unsafe { thrd_current() }.to_ne_bytes()) as usize)
            % num_shards();
        &self.0[index]
    }

    pub fn reset(&self) {
        for shard in &self.0 {
            shard.reset();
        }
    }

    // TODO: https://fxbug.dev/483629131 - Report per-CPU stats.
    pub fn read(&self) -> CacheStats {
        self.0.iter().fold(CacheStats::default(), |acc, stats| &acc + &stats.snapshot())
    }
}

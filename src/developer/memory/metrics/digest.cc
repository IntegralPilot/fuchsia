// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/digest.h"

#include <lib/trace/event.h>

#include <ranges>

#include "src/developer/memory/metrics/bucket_match.h"

namespace memory {

namespace {
constexpr char kUndigested[]{"Undigested"};
constexpr char kOrphaned[]{"Orphaned"};
constexpr char kKernel[]{"Kernel"};
constexpr char kFree[]{"Free"};
constexpr char kPagerTotal[]{"[Addl]PagerTotal"};
constexpr char kPagerNewest[]{"[Addl]PagerNewest"};
constexpr char kPagerOldest[]{"[Addl]PagerOldest"};
constexpr char kDiscardableLocked[]{"[Addl]DiscardableLocked"};
constexpr char kDiscardableUnlocked[]{"[Addl]DiscardableUnlocked"};
constexpr char kZramCompressedBytes[]{"[Addl]ZramCompressedBytes"};
}  // namespace

Digest::Digest(const Capture& capture, Digester* digester) { digester->Digest(capture, this); }

Digester::Digester(const std::vector<BucketMatch>& bucket_matches)
    : bucket_matches_(bucket_matches) {}

void Digester::Digest(const Capture& capture, class Digest* digest) {
  TRACE_DURATION("memory_metrics", "Digester::Digest");

  // Detect renames and invalidate bucket match caches.
  for (const auto& [koid, process] : capture.koid_to_process()) {
    auto [it, inserted] = process_names_.try_emplace(koid, process.name);
    if (!inserted && it->second != process.name) {
      // If the process koid is known, but its name changed, invalidate the process on all buckets
      // and store the new name.
      for (auto& bucket_match : bucket_matches_) {
        bucket_match.InvalidateProcess(koid);
      }
      it->second = process.name;
    }
  }

  // Clean up dead processes from process_names_.
  std::erase_if(process_names_, [&capture](const auto& pair) {
    return !capture.koid_to_process().contains(pair.first);
  });

  digest->time_ = capture.time();
  digest->undigested_vmos_.reserve(capture.koid_to_vmo().size());
  for (const auto& [koid, _] : capture.koid_to_vmo()) {
    digest->undigested_vmos_.emplace(koid);
  }

  digest->buckets_.reserve(bucket_matches_.size());
  for (auto& bucket_match : bucket_matches_) {
    auto& bucket = digest->buckets_.emplace_back(bucket_match.name(), 0);
    for (const auto& [_, process] : capture.koid_to_process()) {
      if (!bucket_match.ProcessMatch(process)) {
        continue;
      }
      for (const auto& v : process.vmos) {
        auto it = digest->undigested_vmos_.find(v);
        if (it == digest->undigested_vmos_.end()) {
          continue;
        }
        const auto& vmo = capture.vmo_for_koid(v);
        if (!bucket_match.VmoMatch(vmo.name)) {
          continue;
        }
        bucket.size_ += vmo.committed_scaled_bytes;
        digest->undigested_vmos_.erase(it);
      }
    }
  }

  {
    TRACE_DURATION("memory_metrics", "Digester::Digest sweep");
    for (auto& bucket_match : bucket_matches_) {
      bucket_match.Sweep();
    }
  }

  FractionalBytes undigested_size{};
  for (auto v : digest->undigested_vmos_) {
    undigested_size += capture.vmo_for_koid(v).committed_scaled_bytes;
  }

  const auto& kmem = capture.kmem();
  FractionalBytes vmo_size{undigested_size};
  for (const auto& bucket : digest->buckets_) {
    vmo_size += bucket.size_;
  }
  digest->buckets_.emplace_back(kUndigested, undigested_size.integral);
  digest->buckets_.emplace_back(
      kOrphaned, std::clamp(kmem.vmo_bytes - vmo_size.integral, 0ul, kmem.vmo_bytes));
  digest->buckets_.emplace_back(kKernel, kmem.wired_bytes + kmem.total_heap_bytes +
                                             kmem.mmu_overhead_bytes + kmem.ipc_bytes +
                                             kmem.other_bytes + kmem.slab_bytes + kmem.cache_bytes);
  digest->buckets_.emplace_back(kFree, kmem.free_bytes);
  const auto& kmem_ext =
      capture.kmem_extended() ? capture.kmem_extended().value() : zx_info_kmem_stats_extended_t{};
  digest->buckets_.emplace_back(kPagerTotal, kmem_ext.vmo_pager_total_bytes);
  digest->buckets_.emplace_back(kPagerNewest, kmem_ext.vmo_pager_newest_bytes);
  digest->buckets_.emplace_back(kPagerOldest, kmem_ext.vmo_pager_oldest_bytes);
  digest->buckets_.emplace_back(kDiscardableLocked, kmem_ext.vmo_discardable_locked_bytes);
  digest->buckets_.emplace_back(kDiscardableUnlocked, kmem_ext.vmo_discardable_unlocked_bytes);
  digest->buckets_.emplace_back(
      kZramCompressedBytes,
      capture.kmem_compression() ? capture.kmem_compression()->compressed_storage_bytes : 0);
}

}  // namespace memory

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_PMR_STATS_RESOURCE_H_
#define SRC_UI_SCENIC_LIB_UTILS_PMR_STATS_RESOURCE_H_

#include <lib/syslog/cpp/macros.h>

#include <atomic>
#include <memory_resource>

namespace utils {

// A std::pmr::memory_resource wrapper that tracks allocation and deallocation statistics.
// It delegates the actual allocation to an upstream memory resource.
class PmrStatsResource : public std::pmr::memory_resource {
 public:
  explicit PmrStatsResource(std::pmr::memory_resource* upstream = std::pmr::get_default_resource())
      : upstream_(upstream) {
    FX_DCHECK(upstream_);
  }

  size_t total_allocation_count() const { return total_allocation_count_.load(); }
  size_t total_allocated_bytes() const { return total_allocated_bytes_.load(); }
  size_t total_deallocation_count() const { return total_deallocation_count_.load(); }
  size_t total_deallocated_bytes() const { return total_deallocated_bytes_.load(); }
  size_t outstanding_allocations() const {
    return total_allocation_count() - total_deallocation_count();
  }
  size_t outstanding_bytes() const { return total_allocated_bytes() - total_deallocated_bytes(); }

  void Reset() {
    FX_DCHECK(outstanding_allocations() == 0);
    FX_DCHECK(outstanding_bytes() == 0);
    total_allocation_count_.store(0, std::memory_order_relaxed);
    total_allocated_bytes_.store(0, std::memory_order_relaxed);
    total_deallocation_count_.store(0, std::memory_order_relaxed);
    total_deallocated_bytes_.store(0, std::memory_order_relaxed);
  }

 protected:
  void* do_allocate(size_t bytes, size_t alignment) override {
    total_allocation_count_.fetch_add(1, std::memory_order_relaxed);
    total_allocated_bytes_.fetch_add(bytes, std::memory_order_relaxed);
    return upstream_->allocate(bytes, alignment);
  }

  void do_deallocate(void* p, size_t bytes, size_t alignment) override {
    total_deallocation_count_.fetch_add(1, std::memory_order_relaxed);
    total_deallocated_bytes_.fetch_add(bytes, std::memory_order_relaxed);
    upstream_->deallocate(p, bytes, alignment);
  }

  bool do_is_equal(const std::pmr::memory_resource& other) const noexcept override {
    return this == &other;
  }

 private:
  std::pmr::memory_resource* upstream_;
  std::atomic<size_t> total_allocation_count_{0};
  std::atomic<size_t> total_allocated_bytes_{0};
  std::atomic<size_t> total_deallocation_count_{0};
  std::atomic<size_t> total_deallocated_bytes_{0};
};

}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_PMR_STATS_RESOURCE_H_

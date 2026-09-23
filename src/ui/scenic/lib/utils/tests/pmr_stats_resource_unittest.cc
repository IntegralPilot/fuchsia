// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/pmr_stats_resource.h"

#include <vector>

#include <gtest/gtest.h>

namespace utils {
namespace test {

TEST(PmrStatsResourceTest, InitialState) {
  PmrStatsResource resource;
  EXPECT_EQ(resource.total_allocation_count(), 0u);
  EXPECT_EQ(resource.total_allocated_bytes(), 0u);
  EXPECT_EQ(resource.total_deallocation_count(), 0u);
  EXPECT_EQ(resource.total_deallocated_bytes(), 0u);
}

TEST(PmrStatsResourceTest, AllocationTracking) {
  PmrStatsResource resource;

  void* p1 = resource.allocate(100, 8);
  EXPECT_EQ(resource.total_allocation_count(), 1u);
  EXPECT_EQ(resource.total_allocated_bytes(), 100u);

  void* p2 = resource.allocate(200, 8);
  EXPECT_EQ(resource.total_allocation_count(), 2u);
  EXPECT_EQ(resource.total_allocated_bytes(), 300u);

  resource.deallocate(p1, 100, 8);
  EXPECT_EQ(resource.total_deallocation_count(), 1u);
  EXPECT_EQ(resource.total_deallocated_bytes(), 100u);

  resource.deallocate(p2, 200, 8);
  EXPECT_EQ(resource.total_deallocation_count(), 2u);
  EXPECT_EQ(resource.total_deallocated_bytes(), 300u);
}

TEST(PmrStatsResourceTest, VectorUsage) {
  PmrStatsResource resource;
  {
    std::pmr::vector<int> vec(&resource);
    vec.reserve(10);  // Allocates space for 10 ints

    EXPECT_GT(resource.total_allocation_count(), 0u);
    EXPECT_GE(resource.total_allocated_bytes(), 10 * sizeof(int));

    size_t alloc_count = resource.total_allocation_count();
    size_t alloc_bytes = resource.total_allocated_bytes();

    vec.push_back(1);  // Should not allocate if capacity is sufficient
    EXPECT_EQ(resource.total_allocation_count(), alloc_count);
    EXPECT_EQ(resource.total_allocated_bytes(), alloc_bytes);
  }
  // Vector destroyed, should deallocate
  EXPECT_GT(resource.total_deallocation_count(), 0u);
  EXPECT_EQ(resource.total_deallocated_bytes(), resource.total_allocated_bytes());
}

TEST(PmrStatsResourceTest, MonotonicBufferResourceUpstream) {
  PmrStatsResource stats_resource;
  {
    char buffer[1024];
    std::pmr::monotonic_buffer_resource pool(buffer, sizeof(buffer), &stats_resource);

    // Monotonic buffer resource might not allocate from upstream immediately if it has enough
    // buffer. Let's allocate something larger than the buffer to force upstream allocation.
    void* p = pool.allocate(2048, 8);

    EXPECT_GT(stats_resource.total_allocation_count(), 0u);
    EXPECT_GE(stats_resource.total_allocated_bytes(), 2048u);

    // Deallocate on pool is a no-op for upstream
    pool.deallocate(p, 2048, 8);
    EXPECT_EQ(stats_resource.total_deallocation_count(), 0u);
  }
  // Pool destroyed, should deallocate chunks from upstream
  EXPECT_GT(stats_resource.total_deallocation_count(), 0u);
  EXPECT_EQ(stats_resource.total_deallocated_bytes(), stats_resource.total_allocated_bytes());
}

}  // namespace test
}  // namespace utils

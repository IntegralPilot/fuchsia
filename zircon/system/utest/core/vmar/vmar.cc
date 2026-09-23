// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <errno.h>
#include <lib/fit/defer.h>
#include <lib/fzl/memory-probe.h>
#include <lib/zircon-internal/align.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <stdalign.h>
#include <sys/mman.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/features.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <atomic>
#include <climits>
#include <iterator>
#include <limits>
#include <thread>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

// These tests focus on the semantics of the VMARs themselves.  For heavier
// testing of the mapping permissions, see the VMO tests.

// Check that these values are consistent.
static_assert(ZX_VMO_OP_DECOMMIT == ZX_VMAR_OP_DECOMMIT);
static_assert(ZX_VMO_OP_ZERO == ZX_VMAR_OP_ZERO);

namespace {

const char kProcessName[] = "test-proc-vmar";

const zx_vm_option_t kRwxMapPerm = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE;
const zx_vm_option_t kRwxAllocPerm =
    ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE;

// Helper routine for other tests.  If bit i (< *page_count*) in *bitmap* is set, then
// checks that *base* + i * zx_system_get_page_size() is mapped.  Otherwise checks that it is not
// mapped.
bool check_pages_mapped(zx_handle_t process, uintptr_t base, uint64_t bitmap, size_t page_count) {
  uint8_t buf[1];
  size_t len;

  size_t i = 0;
  while (bitmap && i < page_count) {
    zx_status_t expected = (bitmap & 1) ? ZX_OK : ZX_ERR_NOT_FOUND;
    if (zx_process_read_memory(process, base + i * zx_system_get_page_size(), buf, 1, &len) !=
        expected) {
      return false;
    }
    ++i;
    bitmap >>= 1;
  }
  return true;
}

TEST(Vmar, DestroyTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  zx_handle_t sub_vmar;
  uintptr_t sub_region_addr;
  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             1024 * zx_system_get_page_size(), &sub_vmar, &sub_region_addr),
            ZX_OK);
  zx_handle_t dup_sub_vmar;
  ASSERT_OK(
      zx_handle_duplicate(sub_vmar, ZX_DEFAULT_VMAR_RIGHTS & ~ZX_RIGHT_OP_CHILDREN, &dup_sub_vmar));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, zx_vmar_destroy(dup_sub_vmar));
  EXPECT_OK(zx_handle_close(dup_sub_vmar));

  EXPECT_EQ(zx_vmar_destroy(sub_vmar), ZX_OK);

  zx_handle_t region;
  uintptr_t region_addr;
  EXPECT_EQ(zx_vmar_allocate(sub_vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             10 * zx_system_get_page_size(), &region, &region_addr),
            ZX_ERR_BAD_STATE);

  EXPECT_EQ(zx_handle_close(sub_vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

TEST(Vmar, BasicAllocateTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t region1, region2;
  uintptr_t region1_addr, region2_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t region1_size = zx_system_get_page_size() * 10;
  const size_t region2_size = zx_system_get_page_size();

  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, region1_size,
                             &region1, &region1_addr),
            ZX_OK);

  ASSERT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, region2_size,
                             &region2, &region2_addr),
            ZX_OK);
  EXPECT_GE(region2_addr, region1_addr);
  EXPECT_LE(region2_addr + region2_size, region1_addr + region1_size);

  EXPECT_EQ(zx_handle_close(region1), ZX_OK);
  EXPECT_EQ(zx_handle_close(region2), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

TEST(Vmar, MapInCompactTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t region;
  uintptr_t region_addr, map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t region_size = zx_system_get_page_size() * 10;
  const size_t map_size = zx_system_get_page_size();

  ASSERT_EQ(zx_vmo_create(map_size, 0, &vmo), ZX_OK);

  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_COMPACT, 0,
                             region_size, &region, &region_addr),
            ZX_OK);

  ASSERT_EQ(zx_vmar_map(region, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, map_size, &map_addr),
            ZX_OK);
  EXPECT_GE(map_addr, region_addr);
  EXPECT_LE(map_addr + map_size, region_addr + region_size);

  // Make a second allocation
  ASSERT_EQ(zx_vmar_map(region, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, map_size, &map_addr),
            ZX_OK);
  EXPECT_GE(map_addr, region_addr);
  EXPECT_LE(map_addr + map_size, region_addr + region_size);

  EXPECT_EQ(zx_handle_close(region), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

TEST(Vmar, MapInUpperLimitTest) {
  const size_t kRegionPages = 100;
  const size_t kSubRegions = kRegionPages / 2;

  zx_handle_t process;
  zx_handle_t process_vmar;
  zx_handle_t vmo;
  zx_handle_t parent_region;
  uintptr_t parent_region_addr;
  uintptr_t map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &process_vmar),
            ZX_OK);

  const size_t region_size = zx_system_get_page_size() * kRegionPages;
  const size_t map_size = zx_system_get_page_size();

  ASSERT_EQ(zx_vmo_create(region_size, 0, &vmo), ZX_OK);

  // Allocate a region and allow mapping to a specific location to enable specifying an upper limit.
  ASSERT_EQ(zx_vmar_allocate(process_vmar,
                             ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                             region_size, &parent_region, &parent_region_addr),
            ZX_OK);

  // Set the upper limit for all maps to the midpoint of the parent region.
  const uintptr_t upper_limit = region_size / 2;
  const zx_vm_option_t options = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_OFFSET_IS_UPPER_LIMIT;

  // An upper limit beyond the end of the parent region should fail.
  ASSERT_EQ(zx_vmar_map(parent_region, options, region_size + zx_system_get_page_size(), vmo, 0,
                        zx_system_get_page_size(), &map_addr),
            ZX_ERR_INVALID_ARGS);

  // A size greater than the upper limit should fail.
  ASSERT_EQ(zx_vmar_map(parent_region, options, zx_system_get_page_size(), vmo, 0,
                        zx_system_get_page_size() * 2, &map_addr),
            ZX_ERR_INVALID_ARGS);

  // A size larger than the parent region should fail.
  ASSERT_EQ(zx_vmar_map(parent_region, options, zx_system_get_page_size(), vmo, 0,
                        region_size + zx_system_get_page_size(), &map_addr),
            ZX_ERR_INVALID_ARGS);

  // A size and upper limit equal to the parent region should succeed.
  ASSERT_EQ(zx_vmar_map(parent_region, options, region_size, vmo, 0, region_size, &map_addr),
            ZX_OK);
  ASSERT_EQ(parent_region_addr, map_addr);

  ASSERT_EQ(zx_vmar_unmap(parent_region, map_addr, region_size), ZX_OK);

  // Every map should conform to the upper limit.
  for (size_t i = 0; i < kSubRegions; i++) {
    ASSERT_EQ(zx_vmar_map(parent_region, options, upper_limit, vmo, 0, map_size, &map_addr), ZX_OK);
    EXPECT_GE(map_addr, parent_region_addr);
    EXPECT_LE(map_addr + map_size, parent_region_addr + upper_limit);
  }

  // Mapping one more time should fail now that all of the VMAR below the upper limit is consumed.
  ASSERT_EQ(zx_vmar_map(parent_region, options, upper_limit, vmo, 0, map_size, &map_addr),
            ZX_ERR_NO_RESOURCES);

  // Mapping one more time without the upper limit should succeed.
  ASSERT_EQ(zx_vmar_map(parent_region, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, map_size,
                        &map_addr),
            ZX_OK);
  EXPECT_GE(map_addr, parent_region_addr);
  EXPECT_LE(map_addr + map_size, parent_region_addr + region_size);

  EXPECT_EQ(zx_handle_close(parent_region), ZX_OK);
  EXPECT_EQ(zx_handle_close(process_vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Attempt to allocate out of the region bounds
TEST(Vmar, AllocateOobTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t region1, region2;
  uintptr_t region1_addr, region2_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t region1_size = zx_system_get_page_size() * 10;

  ASSERT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                       region1_size, &region1, &region1_addr),
      ZX_OK);

  EXPECT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             region1_size, zx_system_get_page_size(), &region2, &region2_addr),
            ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             region1_size - zx_system_get_page_size(),
                             zx_system_get_page_size() * 2, &region2, &region2_addr),
            ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(zx_handle_close(region1), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Attempt to make unsatisfiable allocations
TEST(Vmar, AllocateUnsatisfiableTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t region1, region2, region3;
  uintptr_t region1_addr, region2_addr, region3_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t region1_size = zx_system_get_page_size() * 10;

  ASSERT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                       region1_size, &region1, &region1_addr),
      ZX_OK);

  // Too large to fit in the region should get ZX_ERR_INVALID_ARGS
  EXPECT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             region1_size + zx_system_get_page_size(), &region2, &region2_addr),
            ZX_ERR_INVALID_ARGS);

  // Allocate the whole range, should work
  ASSERT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, region1_size,
                             &region2, &region2_addr),
            ZX_OK);
  EXPECT_EQ(region2_addr, region1_addr);

  // Attempt to allocate a page inside of the full region
  EXPECT_EQ(zx_vmar_allocate(region1, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size(), &region3, &region3_addr),
            ZX_ERR_NO_RESOURCES);

  EXPECT_EQ(zx_handle_close(region2), ZX_OK);
  EXPECT_EQ(zx_handle_close(region1), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Test that virtual address space beginning at 0x200000 is accessible
TEST(Vmar, AllocateAtLowAddressTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  zx_info_vmar_t info;
  ASSERT_EQ(zx_object_get_info(vmar, ZX_INFO_VMAR, &info, sizeof(info), NULL, NULL), ZX_OK);
  ASSERT_LE(info.base, 0x200000);

  zx_vaddr_t addr;
  ASSERT_EQ(zx_vmar_map(vmar, 0, 0x200000 - info.base, vmo, 0, zx_system_get_page_size(), &addr),
            ZX_OK);

  EXPECT_EQ(zx_handle_close(process), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
}

// Validate that when we destroy a VMAR, all operations on it
// and its children fail.
TEST(Vmar, DestroyedVmarTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t region[3] = {};
  uintptr_t region_addr[3];
  uintptr_t map_addr[2];

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             10 * zx_system_get_page_size(), &region[0], &region_addr[0]),
            ZX_OK);

  // Create a mapping in region[0], so we can try to unmap it later
  ASSERT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size(), &map_addr[0]),
            ZX_OK);

  // Create a subregion in region[0], so we can try to operate on it later
  ASSERT_EQ(zx_vmar_allocate(region[0], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size(), &region[1], &region_addr[1]),
            ZX_OK);

  // Create a mapping in region[1], so we can try to unmap it later
  ASSERT_EQ(zx_vmar_map(region[1], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size(), &map_addr[1]),
            ZX_OK);

  // Check that both mappings work
  {
    uint8_t buf = 5;
    size_t len;
    EXPECT_EQ(zx_process_write_memory(process, map_addr[0], &buf, 1, &len), ZX_OK);
    EXPECT_EQ(len, 1U);

    buf = 0;
    EXPECT_EQ(zx_process_read_memory(process, map_addr[1], &buf, 1, &len), ZX_OK);
    EXPECT_EQ(len, 1U);
    EXPECT_EQ(buf, 5U);
  }

  // Destroy region[0], which should also destroy region[1]
  ASSERT_EQ(zx_vmar_destroy(region[0]), ZX_OK);

  for (size_t i = 0; i < 2; ++i) {
    // Make sure the handles are still valid
    EXPECT_EQ(zx_handle_check_valid(region[i]), ZX_OK);

    // Make sure we can't access the memory mappings anymore
    {
      uint8_t buf;
      size_t read;
      EXPECT_STATUS(zx_process_read_memory(process, map_addr[i], &buf, 1, &read), ZX_ERR_NOT_FOUND);
    }

    // All operations on region[0] and region[1] should fail with ZX_ERR_BAD_STATE
    EXPECT_EQ(zx_vmar_destroy(region[i]), ZX_ERR_BAD_STATE);
    EXPECT_EQ(zx_vmar_allocate(region[i], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                               zx_system_get_page_size(), &region[1], &region_addr[2]),
              ZX_ERR_BAD_STATE);
    EXPECT_EQ(zx_vmar_unmap(region[i], map_addr[i], zx_system_get_page_size()), ZX_ERR_BAD_STATE);
    EXPECT_EQ(zx_vmar_protect(region[i], ZX_VM_PERM_READ, map_addr[i], zx_system_get_page_size()),
              ZX_ERR_BAD_STATE);
    EXPECT_EQ(
        zx_vmar_map(region[i], ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(), &map_addr[i]),
        ZX_ERR_BAD_STATE);
  }

  // Make sure we can still operate on the parent of region[0]
  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size(), &region[2], &region_addr[2]),
            ZX_OK);

  for (zx_handle_t h : region) {
    EXPECT_EQ(zx_handle_close(h), ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Create a mapping, destroy the VMAR it is in, then attempt to create a new
// mapping over it.
TEST(Vmar, MapOverDestroyedTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo, vmo2;
  zx_handle_t region[2] = {};
  uintptr_t region_addr[2];
  uintptr_t map_addr;
  size_t len;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo2), ZX_OK);

  ASSERT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                       10 * zx_system_get_page_size(), &region[0], &region_addr[0]),
      ZX_OK);

  // Create a subregion in region[0], so we can try to operate on it later
  ASSERT_EQ(zx_vmar_allocate(region[0], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size(), &region[1], &region_addr[1]),
            ZX_OK);

  // Create a mapping in region[1], so we can try to unmap it later
  ASSERT_EQ(zx_vmar_map(region[1], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size(), &map_addr),
            ZX_OK);

  // Check that the mapping worked
  {
    uint8_t buf = 5;
    ASSERT_EQ(zx_vmo_write(vmo, &buf, 0, 1), ZX_OK);

    buf = 0;
    EXPECT_EQ(zx_process_read_memory(process, map_addr, &buf, 1, &len), ZX_OK);
    EXPECT_EQ(len, 1U);
    EXPECT_EQ(buf, 5U);
  }

  // Destroy region[1], which should unmap the VMO
  ASSERT_EQ(zx_vmar_destroy(region[1]), ZX_OK);

  // Make sure we can't access the memory mappings anymore
  {
    uint8_t buf;
    size_t read;
    EXPECT_STATUS(zx_process_read_memory(process, map_addr, &buf, 1, &read), ZX_ERR_NOT_FOUND);
  }

  uintptr_t new_map_addr;
  EXPECT_EQ(
      zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  map_addr - region_addr[0], vmo2, 0, zx_system_get_page_size(), &new_map_addr),
      ZX_OK);
  EXPECT_EQ(new_map_addr, map_addr);

  // Make sure we can read, and we don't see the old memory mapping
  {
    uint8_t buf;
    size_t read;
    EXPECT_EQ(zx_process_read_memory(process, map_addr, &buf, 1, &read), ZX_OK);
    EXPECT_EQ(read, 1U);
    EXPECT_EQ(buf, 0U);
  }

  for (zx_handle_t h : region) {
    EXPECT_EQ(zx_handle_close(h), ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo2), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

struct AlignTestdata {
  zx_vm_option_t alignment;
  int zero_bits;
};

AlignTestdata align_data[]{
    {ZX_VM_ALIGN_1KB, 10},   {ZX_VM_ALIGN_2KB, 11},   {ZX_VM_ALIGN_4KB, 12},
    {ZX_VM_ALIGN_8KB, 13},   {ZX_VM_ALIGN_16KB, 14},  {ZX_VM_ALIGN_32KB, 15},
    {ZX_VM_ALIGN_64KB, 16},  {ZX_VM_ALIGN_128KB, 17}, {ZX_VM_ALIGN_256KB, 18},
    {ZX_VM_ALIGN_512KB, 19}, {ZX_VM_ALIGN_1MB, 20},   {ZX_VM_ALIGN_2MB, 21},
    {ZX_VM_ALIGN_4MB, 22},   {ZX_VM_ALIGN_8MB, 23},   {ZX_VM_ALIGN_16MB, 24},
    {ZX_VM_ALIGN_32MB, 25},  {ZX_VM_ALIGN_64MB, 26},  {ZX_VM_ALIGN_128MB, 27},
    {ZX_VM_ALIGN_256MB, 28}, {ZX_VM_ALIGN_512MB, 29}, {ZX_VM_ALIGN_1GB, 30},
    {ZX_VM_ALIGN_2GB, 31},   {ZX_VM_ALIGN_4GB, 32}};

// Create a vmar with |alignment| of size |vmar_size|, this is needed only
// needed when testing the alignment flags.
zx_status_t MakeManualAlignedVmar(size_t vmar_size, size_t alignment, zx_handle_t* vmar) {
  zx_info_vmar_t vmar_info;
  auto status = zx_object_get_info(zx_vmar_root_self(), ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info),
                                   NULL, NULL);
  if (status != ZX_OK) {
    return status;
  }

  const size_t root_vmar_end = vmar_info.base + vmar_info.len;
  size_t start = fbl::round_up(vmar_info.base, alignment);

  zx_vaddr_t root_addr = 0u;
  for (; start < root_vmar_end; start += alignment) {
    status = zx_vmar_allocate(
        zx_vmar_root_self(),
        ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_SPECIFIC,
        (start - vmar_info.base), vmar_size, vmar, &root_addr);
    if (status == ZX_OK) {
      break;
    }
  }
  return status;
}

TEST(Vmar, AlignmentVmarMapTest) {
  const size_t size = zx_system_get_page_size() * 2;
#if defined(__riscv) && __has_feature(address_sanitizer)
  // The reduced address space of riscv along with additional address space utilization with ASAN
  // does not allow large VMAR allocations to succeed. So scale down the VMAR size, which should
  // prevent testing of only three of the higher alignment flags.
  const auto vmar_size = (512ull * 1024 * 1024);
  const auto vmar_alignment = (32ull * 1024 * 1024);
#else
  const auto vmar_size = (8ull * 1024 * 1024 * 1024);
  const auto vmar_alignment = (512ull * 1024 * 1024);
#endif

  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);
  zx_handle_t vmar = ZX_HANDLE_INVALID;
  ASSERT_EQ(MakeManualAlignedVmar(vmar_size, vmar_alignment, &vmar), ZX_OK);

  // Specific base + offset does not meet the alignment, so it fails.
  zx_vaddr_t addr;
  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALIGN_64KB | ZX_VM_SPECIFIC,
                  4096, vmo, 0, size, &addr),
      ZX_ERR_INVALID_ARGS);

  // Specific base + offset meets alignment, it should succeed.
  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALIGN_64KB | ZX_VM_SPECIFIC,
                  64 * 1024, vmo, 0, size, &addr),
      ZX_OK);
  ASSERT_EQ(zx_vmar_unmap(vmar, addr, size), ZX_OK);

  // Minimum supported alignments range is 2^10 to 2^32
  zx_vm_option_t bad_align_low = (9u << ZX_VM_ALIGN_BASE);
  zx_vm_option_t bad_align_high = (33u << ZX_VM_ALIGN_BASE);

  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | bad_align_low, 0, vmo, 0, size, &addr),
      ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | bad_align_high, 0, vmo, 0, size,
                        &addr),
            ZX_ERR_INVALID_ARGS);

  // Test all supported alignments.
  for (const auto& d : align_data) {
    zx_vaddr_t mapping_addr = 0u;
    zx_status_t status = zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | d.alignment, 0, vmo,
                                     0, size, &mapping_addr);
    if (status != ZX_OK) {
      if (1ull << d.zero_bits > vmar_size) {
        // If the requested alignment was greater than the VMAR size, the map should fail with
        // ZX_ERR_NO_RESOURCES. Any other failure code is unexpected.
        ASSERT_EQ(ZX_ERR_NO_RESOURCES, status, "alignment zero bits %d", d.zero_bits);
        continue;
      }
    }
    ASSERT_OK(status, "alignment zero bits %d", d.zero_bits);

    ASSERT_NE(mapping_addr, 0u);
    EXPECT_GE(__builtin_ctzll(mapping_addr), d.zero_bits);
    // touch memory and unmap.
    reinterpret_cast<uint64_t*>(mapping_addr)[1] = 0x1234321;
    ASSERT_EQ(zx_vmar_unmap(vmar, mapping_addr, size), ZX_OK);
  }

  ASSERT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  ASSERT_EQ(zx_handle_close(vmar), ZX_OK);
  ASSERT_EQ(zx_handle_close(vmo), ZX_OK);
}

TEST(Vmar, AlignmentVmarAllocateTest) {
  const size_t size = zx_system_get_page_size() * 16;
#if defined(__riscv) && __has_feature(address_sanitizer)
  // The reduced address space of riscv along with additional address space utilization with ASAN
  // does not allow large VMAR allocations to succeed. So scale down the VMAR size, which should
  // prevent testing of only three of the higher alignment flags.
  const auto vmar_size = (512ull * 1024 * 1024);
  const auto vmar_alignment = (32ull * 1024 * 1024);
#else
  const auto vmar_size = (8ull * 1024 * 1024 * 1024);
  const auto vmar_alignment = (512ull * 1024 * 1024);
#endif

  zx_handle_t vmar = ZX_HANDLE_INVALID;
  ASSERT_EQ(MakeManualAlignedVmar(vmar_size, vmar_alignment, &vmar), ZX_OK);

  // Specific base + offset does not meet the alignment, so it fails.
  zx_vaddr_t addr;
  zx_handle_t sub_vmar;
  EXPECT_EQ(zx_vmar_allocate(
                vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_ALIGN_64KB | ZX_VM_SPECIFIC,
                4096, size, &sub_vmar, &addr),
            ZX_ERR_INVALID_ARGS);

  // Specific base + offset meets alignment, it should succeed.
  EXPECT_EQ(zx_vmar_allocate(
                vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_ALIGN_64KB | ZX_VM_SPECIFIC,
                64 * 1024, size, &sub_vmar, &addr),
            ZX_OK);
  ASSERT_EQ(zx_vmar_destroy(sub_vmar), ZX_OK);
  ASSERT_EQ(zx_handle_close(sub_vmar), ZX_OK);

  // Minimum supported alignments range is 2^10 to 2^32
  const zx_vm_option_t bad_align_low = (9u << ZX_VM_ALIGN_BASE);
  const zx_vm_option_t bad_align_high = (33u << ZX_VM_ALIGN_BASE);

  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | bad_align_low, 0,
                             size, &sub_vmar, &addr),
            ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | bad_align_high, 0,
                             size, &sub_vmar, &addr),
            ZX_ERR_INVALID_ARGS);

  // Test all supported alignments.
  for (const auto& d : align_data) {
    zx_handle_t child_vmar;
    uintptr_t mapping_addr = 0u;
    zx_status_t status =
        zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | d.alignment, 0, size,
                         &child_vmar, &mapping_addr);
    if (status != ZX_OK) {
      // If the requested alignment was greater than the VMAR size, the allocation should fail with
      // ZX_ERR_NO_RESOURCES. Any other failure code is unexpected.
      if (1ull << d.zero_bits > vmar_size) {
        ASSERT_EQ(ZX_ERR_NO_RESOURCES, status, "alignment zero bits %d", d.zero_bits);
        continue;
      }
    }
    ASSERT_OK(status, "alignment zero bits %d", d.zero_bits);

    ASSERT_NE(mapping_addr, 0u);
    EXPECT_GE(__builtin_ctzll(mapping_addr), d.zero_bits);
    ASSERT_EQ(zx_vmar_destroy(child_vmar), ZX_OK);
    ASSERT_EQ(zx_handle_close(child_vmar), ZX_OK);
  }

  ASSERT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  ASSERT_EQ(zx_handle_close(vmar), ZX_OK);
}

// Test to ensure we can map from a given VMO offset with MAP_RANGE enabled. This tests against
// a bug found when creating MmioBuffer with a provided VMO and an offset.
TEST(Vmar, VmarMapRangeOffsetTest) {
  zx::vmar vmar;
  zx::vmo vmo;
  zx::process process;
  ASSERT_EQ(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName) - 1, 0,
                                &process, &vmar),
            ZX_OK);
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo), ZX_OK);
  uintptr_t mapping;
  EXPECT_EQ(vmar.map(ZX_VM_MAP_RANGE, 0, vmo, 0x2000, 0x1000, &mapping), ZX_OK);
}

// Attempt overmapping with FLAG_SPECIFIC to ensure it fails
TEST(Vmar, OvermappingTest) {
  zx_handle_t process;
  zx_handle_t region[3] = {};
  zx_handle_t vmar;
  zx_handle_t vmo, vmo2;
  uintptr_t region_addr[3];
  uintptr_t map_addr[2];

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size() * 4, 0, &vmo2), ZX_OK);

  ASSERT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                       10 * zx_system_get_page_size(), &region[0], &region_addr[0]),
      ZX_OK);

  // Create a mapping, and try to map on top of it
  ASSERT_EQ(
      zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  zx_system_get_page_size(), vmo, 0, 2 * zx_system_get_page_size(), &map_addr[0]),
      ZX_OK);

  // Attempt a full overmapping
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        map_addr[0] - region_addr[0], vmo2, 0, 2 * zx_system_get_page_size(),
                        &map_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Attempt a partial overmapping
  EXPECT_EQ(
      zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  map_addr[0] - region_addr[0], vmo2, 0, zx_system_get_page_size(), &map_addr[1]),
      ZX_ERR_ALREADY_EXISTS);

  // Attempt an overmapping that is larger than the original mapping
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        map_addr[0] - region_addr[0], vmo2, 0, 4 * zx_system_get_page_size(),
                        &map_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Attempt to allocate a region on top
  EXPECT_EQ(zx_vmar_allocate(region[0], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             map_addr[0] - region_addr[0], zx_system_get_page_size(), &region[1],
                             &region_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Unmap the mapping
  ASSERT_EQ(zx_vmar_unmap(region[0], map_addr[0], 2 * zx_system_get_page_size()), ZX_OK);

  // Create a region, and try to map on top of it
  ASSERT_EQ(zx_vmar_allocate(region[0], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             zx_system_get_page_size(), 2 * zx_system_get_page_size(), &region[1],
                             &region_addr[1]),
            ZX_OK);

  // Attempt a full overmapping
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        region_addr[1] - region_addr[0], vmo2, 0, 2 * zx_system_get_page_size(),
                        &map_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Attempt a partial overmapping
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        region_addr[1] - region_addr[0], vmo2, 0, zx_system_get_page_size(),
                        &map_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Attempt an overmapping that is larger than the original region
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        region_addr[1] - region_addr[0], vmo2, 0, 4 * zx_system_get_page_size(),
                        &map_addr[1]),
            ZX_ERR_ALREADY_EXISTS);

  // Attempt to allocate a region on top
  EXPECT_EQ(zx_vmar_allocate(region[0], ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             region_addr[1] - region_addr[0], zx_system_get_page_size(), &region[2],
                             &region_addr[2]),
            ZX_ERR_ALREADY_EXISTS);

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo2), ZX_OK);
  EXPECT_EQ(zx_handle_close(region[0]), ZX_OK);
  EXPECT_EQ(zx_handle_close(region[1]), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Test passing in bad arguments
TEST(Vmar, InvalidArgsTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t region;
  uintptr_t region_addr, map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  ASSERT_EQ(zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo), ZX_OK);

  // Bad handle
  EXPECT_EQ(zx_vmar_destroy(vmo), ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(zx_vmar_allocate(vmo, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             10 * zx_system_get_page_size(), &region, &region_addr),
            ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(zx_vmar_map(vmo, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, process, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(zx_vmar_unmap(vmo, 0, 0), ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(zx_vmar_protect(vmo, ZX_VM_PERM_READ, 0, 0), ZX_ERR_WRONG_TYPE);

  // Allocating with non-zero offset and without FLAG_SPECIFIC or FLAG_OFFSET_IS_UPPER_LIMIT.
  EXPECT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, zx_system_get_page_size(),
                       10 * zx_system_get_page_size(), &region, &region_addr),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, zx_system_get_page_size(), vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_ERR_INVALID_ARGS);

  // Allocating with non-zero offset with both SPECIFIC* and OFFSET_IS_UPPER_LIMIT.
  EXPECT_EQ(
      zx_vmar_allocate(
          vmar,
          ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC | ZX_VM_OFFSET_IS_UPPER_LIMIT,
          zx_system_get_page_size(), 10 * zx_system_get_page_size(), &region, &region_addr),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(
      zx_vmar_map(vmar,
                  ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_OFFSET_IS_UPPER_LIMIT,
                  zx_system_get_page_size(), vmo, 0, 4 * zx_system_get_page_size(), &map_addr),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_allocate(vmar,
                             ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC_OVERWRITE |
                                 ZX_VM_OFFSET_IS_UPPER_LIMIT,
                             zx_system_get_page_size(), 10 * zx_system_get_page_size(), &region,
                             &region_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(
      zx_vmar_map(vmar,
                  ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE |
                      ZX_VM_OFFSET_IS_UPPER_LIMIT,
                  zx_system_get_page_size(), vmo, 0, 4 * zx_system_get_page_size(), &map_addr),
      ZX_ERR_INVALID_ARGS);

  // Allocate with ZX_VM_PERM_READ.
  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_PERM_READ, zx_system_get_page_size(),
                             10 * zx_system_get_page_size(), &region, &region_addr),
            ZX_ERR_INVALID_ARGS);

  // Using MAP_RANGE with SPECIFIC_OVERWRITE
  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_SPECIFIC_OVERWRITE | ZX_VM_MAP_RANGE,
                  zx_system_get_page_size(), vmo, 0, 4 * zx_system_get_page_size(), &map_addr),
      ZX_ERR_INVALID_ARGS);

  // Non-page-aligned arguments
  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size() - 1, &region, &region_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_allocate(
                vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC,
                zx_system_get_page_size() - 1, zx_system_get_page_size(), &region, &region_addr),
            ZX_ERR_INVALID_ARGS);
  // Try the invalid maps with and without ZX_VM_MAP_RANGE.
  for (size_t i = 0; i < 2; ++i) {
    const uint32_t map_range = i ? ZX_VM_MAP_RANGE : 0;
    // Specific, misaligned vmar offset
    EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | map_range,
                          zx_system_get_page_size() - 1, vmo, 0, 4 * zx_system_get_page_size(),
                          &map_addr),
              ZX_ERR_INVALID_ARGS);
    // Specific, misaligned vmo offset
    EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | map_range,
                          zx_system_get_page_size(), vmo, zx_system_get_page_size() - 1,
                          3 * zx_system_get_page_size(), &map_addr),
              ZX_ERR_INVALID_ARGS);
    // Non-specific, misaligned vmo offset
    EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | map_range, 0, vmo,
                          zx_system_get_page_size() - 1, 3 * zx_system_get_page_size(), &map_addr),
              ZX_ERR_INVALID_ARGS);
  }
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr + 1, zx_system_get_page_size()), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, map_addr + 1, zx_system_get_page_size()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size()), ZX_OK);

  // Overflowing vmo_offset
  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo,
                  UINT64_MAX + 1 - zx_system_get_page_size(), zx_system_get_page_size(), &map_addr),
      ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo,
                        UINT64_MAX + 1 - 2 * zx_system_get_page_size(), zx_system_get_page_size(),
                        &map_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, zx_system_get_page_size()), ZX_OK);

  // size=0
  EXPECT_EQ(
      zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, 0, &region, &region_addr),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, 0, &map_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 0), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, map_addr, 0), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size()), ZX_OK);

  // size rounds up to 0
  const size_t bad_size = std::numeric_limits<size_t>::max() - zx_system_get_page_size() + 2;
  assert(((bad_size + zx_system_get_page_size() - 1) & ~(zx_system_get_page_size() - 1)) == 0);
  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, bad_size, &region,
                             &region_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, bad_size, &map_addr),
            ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0, vmo, 0, bad_size, &map_addr),
            ZX_ERR_OUT_OF_RANGE);
  // Attempt bad protect/unmaps
  EXPECT_EQ(
      zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  zx_system_get_page_size(), vmo, 0, 4 * zx_system_get_page_size(), &map_addr),
      ZX_OK);
  for (ssize_t i = -1; i < 2; ++i) {
    EXPECT_EQ(
        zx_vmar_protect(vmar, ZX_VM_PERM_READ, map_addr + zx_system_get_page_size() * i, bad_size),
        ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(zx_vmar_unmap(vmar, map_addr + zx_system_get_page_size() * i, bad_size),
              ZX_ERR_INVALID_ARGS);
  }
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size()), ZX_OK);

  // Flags with invalid bits set
  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_PERM_READ | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             4 * zx_system_get_page_size(), &region, &region_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | (1 << 31), 0, 4 * zx_system_get_page_size(),
                             &region, &region_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_CAN_MAP_EXECUTE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | (1 << 31), 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &map_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ | ZX_VM_CAN_MAP_WRITE, map_addr,
                            4 * zx_system_get_page_size()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(
      zx_vmar_protect(vmar, ZX_VM_PERM_READ | (1 << 31), map_addr, 4 * zx_system_get_page_size()),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size()), ZX_OK);

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Test passing in unaligned lens to unmap/protect
TEST(Vmar, UnalignedLenTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  ASSERT_EQ(zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo), ZX_OK);

  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ, 0, vmo, 0, 4 * zx_system_get_page_size(), &map_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, map_addr,
                            4 * zx_system_get_page_size() - 1),
            ZX_OK);
  EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size() - 1), ZX_OK);

  // Make sure we can't access the last page of the memory mappings anymore
  {
    uint8_t buf;
    size_t read;
    EXPECT_STATUS(
        zx_process_read_memory(process, map_addr + 3 * zx_system_get_page_size(), &buf, 1, &read),
        ZX_ERR_NOT_FOUND);
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Test passing in unaligned lens to map
TEST(Vmar, UnalignedLenMapTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  ASSERT_EQ(zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo), ZX_OK);

  for (size_t i = 0; i < 2; ++i) {
    const uint32_t map_range = i ? ZX_VM_MAP_RANGE : 0;
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | map_range, 0, vmo, 0,
                          4 * zx_system_get_page_size() - 1, &map_addr),
              ZX_OK);

    // Make sure we can access the last page of the memory mapping
    {
      uint8_t buf;
      size_t read;
      EXPECT_EQ(
          zx_process_read_memory(process, map_addr + 3 * zx_system_get_page_size(), &buf, 1, &read),
          ZX_OK);
    }

    EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, 4 * zx_system_get_page_size() - 1), ZX_OK);
    // Make sure we can't access the last page of the memory mappings anymore
    {
      uint8_t buf;
      size_t read;
      EXPECT_STATUS(
          zx_process_read_memory(process, map_addr + 3 * zx_system_get_page_size(), &buf, 1, &read),
          ZX_ERR_NOT_FOUND);
    }
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Validate that dropping vmar handle rights affects mapping privileges
TEST(Vmar, RightsDropTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t region;
  uintptr_t map_addr;
  uintptr_t region_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_replace_as_executable(vmo, ZX_HANDLE_INVALID, &vmo), ZX_OK);

  const uint32_t test_rights[][3] = {
      {ZX_RIGHT_READ, ZX_VM_PERM_READ},
      {ZX_RIGHT_READ | ZX_RIGHT_WRITE, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE},
      {ZX_RIGHT_READ | ZX_RIGHT_EXECUTE, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE},
  };
  for (size_t i = 0; i < std::size(test_rights); ++i) {
    uint32_t right = test_rights[i][0];
    uint32_t perm = test_rights[i][1];

    zx_handle_t new_h;
    ASSERT_EQ(zx_handle_duplicate(vmar, right, &new_h), ZX_OK);

    // Try to create a mapping with permissions we don't have
    EXPECT_EQ(zx_vmar_map(new_h, kRwxMapPerm, 0, vmo, 0, zx_system_get_page_size(), &map_addr),
              ZX_ERR_ACCESS_DENIED);

    // Try to create a mapping with permissions we do have
    ASSERT_EQ(zx_vmar_map(new_h, perm, 0, vmo, 0, zx_system_get_page_size(), &map_addr), ZX_OK);

    // Attempt to use protect to increase privileges
    EXPECT_EQ(zx_vmar_protect(new_h, kRwxMapPerm, map_addr, zx_system_get_page_size()),
              ZX_ERR_ACCESS_DENIED);

    EXPECT_EQ(zx_vmar_unmap(new_h, map_addr, zx_system_get_page_size()), ZX_OK);

    // Attempt to create a region that can map write (this would allow us to
    // then make writeable mappings inside of it).
    EXPECT_EQ(zx_vmar_allocate(new_h, kRwxAllocPerm, 0, 10 * zx_system_get_page_size(), &region,
                               &region_addr),
              ZX_ERR_ACCESS_DENIED);

    EXPECT_EQ(zx_handle_close(new_h), ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Validate that protect can't be used to escalate mapping privileges beyond
// the VMAR handle's and the original VMO handle's
TEST(Vmar, ProtectTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_replace_as_executable(vmo, ZX_HANDLE_INVALID, &vmo), ZX_OK);

  const uint32_t test_rights[][3] = {
      {ZX_RIGHT_READ, ZX_VM_PERM_READ},
      {ZX_RIGHT_READ | ZX_RIGHT_WRITE, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE},
      {ZX_RIGHT_READ | ZX_RIGHT_EXECUTE, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE},
  };
  for (size_t i = 0; i < std::size(test_rights); ++i) {
    uint32_t right = test_rights[i][0];
    zx_vm_option_t perm = test_rights[i][1];

    zx_handle_t new_h;
    ASSERT_EQ(zx_handle_duplicate(vmo, right | ZX_RIGHT_MAP, &new_h), ZX_OK);

    // Try to create a mapping with permissions we don't have
    EXPECT_EQ(zx_vmar_map(vmar, kRwxMapPerm, 0, new_h, 0, zx_system_get_page_size(), &map_addr),
              ZX_ERR_ACCESS_DENIED);

    // Try to create a mapping with permissions we do have
    ASSERT_EQ(zx_vmar_map(vmar, perm, 0, new_h, 0, zx_system_get_page_size(), &map_addr), ZX_OK);

    // Attempt to use protect to increase privileges to a level allowed by
    // the VMAR but not by the VMO handle
    EXPECT_EQ(zx_vmar_protect(vmar, kRwxMapPerm, map_addr, zx_system_get_page_size()),
              ZX_ERR_ACCESS_DENIED);

    EXPECT_EQ(zx_handle_close(new_h), ZX_OK);

    // Try again now that we closed the VMO handle
    EXPECT_EQ(zx_vmar_protect(vmar, kRwxMapPerm, map_addr, zx_system_get_page_size()),
              ZX_ERR_ACCESS_DENIED);

    EXPECT_EQ(zx_vmar_unmap(vmar, map_addr, zx_system_get_page_size()), ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Protect should only reduce the active permissions in a sub-region, never increase them.
TEST(Vmar, ProtectSubregionTest) {
  zx::vmar vmar;

  uintptr_t addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                                            zx_system_get_page_size(), &vmar, &addr));

  zx::vmar child_vmar;
  uintptr_t child_addr;
  ASSERT_OK(vmar.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, zx_system_get_page_size(),
                          &child_vmar, &child_addr));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Map the VMO in with its full read/write permissions.
  zx_vaddr_t map_addr;
  EXPECT_OK(child_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, zx_system_get_page_size(),
                           &map_addr));

  // All the addresses should be the same.
  EXPECT_EQ(addr, child_addr);
  EXPECT_EQ(addr, map_addr);

  // Use the parent vmar to protect away the write permission.
  EXPECT_OK(vmar.protect(ZX_VM_PERM_READ, addr, zx_system_get_page_size()));

  // Cannot put the write permission back using the parent.
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            vmar.protect(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, addr, zx_system_get_page_size()));

  // The child vmar can protect the write permission back though.
  EXPECT_OK(child_vmar.protect(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, child_addr,
                               zx_system_get_page_size()));

  // Removing the OP_CHILDREN right should not allow even removing of permissions with protect.
  zx_info_handle_basic_t handle_info;
  EXPECT_OK(
      vmar.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr, nullptr));
  zx::vmar dup_vmar;
  EXPECT_OK(vmar.duplicate(handle_info.rights & ~ZX_RIGHT_OP_CHILDREN, &dup_vmar));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.protect(ZX_VM_PERM_READ, addr, zx_system_get_page_size()));

  // Cleanup.
  vmar.destroy();
}

// Validate that a region can't be created with higher RWX privileges than its
// parent.
TEST(Vmar, NestedRegionPermsTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t region[2] = {};
  uintptr_t region_addr[2];
  uintptr_t map_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_replace_as_executable(vmo, ZX_HANDLE_INVALID, &vmo), ZX_OK);

  // List of pairs of alloc/map perms to try to exclude
  const zx_vm_option_t test_perm[][2] = {
      {ZX_VM_CAN_MAP_READ, ZX_VM_PERM_READ},
      {ZX_VM_CAN_MAP_WRITE, ZX_VM_PERM_WRITE},
      {ZX_VM_CAN_MAP_EXECUTE, ZX_VM_PERM_EXECUTE},
  };

  for (size_t i = 0; i < std::size(test_perm); ++i) {
    const zx_vm_option_t excluded_alloc_perm = test_perm[i][0];
    const zx_vm_option_t excluded_map_perm = test_perm[i][1];

    ASSERT_EQ(zx_vmar_allocate(vmar, kRwxAllocPerm ^ excluded_alloc_perm, 0,
                               10 * zx_system_get_page_size(), &region[0], &region_addr[0]),
              ZX_OK);

    // Should fail since region[0] does not have the right perms
    EXPECT_EQ(zx_vmar_allocate(region[0], kRwxAllocPerm, 0, zx_system_get_page_size(), &region[1],
                               &region_addr[1]),
              ZX_ERR_ACCESS_DENIED);

    // Try to create a mapping in region[0] with the dropped rights
    EXPECT_EQ(zx_vmar_map(region[0], kRwxMapPerm, 0, vmo, 0, zx_system_get_page_size(), &map_addr),
              ZX_ERR_ACCESS_DENIED);

    // Successfully create a mapping in region[0] (skip if we excluded READ,
    // since all mappings must be readable on most CPUs)
    if (excluded_map_perm != ZX_VM_PERM_READ) {
      EXPECT_EQ(zx_vmar_map(region[0], kRwxMapPerm ^ excluded_map_perm, 0, vmo, 0,
                            zx_system_get_page_size(), &map_addr),
                ZX_OK);
      EXPECT_EQ(zx_vmar_unmap(region[0], map_addr, zx_system_get_page_size()), ZX_OK);
    }

    // Successfully create a subregion in region[0]
    EXPECT_EQ(zx_vmar_allocate(region[0], kRwxAllocPerm ^ excluded_alloc_perm, 0,
                               zx_system_get_page_size(), &region[1], &region_addr[1]),
              ZX_OK);
    EXPECT_EQ(zx_vmar_destroy(region[1]), ZX_OK);
    EXPECT_EQ(zx_handle_close(region[1]), ZX_OK);

    EXPECT_EQ(zx_vmar_destroy(region[0]), ZX_OK);
    EXPECT_EQ(zx_handle_close(region[0]), ZX_OK);
  }

  // Make sure we can't use SPECIFIC in a region without CAN_MAP_SPECIFIC
  ASSERT_EQ(zx_vmar_allocate(vmar, kRwxAllocPerm, 0, 10 * zx_system_get_page_size(), &region[0],
                             &region_addr[0]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_SPECIFIC | ZX_VM_PERM_READ, zx_system_get_page_size(), vmo,
                        0, zx_system_get_page_size(), &map_addr),
            ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(zx_vmar_map(region[0], ZX_VM_SPECIFIC_OVERWRITE | ZX_VM_PERM_READ,
                        zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(), &map_addr),
            ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(zx_vmar_destroy(region[0]), ZX_OK);
  EXPECT_EQ(zx_handle_close(region[0]), ZX_OK);

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

TEST(Vmar, ObjectInfoTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t region;
  uintptr_t region_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t region_size = zx_system_get_page_size() * 10;

  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, region_size,
                             &region, &region_addr),
            ZX_OK);

  zx_info_vmar_t info;
  ASSERT_EQ(zx_object_get_info(region, ZX_INFO_VMAR, &info, sizeof(info), NULL, NULL), ZX_OK);
  EXPECT_EQ(info.base, region_addr);
  EXPECT_EQ(info.len, region_size);

  EXPECT_EQ(zx_handle_close(region), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can split a single mapping with an unmap call
TEST(Vmar, UnmapSplitTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t mapping_addr[3];

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo), ZX_OK);

  // Set up mappings to test on
  for (uintptr_t& addr : mapping_addr) {
    EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                          4 * zx_system_get_page_size(), &addr),
              ZX_OK);
  }

  // Unmap from the left
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 2 * zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1100, 4));
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0] + 2 * zx_system_get_page_size(),
                          2 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000, 4));

  // Unmap from the right
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1] + 2 * zx_system_get_page_size(),
                          2 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[1], 0b0011, 4));
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1], 2 * zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[1], 0b0000, 4));

  // Unmap from the center
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[2] + zx_system_get_page_size(),
                          2 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[2], 0b1001, 4));
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[2], zx_system_get_page_size()), ZX_OK);
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[2] + 3 * zx_system_get_page_size(),
                          zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[2], 0b0000, 4));

  zx_info_vmar_t info;
  ASSERT_EQ(zx_object_get_info(vmar, ZX_INFO_VMAR, &info, sizeof(info), NULL, NULL), ZX_OK);

  // Make sure we can map over these again
  for (uintptr_t addr : mapping_addr) {
    const size_t offset = addr - info.base;
    EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, offset, vmo, 0,
                          4 * zx_system_get_page_size(), &addr),
              ZX_OK);
    EXPECT_TRUE(check_pages_mapped(process, addr, 0b1111, 4));
    EXPECT_EQ(zx_vmar_unmap(vmar, addr, 4 * zx_system_get_page_size()), ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can unmap multiple ranges simultaneously
TEST(Vmar, UnmapMultipleTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  zx_handle_t subregion;
  uintptr_t mapping_addr[3];
  uintptr_t subregion_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t mapping_size = 4 * zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(mapping_size, 0, &vmo), ZX_OK);

  // Create two mappings
  for (size_t i = 0; i < 2; ++i) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          i * mapping_size, vmo, 0, mapping_size, &mapping_addr[i]),
              ZX_OK);
  }
  EXPECT_EQ(mapping_addr[0] + mapping_size, mapping_addr[1]);
  // Unmap from the right of the first and the left of the second
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0] + 2 * zx_system_get_page_size(),
                          3 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1110'0011, 8), "");
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 2 * zx_system_get_page_size()), ZX_OK, "");
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1] + 1 * zx_system_get_page_size(),
                          3 * zx_system_get_page_size()),
            ZX_OK, "");
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000, 8));

  // Create two mappings with a gap, and verify we can unmap them
  for (size_t i = 0; i < 2; ++i) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          2 * i * mapping_size, vmo, 0, mapping_size, &mapping_addr[i]),
              ZX_OK);
  }
  EXPECT_EQ(mapping_addr[0] + 2 * mapping_size, mapping_addr[1]);
  // Unmap all of the left one and some of the right one
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 2 * mapping_size + zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1110'0000'0000, 12));
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1] + 1 * zx_system_get_page_size(),
                          3 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));

  // Create two mappings with a subregion between, should be able to unmap
  // them (and destroy the subregion in the process).
  for (size_t i = 0; i < 2; ++i) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          2 * i * mapping_size, vmo, 0, mapping_size, &mapping_addr[i]),
              ZX_OK);
  }
  ASSERT_EQ(
      zx_vmar_allocate(
          vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_SPECIFIC,
          mapping_size, mapping_size, &subregion, &subregion_addr),
      ZX_OK);
  ASSERT_EQ(zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        zx_system_get_page_size(), &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(mapping_addr[0] + 2 * mapping_size, mapping_addr[1]);
  EXPECT_EQ(mapping_addr[0] + mapping_size, mapping_addr[2]);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'0001'1111, 12));
  // Unmap all of the left one and some of the right one
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 2 * mapping_size + zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1110'0000'0000, 12));
  // Try to map in the subregion again, should fail due to being destroyed
  ASSERT_EQ(
      zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(), &mapping_addr[2]),
      ZX_ERR_BAD_STATE);
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1] + 1 * zx_system_get_page_size(),
                          3 * zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));
  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  // Create two mappings with a subregion after.  Partial unmap of the
  // subregion should fail, full unmap should succeed.
  for (size_t i = 0; i < 2; ++i) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          i * mapping_size, vmo, 0, mapping_size, &mapping_addr[i]),
              ZX_OK);
  }
  ASSERT_EQ(
      zx_vmar_allocate(
          vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_SPECIFIC,
          2 * mapping_size, mapping_size, &subregion, &subregion_addr),
      ZX_OK);
  ASSERT_EQ(zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        zx_system_get_page_size(), &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(mapping_addr[0] + mapping_size, mapping_addr[1]);
  EXPECT_EQ(mapping_addr[0] + 2 * mapping_size, mapping_addr[2]);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0001'1111'1111, 12));
  // Unmap some of the left one through to all but the last page of the subregion
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0] + zx_system_get_page_size(),
                          3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0001'1111'1111, 12));
  // Try again, but unmapping all of the subregion
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0] + zx_system_get_page_size(),
                          3 * mapping_size - zx_system_get_page_size()),
            ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0001, 12));
  // Try to map in the subregion again, should fail due to being destroyed
  ASSERT_EQ(
      zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(), &mapping_addr[2]),
      ZX_ERR_BAD_STATE);
  // Unmap the rest
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));
  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  // Create two mappings with a subregion before.  Partial unmap of the
  // subregion should fail, full unmap should succeed.
  for (size_t i = 0; i < 2; ++i) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          (i + 1) * mapping_size, vmo, 0, mapping_size, &mapping_addr[i]),
              ZX_OK);
  }
  ASSERT_EQ(
      zx_vmar_allocate(
          vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_SPECIFIC,
          0, mapping_size, &subregion, &subregion_addr),
      ZX_OK);
  ASSERT_EQ(zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        mapping_size - zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(),
                        &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(subregion_addr + mapping_size, mapping_addr[0]);
  EXPECT_EQ(subregion_addr + 2 * mapping_size, mapping_addr[1]);
  EXPECT_TRUE(check_pages_mapped(process, subregion_addr, 0b1111'1111'1000, 12));
  // Try to unmap everything except the first page of the subregion
  EXPECT_EQ(zx_vmar_unmap(vmar, subregion_addr + zx_system_get_page_size(),
                          3 * mapping_size - zx_system_get_page_size()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_TRUE(check_pages_mapped(process, subregion_addr, 0b1111'1111'1000, 12));
  // Try again, but unmapping all of the subregion
  EXPECT_EQ(zx_vmar_unmap(vmar, subregion_addr, 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, subregion_addr, 0b0000'0000'0000, 12));
  // Try to map in the subregion again, should fail due to being destroyed
  ASSERT_EQ(
      zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                  zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(), &mapping_addr[2]),
      ZX_ERR_BAD_STATE);
  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can unmap multiple ranges simultaneously
TEST(Vmar, UnmapBaseNotMappedTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t mapping_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t mapping_size = 4 * zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(mapping_size, 0, &vmo), ZX_OK);

  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        zx_system_get_page_size(), vmo, 0, mapping_size, &mapping_addr),
            ZX_OK);
  ASSERT_EQ(zx_vmar_unmap(vmar, mapping_addr - zx_system_get_page_size(),
                          mapping_size + zx_system_get_page_size()),
            ZX_OK);

  // Try again, but this time with a mapping below where base is
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr),
            ZX_OK);
  for (size_t gap = zx_system_get_page_size(); gap < 3 * zx_system_get_page_size();
       gap += zx_system_get_page_size()) {
    ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                          mapping_size + gap, vmo, 0, mapping_size, &mapping_addr),
              ZX_OK);
    ASSERT_EQ(zx_vmar_unmap(vmar, mapping_addr - zx_system_get_page_size(),
                            mapping_size + zx_system_get_page_size()),
              ZX_OK);
  }

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can restrict sub-region destruction with ZX_RIGHT_OP_CHILDREN
TEST(Vmar, UnmapOpChildren) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                                            zx_system_get_page_size(), &vmar, &vmar_addr));

  // Make a sub vmar.
  zx::vmar child_vmar;
  uintptr_t child_vmar_addr;
  ASSERT_OK(vmar.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, zx_system_get_page_size(),
                          &child_vmar, &child_vmar_addr));

  // Map the VMO into the child.
  zx_vaddr_t map_addr;
  EXPECT_OK(child_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, zx_system_get_page_size(),
                           &map_addr));

  // All the addresses should've ended up the same.
  EXPECT_EQ(vmar_addr, child_vmar_addr);
  EXPECT_EQ(vmar_addr, map_addr);

  // Duplicate the vmar handle and remove the op_children permission.
  zx::vmar dup_vmar;
  ASSERT_OK(vmar.duplicate(ZX_DEFAULT_VMAR_RIGHTS & ~ZX_RIGHT_OP_CHILDREN, &dup_vmar));

  // Unmap using the duplicate should fail.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, dup_vmar.unmap(vmar_addr, zx_system_get_page_size()));

  // Unmap on the original should succeed.
  EXPECT_OK(vmar.unmap(vmar_addr, zx_system_get_page_size()));

  // Cleanup
  vmar.destroy();
}

// Verify that we can overwrite subranges and multiple ranges simultaneously
TEST(Vmar, MapSpecificOverwriteTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo, vmo2;
  zx_handle_t subregion;
  uintptr_t mapping_addr[2];
  uintptr_t subregion_addr;
  uint8_t buf[1];
  size_t len;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  const size_t mapping_size = 4 * zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(mapping_size * 2, 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_vmo_create(mapping_size * 2, 0, &vmo2), ZX_OK);

  // Tag each page of the VMOs so we can identify which mappings are from
  // which.
  for (size_t i = 0; i < mapping_size / zx_system_get_page_size(); ++i) {
    buf[0] = 1;
    ASSERT_EQ(zx_vmo_write(vmo, buf, i * zx_system_get_page_size(), 1), ZX_OK);
    buf[0] = 2;
    ASSERT_EQ(zx_vmo_write(vmo2, buf, i * zx_system_get_page_size(), 1), ZX_OK);
  }

  // Create a single mapping and overwrite it
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        zx_system_get_page_size(), vmo, 0, mapping_size, &mapping_addr[0]),
            ZX_OK);
  // Try over mapping with SPECIFIC but not SPECIFIC_OVERWRITE
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                        zx_system_get_page_size(), vmo2, 0, mapping_size, &mapping_addr[1]),
            ZX_ERR_ALREADY_EXISTS);
  // Try again with SPECIFIC_OVERWRITE
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE,
                        zx_system_get_page_size(), vmo2, 0, mapping_size, &mapping_addr[1]),
            ZX_OK);
  EXPECT_EQ(mapping_addr[0], mapping_addr[1]);
  for (size_t i = 0; i < mapping_size / zx_system_get_page_size(); ++i) {
    EXPECT_EQ(zx_process_read_memory(process, mapping_addr[0] + i * zx_system_get_page_size(), buf,
                                     1, &len),
              ZX_OK);
    EXPECT_EQ(buf[0], 2u);
  }

  // Overmap the middle of it
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE,
                        2 * zx_system_get_page_size(), vmo, 0, 2 * zx_system_get_page_size(),
                        &mapping_addr[0]),
            ZX_OK);
  EXPECT_EQ(mapping_addr[0], mapping_addr[1] + zx_system_get_page_size());
  for (size_t i = 0; i < mapping_size / zx_system_get_page_size(); ++i) {
    EXPECT_EQ(zx_process_read_memory(process, mapping_addr[1] + i * zx_system_get_page_size(), buf,
                                     1, &len),
              ZX_OK);
    EXPECT_EQ(buf[0], (i == 0 || i == 3) ? 2u : 1u);
  }

  // Create an adjacent sub-region, try to overmap it
  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             zx_system_get_page_size() + mapping_size, mapping_size, &subregion,
                             &subregion_addr),
            ZX_OK);
  EXPECT_EQ(subregion_addr, mapping_addr[1] + mapping_size);
  EXPECT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE,
                        zx_system_get_page_size(), vmo2, 0, 2 * mapping_size, &mapping_addr[0]),
            ZX_ERR_INVALID_ARGS);
  // Tear it all down
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[1], 2 * mapping_size), ZX_OK);

  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo2), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can split a single mapping with a protect call
TEST(Vmar, ProtectSplitTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo;
  uintptr_t mapping_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);

  ASSERT_EQ(zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo), ZX_OK);

  // Protect from the left
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr, 2 * zx_system_get_page_size()),
            ZX_OK);
  // TODO(teisenbe): Test to validate perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b1111, 4));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr, 4 * zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b0000, 4));

  // Protect from the right
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr + 2 * zx_system_get_page_size(),
                            2 * zx_system_get_page_size()),
            ZX_OK);
  // TODO(teisenbe): Test to validate perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b1111, 4));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr, 4 * zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b0000, 4));

  // Protect from the center
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        4 * zx_system_get_page_size(), &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr + zx_system_get_page_size(),
                            2 * zx_system_get_page_size()),
            ZX_OK);
  // TODO(teisenbe): Test to validate perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b1111, 4));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr, 4 * zx_system_get_page_size()), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr, 0b0000, 4));

  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Validate that protect can be used across multiple mappings.  Make sure intersecting a subregion
// or gap fails
TEST(Vmar, ProtectMultipleTest) {
  zx_handle_t process;
  zx_handle_t vmar;
  zx_handle_t vmo, vmo2;
  zx_handle_t subregion;
  uintptr_t mapping_addr[3];
  uintptr_t subregion_addr;

  ASSERT_EQ(zx_process_create(zx_job_default(), kProcessName, sizeof(kProcessName) - 1, 0, &process,
                              &vmar),
            ZX_OK);
  const size_t mapping_size = 4 * zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(mapping_size, 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_handle_duplicate(vmo, ZX_RIGHT_MAP | ZX_RIGHT_READ, &vmo2), ZX_OK);

  // Protect from the right on the first mapping, all of the second mapping,
  // and from the left on the third mapping.
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[0]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[1]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 2 * mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr[0] + zx_system_get_page_size(),
                            3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_OK);
  // TODO(teisenbe): Test to validate perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'1111'1111, 12));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));

  // Same thing, but map middle region with a VMO without the WRITE right
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[0]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_SPECIFIC, mapping_size, vmo2, 0, mapping_size,
                        &mapping_addr[1]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 2 * mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                            mapping_addr[0] + zx_system_get_page_size(),
                            3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_ERR_ACCESS_DENIED);
  // TODO(teisenbe): Test to validate no perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'1111'1111, 12));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));

  // Try to protect across a gap
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[0]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 2 * mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr[0] + zx_system_get_page_size(),
                            3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_ERR_NOT_FOUND);
  // TODO(teisenbe): Test to validate no perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'0000'1111, 12));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));

  // Try to protect across an empty subregion
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[0]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_allocate(vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
                             mapping_size, mapping_size, &subregion, &subregion_addr),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 2 * mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr[0] + zx_system_get_page_size(),
                            3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_ERR_NOT_FOUND);
  // TODO(teisenbe): Test to validate no perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'0000'1111, 12));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));
  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  // Try to protect across a subregion filled with mappings
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[0]),
            ZX_OK);
  ASSERT_EQ(
      zx_vmar_allocate(
          vmar, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC | ZX_VM_CAN_MAP_SPECIFIC,
          mapping_size, mapping_size, &subregion, &subregion_addr),
      ZX_OK);
  ASSERT_EQ(zx_vmar_map(subregion, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0,
                        mapping_size, &mapping_addr[1]),
            ZX_OK);
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 2 * mapping_size,
                        vmo, 0, mapping_size, &mapping_addr[2]),
            ZX_OK);
  EXPECT_EQ(zx_vmar_protect(vmar, ZX_VM_PERM_READ, mapping_addr[0] + zx_system_get_page_size(),
                            3 * mapping_size - 2 * zx_system_get_page_size()),
            ZX_OK);
  // TODO(teisenbe): Test to validate no perms changed, need to export more debug
  // info
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b1111'1111'1111, 12));
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr[0], 3 * mapping_size), ZX_OK);
  EXPECT_TRUE(check_pages_mapped(process, mapping_addr[0], 0b0000'0000'0000, 12));
  EXPECT_EQ(zx_handle_close(subregion), ZX_OK);

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo2), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(process), ZX_OK);
}

// Verify that we can change protections on a demand paged mapping successfully.
TEST(Vmar, ProtectOverDemandPagedTest) {
  zx_handle_t vmo;
  const size_t size = 100 * zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);

  // TODO(teisenbe): Move this into a separate process; currently we don't
  // have an easy way to run a small test routine in another process.
  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                        &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  std::atomic_uint8_t* target = reinterpret_cast<std::atomic_uint8_t*>(mapping_addr);
  target[0].store(5);
  target[size / 2].store(6);
  target[size - 1].store(7);

  ASSERT_EQ(zx_vmar_protect(zx_vmar_root_self(), ZX_VM_PERM_READ, mapping_addr, size), ZX_OK);

  // Attempt to write to the mapping again
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr)), ZX_ERR_ACCESS_DENIED,
                "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 4)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 2)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size - 1)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");

  EXPECT_EQ(zx_vmar_unmap(zx_vmar_root_self(), mapping_addr, size), ZX_OK);
}

// Verify that we can change protections on unmapped pages successfully.
TEST(Vmar, ProtectLargeUncomittedTest) {
  zx_handle_t vmo;
  // Create a 1GB VMO
  const size_t size = 1ull << 30;
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);

  // TODO(teisenbe): Move this into a separate process; currently we don't
  // have an easy way to run a small test routine in another process.
  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                        &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  // Make sure some pages exist
  std::atomic_uint8_t* target = reinterpret_cast<std::atomic_uint8_t*>(mapping_addr);
  target[0].store(5);
  target[size / 2].store(6);
  target[size - 1].store(7);

  // Ensure we're misaligned relative to a larger paging structure level.
  // TODO(teisenbe): Would be nice for this to be more arch aware.
  const uintptr_t base =
      ZX_ROUNDUP(mapping_addr, 512 * zx_system_get_page_size()) + zx_system_get_page_size();
  const size_t protect_size = mapping_addr + size - base;
  ASSERT_EQ(zx_vmar_protect(zx_vmar_root_self(), ZX_VM_PERM_READ, base, protect_size), ZX_OK);

  // Attempt to write to the mapping again
  EXPECT_OK(probe_for_write(reinterpret_cast<void*>(mapping_addr)),
            "mapping should still be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 4)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 2)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size - 1)),
                ZX_ERR_ACCESS_DENIED, "mapping should no longer be writeable");

  EXPECT_EQ(zx_vmar_unmap(zx_vmar_root_self(), mapping_addr, size), ZX_OK);
}

// Verify vmar_op_range() commit/decommit of mapped VMO pages.
TEST(Vmar, RangeOpCommitVmoPages) {
  // Create a VMO to map parts of into a VMAR.
  const size_t kVmoSize = zx_system_get_page_size() * 5;
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_vmo_create(kVmoSize, 0, &vmo), ZX_OK);

  // Create a VMAR to guarantee some pages remain unmapped.
  zx_vaddr_t vmar_base = 0u;
  zx_handle_t vmar = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_vmar_allocate(zx_vmar_root_self(),
                             ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             kVmoSize, &vmar, &vmar_base),
            ZX_OK);

  zx_vaddr_t mapping_addr = 0u;
  // Map one writable page to the VMO.
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size() * 2, &mapping_addr),
            ZX_OK);
  ASSERT_EQ(vmar_base, mapping_addr);

  // Map second page to a different part of the VMO.
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                        zx_system_get_page_size() * 2, vmo, zx_system_get_page_size() * 3,
                        zx_system_get_page_size(), &mapping_addr),
            ZX_OK);

  // Map fourth page read-only.
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_SPECIFIC | ZX_VM_PERM_READ, zx_system_get_page_size() * 4, vmo,
                        zx_system_get_page_size(), zx_system_get_page_size(), &mapping_addr),
            ZX_OK);

  // Verify decommit of only part of a mapping.
  std::atomic_uint8_t* target = reinterpret_cast<std::atomic_uint8_t*>(vmar_base);
  target[0].store(5);
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_DECOMMIT, vmar_base, zx_system_get_page_size(),
                             nullptr, 0u),
            ZX_OK);
  EXPECT_EQ(target[0].load(), 0u);
  target[zx_system_get_page_size()].store(7);
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMO_OP_DECOMMIT, vmar_base + zx_system_get_page_size(),
                             zx_system_get_page_size(), nullptr, 0u),
            ZX_OK);
  EXPECT_EQ(target[zx_system_get_page_size()].load(), 0u);

  // Verify decommit across two adjacent mappings.
  target[zx_system_get_page_size()].store(5);
  target[zx_system_get_page_size() * 2].store(6);
  EXPECT_EQ(target[zx_system_get_page_size() * 4].load(), 5u);
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_DECOMMIT, vmar_base + zx_system_get_page_size(),
                             zx_system_get_page_size() * 2, nullptr, 0u),
            ZX_OK);
  EXPECT_EQ(target[zx_system_get_page_size()].load(), 0u);
  EXPECT_EQ(target[zx_system_get_page_size() * 2].load(), 0u);
  EXPECT_EQ(target[zx_system_get_page_size() * 4].load(), 0u);

  // Verify decommit including an unmapped region fails.
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_DECOMMIT, vmar_base + zx_system_get_page_size(),
                             zx_system_get_page_size() * 3, nullptr, 0u),
            ZX_ERR_BAD_STATE);

  // Decommit of a non-writable mapping succeeds if the mapping can be made
  // writable by the caller, i.e. it was created with a writable VMO handle.
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMO_OP_DECOMMIT, vmar_base + (zx_system_get_page_size() * 4),
                             zx_system_get_page_size(), nullptr, 0u),
            ZX_OK);

  // Decommit of a non-writable mapping fails if the caller cannot make the
  // mapping writable, i.e. it was created from a read-only VMO handle.
  zx_handle_t readonly_vmo = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_handle_duplicate(vmo, ZX_RIGHT_MAP | ZX_RIGHT_READ, &readonly_vmo), ZX_OK);
  zx_vaddr_t readonly_mapping_addr = 0u;
  ASSERT_EQ(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, readonly_vmo, 0,
                        zx_system_get_page_size(), &readonly_mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_vmar_op_range(zx_vmar_root_self(), ZX_VMAR_OP_DECOMMIT, readonly_mapping_addr,
                             zx_system_get_page_size(), nullptr, 0u),
            ZX_ERR_ACCESS_DENIED);
  EXPECT_EQ(zx_vmar_unmap(zx_vmar_root_self(), readonly_mapping_addr, zx_system_get_page_size()),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(readonly_vmo), ZX_OK);

  // Clean up the test VMAR and VMO.
  EXPECT_EQ(zx_vmar_unmap(vmar, vmar_base, kVmoSize), ZX_OK);
  EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
}

// Verify vmar_range_op map range of committed mapped VMO pages.
TEST(Vmar, RangeOpMapRange) {
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  const size_t vmo_size = zx_system_get_page_size() * 4;

  ASSERT_EQ(zx_vmo_create(vmo_size, 0, &vmo), ZX_OK);

  zx_handle_t vmar = ZX_HANDLE_INVALID;
  zx_vaddr_t vmar_base = 0;

  ASSERT_EQ(zx_vmar_allocate(zx_vmar_root_self(), ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             vmo_size, &vmar, &vmar_base),
            ZX_OK);

  zx_vaddr_t map_base = 0;

  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, vmo_size, &map_base),
            ZX_OK);

  // Verify the ZX_VMAR_OP_MAP_RANGE op with zx_vmar_op_range.

  // Attempting to map range uncommitted pages should succeed.
  ASSERT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_MAP_RANGE, map_base, vmo_size, nullptr, 0), ZX_OK);

  // Commit the first page in the VMO.
  ASSERT_EQ(zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0),
            ZX_OK);

  // Attempting to map range partially committed contiguous pages should succeed.
  ASSERT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_MAP_RANGE, map_base, vmo_size, nullptr, 0), ZX_OK);

  // Commit the second and last page in the VMO, leaving a discontiguous hole.
  ASSERT_EQ(zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, zx_system_get_page_size(),
                            zx_system_get_page_size(), nullptr, 0),
            ZX_OK);
  ASSERT_EQ(zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, zx_system_get_page_size() * 3,
                            zx_system_get_page_size(), nullptr, 0),
            ZX_OK);

  // Attempting to map range partially committed dicontiguous pages should succeed.
  ASSERT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_MAP_RANGE, map_base + zx_system_get_page_size(),
                             vmo_size - zx_system_get_page_size(), nullptr, 0),
            ZX_OK);

  // Commit all of the pages in the VMO.
  ASSERT_EQ(zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0, vmo_size, nullptr, 0), ZX_OK);

  // Attempting to map range the hole should succeed.
  ASSERT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_MAP_RANGE, vmar_base + zx_system_get_page_size() * 2,
                             zx_system_get_page_size(), nullptr, 0),
            ZX_OK);

  EXPECT_EQ(zx_vmar_unmap(vmar, map_base, vmo_size), ZX_OK);
  EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
}

// Verify vmar_range_op zero of committed mapped VMO pages.
TEST(Vmar, RangeOpZero) {
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  const size_t vmo_size = zx_system_get_page_size() * 4;

  ASSERT_EQ(zx_vmo_create(vmo_size, 0, &vmo), ZX_OK);

  std::vector<uint8_t> vmo_data(vmo_size);
  std::fill(vmo_data.begin(), vmo_data.end(), 0x55);
  ASSERT_EQ(zx_vmo_write(vmo, vmo_data.data(), 0, vmo_size), ZX_OK);

  zx_handle_t vmar = ZX_HANDLE_INVALID;
  zx_vaddr_t vmar_base = 0;

  ASSERT_EQ(zx_vmar_allocate(zx_vmar_root_self(), ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             vmo_size, &vmar, &vmar_base),
            ZX_OK);

  zx_vaddr_t map_base = 0;

  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, vmo_size, &map_base),
            ZX_OK);

  // Verify the ZX_VMAR_OP_ZERO op with zx_vmar_op_range.

  // Attempting to zero range uncommitted pages should succeed.
  ASSERT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_ZERO, map_base, vmo_size, nullptr, 0), ZX_OK);

  // Verify the VMO data is zeroed.
  ASSERT_EQ(zx_vmo_read(vmo, vmo_data.data(), 0, vmo_size), ZX_OK);
  for (size_t i = 0; i < vmo_size; i++) {
    EXPECT_EQ(vmo_data[i], 0);
  }

  // Misaligned address should fail.
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_ZERO, map_base + 1, vmo_size, nullptr, 0),
            ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(zx_vmar_unmap(vmar, map_base, vmo_size), ZX_OK);

  zx_handle_t vmo_no_write;
  ASSERT_EQ(zx_handle_duplicate(vmo, ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_WRITE, &vmo_no_write),
            ZX_OK);

  // Zeroing a read-only mapping should fail.
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ, 0, vmo_no_write, 0, vmo_size, &map_base), ZX_OK);
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_ZERO, map_base, vmo_size, nullptr, 0),
            ZX_ERR_ACCESS_DENIED);

  // Clean up.
  EXPECT_EQ(zx_vmar_unmap(vmar, map_base, vmo_size), ZX_OK);
  EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmar), ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);
}

// Validate that the ZX_RIGHT_OP_CHILDREN is needed to operate on subregions with op_range.
TEST(Vmar, RangeOpSubRegions) {
  zx::vmar vmar;

  uintptr_t addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                                            zx_system_get_page_size(), &vmar, &addr));

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR later.
    vmar.destroy();
  });

  zx::vmar child_vmar;
  uintptr_t child_addr;
  ASSERT_OK(vmar.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, zx_system_get_page_size(),
                          &child_vmar, &child_addr));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_vaddr_t map_addr;
  EXPECT_OK(child_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, zx_system_get_page_size(),
                           &map_addr));

  // All the addresses should be the same.
  EXPECT_EQ(addr, child_addr);
  EXPECT_EQ(addr, map_addr);

  // Remove the OP_CHILDREN right.
  zx_info_handle_basic_t handle_info;
  EXPECT_OK(
      vmar.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr, nullptr));
  zx::vmar dup_vmar;
  EXPECT_OK(vmar.duplicate(handle_info.rights & ~ZX_RIGHT_OP_CHILDREN, &dup_vmar));

  // Validate most operations work on the regular parent vmar, but do not work with OP_CHILDREN
  // removed.
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_MAP_RANGE, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_DONT_NEED, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, addr, zx_system_get_page_size(), nullptr, 0));

  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.op_range(ZX_VMAR_OP_MAP_RANGE, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.op_range(ZX_VMAR_OP_DONT_NEED, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, addr, zx_system_get_page_size(), nullptr, 0));

  // The commit and decommit ops will not work with or without OP_CHILDREN.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.op_range(ZX_VMAR_OP_COMMIT, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmar.op_range(ZX_VMAR_OP_COMMIT, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            dup_vmar.op_range(ZX_VMAR_OP_DECOMMIT, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmar.op_range(ZX_VMAR_OP_DECOMMIT, addr, zx_system_get_page_size(), nullptr, 0));
}

// Attempt to unmap a large mostly uncommitted VMO
TEST(Vmar, UnmapLargeUncommittedTest) {
  zx_handle_t vmo;
  // Create a 1GB VMO
  const size_t size = 1ull << 30;
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);

  // TODO(teisenbe): Move this into a separate process; currently we don't
  // have an easy way to run a small test routine in another process.
  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                        &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  // Make sure some pages exist
  std::atomic_uint8_t* target = reinterpret_cast<std::atomic_uint8_t*>(mapping_addr);
  target[0].store(5);
  target[size / 2].store(6);
  target[size - 1].store(7);

  // Ensure we're misaligned relative to a larger paging structure level.
  // TODO(teisenbe): Would be nice for this to be more arch aware.
  const uintptr_t base =
      ZX_ROUNDUP(mapping_addr, 512 * zx_system_get_page_size()) + zx_system_get_page_size();
  const size_t unmap_size = mapping_addr + size - base;
  ASSERT_EQ(zx_vmar_unmap(zx_vmar_root_self(), base, unmap_size), ZX_OK);

  // Attempt to write to the mapping again
  EXPECT_OK(probe_for_write(reinterpret_cast<void*>(mapping_addr)),
            "mapping should still be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 4)), ZX_ERR_NOT_FOUND,
                "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size / 2)), ZX_ERR_NOT_FOUND,
                "mapping should no longer be writeable");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(mapping_addr + size - 1)), ZX_ERR_NOT_FOUND,
                "mapping should no longer be writeable");

  EXPECT_EQ(zx_vmar_unmap(zx_vmar_root_self(), mapping_addr, size), ZX_OK);
}

TEST(Vmar, PartialUnmapAndRead) {
  // Map a two-page VMO.
  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size() * 2, 0, &vmo), ZX_OK);

  // Use a sub-VMAR so that unmapped addresses can remain reserved.
  uintptr_t vmar_base;
  zx_handle_t vmar;
  ASSERT_OK(zx_vmar_allocate(zx_vmar_root_self(), ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size() * 2, &vmar, &vmar_base));
  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR later.
    EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  });

  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size() * 2, &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  char* ptr = (char*)mapping_addr;
  memset(ptr, 0, zx_system_get_page_size() * 2);

  // Unmap the second page.
  ASSERT_OK(
      zx_vmar_unmap(vmar, mapping_addr + zx_system_get_page_size(), zx_system_get_page_size()));

  char buffer[zx_system_get_page_size() * 2];
  size_t actual_read;

  // First page succeeds.
  EXPECT_EQ(zx_process_read_memory(zx_process_self(), mapping_addr, buffer,
                                   zx_system_get_page_size(), &actual_read),
            ZX_OK);
  EXPECT_EQ(actual_read, zx_system_get_page_size());

  // Second page fails.
  EXPECT_STATUS(zx_process_read_memory(zx_process_self(), mapping_addr + zx_system_get_page_size(),
                                       buffer, zx_system_get_page_size(), &actual_read),
                ZX_ERR_NOT_FOUND);

  // Reading the whole region succeeds, but only reads the first page.
  EXPECT_EQ(zx_process_read_memory(zx_process_self(), mapping_addr, buffer,
                                   zx_system_get_page_size() * 2, &actual_read),
            ZX_OK);
  EXPECT_EQ(actual_read, zx_system_get_page_size());

  // Read at the boundary straddling the pages.
  EXPECT_EQ(zx_process_read_memory(zx_process_self(), mapping_addr + zx_system_get_page_size() - 1,
                                   buffer, 2, &actual_read),
            ZX_OK);
  EXPECT_EQ(actual_read, 1);

  // Unmap the left over first page.
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr, zx_system_get_page_size()), ZX_OK);
}

TEST(Vmar, PartialUnmapAndWrite) {
  // Map a two-page VMO.
  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size() * 2, 0, &vmo), ZX_OK);

  // Use a sub-VMAR so that unmapped addresses can remain reserved.
  uintptr_t vmar_base;
  zx_handle_t vmar;
  ASSERT_OK(zx_vmar_allocate(zx_vmar_root_self(), ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             zx_system_get_page_size() * 2, &vmar, &vmar_base));
  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR later.
    EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  });

  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size() * 2, &mapping_addr),
            ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  char* ptr = (char*)mapping_addr;
  memset(ptr, 0, zx_system_get_page_size() * 2);

  // Unmap the second page.
  ASSERT_OK(
      zx_vmar_unmap(vmar, mapping_addr + zx_system_get_page_size(), zx_system_get_page_size()));

  char buffer[zx_system_get_page_size() * 2];
  size_t actual_written;
  memset(buffer, 0, zx_system_get_page_size() * 2);

  // First page succeeds.
  EXPECT_EQ(zx_process_write_memory(zx_process_self(), mapping_addr, buffer,
                                    zx_system_get_page_size(), &actual_written),
            ZX_OK);
  EXPECT_EQ(actual_written, zx_system_get_page_size());

  // Second page fails.
  EXPECT_STATUS(zx_process_write_memory(zx_process_self(), mapping_addr + zx_system_get_page_size(),
                                        buffer, zx_system_get_page_size(), &actual_written),
                ZX_ERR_NOT_FOUND);

  // Writing to the whole region succeeds, but only writes the first page.
  EXPECT_EQ(zx_process_write_memory(zx_process_self(), mapping_addr, buffer,
                                    zx_system_get_page_size() * 2, &actual_written),
            ZX_OK);
  EXPECT_EQ(actual_written, zx_system_get_page_size());

  // Write at the boundary straddling the pages.
  EXPECT_EQ(zx_process_write_memory(zx_process_self(), mapping_addr + zx_system_get_page_size() - 1,
                                    buffer, 2, &actual_written),
            ZX_OK);
  EXPECT_EQ(actual_written, 1);

  // Unmap the left over first page.
  EXPECT_EQ(zx_vmar_unmap(vmar, mapping_addr, zx_system_get_page_size()), ZX_OK);
}

TEST(Vmar, PartialUnmapWithVmarOffset) {
  constexpr size_t kOffset = 0x1000;
  const size_t kVmoSize = zx_system_get_page_size() * 10;
  // Map a VMO, using an offset into the VMO.
  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(kVmoSize, 0, &vmo), ZX_OK);

  // Use a sub-VMAR so that we can test accesses beyond the mapped range.
  uintptr_t vmar_base;
  zx_handle_t vmar;
  ASSERT_OK(zx_vmar_allocate(zx_vmar_root_self(),
                             ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                             kVmoSize, &vmar, &vmar_base));
  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR later.
    EXPECT_EQ(zx_vmar_destroy(vmar), ZX_OK);
  });

  uintptr_t mapping_addr;
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, kOffset,
                        kVmoSize - kOffset, &mapping_addr),
            ZX_OK);

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  char* ptr = (char*)mapping_addr;
  memset(ptr, 0, kVmoSize - kOffset);

  // Make sure both reads and writes to both the beginning and the end are allowed.
  char buffer[kVmoSize - kOffset];
  size_t actual;
  EXPECT_EQ(
      zx_process_write_memory(zx_process_self(), mapping_addr, buffer, kVmoSize - kOffset, &actual),
      ZX_OK);
  EXPECT_EQ(actual, kVmoSize - kOffset);

  EXPECT_EQ(
      zx_process_read_memory(zx_process_self(), mapping_addr, buffer, kVmoSize - kOffset, &actual),
      ZX_OK);
  EXPECT_EQ(actual, kVmoSize - kOffset);

  // That reads and writes right at the end are OK.
  EXPECT_EQ(zx_process_write_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset - 1,
                                    buffer, 1, &actual),
            ZX_OK);
  EXPECT_EQ(zx_process_read_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset - 1, buffer,
                                   1, &actual),
            ZX_OK);

  // That reads and writes one past the end fail.
  EXPECT_STATUS(zx_process_write_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset,
                                        buffer, 1, &actual),
                ZX_ERR_NOT_FOUND);
  EXPECT_STATUS(zx_process_read_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset, buffer,
                                       1, &actual),
                ZX_ERR_NOT_FOUND);

  // And crossing the boundary works as expected.
  EXPECT_EQ(zx_process_write_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset - 1,
                                    buffer, 2, &actual),
            ZX_OK);
  EXPECT_EQ(actual, 1);
  EXPECT_EQ(zx_process_read_memory(zx_process_self(), mapping_addr + kVmoSize - kOffset - 1, buffer,
                                   2, &actual),
            ZX_OK);
  EXPECT_EQ(actual, 1);
}

TEST(Vmar, AllowFaultsTest) {
  // No-op test that checks the current default behavior.
  // TODO(stevensd): Add meaningful tests once the flag is actually implemented.
  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo), ZX_OK);
  uintptr_t mapping_addr;
  ASSERT_EQ(
      zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS, 0,
                  vmo, 0, zx_system_get_page_size(), &mapping_addr),
      ZX_OK);
  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  EXPECT_EQ(zx_vmar_unmap(zx_vmar_root_self(), mapping_addr, zx_system_get_page_size()), ZX_OK);
}

// Regression test for a scenario where process_read_memory could use a stale RefPtr<VmObject>
// This will not always detect the failure scenario, but will never false positive.
TEST(Vmar, ConcurrentUnmapReadMemory) {
  auto root_vmar = zx::vmar::root_self();

  zx::vmar child_vmar;
  uintptr_t addr;
  ASSERT_EQ(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ, 0,
                                zx_system_get_page_size() * 4, &child_vmar, &addr),
            ZX_OK);

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR later.
    child_vmar.destroy();
  });

  std::atomic<bool> running = true;
  std::thread t = std::thread([addr, &running] {
    auto self = zx::process::self();
    while (running) {
      uint64_t data;
      size_t temp;
      self->read_memory(addr, &data, sizeof(data), &temp);
    }
  });

  // Iterate some number of times to attempt to hit the race condition. This is a best effort and
  // even when the bug is present it could take minutes of running to trigger it.
  for (int i = 0; i < 1000; i++) {
    uintptr_t temp;
    // vmo must be created in the loop so that it is destroyed each iteration leading to there
    // being no references to the underlying VmObject in the kernel.
    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
    ASSERT_EQ(child_vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(),
                             &temp),
              ZX_OK);
    ASSERT_EQ(child_vmar.unmap(addr, zx_system_get_page_size()), ZX_OK);
  }

  running = false;
  t.join();
}

// Test for bug https://fxbug.dev/42069384.
// Issuing a vmar operation on a range that is next to a vmar should not crash.
TEST(Vmar, OpOnRangeWithChildVmarNextToIt) {
  auto root_vmar = zx::vmar::root_self();

  const size_t kVmoSize = zx_system_get_page_size();
  // VMAR topology:
  // [   PARENT VMAR     ]
  // [[    VMAR_A       ]]
  // [[ VMO_A [ VMAR_B ]]]

  zx::vmo vmo_a;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo_a));

  zx::vmar vmar_parent;
  uintptr_t vmar_parent_base;
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmoSize * 2, &vmar_parent, &vmar_parent_base));

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR.
    vmar_parent.destroy();
  });

  zx::vmar vmar_a;
  uintptr_t vmar_a_base;
  ASSERT_OK(vmar_parent.allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                 0, kVmoSize * 2, &vmar_a, &vmar_a_base));

  zx::vmar vmar_b;
  uintptr_t vmar_b_base;
  ASSERT_OK(vmar_a.allocate(
      ZX_VM_SPECIFIC | ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0x1000,
      kVmoSize, &vmar_b, &vmar_b_base));

  uintptr_t vmo_a_addr;
  ASSERT_OK(vmar_a.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo_a, 0, kVmoSize,
                       &vmo_a_addr));

  // Doing an operation on a vmar on a range that has a child vmar next to it
  // should work.
  EXPECT_OK(vmar_parent.op_range(ZX_VMAR_OP_MAP_RANGE, vmo_a_addr, kVmoSize, nullptr, 0));
}

// Test DECOMMIT on a vmar with two non-contiguous mappings (https://fxbug.dev/42147277)
TEST(Vmar, RangeOpCommitVmoPages2) {
  auto root_vmar = zx::vmar::root_self();

  // Create a VMO and VMAR large enough to support two multipage mappings.
  const size_t kVmoSize = zx_system_get_page_size() * 10;
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_vmo_create(kVmoSize, 0, &vmo), ZX_OK);

  zx_vaddr_t vmar_base = 0u;
  zx_handle_t vmar = ZX_HANDLE_INVALID;
  ASSERT_EQ(zx_vmar_allocate(zx_vmar_root_self(),
                             ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                             kVmoSize, &vmar, &vmar_base),
            ZX_OK);

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR.
    zx_vmar_destroy(vmar);
  });

  // Create one mapping in the VMAR
  const size_t kMappingSize = 5 * zx_system_get_page_size();
  zx_vaddr_t mapping_addr = 0u;
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        kMappingSize, &mapping_addr),
            ZX_OK);
  ASSERT_EQ(vmar_base, mapping_addr);

  // Create a second mapping in the VMAR, with one unmapped page separting this from the prior
  // mapping
  const size_t kMappingSize2 = 4 * zx_system_get_page_size();
  zx_vaddr_t mapping_addr2 = 0u;
  ASSERT_EQ(zx_vmar_map(vmar, ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                        kMappingSize + zx_system_get_page_size(), vmo,
                        zx_system_get_page_size() * 5, kMappingSize2, &mapping_addr2),
            ZX_OK);
  ASSERT_NE(mapping_addr, mapping_addr2);

  memset(reinterpret_cast<void*>(mapping_addr), 0x0, kMappingSize2);
  memset(reinterpret_cast<void*>(mapping_addr2), 0x0, kMappingSize2);

  // Decommit the second mapping; the presence of the first mapping should not cause the decommit op
  // to panic or to be invoked on the wrong range.
  EXPECT_EQ(zx_vmar_op_range(vmar, ZX_VMAR_OP_DECOMMIT, mapping_addr2, kMappingSize2, nullptr, 0u),
            ZX_OK);
}

// Test that commits and decommits are not allowed through a nested vmar.
TEST(Vmar, BadRangeOpNestedVmar) {
  auto root_vmar = zx::vmar::root_self();

  // Create an intermediate vmar.

  zx::vmar intermediate_vmar;
  uintptr_t addr;
  ASSERT_EQ(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, zx_system_get_page_size() * 8, &intermediate_vmar, &addr),
            ZX_OK);

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR.
    intermediate_vmar.destroy();
  });

  // Place mapping in the intermediate vmar.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  uint64_t mapping_addr;
  ASSERT_OK(intermediate_vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                  zx_system_get_page_size(), vmo, 0, zx_system_get_page_size(),
                                  &mapping_addr));

  // Commit and decommit ops should not be allowed on the root vmar for this range.
  EXPECT_EQ(
      root_vmar->op_range(ZX_VMAR_OP_COMMIT, mapping_addr, zx_system_get_page_size(), nullptr, 0),
      ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(
      root_vmar->op_range(ZX_VMAR_OP_DECOMMIT, mapping_addr, zx_system_get_page_size(), nullptr, 0),
      ZX_ERR_INVALID_ARGS);
}

// Test zx_vmar_op_range ZX_VMAR_OP_COMMIT.
TEST(Vmar, RangeOpCommit) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 20 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));

  auto destroy = fit::defer([&]() {
    // Cleanup the VMAR.
    vmar.destroy();
  });

  // Create two sub-VMARs to hold the mappings.
  zx::vmar sub_vmar1, sub_vmar2;
  zx_vaddr_t base_addr1, base_addr2;
  const uint64_t kSubVmarSize = 8 * zx_system_get_page_size();
  ASSERT_OK(vmar.allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
      zx_system_get_page_size(), kSubVmarSize, &sub_vmar1, &base_addr1));
  ASSERT_EQ(base_addr1, base_addr + zx_system_get_page_size());
  ASSERT_OK(vmar.allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC,
      kSubVmarSize + 2 * zx_system_get_page_size(), kSubVmarSize, &sub_vmar2, &base_addr2));
  ASSERT_EQ(base_addr2, base_addr1 + kSubVmarSize + zx_system_get_page_size());

  // Create a VMO and clone it.
  zx::vmo vmo, clone;
  const uint64_t kVmoSize = 5 * zx_system_get_page_size();
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, &clone));

  // Map the VMO and its clone.
  zx_vaddr_t addr1, addr2;
  ASSERT_OK(sub_vmar1.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, kVmoSize,
                          &addr1));
  ASSERT_EQ(base_addr1, addr1);
  ASSERT_OK(sub_vmar2.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone, 0,
                          kVmoSize, &addr2));
  ASSERT_EQ(base_addr2, addr2);

  // Commit pages 1 and 2 in the parent.
  ASSERT_OK(sub_vmar1.op_range(ZX_VMAR_OP_COMMIT, addr1 + zx_system_get_page_size(),
                               2 * zx_system_get_page_size(), nullptr, 0));
  // Commit pages 2 and 3 in the clone.
  ASSERT_OK(sub_vmar2.op_range(ZX_VMAR_OP_COMMIT, addr2 + 2 * zx_system_get_page_size(),
                               2 * zx_system_get_page_size(), nullptr, 0));

  // The VMOs should have at most two pages committed. They might have less since its possible for
  // reclamation to have already happened.
  zx_info_vmo_t info;
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(2 * zx_system_get_page_size(), info.populated_bytes);
  ASSERT_OK(clone.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(2 * zx_system_get_page_size(), info.populated_bytes);

  // Commit all pages in the clone.
  ASSERT_OK(sub_vmar2.op_range(ZX_VMAR_OP_COMMIT, addr2, kVmoSize, nullptr, 0));

  // The clone now might have all its pages committed, but the parent should still be capped at 2.
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(2 * zx_system_get_page_size(), info.populated_bytes);
  ASSERT_OK(clone.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(kVmoSize, info.populated_bytes);

  // Map a single page as read-only and try to commit it. The commit should fail.
  zx::vmo readonly_vmo;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_MAP | ZX_RIGHT_READ, &readonly_vmo), ZX_OK);
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, kVmarSize - zx_system_get_page_size(),
                     readonly_vmo, 0, zx_system_get_page_size(), &addr));
  ASSERT_EQ(base_addr + kVmarSize - zx_system_get_page_size(), addr);
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED,
            vmar.op_range(ZX_VMAR_OP_COMMIT, addr, zx_system_get_page_size(), nullptr, 0));

  // The commit counts should not have changed (modulo any reclamation).
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(2 * zx_system_get_page_size(), info.populated_bytes);
  ASSERT_OK(clone.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_GE(kVmoSize, info.populated_bytes);

  // Some trivial failure cases.
  // Out of range.
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr, 2 * kVmarSize, nullptr, 0));
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, vmar.op_range(ZX_VMAR_OP_COMMIT, 0, kVmarSize, nullptr, 0));
  // Various combinations of gaps.
  ASSERT_EQ(ZX_ERR_BAD_STATE, vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr, kVmarSize, nullptr, 0));
  ASSERT_EQ(ZX_ERR_BAD_STATE, vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr,
                                            base_addr1 + kSubVmarSize - base_addr, nullptr, 0));
}

TEST(Vmar, ProtectCowWritable) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));

  uint64_t val = 42;
  EXPECT_OK(vmo.write(&val, 0, sizeof(uint64_t)));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &clone));

  // Map the clone in read/write.
  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone, 0,
                                       zx_system_get_page_size() * 2, &addr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(addr, zx_system_get_page_size() * 2);
  });

  // Protect it read-only.
  EXPECT_OK(zx::vmar::root_self()->protect(ZX_VM_PERM_READ, addr, zx_system_get_page_size() * 2));

  // Perform some reads to ensure there are mappings.
  uint64_t val2 = *reinterpret_cast<uint64_t*>(addr);
  EXPECT_OK(clone.read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(val2, val);

  // Now protect the first page back to write.
  EXPECT_OK(zx::vmar::root_self()->protect(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, addr,
                                           zx_system_get_page_size()));

  // Write to the page.
  *reinterpret_cast<volatile uint64_t*>(addr) = 77;

  // Original vmo should be unchanged.
  EXPECT_OK(vmo.read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(42, val);

  // Clone should have been modified.
  EXPECT_OK(clone.read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(77, val);
}

TEST(Vmar, MapReadIfXomUnsupported) {
  zx_handle_t vmo;
  size_t size = zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);

  uintptr_t addr;
  ASSERT_EQ(
      zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED, 0, vmo, 0, size, &addr),
      ZX_OK);
  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(addr, size);
  });

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  uint32_t features = 0;
  ASSERT_EQ(zx_system_get_features(ZX_FEATURE_KIND_VM, &features), ZX_OK);
  bool xomUnsupported = !(features & ZX_VM_FEATURE_CAN_MAP_XOM);

  const zx_status_t expected = xomUnsupported ? ZX_OK : ZX_ERR_ACCESS_DENIED;
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr)), expected);
}

TEST(Vmar, ProtectReadIfXomUnsupported) {
  zx_handle_t vmo;
  size_t size = zx_system_get_page_size();
  ASSERT_EQ(zx_vmo_create(size, 0, &vmo), ZX_OK);

  uintptr_t addr;
  ASSERT_EQ(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, size, &addr), ZX_OK);
  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(addr, size);
  });

  EXPECT_EQ(zx_handle_close(vmo), ZX_OK);

  ASSERT_OK(probe_for_read(reinterpret_cast<void*>(addr)));

  ASSERT_EQ(zx_vmar_protect(zx_vmar_root_self(), ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED, addr, size),
            ZX_OK);

  uint32_t features = 0;
  ASSERT_EQ(zx_system_get_features(ZX_FEATURE_KIND_VM, &features), ZX_OK);
  bool xomUnsupported = !(features & ZX_VM_FEATURE_CAN_MAP_XOM);

  const zx_status_t expected = xomUnsupported ? ZX_OK : ZX_ERR_ACCESS_DENIED;
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr)), expected);
}

// Check that map range works correctly on resizable VMOs when the mapping extends beyond the
// current VMO size.
TEST(Vmar, MapRangeResizable) {
  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));

  zx_vaddr_t vaddr = 0;
  // In range
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_MAP_RANGE, 0, vmo, 0, zx_system_get_page_size(), &vaddr));
  zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size());
  // Extend out of range
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_MAP_RANGE | ZX_VM_PERM_READ, 0, vmo, 0,
                                       zx_system_get_page_size() * 2, &vaddr));

  // Mapping should be trimmed (can't read past end of VMO).
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(vaddr + zx_system_get_page_size())),
                ZX_ERR_OUT_OF_RANGE);
  zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size() * 2);

  // Start and end fully out of range
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_MAP_RANGE, 0, vmo, zx_system_get_page_size() * 2,
                                       zx_system_get_page_size() * 2, &vaddr));
  zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size() * 2);

  // Resize VMO up to 2 pages & write to both.
  vmo.set_size(_zx_system_get_page_size() * 2);

  // Map both pages.
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_MAP_RANGE | ZX_VM_PERM_READ, 0, vmo, 0,
                                       zx_system_get_page_size() * 2, &vaddr));

  // Resize VMO back down to 1.
  vmo.set_size(zx_system_get_page_size());

  // Read from both pages, the nonexistent page should fault.
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(vaddr)), ZX_OK);
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(vaddr + zx_system_get_page_size())),
                ZX_ERR_OUT_OF_RANGE);
}

// Prefetch on an anonymous VMO only has an effect of decompressing pages, which we cannot trigger
// and check here in userspace, so this test just validates general syscall errors.
TEST(Vmar, Prefetch) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));

  // Map the VMO into a sub vmar, making the sub vmar slightly larger to ensure a fault gap.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 4, &vmar, &vmar_addr));
  auto destroy = fit::defer([&]() {
    // Cleanup the vmar.
    vmar.destroy();
  });

  zx_vaddr_t addr;
  EXPECT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, zx_system_get_page_size(),
                     vmo, 0, zx_system_get_page_size() * 2, &addr));
  EXPECT_EQ(vmar_addr + zx_system_get_page_size(), addr);

  // Correct ranges.
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size(), nullptr, 0));
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size() * 2, nullptr, 0));
  // Zero length is an error.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, 0, nullptr, 0));
  // Unaligned ranges generate an error.
  EXPECT_EQ(
      ZX_ERR_INVALID_ARGS,
      vmar.op_range(ZX_VMAR_OP_PREFETCH, addr + zx_system_get_page_size() - 1, 2, nullptr, 0));
  // Out of range of the entire vmar gives out of range.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar.op_range(ZX_VMAR_OP_PREFETCH, addr + zx_system_get_page_size() * 2,
                          zx_system_get_page_size() * 2, nullptr, 0));
  // Out of range of the mapping, but into the unmapped portion is a bad state.
  EXPECT_EQ(ZX_ERR_BAD_STATE,
            vmar.op_range(ZX_VMAR_OP_PREFETCH, addr + zx_system_get_page_size() * 2,
                          zx_system_get_page_size(), nullptr, 0));

  // Operating on the root vmar should fail as prefetch can never operate on child mappings.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            zx::vmar::root_self()->op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size(),
                                            nullptr, 0));

  // Protecting the mapping to not have the read permissions does not prevent prefetch.
  EXPECT_OK(vmar.protect(0, addr, zx_system_get_page_size() * 2));
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size() * 2, nullptr, 0));
}

// Tests general functionality of mapping a VMO with ZX_VM_FAULT_BEYOND_STREAM_SIZE option.
TEST(Vmar, FaultBeyondStreamSize) {
  // 6 page VMAR.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 6, &vmar, &vmar_addr));

  // Unbounded VMO with stream size of 3 pages.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 3, ZX_VMO_UNBOUNDED, &vmo));

  uint64_t stream_size = 0;
  uint64_t vmo_size = 0;

  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 3);
  EXPECT_OK(vmo.get_size(&vmo_size));
  EXPECT_GT(vmo_size, stream_size);

  // ZX_VM_ALLOW_FAULTS is required with ZX_VM_FAULT_BEYOND_STREAM_SIZE.
  zx_vaddr_t addr;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                                              ZX_VM_FAULT_BEYOND_STREAM_SIZE,
                                          0, vmo, 0, zx_system_get_page_size() * 4, &addr));

  // 4 page, fault-beyond-stream-size mapping.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                         ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                     0, vmo, 0, zx_system_get_page_size() * 4, &addr));

  // Memory access within stream size is OK.
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr)));

  // Memory access inside mapping but outside of stream size should fault.
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "write");

  // Write to the first 4 pages of the VMO (one page outside of the stream size).
  char buffer1[zx_system_get_page_size() * 4];
  memset(buffer1, 0, zx_system_get_page_size() * 4);
  EXPECT_OK(vmo.write(buffer1, 0, zx_system_get_page_size() * 4));

  char vmo_buffer1[zx_system_get_page_size() * 4];
  EXPECT_OK(vmo.read(vmo_buffer1, 0, zx_system_get_page_size() * 4));
  EXPECT_BYTES_EQ(buffer1, vmo_buffer1, zx_system_get_page_size() * 4);

  // Should still fault beyond stream size.
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  // Pages still in VMO.
  EXPECT_OK(vmo.read(vmo_buffer1, 0, zx_system_get_page_size() * 4));
  EXPECT_BYTES_EQ(buffer1, vmo_buffer1, zx_system_get_page_size() * 4);

  // Can write from mapping.
  EXPECT_OK(probe_for_write(reinterpret_cast<void*>(addr)));

  // Reduce stream size to 2.
  vmo.set_stream_size(2 * _zx_system_get_page_size());

  // Range in between old & new stream size should now fault.
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "write");
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  // Unbounded VMO with stream size of 0.5 pages.
  zx::vmo vmo2;
  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() / 2), ZX_VMO_UNBOUNDED, &vmo2));

  EXPECT_OK(vmo2.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() / 2);
  EXPECT_OK(vmo2.get_size(&vmo_size));
  EXPECT_GT(vmo_size, stream_size);

  // Can't overwrite mapping without overwrite flag.
  zx_vaddr_t addr2;
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, vmar.map(ZX_VM_PERM_READ | ZX_VM_SPECIFIC |
                                                ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                                            0, vmo2, 0, zx_system_get_page_size() * 2, &addr2));

  // 2 page mapping with overwrite and no write perms, will overwrite first 2 pages of existing 4
  // page mapping.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_SPECIFIC_OVERWRITE | ZX_VM_FAULT_BEYOND_STREAM_SIZE |
                         ZX_VM_ALLOW_FAULTS,
                     0, vmo2, 0, zx_system_get_page_size() * 2, &addr2));

  // Check that the previous mapping was overwritten & can no longer write.
  EXPECT_EQ(addr, addr2);
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(addr)), ZX_ERR_ACCESS_DENIED, "write");

  // Memory accesses are ok before the stream size and past the stream size but on the page
  // containing the stream size.
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr2)));
  EXPECT_OK(probe_for_read(
      reinterpret_cast<void*>(addr2 + zx_system_get_page_size() - sizeof(zx_vaddr_t))));

  // Memory accesses on pages past the stream-size containing page fault.
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr2 + zx_system_get_page_size())),
                ZX_ERR_OUT_OF_RANGE, "read");

  vmar.destroy();

  // Fresh, 6 page VMAR.
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 6, &vmar, &vmar_addr));

  // Unbounded VMO with stream size of 2 pages.
  zx::vmo vmo3;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, ZX_VMO_UNBOUNDED, &vmo3));

  EXPECT_OK(vmo3.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 2);
  EXPECT_OK(vmo3.get_size(&vmo_size));
  EXPECT_GT(vmo_size, stream_size);

  // Write to first 4 pages of VMO.
  char buffer3[zx_system_get_page_size() * 4];
  memset(buffer3, 0, zx_system_get_page_size() * 4);
  EXPECT_OK(vmo3.write(buffer3, 0, zx_system_get_page_size() * 4));

  char vmo_buffer3[zx_system_get_page_size() * 4];
  EXPECT_OK(vmo3.read(vmo_buffer3, 0, zx_system_get_page_size() * 4));
  EXPECT_BYTES_EQ(buffer3, vmo_buffer3, zx_system_get_page_size() * 4);

  // 6 Page mapping of VMO.
  zx_vaddr_t addr3;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                         ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                     0, vmo3, 0, zx_system_get_page_size() * 6, &addr3));

  // Should fault beyond stream size even though VMO has pages.
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr3 + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  // Increase stream size to 4 pages & check memory access in new range.
  vmo3.set_stream_size(_zx_system_get_page_size() * 4);
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr3 + (2 * zx_system_get_page_size()))));

  // Decrease back to 2 pages, which should cause that range to fault.
  vmo3.set_stream_size(_zx_system_get_page_size() * 2);
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr3 + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  vmar.destroy();
}

// Tests that reducing the stream size will remove mappings & the range between the new (smaller)
// stream size and the old (larger) stream size continues to fault.
TEST(Vmar, FaultBeyondReducedStreamSize) {
  // 6 page VMAR.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 6, &vmar, &vmar_addr));

  // Unbounded VMO with stream size of 5 pages.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 5, ZX_VMO_UNBOUNDED, &vmo));

  zx_vaddr_t addr;

  // 6 page, fault-beyond-stream-size mapping.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                         ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                     0, vmo, 0, zx_system_get_page_size() * 6, &addr));

  // Write to 5 pages using the mapping address.
  *reinterpret_cast<uint64_t*>(addr) = 0xdeadbeef;
  *reinterpret_cast<uint64_t*>(addr + zx_system_get_page_size()) = 0xdeadbeef;
  *reinterpret_cast<uint64_t*>(addr + (2 * zx_system_get_page_size())) = 0xdeadbeef;
  *reinterpret_cast<uint64_t*>(addr + (3 * zx_system_get_page_size())) = 0xdeadbeef;
  *reinterpret_cast<uint64_t*>(addr + (4 * zx_system_get_page_size())) = 0xdeadbeef;

  // Write on last page (beyond stream size) should fault
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(addr + (5 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "write");

  // Check some pages in VMO.
  uint64_t val;
  EXPECT_OK(vmo.read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(0xdeadbeef, val);
  EXPECT_OK(vmo.read(&val, 4 * zx_system_get_page_size(), sizeof(uint64_t)));
  EXPECT_EQ(0xdeadbeef, val);

  // Size stream down -1 page, which should cause trimmed page to fault.
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr + (4 * zx_system_get_page_size()))),
            "read");
  vmo.set_stream_size(4 * zx_system_get_page_size());
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (4 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  // Write data to the end of the 4th page.
  *reinterpret_cast<uint64_t*>(addr + (4 * zx_system_get_page_size() - sizeof(uint64_t))) =
      0xdeadbeef;

  // Size stream down -0.5 pages, which should not cause the trimmed page to fault.
  vmo.set_stream_size((3 * zx_system_get_page_size()) + zx_system_get_page_size() / 2);
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))));

  // Trimmed part of full page should be zeroed in VMO.
  EXPECT_OK(vmo.read(&val, (3 * zx_system_get_page_size() + (zx_system_get_page_size() / 2)),
                     sizeof(uint64_t)));
  EXPECT_EQ(0, val);
  // Page containing stream size should retain data within the stream size.
  EXPECT_OK(vmo.read(&val, 3 * zx_system_get_page_size(), sizeof(uint64_t)));
  EXPECT_EQ(0xdeadbeef, val);
  // Data passed the trimmed stream size but on the same should be zeroed, even though it doesn't
  // fault.
  EXPECT_OK(vmo.read(&val, (4 * zx_system_get_page_size()) - sizeof(uint64_t), sizeof(uint64_t)));
  EXPECT_EQ(0, val);

  // Use legacy set_prop_content_size to resize the stream size down to 2 pages (trimming 1.5
  // pages) and check that the trimmed pages fault.
  EXPECT_OK(vmo.set_prop_content_size(2 * _zx_system_get_page_size()));

  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  // First two pages ok.
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr + zx_system_get_page_size())), "read");
  EXPECT_OK(probe_for_read(reinterpret_cast<void*>(addr)), "read");

  vmar.destroy();

  // 4 page VMAR.
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 4, &vmar, &vmar_addr));

  // 4 page VMO without unbounded flag.
  zx::vmo vmo_bounded;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0u, &vmo_bounded));

  // Map whole VMO.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                         ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                     0, vmo_bounded, 0, zx_system_get_page_size() * 4, &addr));

  // Reduce stream size to 2 pages.
  vmo_bounded.set_stream_size(_zx_system_get_page_size() * 2);

  // Write to all pages.
  char buffer[zx_system_get_page_size() * 4];
  memset(buffer, 1, zx_system_get_page_size() * 4);
  EXPECT_OK(vmo.write(buffer, 0, zx_system_get_page_size() * 4));

  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (3 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");
}

// Test zx_vmar_op_range operations on a ZX_VM_FAULT_BEYOND_STREAM_SIZE mapping.
TEST(Vmar, FaultBeyondStreamSizeRange) {
  // 6 page VMAR.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 6, &vmar, &vmar_addr));

  // Unbounded VMO with stream size of 2 pages.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, ZX_VMO_UNBOUNDED, &vmo));

  // Write to 4 pages of VMO (2 pages past the stream size).
  char buffer[zx_system_get_page_size() * 4];
  memset(buffer, 1, zx_system_get_page_size() * 4);
  EXPECT_OK(vmo.write(buffer, 0, zx_system_get_page_size() * 4));

  char vmo_buffer[zx_system_get_page_size() * 4];
  EXPECT_OK(vmo.read(vmo_buffer, 0, zx_system_get_page_size() * 4));
  EXPECT_BYTES_EQ(buffer, vmo_buffer, zx_system_get_page_size() * 4);

  zx_vaddr_t addr;

  // 6 page mapping of VMO with ZX_VM_MAP_RANGE option.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                         ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS | ZX_VM_MAP_RANGE,
                     0, vmo, 0, zx_system_get_page_size() * 6, &addr));

  // Prefetch, commit and decommit should fail with out-of-range error as gaps are not allowed.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size() * 3, nullptr, 0));
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");

  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar.op_range(ZX_VMAR_OP_COMMIT, addr, zx_system_get_page_size() * 3, nullptr, 0));
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "read");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void*>(addr + (2 * zx_system_get_page_size()))),
                ZX_ERR_OUT_OF_RANGE, "write");

  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar.op_range(ZX_VMAR_OP_DECOMMIT, addr, zx_system_get_page_size() * 3, nullptr, 0));
  EXPECT_OK(vmo.read(vmo_buffer, 0, zx_system_get_page_size() * 4));
  EXPECT_BYTES_EQ(buffer, vmo_buffer, zx_system_get_page_size() * 4);

  // Can prefetch, commit and decommit up to the stream size.
  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_PREFETCH, addr, zx_system_get_page_size() * 2, nullptr, 0));

  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_COMMIT, addr, zx_system_get_page_size() * 2, nullptr, 0));

  EXPECT_OK(vmar.op_range(ZX_VMAR_OP_DECOMMIT, addr, zx_system_get_page_size() * 2, nullptr, 0));

  // Out of range error when op length length = stream length, but not from start of mapping.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmar.op_range(ZX_VMAR_OP_COMMIT, addr + zx_system_get_page_size(),
                                               zx_system_get_page_size() * 2, nullptr, 0));

  char buffer_half[zx_system_get_page_size() * 4];
  memset(buffer_half, 1, zx_system_get_page_size() * 4);
  char zero_buffer_half[zx_system_get_page_size() * 2];
  memset(zero_buffer_half, 0, zx_system_get_page_size() * 2);

  char vmo_buffer_half[zx_system_get_page_size() * 2];
  EXPECT_OK(vmo.read(vmo_buffer_half, 0, zx_system_get_page_size() * 2));
  EXPECT_BYTES_EQ(vmo_buffer_half, zero_buffer_half, zx_system_get_page_size() * 2);

  EXPECT_OK(
      vmo.read(vmo_buffer_half, zx_system_get_page_size() * 2, zx_system_get_page_size() * 2));
  EXPECT_BYTES_EQ(vmo_buffer_half, buffer_half, zx_system_get_page_size() * 2);
}

TEST(Vmar, SpecificOverwriteMergeMappings) {
  size_t page_size = zx_system_get_page_size();

  // Allocate a 16-pages VMAR.
  zx::vmar vmar;
  zx_vaddr_t vmar_base;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0, 6 * page_size, &vmar,
      &vmar_base));

  // Create a "placeholder" empty VMO and another VMO with 2 pages.
  zx::vmo placeholder, vmo;
  ASSERT_OK(zx::vmo::create(0, 0, &placeholder));
  placeholder.set_property(ZX_PROP_NAME, "placeholder", 11);
  ASSERT_OK(zx::vmo::create(2 * page_size, ZX_VMO_RESIZABLE, &vmo));
  vmo.set_property(ZX_PROP_NAME, "example", 7);

  // Reserve 4 pages by mapping the placeholder.
  zx_vaddr_t base_addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS | ZX_VM_SPECIFIC, 0,
                     placeholder, 0, 4 * page_size, &base_addr));

  // Map 3 pages of the VMO at `base_addr + PageSize`.
  zx_vaddr_t addr1;
  ASSERT_OK(
      vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE | ZX_VM_ALLOW_FAULTS,
               base_addr + page_size - vmar_base, vmo, 0, 3 * page_size, &addr1));

  // Map two more pages of the VMO right after (`base_addr + PageSize + 3 * PageSize`).
  zx_vaddr_t addr2;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_ALLOW_FAULTS,
                     base_addr + (4 * page_size) - vmar_base, vmo, 3 * page_size, 2 * page_size,
                     &addr2));

  // Resize VMO to make its size equal to 4 pages.
  EXPECT_OK(vmo.set_size(4 * page_size));

  // Fetch the mapping info
  // Should be exactly three entries, these being our VMAR, remainder of the placeholder mapping and
  // a single merged mapping to the final vmo.
  size_t actual, avail;
  zx_info_maps_t maps[3];
  ASSERT_OK(vmar.get_info(ZX_INFO_VMAR_MAPS, maps, 3 * sizeof(zx_info_maps_t), &actual, &avail));
  ASSERT_EQ(actual, 3u);

  EXPECT_EQ(maps[0].type, ZX_INFO_MAPS_TYPE_VMAR);
  EXPECT_EQ(maps[1].type, ZX_INFO_MAPS_TYPE_MAPPING);
  EXPECT_EQ(maps[1].size, page_size);
  EXPECT_EQ(maps[2].type, ZX_INFO_MAPS_TYPE_MAPPING);
  EXPECT_EQ(maps[2].size, page_size * 5);
}

// Test to validate that VmMapping::ForceWritable behaves correctly, triggered by using
// zx_process_write_memory.
TEST(Vmar, WriteReadOnlyMemory) {
  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::vmo read_vmo;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ | ZX_RIGHT_MAP, &read_vmo));
  vmo.reset();

  uintptr_t addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, read_vmo, 0, zx_system_get_page_size(),
                                       &addr));
  read_vmo.reset();
  auto unmap = fit::defer([&]() { zx::vmar::root_self()->unmap(addr, zx_system_get_page_size()); });

  size_t actual = 0;
  EXPECT_OK(zx::process::self()->write_memory(addr, &addr, 8, &actual));
}

// Test that zx_vmar_op_range checks handle rights before allowing operation to proceed.
TEST(Vmar, OpRangeRights) {
  // Limit interference between operations by providing every op with a separate vmar.
  struct Setup {
    zx::vmar no_rights, read_only, read_write;
    zx_vaddr_t addr;
    fit::deferred_action<fit::closure> cleanup;
  };

  auto make_one_page_vmar = []() -> zx::result<Setup> {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(zx_system_get_page_size(), 0, &vmo);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    zx::vmar sub;
    zx_vaddr_t addr;
    status = zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                                             zx_system_get_page_size(), &sub, &addr);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    status =
        sub.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, zx_system_get_page_size(), &addr);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    zx::vmar sub_no_rights, sub_read_only, sub_read_write;
    status = sub.duplicate(0, &sub_no_rights);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    status = sub.duplicate(ZX_RIGHT_READ, &sub_read_only);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    status = sub.duplicate(ZX_RIGHT_READ | ZX_RIGHT_WRITE, &sub_read_write);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok(Setup{
        .no_rights = std::move(sub_no_rights),
        .read_only = std::move(sub_read_only),
        .read_write = std::move(sub_read_write),
        .addr = addr,
        .cleanup = fit::defer(
            fit::closure([sub = std::move(sub)]() { ZX_ASSERT(ZX_OK == sub.destroy()); })),
    });
  };

  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->no_rights.op_range(ZX_VMAR_OP_COMMIT, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->read_only.op_range(ZX_VMAR_OP_COMMIT, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_OK(
        s->read_write.op_range(ZX_VMAR_OP_COMMIT, s->addr, zx_system_get_page_size(), nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->no_rights.op_range(ZX_VMAR_OP_DECOMMIT, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->read_only.op_range(ZX_VMAR_OP_DECOMMIT, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_OK(s->read_write.op_range(ZX_VMAR_OP_DECOMMIT, s->addr, zx_system_get_page_size(),
                                     nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_STATUS(ZX_ERR_ACCESS_DENIED,
                  s->no_rights.op_range(ZX_VMAR_OP_MAP_RANGE, s->addr, zx_system_get_page_size(),
                                        nullptr, 0));
    EXPECT_OK(s->read_only.op_range(ZX_VMAR_OP_MAP_RANGE, s->addr, zx_system_get_page_size(),
                                    nullptr, 0));
    EXPECT_OK(s->read_write.op_range(ZX_VMAR_OP_MAP_RANGE, s->addr, zx_system_get_page_size(),
                                     nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->no_rights.op_range(ZX_VMAR_OP_ZERO, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->read_only.op_range(ZX_VMAR_OP_ZERO, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_OK(
        s->read_write.op_range(ZX_VMAR_OP_ZERO, s->addr, zx_system_get_page_size(), nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_OK(s->no_rights.op_range(ZX_VMAR_OP_DONT_NEED, s->addr, zx_system_get_page_size(),
                                    nullptr, 0));
    EXPECT_OK(s->read_only.op_range(ZX_VMAR_OP_DONT_NEED, s->addr, zx_system_get_page_size(),
                                    nullptr, 0));
    EXPECT_OK(s->read_write.op_range(ZX_VMAR_OP_DONT_NEED, s->addr, zx_system_get_page_size(),
                                     nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_OK(s->no_rights.op_range(ZX_VMAR_OP_ALWAYS_NEED, s->addr, zx_system_get_page_size(),
                                    nullptr, 0));
    EXPECT_OK(s->read_only.op_range(ZX_VMAR_OP_ALWAYS_NEED, s->addr, zx_system_get_page_size(),
                                    nullptr, 0));
    EXPECT_OK(s->read_write.op_range(ZX_VMAR_OP_ALWAYS_NEED, s->addr, zx_system_get_page_size(),
                                     nullptr, 0));
  }
  {
    zx::result<Setup> s = make_one_page_vmar();
    ASSERT_OK(s.status_value());
    EXPECT_STATUS(
        ZX_ERR_ACCESS_DENIED,
        s->no_rights.op_range(ZX_VMAR_OP_PREFETCH, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_OK(
        s->read_only.op_range(ZX_VMAR_OP_PREFETCH, s->addr, zx_system_get_page_size(), nullptr, 0));
    EXPECT_OK(s->read_write.op_range(ZX_VMAR_OP_PREFETCH, s->addr, zx_system_get_page_size(),
                                     nullptr, 0));
  }
}

// Tests that mapping with ZX_VM_MAP_RANGE trims ranges correctly at and beyond stream size.
TEST(Vmar, MapRangeStreamSizeWraparound) {
  const size_t page_size = zx_system_get_page_size();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, ZX_VMO_UNBOUNDED, &vmo));

  // Ensure there is a page table entry installed so that the probe has something to work with.
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, page_size * 2, page_size, nullptr, 0));

  zx_vaddr_t map_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE |
                                           ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS |
                                           ZX_VM_MAP_RANGE,
                                       0, vmo, page_size * 2, page_size, &map_addr));

  EXPECT_STATUS(probe_for_read(reinterpret_cast<void*>(map_addr)), ZX_ERR_OUT_OF_RANGE);

  EXPECT_OK(zx::vmar::root_self()->unmap(map_addr, page_size));
}

// Test that attempting to access the stream size of a read-only VMO subject to
// zx_process_write_memory doesn't crash, and that the inherited stream size is correct. Regression
// test for https://fxbug.dev/506555297.
TEST(Vmar, FaultBeyondStreamSizeForceWritable) {
  class ForceWritableMapping {
   public:
    struct Params {
      const uint64_t vmo_size;
      const uint64_t stream_size;
      const uint64_t map_offset;
      const uint64_t map_size;
    };

    static zx::result<ForceWritableMapping> Create(Params params) {
      zx::vmo vmo;
      if (zx_status_t status = zx::vmo::create(params.vmo_size, 0, &vmo); status != ZX_OK) {
        return zx::error(status);
      }
      if (zx_status_t status = vmo.set_stream_size(params.stream_size); status != ZX_OK) {
        return zx::error(status);
      }
      zx::vmo read_vmo;
      if (zx_status_t status = vmo.duplicate(
              ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY,
              &read_vmo);
          status != ZX_OK) {
        return zx::error(status);
      }
      zx_vaddr_t addr;
      if (zx_status_t status = zx::vmar::root_self()->map(
              ZX_VM_PERM_READ | ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS, 0, read_vmo,
              params.map_offset, params.map_size, &addr);
          status != ZX_OK) {
        return zx::error(status);
      }
      fit::deferred_action<fit::closure> cleanup =
          fit::defer(fit::closure([addr = addr, size = params.map_size]() {
            ZX_ASSERT(ZX_OK == zx::vmar::root_self()->unmap(addr, size));
          }));
      uint64_t actual;
      constexpr char source[] = "Trigger a call to VmMapping::ForceWritable.";
      if (zx_status_t status = zx::process::self()->write_memory(addr, std::data(source),
                                                                 std::size(source), &actual);
          status != ZX_OK) {
        return zx::error(status);
      }
      return zx::ok(ForceWritableMapping(addr, std::move(cleanup)));
    }

    char* Get() const { return reinterpret_cast<char*>(addr_); }

   private:
    ForceWritableMapping(uint64_t addr, fit::deferred_action<fit::closure> cleanup)
        : addr_(addr), cleanup_(std::move(cleanup)) {}
    const uint64_t addr_;
    fit::deferred_action<fit::closure> cleanup_;
  };

  {
    // Ensure that ZX_VMAR_OP_PREFETCH, which triggers a lookup of the stream size manager, doesn't
    // fail.
    zx::result<ForceWritableMapping> map = ForceWritableMapping::Create({
        .vmo_size = 2 * zx_system_get_page_size(),
        .stream_size = 2 * zx_system_get_page_size(),
        .map_offset = 0,
        .map_size = 2 * zx_system_get_page_size(),
    });
    ASSERT_OK(map.status_value());
    const zx_vaddr_t addr = reinterpret_cast<zx_vaddr_t>(map->Get());
    EXPECT_OK(zx::vmar::root_self()->op_range(ZX_VMAR_OP_PREFETCH, addr,
                                              2 * zx_system_get_page_size(), nullptr, 0));
  }

  {
    // We expect the read probes to not be limited by the stream size.
    zx::result<ForceWritableMapping> map = ForceWritableMapping::Create({
        .vmo_size = 2 * zx_system_get_page_size(),
        .stream_size = 2 * zx_system_get_page_size(),
        .map_offset = 0,
        .map_size = 2 * zx_system_get_page_size(),
    });
    ASSERT_OK(map.status_value());
    EXPECT_OK(probe_for_read(map->Get()));
    EXPECT_OK(probe_for_read(map->Get() + zx_system_get_page_size()));
    // Write probes will always fail because we have a read-only mapping.
    EXPECT_STATUS(probe_for_write(map->Get()), ZX_ERR_ACCESS_DENIED);
    EXPECT_STATUS(probe_for_write(map->Get() + zx_system_get_page_size()), ZX_ERR_ACCESS_DENIED);
  }

  {
    // Use a smaller stream size, and expect the read probe at the boundary to fail. We inherit the
    // stream size.
    zx::result<ForceWritableMapping> map = ForceWritableMapping::Create({
        .vmo_size = 2 * zx_system_get_page_size(),
        .stream_size = 1 * zx_system_get_page_size(),
        .map_offset = 0,
        .map_size = 2 * zx_system_get_page_size(),
    });
    ASSERT_OK(map.status_value());
    EXPECT_OK(probe_for_read(map->Get()));
    // ZX_ERR_OUT_OF_RANGE: Exceed the stream size (fault after stream size).
    EXPECT_STATUS(probe_for_read(map->Get() + zx_system_get_page_size()), ZX_ERR_OUT_OF_RANGE);
  }

  {
    // We handle offset mappings correctly. The first page of the mapping is visible and within the
    // stream size, the second page isn't within the stream size.
    zx::result<ForceWritableMapping> map = ForceWritableMapping::Create({
        .vmo_size = 10 * zx_system_get_page_size(),
        .stream_size = 2 * zx_system_get_page_size(),
        .map_offset = zx_system_get_page_size(),
        .map_size = 2 * zx_system_get_page_size(),
    });
    ASSERT_OK(map.status_value());
    EXPECT_OK(probe_for_read(map->Get()));
    EXPECT_STATUS(probe_for_read(map->Get() + zx_system_get_page_size()), ZX_ERR_OUT_OF_RANGE);
  }

  {
    // The inherited stream size is zero, so no read probes will succeed. We inherit zero because
    // map_offset >= stream_size.
    zx::result<ForceWritableMapping> map = ForceWritableMapping::Create({
        .vmo_size = 5 * zx_system_get_page_size(),
        .stream_size = 1 * zx_system_get_page_size(),
        .map_offset = 2 * zx_system_get_page_size(),
        .map_size = 3 * zx_system_get_page_size(),
    });
    ASSERT_OK(map.status_value());
    EXPECT_STATUS(probe_for_read(map->Get()), ZX_ERR_OUT_OF_RANGE);
    EXPECT_STATUS(probe_for_read(map->Get() + zx_system_get_page_size()), ZX_ERR_OUT_OF_RANGE);
    EXPECT_STATUS(probe_for_read(map->Get() + 2 * zx_system_get_page_size()), ZX_ERR_OUT_OF_RANGE);
  }
}

// Tests that zx_vmar_op_range rejects ranges beyond stream size when vmo_offset exceeds
// stream size.
TEST(Vmar, RangeOpStreamSizeWraparound) {
  const size_t page_size = zx_system_get_page_size();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, ZX_VMO_UNBOUNDED, &vmo));

  zx_vaddr_t map_addr;
  ASSERT_OK(zx::vmar::root_self()->map(
      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS, 0,
      vmo, page_size * 2, page_size, &map_addr));

  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx::vmar::root_self()->op_range(ZX_VMAR_OP_DONT_NEED, map_addr, page_size, nullptr, 0));

  EXPECT_OK(zx::vmar::root_self()->unmap(map_addr, page_size));
}

}  // namespace

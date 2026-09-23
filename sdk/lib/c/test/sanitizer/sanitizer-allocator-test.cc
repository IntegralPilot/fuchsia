// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file has tests which ensure various allocator functions that are
// expected to be provided by a sanitizer runtime use the sanitizer implementations
// rather than the default ones. This can be re-used for multiple sanitizers,
// but the only relevant ones for Fuchsia at the moment are asan, lsan, and
// hwasan.
//
// This should exercise every allocator entry point used in Fuchsia.

#define REPLACES_ALLOCATOR                                             \
  __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
      __has_feature(hwaddress_sanitizer)

#if REPLACES_ALLOCATOR

#include <lib/tbi/tbi.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <malloc.h>

#include <new>

#include <sanitizer/allocator_interface.h>

#include "test-utils.h"

// All sanitizers that replace the default allocator eventually call
// __sanitizer_malloc/free_hook on allocs/frees, so we can check that we go
// through the sanitizer implementation if these are called.
static size_t gMallocHookCounter = 0;
static size_t gFreeHookCounter = 0;

extern "C" __EXPORT void __sanitizer_malloc_hook(const volatile void *ptr, size_t size) {
  gMallocHookCounter++;
}

extern "C" __EXPORT void __sanitizer_free_hook(const volatile void *ptr) { gFreeHookCounter++; }

namespace {

class SanitizerAllocatorTest : public ::testing::Test {
 public:
  void SetUp() override {
    gMallocHookCounter = 0;
    gFreeHookCounter = 0;
  }
};

constexpr size_t kAllocSize = 16;

TEST_F(SanitizerAllocatorTest, Malloc) {
  void *alloc = malloc(kAllocSize);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
  EXPECT_EQ(gFreeHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, Calloc) {
  void *alloc = calloc(1, kAllocSize);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
  EXPECT_EQ(gFreeHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, Realloc) {
  void *alloc = malloc(kAllocSize);
  EXPECT_NE(alloc, nullptr);
  alloc = realloc(alloc, kAllocSize * 2);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 2u);
  EXPECT_GE(gFreeHookCounter, 1u);  // Realloc may call free.
}

TEST_F(SanitizerAllocatorTest, Memalign) {
  void *alloc = memalign(kAllocSize, kAllocSize);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
  EXPECT_EQ(gFreeHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, AlignedAlloc) {
  void *alloc = aligned_alloc(kAllocSize, kAllocSize);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
  EXPECT_EQ(gFreeHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, PosixMemalign) {
  void *alloc = nullptr;
  ASSERT_EQ(posix_memalign(&alloc, kAllocSize, kAllocSize), 0);
  EXPECT_NE(alloc, nullptr);
  free(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
  EXPECT_EQ(gFreeHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, MallocUsableSize) {
  void *alloc = malloc(kAllocSize);
  EXPECT_NE(alloc, nullptr);
  ASSERT_EQ(malloc_usable_size(alloc), __sanitizer_get_allocated_size(alloc));
  free(alloc);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDelete) {
  void *alloc = operator new(kAllocSize);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteSize) {
  void *alloc = operator new(kAllocSize);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc, kAllocSize);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteArray) {
  void *alloc = operator new[](kAllocSize);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteArraySize) {
  void *alloc = operator new[](kAllocSize);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc, kAllocSize);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteNoThrow) {
  void *alloc = operator new(kAllocSize, std::nothrow);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc, std::nothrow);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteArrayNoThrow) {
  void *alloc = operator new[](kAllocSize, std::nothrow);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc, std::nothrow);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

#ifdef __cpp_aligned_new

constexpr std::align_val_t kAllocAlign{kAllocSize};

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlign) {
  void *alloc = operator new(kAllocSize, kAllocAlign);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc, kAllocAlign);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlignSize) {
  void *alloc = operator new(kAllocSize, kAllocAlign);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc, kAllocSize, kAllocAlign);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlignArray) {
  void *alloc = operator new[](kAllocSize, kAllocAlign);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc, kAllocAlign);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlignArraySize) {
  void *alloc = operator new[](kAllocSize, kAllocAlign);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc, kAllocSize, kAllocAlign);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlignNoThrow) {
  void *alloc = operator new(kAllocSize, kAllocAlign, std::nothrow);
  EXPECT_NE(alloc, nullptr);
  operator delete(alloc, kAllocAlign, std::nothrow);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

TEST_F(SanitizerAllocatorTest, OperatorNewDeleteAlignArraySizeNoThrow) {
  void *alloc = operator new[](kAllocSize, kAllocAlign, std::nothrow);
  EXPECT_NE(alloc, nullptr);
  operator delete[](alloc, kAllocAlign, std::nothrow);
  EXPECT_EQ(gMallocHookCounter, 1u);
}

#endif  // __cpp_aligned_new

constexpr size_t kMaxVmos = 8192;
zx_info_vmo_t vmos[kMaxVmos];

constexpr size_t kMaxMaps = 8192;
zx_info_maps_t maps[kMaxMaps];

zx_status_t GetVmoInfo(zx_koid_t vmo_koid, zx_info_vmo_t *vmo_info) {
  size_t actual, available;
  zx_status_t res =
      zx::process::self()->get_info(ZX_INFO_PROCESS_VMOS, vmos, sizeof(vmos), &actual, &available);
  if (res != ZX_OK)
    return res;
  if (available > kMaxVmos)
    return ZX_ERR_NO_RESOURCES;

  for (size_t i = 0; i < actual; i++) {
    if (vmos[i].koid == vmo_koid) {
      *vmo_info = vmos[i];
      return ZX_OK;
    }
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t GetVmoKoid(uintptr_t alloc, zx_koid_t *vmo_koid) {
  size_t actual, available;
  zx_status_t res =
      zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, maps, sizeof(maps), &actual, &available);
  if (res != ZX_OK)
    return res;
  if (available > kMaxMaps)
    return ZX_ERR_NO_RESOURCES;

  for (size_t i = 0; i < actual; i++) {
    if (maps[i].type != ZX_INFO_MAPS_TYPE_MAPPING)
      continue;
    if (alloc >= maps[i].base && alloc < maps[i].base + maps[i].size) {
      *vmo_koid = maps[i].u.mapping.vmo_koid;
      return ZX_OK;
    }
  }

  return ZX_ERR_NOT_FOUND;
}

// This tests that the allocator does not explicitly memset a large buffer
// allocated via calloc. This is a regression test for fxbug.dev/125426 where
// there can be a brief instance where we can allocate a large buffer via
// calloc. If those pages are committed, like with a memset(0), then there's a
// very brief window after those pages are committed but before the
// zero-page-scanner kicks in where we could end up using a lot of physical
// memory, and potentially lead to an OOM. This ensures that any allocator that
// requests a sufficiently large zero-allocation does not explicitly touch the
// pages as if they come from the secondary allocator, they should be from
// mmap/zx_vmo_create which already has zero-init'd pages.
//
// Note that sanitizer which replace the allocator use a combined allocator,
// where small enough allocations that fit into specific size classes go into
// the primary allocator where as the rest go into the secondary allocator
// (which is mmap'd). Allocations going into the primary allocator will need to
// be explicitly zero'd and go up to 1024 bytes. For the purposes of this test,
// a sufficiently large allocation would be something that could OOM the system.
TEST_F(SanitizerAllocatorTest, NoCommittedCallocPages) {
  // For hwasan at least, it's important that the allocation be a multiple of
  // the granule size (16 bytes) since short granules would write a non-zero
  // value in the very last granule which would commit the page.
  constexpr size_t kAllocSize = 1ULL << 32;
  void *alloc = calloc(1, kAllocSize);
  EXPECT_NE(alloc, nullptr);

  // Attempt to get the underlying vmo handle for this allocation via:
  // 1. Finding the vmar in this process whose base address matches the
  //    allocation.
  // 2. Get the koid for the vmo for mapped into that vmar.
  // 3. Iterate through all process vmos until we find the one whose koid
  //    matches the koid found in (2).
  zx_koid_t vmo_koid;
  ASSERT_OK(GetVmoKoid(tbi::RemoveTag(reinterpret_cast<uintptr_t>(alloc)), &vmo_koid));

  zx_info_vmo_t vmo_info;
  ASSERT_OK(GetVmoInfo(vmo_koid, &vmo_info));

  // Here we assume that the allocation comes from a single contiguous VMO,
  // so we expect this VMO to at least be as large as the original allocation.
  // Depending on the allocator, the VMO may be larger than the requested size
  // for things like chunk headers or redzones in the case of ASan.
  ASSERT_GE(vmo_info.size_bytes, kAllocSize);

  // `populated_bytes` indicates the number of committed bytes in this VMO. This
  // accounts for both normal committed bytes and bytes that were previously committed
  // but have been compressed and reclaimed. Because we expect `calloc` to come
  // from zero-inited pages which should be deduped from a singleton zero page,
  // we should not expect the portion of the VMO backing the `calloc` to be
  // committed. However, the remainder of the VMO could be committed for things like
  // malloc headers or redzones, meaning fields like `committed_bytes` could report
  // non-zero values in the unlikely event the page scanner chooses to reclaim
  // these pages. Therefore, to assert the pages backing the `calloc` are uncommitted,
  // we can check that the number of uncommitted bytes is at least the requested
  // allocation size.
  size_t uncommitted_bytes = vmo_info.size_bytes - vmo_info.populated_bytes;
  EXPECT_GE(uncommitted_bytes, kAllocSize);

  free(alloc);
}

}  // namespace

#endif  // REPLACES_ALLOCATOR

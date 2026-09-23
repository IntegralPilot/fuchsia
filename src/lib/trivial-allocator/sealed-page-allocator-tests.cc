// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trivial-allocator/posix.h>
#include <lib/trivial-allocator/sealed-page-allocator.h>

#ifdef __Fuchsia__
#include <lib/trivial-allocator/zircon.h>
#endif

#include <array>

#include <gtest/gtest.h>

namespace {

template <class Memory, size_t Reserve, typename... Args>
void SealedPageAllocatorTestWithReserve(Args&&... args) {
  trivial_allocator::SealedPageAllocator<Memory, Reserve> allocator(args...);

  auto& memory = allocator.memory();
  static_assert(std::is_same_v<decltype(memory), Memory&>);

  const auto& const_allocator = allocator;
  auto& const_memory = const_allocator.memory();
  static_assert(std::is_same_v<decltype(const_memory), const Memory&>);

  const size_t pagesize = const_memory.page_size();

  struct Live {
    decltype(allocator(std::declval<size_t&>(), size_t{})) allocation;
    int* ptr = nullptr;
  };

  // The allocator should keep exactly this many released allocations writable.
  constexpr size_t kUnsealedCount = Reserve + 1;

  std::array<Live, kUnsealedCount> live;

  for (auto& l : live) {
    size_t size = 1;
    l.allocation = allocator(size, 1);
    EXPECT_TRUE(l.allocation);
    EXPECT_EQ(pagesize, size);
    int* iptr = reinterpret_cast<int*>(l.allocation.get());
    EXPECT_EQ(0, *iptr);
    *iptr = 17;

    l.ptr = reinterpret_cast<int*>(l.allocation.release());
    EXPECT_EQ(iptr, l.ptr);
  }

  // After all those are released, they should still be writable.
  for (auto& l : live) {
    volatile int* vptr = l.ptr;
    EXPECT_EQ(17, *vptr);
    *vptr = 23;
  }
  for (auto& l : live) {
    EXPECT_EQ(23, *l.ptr);
  }

  // These allocations should push all those out so they get sealed.
  std::array<Live, kUnsealedCount> extra;
  for (auto& l : extra) {
    size_t size = 1;
    l.allocation = allocator(size, 1);
    EXPECT_TRUE(l.allocation);
    EXPECT_EQ(pagesize, size);
    int* iptr = reinterpret_cast<int*>(l.allocation.get());
    EXPECT_EQ(0, *iptr);
    *iptr = 17;

    l.ptr = reinterpret_cast<int*>(l.allocation.release());
    EXPECT_EQ(iptr, l.ptr);
  }

  // The old pointers should now be read-only.
  for (auto& l : live) {
    volatile int* vptr = l.ptr;
    EXPECT_EQ(23, *vptr);
    EXPECT_DEATH({ *vptr = 42; }, "") << "unsealed count " << kUnsealedCount;
  }

  // Finish off the allocator so everything is sealed.
  std::move(allocator).Seal();

  // These pointers should now be sealed too.
  for (auto& l : extra) {
    volatile int* vptr = l.ptr;
    EXPECT_EQ(17, *vptr);
    EXPECT_DEATH({ *vptr = 23; }, "");
  }

  // Allocate and release some pages but don't seal them.
  for (auto& l : live) {
    size_t size = 1;
    l.allocation = allocator(size, 1);
    EXPECT_TRUE(l.allocation);
    EXPECT_EQ(pagesize, size);
    int* iptr = reinterpret_cast<int*>(l.allocation.get());
    EXPECT_EQ(0, *iptr);
    *iptr = 17;

    l.ptr = reinterpret_cast<int*>(l.allocation.release());
    EXPECT_EQ(iptr, l.ptr);
  }

  // Get a fresh allocator instance and some fresh allocations.
  allocator = decltype(allocator)(args...);
  for (auto& l : live) {
    size_t size = 1;
    l.allocation = allocator(size, 1);
    EXPECT_TRUE(l.allocation);
    EXPECT_EQ(pagesize, size);
    int* iptr = reinterpret_cast<int*>(l.allocation.get());
    EXPECT_EQ(0, *iptr);
    *iptr = 17;

    l.ptr = reinterpret_cast<int*>(l.allocation.release());
    EXPECT_EQ(iptr, l.ptr);
  }

  // Destroy the allocator without sealing.
  allocator = decltype(allocator)();

  // All those live allocations should have been unmapped.
  for (auto& l : live) {
    volatile int* vptr = l.ptr;
    EXPECT_DEATH({ *vptr = 42; }, "");
  }
}

template <class Memory, typename... Args>
void SealedPageAllocatorTest(Args&&... args) {
  SealedPageAllocatorTestWithReserve<Memory, 0>(args...);
  SealedPageAllocatorTestWithReserve<Memory, 1>(args...);
  SealedPageAllocatorTestWithReserve<Memory, 2>(args...);
  SealedPageAllocatorTestWithReserve<Memory, 3>(args...);
}

TEST(TrivialAllocatorTests, SealedPageAllocatorMmap) {
  ASSERT_NO_FATAL_FAILURE(SealedPageAllocatorTest<trivial_allocator::PosixMmap>());
}

#ifdef __Fuchsia__

TEST(TrivialAllocatorTests, SealedPageAllocatorVmar) {
  ASSERT_NO_FATAL_FAILURE(
      SealedPageAllocatorTest<trivial_allocator::ZirconVmar<>>(*zx::vmar::root_self()));
}

#endif

}  // namespace

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trivial-allocator/page-allocator.h>
#include <lib/trivial-allocator/posix.h>
#include <unistd.h>

#ifdef __Fuchsia__
#include <lib/trivial-allocator/zircon.h>
#endif

#include <gtest/gtest.h>

namespace {

template <class Memory, typename... Args>
void PageAllocatorTest(Args&&... args) {
  trivial_allocator::PageAllocator<Memory> allocator(std::forward<Args>(args)...);

  auto& memory = allocator.memory();
  static_assert(std::is_same_v<decltype(memory), Memory&>);

  const size_t pagesize = memory.page_size();

  const auto& const_allocator = allocator;
  auto& const_memory = const_allocator.memory();
  static_assert(std::is_same_v<decltype(const_memory), const Memory&>);

  size_t size = 1;
  auto allocation = allocator(size, 1);
  EXPECT_TRUE(allocation);
  EXPECT_EQ(pagesize, size);
  int* iptr = reinterpret_cast<int*>(allocation.get());
  EXPECT_EQ(0, *iptr);
  *iptr = 17;

  volatile int* vptr = iptr;
  EXPECT_EQ(17, *vptr);

  // Reset should unmap the page.  The unmap must happen inside the death
  // expression because gtest runs it in a subprocess that might otherwise have
  // mapped something else at the same address if the parent does the unmap.
  ASSERT_DEATH(
      {
        allocation.reset();
        [[maybe_unused]] int i = *vptr;
      },
      "");

  allocation = allocator(size, 1);
  EXPECT_TRUE(allocation);
  iptr = reinterpret_cast<int*>(allocation.get());
  int* release_iptr = reinterpret_cast<int*>(allocation.release());
  EXPECT_EQ(iptr, release_iptr);
  *iptr = 17;

  // Reset after release should not unmap the page.  (We leak it here.)
  vptr = release_iptr;
  allocation.reset();
  EXPECT_EQ(17, *vptr);
  *vptr = 23;
  EXPECT_EQ(23, *release_iptr);

  // Large and overaligned allocations are OK, though alignment is not met.
  size = pagesize + 1;
  allocation = allocator(size, pagesize * 2);
  EXPECT_TRUE(allocation);
  EXPECT_EQ(pagesize * 2, size);
  iptr = reinterpret_cast<int*>(allocation.get());
  EXPECT_EQ(0, *iptr);
  *iptr = 23;
  vptr = iptr;

  // Make it read-only;
  std::move(allocation).Seal();
  EXPECT_EQ(23, *vptr);

  ASSERT_DEATH({ *vptr = 17; }, "");
}

TEST(TrivialAllocatorDeathTest, PageAllocatorMmap) {
  ASSERT_NO_FATAL_FAILURE(PageAllocatorTest<trivial_allocator::PosixMmap>());
}

#ifdef __Fuchsia__

TEST(TrivialAllocatorDeathTest, PageAllocatorVmar) {
  ASSERT_NO_FATAL_FAILURE(
      PageAllocatorTest<trivial_allocator::ZirconVmar<>>(*zx::vmar::root_self()));
}

#endif

TEST(TrivialAllocatorTests, PageAllocatorRelease) {
  static constexpr uint64_t kPageSize = 0x1000;

  struct Memory {
    struct Capability {};

    Memory(bool& deallocated, bool& released, bool& sealed)
        : deallocated(deallocated), released(released), sealed(sealed) {}

    size_t page_size() const { return kPageSize; }

    // `this` gives a canonical (albeit nonsensical) pointer.
    std::pair<void*, Capability> Allocate(size_t) {
      return {this, {}};
    }

    void Deallocate(Capability, void* ptr, size_t size) {
      ExpectAddressAndSize(ptr, size);
      deallocated = true;
    }

    void Release(Capability, void* ptr, size_t size) {
      ExpectAddressAndSize(ptr, size);
      released = true;
    }

    void Seal(Capability, void* ptr, size_t size) {
      ExpectAddressAndSize(ptr, size);
      deallocated = true;
    }

    void ExpectAddressAndSize(void* ptr, size_t size) {
      EXPECT_EQ(this, ptr);
      EXPECT_EQ(kPageSize, size);
    }

    bool& deallocated;
    bool& released;
    bool& sealed;
  };

  bool deallocated = false;
  bool released = false;
  bool sealed = false;
  {
    trivial_allocator::PageAllocator<Memory> allocator(deallocated, released, sealed);
    EXPECT_FALSE(deallocated);
    EXPECT_FALSE(released);
    EXPECT_FALSE(sealed);

    size_t size = 1;
    auto allocation = allocator(size, 1);
    allocation.release();

    EXPECT_FALSE(deallocated);
    EXPECT_TRUE(released);
    EXPECT_FALSE(sealed);
  }

  EXPECT_FALSE(deallocated);
  EXPECT_FALSE(sealed);
}

}  // namespace

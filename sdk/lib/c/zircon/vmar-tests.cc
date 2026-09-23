// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>

#include <zxtest/zxtest.h>

#include "vmar.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

// This prevents the compiler from eliding a memory fetch.
template <typename T>
T Launder(const T* ptr) {
  __asm__ volatile("" : "=r"(ptr) : "0"(ptr));

  T val;
  __asm__ volatile("" : "=r"(val) : "r"(*ptr));
  return val;
}

TEST(LibcVmarTests, PageRoundedSize) {
  // Default-constructed (only) can be constinit and constexpr.
  constexpr PageRoundedSize kConstexprZero{};
  EXPECT_EQ(kConstexprZero.get(), 0u);
  static constinit PageRoundedSize kConstinitZero{};
  EXPECT_EQ(kConstinitZero.get(), 0u);

  const PageRoundedSize two_page = *PageRoundedSize::Pages(2);
  EXPECT_EQ(two_page.get(), 2 * zx_system_get_page_size());
  EXPECT_EQ(two_page, two_page);
  EXPECT_GT(two_page, kConstexprZero);
  EXPECT_LT(kConstexprZero, two_page);
  EXPECT_LE(kConstexprZero, two_page);

  const PageRoundedSize min = *PageRoundedSize::From(1);
  EXPECT_EQ(min.get(), zx_system_get_page_size());
  EXPECT_EQ(min, min);
  EXPECT_NE(min, kConstexprZero);

  std::optional<PageRoundedSize> incr = std::optional{min} + std::optional{min};
  ASSERT_TRUE(incr);
  EXPECT_EQ(incr->get(), 2 * zx_system_get_page_size());
}

TEST(LibcVmarTests, GuardedPageBlock) {
  constexpr PageRoundedSize kNoGuard{};
  const PageRoundedSize kOnePage = PageRoundedSize::Page();
  const PageRoundedSize kTwoPages = *PageRoundedSize::Pages(2);
  const PageRoundedSize kFourPages = *PageRoundedSize::Pages(4);

  zx::result<AllocationVmo> vmo = AllocationVmo::New(kFourPages);
  ASSERT_TRUE(vmo.is_ok()) << vmo.status_string();
  EXPECT_EQ(vmo->offset, 0u);
  EXPECT_TRUE(vmo->vmo.is_valid());
  uint64_t vmo_size;
  EXPECT_OK(vmo->vmo.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, kFourPages.get());

  {
    GuardedPageBlock no_guards;
    zx::result result = no_guards.Allocate(AllocationVmar(), *vmo, kTwoPages, kNoGuard, kNoGuard);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    EXPECT_EQ(vmo->offset, kTwoPages.get());

    std::span<std::byte> mapped = *result;
    ASSERT_EQ(mapped.size_bytes(), kTwoPages.get());
    for (size_t i = 0; i < mapped.size(); ++i) {
      EXPECT_EQ(Launder(&mapped[i]), std::byte{}, "mapped[%zu]", i);
      const std::byte byte{static_cast<uint8_t>(i % 256)};
      mapped[i] = byte;
      EXPECT_EQ(Launder(&mapped[i]), byte, "mutated mapped[%zu]", i);
    }

    // Assume nothing else gets mapped after .reset() unmaps it.
    no_guards.reset();
    ASSERT_DEATH(([mapped] { std::ignore = Launder(&mapped.front()); }));
  }

  ASSERT_OK(vmo->vmo.op_range(ZX_VMO_OP_ZERO, 0, vmo->offset, nullptr, 0));
  vmo->offset = 0;
  {
    GuardedPageBlock two_guards;
    zx::result result = two_guards.Allocate(AllocationVmar(), *vmo, kTwoPages, kOnePage, kOnePage);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    EXPECT_EQ(vmo->offset, kTwoPages.get());

    std::span<std::byte> mapped = *result;
    ASSERT_EQ(mapped.size_bytes(), kTwoPages.get());
    for (size_t i = 0; i < mapped.size(); ++i) {
      EXPECT_EQ(Launder(&mapped[i]), std::byte{}, "mapped[%zu]", i);
      const std::byte byte{static_cast<uint8_t>(i % 256)};
      mapped[i] = byte;
      EXPECT_EQ(Launder(&mapped[i]), byte, "mutated mapped[%zu]", i);
    }

    std::span below{mapped.data() - kOnePage.get(), kOnePage.get()};
    ASSERT_DEATH(([below] { std::ignore = Launder(&below.front()); }));
    ASSERT_DEATH(([below] { std::ignore = Launder(&below.back()); }));

    std::span above{mapped.data() + mapped.size(), kOnePage.get()};
    ASSERT_DEATH(([above] { std::ignore = Launder(&above.front()); }));
    ASSERT_DEATH(([above] { std::ignore = Launder(&above.back()); }));
  }

  ASSERT_OK(vmo->vmo.op_range(ZX_VMO_OP_ZERO, 0, vmo->offset, nullptr, 0));
  vmo->offset = 0;
  {
    GuardedPageBlock guard_below;
    zx::result result = guard_below.Allocate(AllocationVmar(), *vmo, kOnePage, kTwoPages, kNoGuard);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    EXPECT_EQ(vmo->offset, kOnePage.get());

    std::span<std::byte> mapped = *result;
    ASSERT_EQ(mapped.size_bytes(), kOnePage.get());
    for (size_t i = 0; i < mapped.size(); ++i) {
      EXPECT_EQ(Launder(&mapped[i]), std::byte{}, "mapped[%zu]", i);
      const std::byte byte{static_cast<uint8_t>(i % 256)};
      mapped[i] = byte;
      EXPECT_EQ(Launder(&mapped[i]), byte, "mutated mapped[%zu]", i);
    }

    std::span below{mapped.data() - kOnePage.get(), kOnePage.get()};
    ASSERT_DEATH(([below] { std::ignore = Launder(&below.front()); }));
    ASSERT_DEATH(([below] { std::ignore = Launder(&below.back()); }));
  }

  ASSERT_OK(vmo->vmo.op_range(ZX_VMO_OP_ZERO, 0, vmo->offset, nullptr, 0));
  vmo->offset = 0;
  {
    GuardedPageBlock guard_above;
    zx::result result = guard_above.Allocate(AllocationVmar(), *vmo, kOnePage, kNoGuard, kTwoPages);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    EXPECT_EQ(vmo->offset, kOnePage.get());

    std::span<std::byte> mapped = *result;
    ASSERT_EQ(mapped.size_bytes(), kOnePage.get());
    for (size_t i = 0; i < mapped.size(); ++i) {
      EXPECT_EQ(Launder(&mapped[i]), std::byte{}, "mapped[%zu]", i);
      const std::byte byte{static_cast<uint8_t>(i % 256)};
      mapped[i] = byte;
      EXPECT_EQ(Launder(&mapped[i]), byte, "mutated mapped[%zu]", i);
    }

    std::span above{mapped.data() + mapped.size(), kOnePage.get()};
    ASSERT_DEATH(([above] { std::ignore = Launder(&above.front()); }));
    ASSERT_DEATH(([above] { std::ignore = Launder(&above.back()); }));
  }

  ASSERT_OK(vmo->vmo.op_range(ZX_VMO_OP_ZERO, 0, vmo->offset, nullptr, 0));
  vmo->offset = 0;
  {
    GuardedPageBlock block;
    zx::result result = block.Allocate(AllocationVmar(), *vmo, kTwoPages, kNoGuard, kNoGuard);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    EXPECT_EQ(vmo->offset, kTwoPages.get());

    std::span<std::byte> mapped = *result;
    ASSERT_EQ(mapped.size_bytes(), kTwoPages.get());

    auto cleanup = fit::defer([mapped] {
      ASSERT_OK(
          AllocationVmar()->unmap(reinterpret_cast<uintptr_t>(mapped.data()), mapped.size_bytes()));
    });

    zx::unowned vmar{block.vmar()};
    EXPECT_EQ(vmar, AllocationVmar());
    uintptr_t address = block.release();
    EXPECT_EQ(block.size_bytes(), 0u);
    EXPECT_FALSE(block.vmar());
    block.reset();  // Does nothing, mapped pointer still valid.

    EXPECT_EQ(reinterpret_cast<std::byte*>(address), mapped.data());

    for (size_t i = 0; i < mapped.size(); ++i) {
      EXPECT_EQ(Launder(&mapped[i]), std::byte{}, "mapped[%zu]", i);
      const std::byte byte{static_cast<uint8_t>(i % 256)};
      mapped[i] = byte;
      EXPECT_EQ(Launder(&mapped[i]), byte, "mutated mapped[%zu]", i);
    }
  }
}

TEST(LibcVmarTests, GuardedPageBlockTooLarge) {
  const PageRoundedSize kOnePage = PageRoundedSize::Page();
  constexpr PageRoundedSize kNoGuard{};

  const PageRoundedSize kTwoPages = *PageRoundedSize::Pages(2);

  zx::vmar child_vmar;
  uintptr_t child_base;
  ASSERT_OK(AllocationVmar()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, kOnePage.get(),
                                       &child_vmar, &child_base));
  auto cleanup_child = fit::defer([&child_vmar] { child_vmar.destroy(); });

  zx::result<AllocationVmo> vmo = AllocationVmo::New(kTwoPages);
  ASSERT_TRUE(vmo.is_ok()) << vmo.status_string();

  GuardedPageBlock block;
  // Try to allocate 2 pages in a 1 page VMAR.
  zx::result result = block.Allocate(child_vmar.borrow(), *vmo, kTwoPages, kNoGuard, kNoGuard);
  EXPECT_EQ(result.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST(LibcVmarTests, GuardedPageBlockVmarFull) {
  const PageRoundedSize kOnePage = PageRoundedSize::Page();
  constexpr PageRoundedSize kNoGuard{};

  zx::vmar child_vmar;
  uintptr_t child_base;
  ASSERT_OK(AllocationVmar()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, kOnePage.get(),
                                       &child_vmar, &child_base));
  auto cleanup_child = fit::defer([&child_vmar] { child_vmar.destroy(); });

  zx::result<AllocationVmo> vmo = AllocationVmo::New(kOnePage);
  ASSERT_TRUE(vmo.is_ok()) << vmo.status_string();

  GuardedPageBlock block1;
  zx::result result1 = block1.Allocate(child_vmar.borrow(), *vmo, kOnePage, kNoGuard, kNoGuard);
  ASSERT_TRUE(result1.is_ok()) << result1.status_string();

  GuardedPageBlock block2;
  zx::result result2 = block2.Allocate(child_vmar.borrow(), *vmo, kOnePage, kNoGuard, kNoGuard);

  EXPECT_EQ(result2.status_value(), ZX_ERR_NO_RESOURCES)
      << "Expected ZX_ERR_NO_RESOURCES when the allocation vmar is full: "
      << result2.status_string();
}

TEST(LibcVmarTests, GuardedPageBlockOverflow) {
  const PageRoundedSize kOnePage = PageRoundedSize::Page();
  constexpr PageRoundedSize kNoPage{};

  zx::result<AllocationVmo> vmo = AllocationVmo::New(kOnePage);
  ASSERT_TRUE(vmo.is_ok()) << vmo.status_string();

  // Use a very large size that will cause overflow when added.
  // We use half of UINT64_MAX + 1 to guarantee overflow when two are added.
  constexpr uint64_t kHugeSize = std::numeric_limits<uint64_t>::max() / 2 + 1;
  const PageRoundedSize huge = *PageRoundedSize::From(kHugeSize);

  // Test combinations where the sum of sizes overflows.
  {
    // Huge data + huge guard below + no guard above
    GuardedPageBlock block;
    zx::result result = block.Allocate(AllocationVmar(), *vmo, huge, huge, kNoPage);
    EXPECT_EQ(result.status_value(), ZX_ERR_NO_RESOURCES);
  }
  {
    // Huge data + no guard below + huge guard above
    GuardedPageBlock block;
    zx::result result = block.Allocate(AllocationVmar(), *vmo, huge, kNoPage, huge);
    EXPECT_EQ(result.status_value(), ZX_ERR_NO_RESOURCES);
  }
  {
    // No data + huge guard below + huge guard above
    GuardedPageBlock block;
    zx::result result = block.Allocate(AllocationVmar(), *vmo, kNoPage, huge, huge);
    EXPECT_EQ(result.status_value(), ZX_ERR_NO_RESOURCES);
  }
}

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL

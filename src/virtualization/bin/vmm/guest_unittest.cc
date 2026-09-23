// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/bin/vmm/guest.h"

#include <sys/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

// TODO(dahastin): Move out of global namespace once we refactor the vmm to use namespaces.
void PrintTo(const GuestMemoryRegion& region, std::ostream* os) {
  *os << std::hex << "Region range: 0x" << region.base << " - 0x" << region.base + region.size
      << " (Size: 0x" << region.size << " bytes)";
}

namespace {

using ::testing::Pointwise;

MATCHER(GuestMemoryRegionEq, "") {
  const GuestMemoryRegion& first = std::get<0>(arg);
  const GuestMemoryRegion& second = std::get<1>(arg);

  return first.base == second.base && first.size == second.size;
}

MATCHER_P(GuestMemoryRegionEq, expected, "") {
  return arg.base == expected.base && arg.size == expected.size;
}

TEST(GuestTest, GuestMemoryPageAligned) {
  const uint32_t page_size = zx_system_get_page_size();
  const uint64_t expected_guest_memory = static_cast<uint64_t>(page_size) * 10;

  // Already page aligned, so no change.
  EXPECT_EQ(expected_guest_memory, Guest::GetPageAlignedGuestMemory(expected_guest_memory));
}

TEST(GuestTest, RoundUpUnalignedGuestMemory) {
  const uint32_t page_size = zx_system_get_page_size();
  const uint64_t expected_guest_memory = static_cast<uint64_t>(page_size) * 10;

  // Memory is unaligned, so this will be rounded up half a page.
  EXPECT_EQ(expected_guest_memory,
            Guest::GetPageAlignedGuestMemory(expected_guest_memory - page_size / 2));
}

TEST(GuestTest, PageAlignGuestMemoryRegion) {
  const uint64_t page_size = zx_system_get_page_size();

  // Page aligned.
  GuestMemoryRegion region = {page_size, page_size};
  EXPECT_TRUE(Guest::PageAlignGuestMemoryRegion(region));
  EXPECT_THAT(region, GuestMemoryRegionEq(GuestMemoryRegion{page_size, page_size}));

  // End is not page aligned, so round it down.
  region = {page_size, page_size * 3 + page_size / 2};
  EXPECT_TRUE(Guest::PageAlignGuestMemoryRegion(region));
  EXPECT_THAT(region, GuestMemoryRegionEq(GuestMemoryRegion{page_size, page_size * 3}));

  // Start is not page aligned, so round it up (remember that the second field is size, not the
  // ending address which is why it will also change here).
  region = {page_size / 2, page_size * 3 + page_size / 2};
  EXPECT_TRUE(Guest::PageAlignGuestMemoryRegion(region));
  EXPECT_THAT(region, GuestMemoryRegionEq(GuestMemoryRegion{page_size, page_size * 3}));

  // After page aligning this is a zero length region, so drop it.
  region = {page_size / 2, page_size / 2};
  EXPECT_FALSE(Guest::PageAlignGuestMemoryRegion(region));

  // After page aligning this would be a negative length region, so drop it.
  region = {page_size / 2, page_size / 4};
  EXPECT_FALSE(Guest::PageAlignGuestMemoryRegion(region));
}

TEST(GuestTest, PageAlignedMemoryGivesCorrectTotal) {
  const uint64_t page_size = zx_system_get_page_size();

  // Restrict memory between page 2 1/2 and page 4 1/2. This should result in guest memory placed in
  // pages [0, 1], and pages [5, 7] (which is 5 pages in total).
  const uint64_t guest_memory = page_size * 5;
  const GuestMemoryRegion restrictions[] = {{page_size * 2 + page_size / 2, page_size * 2}};

  std::vector<GuestMemoryRegion> regions;
  EXPECT_TRUE(Guest::GenerateGuestMemoryRegions(guest_memory, restrictions, &regions));
  EXPECT_THAT(regions,
              Pointwise(GuestMemoryRegionEq(), {GuestMemoryRegion{0, page_size * 2},
                                                GuestMemoryRegion{page_size * 5, page_size * 3}}));
}

TEST(GuestTest, GetGuestMemoryRegion) {
  // Four GiB of guest memory will extend beyond the PCI device region for x86, but not for arm64.
  const uint64_t guest_memory = Guest::GetPageAlignedGuestMemory(1ul << 32);

  std::vector<GuestMemoryRegion> regions;
  EXPECT_TRUE(Guest::GenerateGuestMemoryRegions(
      guest_memory, Guest::GetDefaultRestrictionsForArchitecture(), &regions));

#if __x86_64__
  const GuestMemoryRegion kExpected[] = {
      {0x8000, 0x78000},                  // 32 KiB to 512 KiB
      {0x100000, 0xf8000000 - 0x100000},  // 1 MiB to start of the PCI device region
      {0x100000000, guest_memory - (0xf8000000 - 0x100000) - 0x78000}  // Remaining memory.
  };
#else
  const GuestMemoryRegion kExpected[] = {{0, guest_memory}};  // All memory in one region.
#endif

  EXPECT_THAT(regions, Pointwise(GuestMemoryRegionEq(), kExpected));
}

TEST(GuestTest, GetTooLargeGuestMemoryRegion) {
  const uint64_t guest_memory = Guest::GetPageAlignedGuestMemory(kFirstDynamicDeviceAddr + 0x1000);

  // The kFirstDynamicDeviceAddr restriction extends to +INF, so requesting enough memory
  // to overlap with kFirstDynamicDeviceAddr will always fail.
  std::vector<GuestMemoryRegion> regions;
  EXPECT_FALSE(Guest::GenerateGuestMemoryRegions(
      guest_memory, Guest::GetDefaultRestrictionsForArchitecture(), &regions));
}

constexpr uint64_t kOneMib = 1u * 1024 * 1024;

TEST(GuestTest, GetPluggableRegionBaseSkipSmallOnesAndAlign) {
  const uint64_t given_base = 160u * kOneMib;
  const uint64_t pluggable_region_size = 1024 * kOneMib;
  const uint64_t guest_block_memory_size = 128 * kOneMib;
  uint64_t result = 0;
  EXPECT_TRUE(Guest::FitPluggableRegionBase(Guest::GetDefaultRestrictionsForArchitecture(),
                                            given_base, pluggable_region_size,
                                            guest_block_memory_size, &result));
  EXPECT_EQ(result % guest_block_memory_size, 0u);
  EXPECT_EQ(result, 256u * kOneMib);
}

TEST(GuestTest, GetPluggableRegionBaseSkipDeviceRangeForx86NoSkipForArm) {
  const uint64_t given_base = 20u * kOneMib;
  const uint64_t pluggable_region_size = 8 * 1024 * kOneMib;
  const uint64_t guest_block_memory_size = 32 * kOneMib;
  uint64_t result = 0;
  EXPECT_TRUE(Guest::FitPluggableRegionBase(Guest::GetDefaultRestrictionsForArchitecture(),
                                            given_base, pluggable_region_size,
                                            guest_block_memory_size, &result));
  EXPECT_EQ(result % guest_block_memory_size, 0u);
#if __aarch64__
  EXPECT_EQ(result, guest_block_memory_size);
#elif __x86_64__
  EXPECT_EQ(result, 4u * 1024 * kOneMib);
#endif
}

TEST(GuestTest, GetPluggableRegionBaseCannotFit) {
  const uint64_t given_base = 160u * kOneMib;
  const uint64_t pluggable_region_size = 64u * 1024 * kOneMib;
  const uint64_t guest_block_memory_size = 64 * kOneMib;
  uint64_t result = 0;
  EXPECT_FALSE(Guest::FitPluggableRegionBase(Guest::GetDefaultRestrictionsForArchitecture(),
                                             given_base, pluggable_region_size,
                                             guest_block_memory_size, &result));
  EXPECT_EQ(result, 0u);
}

}  // namespace

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/ld/vmar.h>
#include <lib/zx/vmar.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

std::vector<zx_info_maps_t> VmarMaps(zx::unowned_vmar vmar) {
  size_t actual = 0;
  size_t avail = 0;
  zx_status_t status = vmar->get_info(ZX_INFO_VMAR_MAPS, nullptr, 0, &actual, &avail);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
  if (status != ZX_OK) {
    return {};
  }

  EXPECT_GT(avail, size_t{0}) << "The buffer should contain at least 1 element (the querying vmar)";

  std::vector<zx_info_maps_t> maps(avail);
  status = vmar->get_info(ZX_INFO_VMAR_MAPS, maps.data(), maps.size() * sizeof(zx_info_maps_t),
                          &actual, &avail);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
  EXPECT_EQ(actual, avail);
  return maps;
}

TEST(LdVmarTests, VmarReservationLifecycle) {
  constexpr size_t kTestVmarSize = 1024 * 1024;  // 1MB

  // Allocate a parent VMAR for testing.
  zx::vmar parent_vmar;
  uintptr_t parent_addr;
  zx_status_t status = zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0, kTestVmarSize,
      &parent_vmar, &parent_addr);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  auto destroy_vmar = fit::defer([&parent_vmar]() { parent_vmar.destroy(); });

  // Get info of the parent VMAR.
  zx_info_vmar_t parent_info;
  status = parent_vmar.get_info(ZX_INFO_VMAR, &parent_info, sizeof(parent_info), nullptr, nullptr);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  {
    // We want to reserve a portion (e.g., the first page).
    zx_info_vmar_t reserve_info = parent_info;
    reserve_info.len = zx_system_get_page_size();

    ld::VmarReservation reservation;
    auto res = reservation.Init(parent_vmar.borrow(), parent_info, reserve_info);
    ASSERT_TRUE(res.is_ok()) << zx_status_get_string(res.error_value());
    EXPECT_TRUE(reservation);
    EXPECT_TRUE(reservation.vmar().is_valid());

    // Verify the parent_vmar contains only the reservation vmar.
    auto mappings = VmarMaps(parent_vmar.borrow());
    EXPECT_THAT(mappings, testing::SizeIs(2));
    EXPECT_THAT(mappings,
                testing::Contains(testing::AllOf(
                    testing::Field(&zx_info_maps_t::base, testing::Eq(reserve_info.base)),
                    testing::Field(&zx_info_maps_t::size, testing::Eq(reserve_info.len)))));
  }  // Reservation goes out of scope here and destructor should destroy the child VMAR.

  // Verify the parent vmar is empty now that the reservation vmar is destroyed.
  // In this case, the only vmar present is the parent_vmar.
  EXPECT_THAT(VmarMaps(parent_vmar.borrow()), testing::SizeIs(1));
}

TEST(LdVmarTests, BoundsCalculations) {
  const size_t page_size = zx_system_get_page_size();

  constexpr struct {
    const char* name;
    zx_info_vmar_t primary;
    zx_info_vmar_t expected_bottom;
    zx_info_vmar_t expected_top;
  } kTestCases[] = {
      // StandardFullAspace39Bit
      {
          .name = "StandardFullAspace39Bit",
          .primary = {.base = 0, .len = 1ull << 38},
          .expected_bottom = {.base = 0, .len = 1ull << 37},
          .expected_top = {.base = 1ull << 37, .len = 1ull << 37},
      },
      // StandardFullAspace48Bit
      {
          .name = "StandardFullAspace48Bit",
          .primary = {.base = 0, .len = 1ull << 47},
          .expected_bottom = {.base = 0, .len = 1ull << 46},
          .expected_top = {.base = 1ull << 46, .len = 1ull << 46},
      },
      // StandardFullAspace56Bit
      {
          .name = "StandardFullAspace56Bit",
          .primary = {.base = 0, .len = 1ull << 55},
          .expected_bottom = {.base = 0, .len = 1ull << 54},
          .expected_top = {.base = 1ull << 54, .len = 1ull << 54},
      },
      // SharedProcessTopHalf39Bit
      {
          .name = "SharedProcessTopHalf39Bit",
          .primary = {.base = 1ull << 37, .len = 1ull << 37},
          .expected_bottom = {.base = 1ull << 37, .len = 1ull << 36},
          .expected_top = {.base = (1ull << 37) + (1ull << 36), .len = 1ull << 36},
      },
      // SharedProcessTopHalf48Bit
      {
          .name = "SharedProcessTopHalf48Bit",
          .primary = {.base = 1ull << 46, .len = 1ull << 46},
          .expected_bottom = {.base = 1ull << 46, .len = 1ull << 45},
          .expected_top = {.base = (1ull << 46) + (1ull << 45), .len = 1ull << 45},
      },
      // SharedProcessTopHalf56Bit
      {
          .name = "SharedProcessTopHalf56Bit",
          .primary = {.base = 1ull << 54, .len = 1ull << 54},
          .expected_bottom = {.base = 1ull << 54, .len = 1ull << 53},
          .expected_top = {.base = (1ull << 54) + (1ull << 53), .len = 1ull << 53},
      },
  };

  for (const auto& tc : kTestCases) {
    SCOPED_TRACE(tc.name);
    zx_info_vmar_t bottom = ld::VmarBottomHalf(tc.primary, page_size);
    EXPECT_EQ(bottom.base, tc.expected_bottom.base);
    EXPECT_EQ(bottom.len, tc.expected_bottom.len);

    zx_info_vmar_t top = ld::VmarTopHalf(tc.primary, page_size);
    EXPECT_EQ(top.base, tc.expected_top.base);
    EXPECT_EQ(top.len, tc.expected_top.len);
  }
}

TEST(LdVmarTests, PageAlignmentRounding) {
  const size_t page_size = zx_system_get_page_size();
  // Primary length is 3 pages.
  // Half length rounded up to nearest page boundary is 2 pages.
  const zx_info_vmar_t primary = {
      .base = 1000 * page_size,
      .len = 3 * page_size,
  };

  zx_info_vmar_t bottom = ld::VmarBottomHalf(primary, page_size);
  EXPECT_EQ(bottom.base, primary.base);
  EXPECT_EQ(bottom.len, 2 * page_size);

  zx_info_vmar_t top = ld::VmarTopHalf(primary, page_size);
  EXPECT_EQ(top.base, primary.base + 2 * page_size);
  EXPECT_EQ(top.len, page_size);
}

}  // namespace

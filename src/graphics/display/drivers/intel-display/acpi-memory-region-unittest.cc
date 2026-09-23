// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/acpi-memory-region.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstdint>
#include <span>
#include <utility>

#include <gtest/gtest.h>

namespace intel_display {

namespace {

class AcpiMemoryRegionTest : public ::testing::Test {
 public:
  void SetUp() override {
    std::pair<zx::vmo, std::span<uint8_t>> vmo_and_span = CreateAndMapVmo(kVmoSize);
    vmo_ = std::move(vmo_and_span).first;
    region_data_ = std::move(vmo_and_span).second;
    ASSERT_TRUE(vmo_.is_valid());
    ASSERT_FALSE(region_data_.empty());

    vmo_unowned_ = vmo_.borrow();
  }

  // Returns an invalid VMO and empty span on failure.
  static std::pair<zx::vmo, std::span<uint8_t>> CreateAndMapVmo(int vmo_size) {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(vmo_size, /*options=*/0, &vmo);
    if (status != ZX_OK) {
      return {};
    }

    zx_vaddr_t mapped_address;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset=*/0, vmo,
                                        /*vmo_offset=*/0, vmo_size, &mapped_address);
    if (status != ZX_OK) {
      return {};
    }

    // NOLINTBEGIN(performance-no-int-to-ptr)
    uint8_t* const mapped_base = reinterpret_cast<uint8_t*>(mapped_address);
    // NOLINTEND(performance-no-int-to-ptr)
    return std::make_pair(std::move(vmo), std::span(mapped_base, kVmoSize));
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  static constexpr int kVmoSize = 16;
  std::span<uint8_t> region_data_;

  zx::vmo vmo_;
  zx::unowned_vmo vmo_unowned_;
};

TEST_F(AcpiMemoryRegionTest, EmptyConstructor) {
  AcpiMemoryRegion memory_region;
  EXPECT_FALSE(memory_region.vmo_for_testing()->is_valid());
  EXPECT_TRUE(memory_region.is_empty());
  EXPECT_TRUE(memory_region.data().empty());
}

TEST_F(AcpiMemoryRegionTest, TestConstructor) {
  AcpiMemoryRegion memory_region(std::move(vmo_), region_data_);
  EXPECT_EQ(memory_region.vmo_for_testing(), vmo_unowned_);
  EXPECT_EQ(memory_region.data().data(), region_data_.data());
  EXPECT_EQ(memory_region.data().size(), region_data_.size());
  EXPECT_FALSE(memory_region.is_empty());
}

TEST_F(AcpiMemoryRegionTest, MoveConstructorEmptiesRhs) {
  AcpiMemoryRegion rhs(std::move(vmo_), region_data_);
  AcpiMemoryRegion lhs = std::move(rhs);

  EXPECT_FALSE(rhs.vmo_for_testing()->is_valid());
  EXPECT_TRUE(rhs.is_empty());
  EXPECT_TRUE(rhs.data().empty());

  EXPECT_EQ(lhs.vmo_for_testing(), vmo_unowned_);
  EXPECT_EQ(lhs.data().data(), region_data_.data());
  EXPECT_EQ(lhs.data().size(), region_data_.size());
  EXPECT_FALSE(lhs.is_empty());
}

TEST_F(AcpiMemoryRegionTest, MoveAssignmentSwapsRhs) {
  AcpiMemoryRegion rhs(std::move(vmo_), region_data_);

  std::pair<zx::vmo, std::span<uint8_t>> vmo_and_span = CreateAndMapVmo(kVmoSize);
  zx::vmo lhs_vmo = std::move(vmo_and_span).first;
  std::span<uint8_t> lhs_region_data = std::move(vmo_and_span).second;
  ASSERT_TRUE(lhs_vmo.is_valid());
  ASSERT_FALSE(lhs_region_data.empty());

  const zx::unowned_vmo lhs_vmo_unowned = lhs_vmo.borrow();

  AcpiMemoryRegion lhs(std::move(lhs_vmo), lhs_region_data);
  lhs = std::move(rhs);

  EXPECT_EQ(rhs.vmo_for_testing(), lhs_vmo_unowned);
  EXPECT_EQ(rhs.data().data(), lhs_region_data.data());
  EXPECT_EQ(rhs.data().size(), lhs_region_data.size());
  EXPECT_FALSE(rhs.is_empty());

  EXPECT_EQ(lhs.vmo_for_testing(), vmo_unowned_);
  EXPECT_EQ(lhs.data().data(), region_data_.data());
  EXPECT_EQ(lhs.data().size(), region_data_.size());
  EXPECT_FALSE(lhs.is_empty());
}

}  // namespace

}  // namespace intel_display

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../mmio.h"

#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <gtest/gtest.h>

#include "dts/reg.h"

namespace fdf_devicetree {
namespace {

class MmioVisitorTester : public testing::VisitorTestHelper<MmioVisitor> {
 public:
  MmioVisitorTester(std::string_view dtb_path)
      : VisitorTestHelper<MmioVisitor>(dtb_path, "MmioVisitorTest") {}
};

TEST(MmioVisitorTest, ReadRegSuccessfully) {
  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<MmioVisitorTester>("/pkg/test-data/mmio.dtb");
  MmioVisitorTester* mmio_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());
  ASSERT_TRUE(mmio_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(mmio_tester->has_visited());
  ASSERT_TRUE(mmio_tester->DoPublish().is_ok());

  // Check MMIO of reg-device.
  auto reg_device_mmio = mmio_tester->GetPbusNodes("reg-device")[0].mmio();

  ASSERT_TRUE(reg_device_mmio);
  ASSERT_EQ(3lu, reg_device_mmio->size());
  ASSERT_EQ(REG_A_BASE, *(*reg_device_mmio)[0].base());
  ASSERT_EQ(static_cast<uint64_t>(REG_A_LENGTH), *(*reg_device_mmio)[0].length());
  ASSERT_TRUE((*reg_device_mmio)[0].name().has_value());
  ASSERT_EQ(*(*reg_device_mmio)[0].name(), "reg-a");

  ASSERT_EQ((uint64_t)REG_B_BASE_WORD0 << 32 | REG_B_BASE_WORD1, *(*reg_device_mmio)[1].base());
  ASSERT_EQ((uint64_t)REG_B_LENGTH_WORD0 << 32 | REG_B_LENGTH_WORD1,
            *(*reg_device_mmio)[1].length());
  ASSERT_TRUE((*reg_device_mmio)[1].name().has_value());
  ASSERT_EQ(*(*reg_device_mmio)[1].name(), "reg-b");

  ASSERT_EQ((uint64_t)REG_C_BASE_WORD0 << 32 | REG_C_BASE_WORD1, *(*reg_device_mmio)[2].base());
  ASSERT_EQ((uint64_t)REG_C_LENGTH_WORD0 << 32 | REG_C_LENGTH_WORD1,
            *(*reg_device_mmio)[2].length());
  ASSERT_TRUE((*reg_device_mmio)[2].name().has_value());
  ASSERT_EQ(*(*reg_device_mmio)[2].name(), "reg-c");

  // Check MMIO of memory-region-device.
  auto memory_region_device_mmio = mmio_tester->GetPbusNodes("memory-region-device")[0].mmio();

  ASSERT_TRUE(memory_region_device_mmio);
  ASSERT_EQ(1lu, memory_region_device_mmio->size());
  ASSERT_EQ(static_cast<uint64_t>(TEST_BASE_1), *(*memory_region_device_mmio)[0].base());
  ASSERT_EQ(static_cast<uint64_t>(TEST_LENGTH_1), *(*memory_region_device_mmio)[0].length());
  ASSERT_TRUE((*memory_region_device_mmio)[0].name().has_value());
  ASSERT_EQ(*(*memory_region_device_mmio)[0].name(), "test-region-1");

  // Check MMIO of combination-device.
  auto combination_device_mmio = mmio_tester->GetPbusNodes("combination-device")[0].mmio();

  ASSERT_TRUE(combination_device_mmio);
  ASSERT_EQ(2lu, combination_device_mmio->size());
  uint32_t mmio_tested = 0;
  for (auto& mmio : *combination_device_mmio) {
    ASSERT_TRUE(mmio.name().has_value());
    if (*mmio.name() == "reg-1") {
      ASSERT_EQ(static_cast<uint64_t>(REG_1_BASE), *mmio.base());
      ASSERT_EQ(static_cast<uint64_t>(REG_1_LENGTH), *mmio.length());
      mmio_tested++;
    } else if (*mmio.name() == "test-region-2") {
      ASSERT_EQ(static_cast<uint64_t>(TEST_BASE_2), *mmio.base());
      ASSERT_EQ(static_cast<uint64_t>(TEST_LENGTH_2), *mmio.length());
      mmio_tested++;
    }
  }
  ASSERT_EQ(2u, mmio_tested);
}

TEST(MmioVisitorTest, TranslateRegSuccessfully) {
  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<MmioVisitorTester>("/pkg/test-data/ranges.dtb");
  MmioVisitorTester* mmio_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());
  ASSERT_TRUE(mmio_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(mmio_tester->has_visited());
  ASSERT_TRUE(mmio_tester->DoPublish().is_ok());

  auto parent_mmio = mmio_tester->GetPbusNodes()[0].mmio();
  auto child_mmio = mmio_tester->GetPbusNodes()[1].mmio();

  ASSERT_TRUE(parent_mmio);
  ASSERT_TRUE(child_mmio);
  ASSERT_EQ(1lu, parent_mmio->size());
  ASSERT_EQ(1lu, child_mmio->size());
  ASSERT_EQ(RANGE_BASE, *(*parent_mmio)[0].base());
  ASSERT_EQ(RANGE_OFFSET + RANGE_BASE, *(*child_mmio)[0].base());
  ASSERT_EQ((uint64_t)RANGE_SIZE, *(*parent_mmio)[0].length());
  ASSERT_EQ((uint64_t)RANGE_OFFSET_SIZE, *(*child_mmio)[0].length());
}

TEST(MmioVisitorTest, IgnoreRegWhichIsNotMmio) {
  VisitorRegistry visitors;
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<MmioVisitorTester>("/pkg/test-data/not-mmio.dtb");
  MmioVisitorTester* mmio_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());
  ASSERT_TRUE(mmio_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(mmio_tester->has_visited());
  ASSERT_TRUE(mmio_tester->DoPublish().is_ok());
  ASSERT_EQ(0lu, mmio_tester->GetPbusNodes().size());
  ASSERT_EQ(3lu, mmio_tester->GetBoardChildNodes().size());
}

}  // namespace
}  // namespace fdf_devicetree

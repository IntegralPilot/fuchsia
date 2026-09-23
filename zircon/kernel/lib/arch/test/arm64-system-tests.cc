// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/arm64/memory.h>
#include <lib/arch/arm64/system.h>

#include <algorithm>
#include <variant>
#include <vector>

#include <gtest/gtest.h>

namespace {

bool ExpectAttrsConsistent(arch::ArmMemoryAttrIndirectionRegister reg) {
  EXPECT_EQ(reg.attr0(), reg.GetAttributeValue(0));
  EXPECT_EQ(reg.attr1(), reg.GetAttributeValue(1));
  EXPECT_EQ(reg.attr2(), reg.GetAttributeValue(2));
  EXPECT_EQ(reg.attr3(), reg.GetAttributeValue(3));
  EXPECT_EQ(reg.attr4(), reg.GetAttributeValue(4));
  EXPECT_EQ(reg.attr5(), reg.GetAttributeValue(5));
  EXPECT_EQ(reg.attr6(), reg.GetAttributeValue(6));
  EXPECT_EQ(reg.attr7(), reg.GetAttributeValue(7));
  return true;
}

TEST(Arm64System, MairGetSetAttribute) {
  arch::ArmMemoryAttrIndirectionRegister val{};

  // Ensure everything consistent to begin with.
  EXPECT_EQ(val.reg_value(), 0u);
  ExpectAttrsConsistent(val);

  // Set some attributes by the setters. Ensure everything is consistent.
  val.set_attr0(0b1111'1111);
  ExpectAttrsConsistent(val);
  val.set_attr3(0b0000'0100);
  ExpectAttrsConsistent(val);
  val.set_attr7(0b0100'0100);
  ExpectAttrsConsistent(val);

  // Set some attributes using SetAttribute. Ensure everything is consistent.
  val.SetAttributeValue(0, 0b0100'0100);
  ExpectAttrsConsistent(val);
  val.SetAttributeValue(3, 0b1111'1111);
  ExpectAttrsConsistent(val);
  val.SetAttributeValue(7, 0b0000'0100);
  ExpectAttrsConsistent(val);
}

TEST(Arm64System, MairAttributeTranslation) {
  constexpr struct {
    const char* name;
    std::optional<arch::ArmMairAttribute> attr;
    uint8_t value;
  } kTestCases[] = {
      {
          .name = "nGnRnE",
          .attr = arch::ArmDeviceMemory::kNonGatheringNonReorderingNoEarlyAck,
          .value = 0b0000'0000,
      },
      {
          .name = "nGnRE",
          .attr = arch::ArmDeviceMemory::kNonGatheringNonReorderingEarlyAck,
          .value = 0b0000'0100,
      },
      {
          .name = "nGRE",
          .attr = arch::ArmDeviceMemory::kNonGatheringReorderingEarlyAck,
          .value = 0b0000'1000,
      },
      {
          .name = "GRE",
          .attr = arch::ArmDeviceMemory::kGatheringReorderingEarlyAck,
          .value = 0b0000'1100,
      },
      {
          .name = "inner/outer kNonCacheable",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kNonCacheable,
                  .outer = arch::ArmCacheabilityAttribute::kNonCacheable,
              },
          .value = 0b0100'0100,
      },
      {
          .name = "inner/outer kWriteBackReadWriteAllocate",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kWriteBackReadWriteAllocate,
                  .outer = arch::ArmCacheabilityAttribute::kWriteBackReadWriteAllocate,
              },
          .value = 0b1111'1111,
      },
      {
          .name = "inner/outer kWriteThroughReadAllocate",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kWriteThroughReadAllocate,
                  .outer = arch::ArmCacheabilityAttribute::kWriteThroughReadAllocate,
              },
          .value = 0b1010'1010,
      },
      {
          .name = "inner/outer kWriteBackReadAllocate",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kWriteBackReadAllocate,
                  .outer = arch::ArmCacheabilityAttribute::kWriteBackReadAllocate,
              },
          .value = 0b1110'1110,
      },
      {
          .name = "inner kNonCacheable; outer kWriteBackReadAllocate",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kNonCacheable,
                  .outer = arch::ArmCacheabilityAttribute::kWriteBackReadAllocate,
              },
          .value = 0b1110'0100,
      },
      {
          .name = "inner kWriteThroughReadAllocate; outer kWriteBackReadWriteAllocate",
          .attr =
              arch::ArmMairNormalAttribute{
                  .inner = arch::ArmCacheabilityAttribute::kWriteThroughReadAllocate,
                  .outer = arch::ArmCacheabilityAttribute::kWriteBackReadWriteAllocate,
              },
          .value = 0b1111'1010,
      },
      {
          .name = "0b1111'0000: invalid",
          .attr = std::nullopt,
          .value = 0b1111'0000,
      },
      {
          .name = "0b1111'1011: valid software; invalid hardware",
          .attr = std::nullopt,
          .value = 0b1111'1011,  // Inner write-through read *and write* allocating.
      },
  };

  for (const auto& test : kTestCases) {
    EXPECT_EQ(test.attr, arch::ArmMemoryAttrIndirectionRegister::AttributeFromValue(test.value))
        << test.name;
    if (test.attr) {
      EXPECT_EQ(test.value, arch::ArmMemoryAttrIndirectionRegister::AttributeToValue(*test.attr))
          << test.name;
    }
  }
}

}  // namespace

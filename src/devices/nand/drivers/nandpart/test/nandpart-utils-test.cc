// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/nand/drivers/nandpart/nandpart-utils.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/stdcompat/span.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace nand {
namespace {

constexpr uint32_t kPageSize = 4096;
constexpr uint32_t kPagesPerBlock = 2;
constexpr uint32_t kNumBlocks = 5;
constexpr uint32_t kOobSize = 8;
constexpr nand_info_t kNandInfo = {
    .page_size = kPageSize,
    .pages_per_block = kPagesPerBlock,
    .num_blocks = kNumBlocks,
    .ecc_bits = 2,
    .oob_size = kOobSize,
    .nand_class = NAND_CLASS_BBS,
    .partition_guid = {},
};

const fuchsia_boot_metadata::PartitionMap kDefaultPartitionMap{{
    .block_count = kNumBlocks * kPagesPerBlock,
    .block_size = kPageSize,
    .reserved = 0,
    .partitions{{}},
}};

fuchsia_boot_metadata::Partition MakePartition(uint32_t first_block, uint32_t last_block) {
  return fuchsia_boot_metadata::Partition{{
      .first_block = first_block,
      .last_block = last_block,
      .flags = 0,
      .name = {},
  }};
}

void ValidatePartition(const fuchsia_boot_metadata::PartitionMap& pmap, size_t partition_number,
                       uint32_t first_block, uint32_t last_block) {
  ASSERT_TRUE(pmap.partitions().has_value());
  ASSERT_GE(pmap.partitions().value().size(), partition_number);
  const auto& partition = pmap.partitions().value()[partition_number];
  EXPECT_EQ(partition.first_block(), first_block);
  EXPECT_EQ(partition.last_block(), last_block);
}

class NandPartUtilsTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_{FUCHSIA_LOG_INFO};
};

TEST_F(NandPartUtilsTest, SanitizeEmptyPartitionMapTest) {
  auto pmap = kDefaultPartitionMap;
  ASSERT_NE(SanitizePartitionMap(pmap, kNandInfo), ZX_OK);
}

TEST_F(NandPartUtilsTest, SanitizeSinglePartitionMapTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().value().emplace_back(MakePartition(0, 9));
  ASSERT_OK(SanitizePartitionMap(pmap, kNandInfo));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 0, 0, 4));
}

TEST_F(NandPartUtilsTest, SanitizeMultiplePartitionMapTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(0, 3), MakePartition(4, 7), MakePartition(8, 9)});

  ASSERT_OK(SanitizePartitionMap(pmap, kNandInfo));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 0, 0, 1));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 1, 2, 3));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 2, 4, 4));
}

TEST_F(NandPartUtilsTest, SanitizeMultiplePartitionMapOutOfOrderTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(4, 9), MakePartition(0, 3)});

  ASSERT_OK(SanitizePartitionMap(pmap, kNandInfo));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 0, 0, 1));
  ASSERT_NO_FATAL_FAILURE(ValidatePartition(pmap, 1, 2, 4));
}

TEST_F(NandPartUtilsTest, SanitizeMultiplePartitionMapOverlappingTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(0, 3), MakePartition(8, 9), MakePartition(4, 8)});

  ASSERT_NE(SanitizePartitionMap(pmap, kNandInfo), ZX_OK);
}

TEST_F(NandPartUtilsTest, SanitizePartitionMapBadRangeTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(1, 0), MakePartition(1, 9)});

  ASSERT_NE(SanitizePartitionMap(pmap, kNandInfo), ZX_OK);
}

TEST_F(NandPartUtilsTest, SanitizePartitionMapUnalignedTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(0, 3), MakePartition(5, 8)});

  ASSERT_NE(SanitizePartitionMap(pmap, kNandInfo), ZX_OK);
}

TEST_F(NandPartUtilsTest, SanitizePartitionMapOutofBoundsTest) {
  auto pmap = kDefaultPartitionMap;
  pmap.partitions().emplace({MakePartition(0, 3), MakePartition(4, 11)});

  ASSERT_NE(SanitizePartitionMap(pmap, kNandInfo), ZX_OK);
}

}  // namespace
}  // namespace nand

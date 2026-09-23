// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/storage/blobfs/common.h"

#include <zircon/errors.h>

#include <cstdint>
#include <limits>

#include <gtest/gtest.h>

#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/format.h"

namespace blobfs {
namespace {

constexpr uint64_t kBlockCount = 1 << 10;

TEST(CommonTest, BlobfsV8UsesPaddedBlobLayoutFormat) {
  Superblock info;
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {}, &info), ZX_OK);
  ASSERT_EQ(info.major_version, 10u);
  ASSERT_EQ(info.oldest_minor_version, 4u);
  // The only thing that changed from V8Rev4 to V10Rev4 is storing the blob layout format in the
  // inode. Since there are no inodes here, the superblock can be easily downgraded to V8Rev4.
  info.major_version = 8;

  Inode inode = {.header = {.flags = kBlobFlagAllocated}};
  EXPECT_EQ(GetBlobLayoutFormat(info, inode), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
}

TEST(CommonTest, BlobfsV9UsesCompactBlobLayoutFormat) {
  Superblock info;
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {}, &info), ZX_OK);
  ASSERT_EQ(info.major_version, 10u);
  ASSERT_EQ(info.oldest_minor_version, 4u);
  // The only thing that changed from V9Rev4 to V10Rev4 is storing the blob layout format in the
  // inode. Since there are no inodes here, the superblock can be easily downgraded to V9Rev4.
  info.major_version = 9;

  Inode inode = {.header = {.flags = kBlobFlagAllocated}};
  EXPECT_EQ(GetBlobLayoutFormat(info, inode), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
}

// TODO: Add test for writing goes to the correct format.
TEST(CommonTest, BlobfsV10UsesTheBlobLayoutFormatFromTheInode) {
  Superblock info;
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {}, &info), ZX_OK);
  ASSERT_EQ(info.major_version, 10u);
  ASSERT_EQ(info.oldest_minor_version, 4u);

  Inode inode = {.header = {.flags = kBlobFlagAllocated}};
  SetBlobLayoutFormat(&inode, BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  EXPECT_EQ(GetBlobLayoutFormat(info, inode), BlobLayoutFormat::kCompactMerkleTreeAtEnd);

  SetBlobLayoutFormat(&inode, BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  EXPECT_EQ(GetBlobLayoutFormat(info, inode), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
}

TEST(CommonTest, PaddedBlobLayoutFormatIsRoundTrippedThroughTheSuperblock) {
  BlobLayoutFormat format = BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart;
  Superblock info;
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {.blob_layout_format = format}, &info), ZX_OK);
  EXPECT_EQ(GetDefaultBlobLayoutFormat(info), format);
}

TEST(CommonTest, CompactBlobLayoutFormatIsRoundTrippedThroughTheSuperblock) {
  BlobLayoutFormat format = BlobLayoutFormat::kCompactMerkleTreeAtEnd;
  Superblock info;
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {.blob_layout_format = format}, &info), ZX_OK);
  EXPECT_EQ(GetDefaultBlobLayoutFormat(info), format);
}

TEST(CommonTest, InodesRoundedUpToFillBlock) {
  Superblock info;
  EXPECT_EQ(
      InitializeSuperblock(
          kBlockCount, {.num_inodes = kBlobfsDefaultInodeCount + kBlobfsInodesPerBlock - 1}, &info),
      ZX_OK);
  EXPECT_EQ(info.inode_count, kBlobfsDefaultInodeCount + kBlobfsInodesPerBlock);
}

TEST(CommonTest, TooFewInodesFailsCheck) {
  Superblock info;
  static_assert(kBlobfsDefaultInodeCount > kBlobfsInodesPerBlock);
  EXPECT_EQ(InitializeSuperblock(kBlockCount, {.num_inodes = 0}, &info), ZX_OK);
  EXPECT_EQ(ZX_ERR_NO_SPACE, CheckSuperblock(&info, std::numeric_limits<uint64_t>::max(), true));
}

}  // namespace
}  // namespace blobfs

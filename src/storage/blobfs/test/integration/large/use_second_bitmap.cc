// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/stat.h>
#include <unistd.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

namespace blobfs {
namespace {

class LargeBlobTest : public BaseBlobfsTest {
 public:
  LargeBlobTest() : BaseBlobfsTest(BlobfsWithFixedDiskSizeTestParam(GetDiskSize())) {}

  static uint64_t GetDataBlockCount() { return kBlobfsBlockBits + 1; }

 private:
  static uint64_t GetDiskSize() {
    // Create blobfs with enough data blocks to ensure 2 block bitmap blocks.
    // Any number above kBlobfsBlockBits should do, and the larger the
    // number, the bigger the disk (and memory used for the test).
    Superblock superblock;
    superblock.flags = 0;
    superblock.inode_count = kBlobfsDefaultInodeCount;
    superblock.journal_block_count = kMinimumJournalBlocks;
    superblock.data_block_count = GetDataBlockCount();
    return TotalBlocks(superblock) * kBlobfsBlockSize;
  }
};

TEST_F(LargeBlobTest, UseSecondBitmap) {
  // Create (and delete) a blob large enough to overflow into the second bitmap block.
  size_t blob_size = ((GetDataBlockCount() / 2) + 1) * kBlobfsBlockSize;
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob_size);

  fprintf(stderr, "Writing %zu bytes...\n", blob_size);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
  fprintf(stderr, "Done writing %zu bytes\n", blob_size);
  ASSERT_EQ(syncfs(root_fd()), 0);
  ASSERT_OK(Unlink(delivery_blob.digest()));
}

}  // namespace
}  // namespace blobfs

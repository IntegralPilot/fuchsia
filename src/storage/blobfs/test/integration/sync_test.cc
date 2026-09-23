// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/errors.h>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <utility>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"
#include "src/storage/blobfs/test/integration/fdio_test.h"
#include "src/storage/fs_test/test_filesystem.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_protocol/block-fifo.h"

namespace blobfs {
namespace {

using SyncFdioTest = FdioTest;

}  // namespace

// Verifies that fdio "syncfs" calls actually sync blobs to the block device.
TEST_F(SyncFdioTest, Sync) {
  auto blob = TestBlobData::Create(64);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);

  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  std::atomic_uint64_t num_writes = 0;
  std::atomic_uint64_t num_flushes = 0;
  block_device()->set_hook([&](const BlockFifoRequest& request, const zx::vmo* vmo) {
    switch (request.command.opcode) {
      case BLOCK_OPCODE_WRITE:
        num_writes++;
        break;
      case BLOCK_OPCODE_FLUSH:
        num_flushes++;
        break;
      default:
        break;
    }
    return ZX_OK;
  });

  // Sync the filesystem.
  EXPECT_EQ(0, syncfs(root_fd()));

  EXPECT_GT(num_writes, 1lu);
  // There are 4 flushes:
  //  1. After writing data blocks but before writing to the journal.
  //  2. After writing to the journal but before writing to the final metadata location.
  //  3. Prior to writing the new info block.
  //  4. Finally, syncing the directory forces the block device to flush.
  EXPECT_EQ(4u, num_flushes);

  block_device()->set_hook({});
}

// Verifies that fdio "sync" actually flushes a NAND device. This tests the fdio, blobfs, block
// device, and FTL layers.
TEST(SyncNandTest, Sync) {
  // Make a VMO to give to the RAM-NAND.
  constexpr size_t kVmoSize{100 * static_cast<size_t>(4096 + 8) * 64};
  fzl::OwnedVmoMapper vmo;
  ASSERT_OK(vmo.CreateAndMap(kVmoSize, "vmo"));
  memset(vmo.start(), 0xff, kVmoSize);

  auto options = BlobfsWithFvmTestParam();
  options.use_ram_nand = true;
  options.vmo = vmo.vmo().borrow();
  options.device_block_count = 0;  // Uses VMO size.
  options.device_block_size = 8192;

  auto blob = TestBlobData::Create(64);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  auto snapshot = std::make_unique<uint8_t[]>(kVmoSize);

  {
    auto fs_or = fs_test::TestFilesystem::Create(options);
    ASSERT_TRUE(fs_or.is_ok()) << "Unable to create file system: " << fs_or.status_string();
    auto fs = std::move(fs_or).value();

    auto blob_creator = BlobCreatorWrapper::Connect(fs.ServiceDirectory());
    auto writer = blob_creator.Create(blob.digest());
    ASSERT_OK(writer->WriteBlob(delivery_blob));

    // This should block until the sync is complete.
    ASSERT_EQ(fsync(fs.GetRootFd().get()), 0);

    // Without closing the filesystem, create a snapshot. This will emulate a power cycle.
    memcpy(snapshot.get(), vmo.start(), kVmoSize);
  }

  // Restore snapshot and remount.
  memcpy(vmo.start(), snapshot.get(), kVmoSize);
  auto fs_or = fs_test::TestFilesystem::Open(options);
  ASSERT_OK(fs_or) << "Unable to open file system";
  auto fs = std::move(fs_or).value();

  auto blob_reader = BlobReaderWrapper::Connect(fs.ServiceDirectory());
  // The blob should exist and be exactly what we wrote.
  ASSERT_OK(blob_reader.VerifyBlob(blob));
}

}  // namespace blobfs

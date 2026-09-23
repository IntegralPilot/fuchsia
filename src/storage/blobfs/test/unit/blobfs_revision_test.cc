// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <zircon/errors.h>

#include <cstdint>
#include <cstdlib>
#include <memory>
#include <utility>

#include <gtest/gtest.h>

#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/compression/external_decompressor.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/fsck.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/vmo_buffer.h"

namespace blobfs {
namespace {

using ::block_client::BlockDevice;
using ::block_client::FakeBlockDevice;

constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = 400 * kBlobfsBlockSize / kBlockSize;

std::unique_ptr<BlockDevice> CreateFakeBlockDevice(uint64_t num_blocks) {
  return std::make_unique<FakeBlockDevice>(num_blocks, kBlockSize);
}

template <uint64_t oldest_minor_version,
          std::unique_ptr<BlockDevice> (*DeviceFactory)(uint64_t) = CreateFakeBlockDevice,
          uint64_t num_blocks = kNumBlocks>
class BlobfsTestAtMinorVersion : public BlobfsTestSetup, public testing::Test {
 public:
  void SetUp() final {
    srand(testing::UnitTest::GetInstance()->random_seed());
    auto connector_or = GetDecompressorCreatorConnector();
    ASSERT_TRUE(connector_or.is_ok());
    connector_ = connector_or.value();
  }

  std::unique_ptr<BlockDevice> CreateAndFormat() {
    FilesystemOptions options{.oldest_minor_version = oldest_minor_version};
    std::unique_ptr<BlockDevice> device = DeviceFactory(num_blocks);
    EXPECT_EQ(FormatFilesystem(device.get(), options), ZX_OK);
    return device;
  }

  MountOptions ReadOnlyOptions() const {
    return MountOptions{
        .writability = Writability::ReadOnlyDisk,
        .decompression_connector = connector_,
    };
  }

  MountOptions ReadWriteOptions() const {
    return MountOptions{
        .writability = Writability::Writable,
        .decompression_connector = connector_,
    };
  }

 private:
  DecompressorCreatorConnector* connector_;
};

using BlobfsTestAtRev2 = BlobfsTestAtMinorVersion<kBlobfsMinorVersionBackupSuperblock>;
using BlobfsTestAtRev4 =
    BlobfsTestAtMinorVersion<kBlobfsMinorVersionHostToolHandlesNullBlobCorrectly>;
using BlobfsTestAtFutureRev = BlobfsTestAtMinorVersion<kBlobfsCurrentMinorVersion + 1>;

// Writing v2 isn't supported, formatting should fail.
TEST_F(BlobfsTestAtRev2, WontFormat) {
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, Mount(CreateAndFormat(), ReadWriteOptions()));
}

// Tests that revision 2 won't load. This is a "rev 4" test because we can't actually write
// revision 2 formats. To bypass this, the test creates a rev 4 image and manually updates the
// superblock to rev 2.
TEST_F(BlobfsTestAtRev4, WontReadRev2) {
  Mount(CreateAndFormat(), ReadWriteOptions());
  auto device = Unmount();

  // Read the superblock block.
  {
    // Scope the vmo buffer. Destroying implicitly on test exit seems to cause ordering issues.
    storage::VmoBuffer buffer;
    ASSERT_EQ(buffer.Initialize(device.get(), 1, kBlobfsBlockSize, "test_buffer"), ZX_OK);
    BlockFifoRequest request{
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = buffer.vmoid(),
        .length = kBlobfsBlockSize / kBlockSize,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_EQ(device->FifoTransaction(&request, 1), ZX_OK);

    // Downgrade to revision 2.
    Superblock* info = reinterpret_cast<Superblock*>(buffer.Data(0));
    info->oldest_minor_version = kBlobfsMinorVersionBackupSuperblock;

    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_EQ(device->FifoTransaction(&request, 1), ZX_OK);
  }

  // The device should now fail to mount.
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, Mount(std::move(device)));
}

TEST_F(BlobfsTestAtRev4, NotUpgraded) {
  Mount(CreateAndFormat(), ReadWriteOptions());
  auto device = Unmount();

  // Read the superblock, verify the oldest revision is unmodified
  uint8_t block[kBlobfsBlockSize] = {};
  static_assert(sizeof(block) >= sizeof(Superblock));
  ASSERT_EQ(block_client::ReaderWriter(*device).Read(0, kBlobfsBlockSize, &block), ZX_OK);
  Superblock* info = reinterpret_cast<Superblock*>(block);
  EXPECT_EQ(info->oldest_minor_version, kBlobfsMinorVersionHostToolHandlesNullBlobCorrectly);

  ASSERT_EQ(Fsck(std::move(device), ReadOnlyOptions()), ZX_OK);
}

TEST_F(BlobfsTestAtFutureRev, OldestMinorVersionSetToDriverMinorVersion) {
  Mount(CreateAndFormat(), ReadWriteOptions());
  auto device = Unmount();

  // Read the superblock, verify the oldest revision is set to the current revision
  uint8_t block[kBlobfsBlockSize] = {};
  static_assert(sizeof(block) >= sizeof(Superblock));
  ASSERT_EQ(block_client::ReaderWriter(*device).Read(0, kBlobfsBlockSize, &block), ZX_OK);
  Superblock* info = reinterpret_cast<Superblock*>(block);
  EXPECT_EQ(info->oldest_minor_version, kBlobfsCurrentMinorVersion);

  ASSERT_EQ(Fsck(std::move(device), ReadOnlyOptions()), ZX_OK);
}

}  // namespace
}  // namespace blobfs

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blobfs_checker.h"

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/fit/function.h>
#include <lib/sync/completion.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <memory>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/blobfs/test/unit/utils.h"
#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/vmo_buffer.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {
namespace {

constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = 400 * kBlobfsBlockSize / kBlockSize;

// Expose access to ReloadSuperblock(). This allows tests to alter the
// Superblock on disk and force blobfs to reload it before running a check.
class TestBlobfs : public Blobfs {
 public:
  zx_status_t Reload() { return ReloadSuperblock(); }
};

class BlobfsCheckerTest : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(setup_.CreateFormatMount(kNumBlocks, kBlockSize));
    srand(testing::UnitTest::GetInstance()->random_seed());
  }

  void TearDown() override {
    // Some blobs can create vmos. The blobs will assert if the vmo is still attached on shutdown,
    // and running the loop ensures that any notifications are delivered.
    setup_.loop().RunUntilIdle();
  }

  // UpdateSuperblock writes the provided superblock to the block device and
  // forces blobfs to reload immediately.
  zx_status_t UpdateSuperblock(Superblock& superblock) {
    size_t superblock_size = kBlobfsBlockSize * SuperblockBlocks(superblock);
    DeviceBlockWrite(blobfs()->Device(), &superblock, superblock_size, kSuperblockOffset);
    return static_cast<TestBlobfs*>(blobfs())->Reload();
  }

  // Sync waits for blobfs to sync with the underlying block device.
  zx_status_t Sync() {
    sync_completion_t completion;
    blobfs()->Sync([&completion](zx_status_t status) { sync_completion_signal(&completion); });
    return sync_completion_wait(&completion, zx::duration::infinite().get());
  }

  // AddRandomBlob creates and writes a random blob to the file system. Optionally returns the block
  // the blob starts at if block_out is provided, the size of the blob if size_out is provided, and
  // the digest if digest_out is provided.
  void AddRandomBlob(size_t size = 1024, uint64_t* block_out = nullptr,
                     uint64_t* size_out = nullptr, Digest* digest_out = nullptr) {
    auto blob_data = TestBlobData::CreateRandom(size);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob_data);
    auto blob = CreateBlob(*blobfs(), delivery_blob);
    ASSERT_OK(blob);

    if (block_out) {
      // Get the block that contains the blob.
      *block_out =
          blobfs()->GetNode(blob->Ino())->extents[0].Start() + DataStartBlock(blobfs()->Info());
    }
    if (size_out) {
      *size_out = blob_data.data().size();
    }
    if (digest_out) {
      *digest_out = blob_data.digest();
    }
  }

  // Creates and writes a corrupt blob to the file system.
  void AddCorruptBlob() {
    uint64_t block, size;
    AddRandomBlob(1024, &block, &size);

    // Unmount.
    std::unique_ptr<block_client::BlockDevice> device = setup_.Unmount();

    // Read the block that contains the blob.
    storage::VmoBuffer buffer;
    ASSERT_OK(buffer.Initialize(device.get(), 1, kBlobfsBlockSize, "test_buffer"));
    BlockFifoRequest request{
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = buffer.vmoid(),
        .length = kBlobfsBlockSize / kBlockSize,
        .vmo_offset = 0,
        .dev_offset = block * kBlobfsBlockSize / kBlockSize,
    };
    ASSERT_OK(device->FifoTransaction(&request, 1));

    // Flip a random bit of the data.
    auto blob_data = static_cast<uint8_t*>(buffer.Data(0));
    size_t rand_index = rand() % size;
    uint8_t old_val = blob_data[rand_index];
    while ((blob_data[rand_index] = static_cast<uint8_t>(rand())) == old_val) {
    }

    // Write the block back.
    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_OK(device->FifoTransaction(&request, 1));

    // Remount.
    ASSERT_OK(setup_.Mount(std::move(device)));
  }

  Blobfs* blobfs() { return setup_.blobfs(); }

  void CorruptNode(fit::callback<void(Inode& node)> corrupt_fn,
                   std::unique_ptr<BlockDevice>* device_out) {
    Digest digest;
    AddRandomBlob(1024, nullptr, nullptr, &digest);
    auto blob = GetBlob(*blobfs(), digest);
    ASSERT_OK(blob);

    const uint32_t inode = blob->Ino();
    const uint64_t node_block =
        NodeMapStartBlock(blobfs()->Info()) + (inode / kBlobfsInodesPerBlock);

    blob.value().reset();

    auto device = setup_.Unmount();

    storage::VmoBuffer buffer;
    ASSERT_OK(buffer.Initialize(device.get(), 1, kBlobfsBlockSize, "test_buffer"));
    BlockFifoRequest request{
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = buffer.vmoid(),
        .length = kBlobfsBlockSize / kBlockSize,
        .vmo_offset = 0,
        .dev_offset = node_block * kBlobfsBlockSize / kBlockSize,
    };
    ASSERT_OK(device->FifoTransaction(&request, 1));

    Inode& node = reinterpret_cast<Inode*>(buffer.Data(0))[inode % kBlobfsInodesPerBlock];

    // Quick check to give us confidence that we got the right node.
    EXPECT_EQ(node.blob_size, 1024u);

    corrupt_fn(node);

    // Write the change back.
    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_OK(device->FifoTransaction(&request, 1));

    *device_out = std::move(device);
  }

  void CorruptExtentContainer(fit::callback<void(ExtentContainer& container)> corrupt_fn,
                              fit::callback<void(Inode& node)> corrupt_node_fn,
                              std::unique_ptr<BlockDevice>* device_out) {
    fbl::RefPtr<fs::Vnode> root;
    ASSERT_OK(blobfs()->OpenRootNode(&root));

    // We need to create a blob that uses an extent container, so to do that, we fragment the free
    // space by creating some blobs and then deleting every other one.
    std::vector<Digest> digests;
    for (int i = 0; i < 16; ++i) {
      Digest digest;
      AddRandomBlob(1024, nullptr, nullptr, &digest);
      digests.push_back(digest);
    }
    for (int i = 0; i < 16; i += 2) {
      ASSERT_OK(root->Unlink(digests[i].ToString(), false));
    }

    // Sync now because the blocks are reserved until the journal is flushed.
    sync_completion_t sync;
    root->Sync([&](zx_status_t status) { sync_completion_signal(&sync); });
    sync_completion_wait(&sync, ZX_TIME_INFINITE);

    // This should end up creating a blob that typically (depending on compression) uses 7 extents.
    Digest digest;
    AddRandomBlob(static_cast<size_t>(6) * 8192, nullptr, nullptr, &digest);

    auto blob = GetBlob(*blobfs(), digest);
    ASSERT_OK(blob);

    const uint32_t inode = blob->Ino();
    auto node = blobfs()->GetNode(inode);

    const uint32_t first_extent_container = node->header.next_node;

    // Check that it did actually use an extent container.
    ASSERT_NE(node->header.next_node, 0u);

    const uint64_t node_map_start_block = NodeMapStartBlock(blobfs()->Info());
    const uint64_t first_extent_container_block =
        node_map_start_block + (first_extent_container / kBlobfsInodesPerBlock);

    // Assume that the inode and extent container are in the same block.
    ASSERT_EQ(first_extent_container / kBlobfsInodesPerBlock, inode / kBlobfsInodesPerBlock);

    node.value().reset();
    blob.value().reset();
    root.reset();

    auto device = setup_.Unmount();

    storage::VmoBuffer buffer;
    ASSERT_OK(buffer.Initialize(device.get(), 1, kBlobfsBlockSize, "test_buffer"));
    BlockFifoRequest request{
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = buffer.vmoid(),
        .length = kBlobfsBlockSize / kBlockSize,
        .vmo_offset = 0,
        .dev_offset = first_extent_container_block * kBlobfsBlockSize / kBlockSize,
    };
    ASSERT_OK(device->FifoTransaction(&request, 1));

    ExtentContainer& container = reinterpret_cast<ExtentContainer*>(
        buffer.Data(0))[first_extent_container % kBlobfsInodesPerBlock];
    EXPECT_EQ(container.previous_node, inode);

    if (corrupt_node_fn) {
      Inode& node = reinterpret_cast<Inode*>(buffer.Data(0))[inode % kBlobfsInodesPerBlock];
      corrupt_node_fn(node);
    }
    corrupt_fn(container);

    // Write the change back.
    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_OK(device->FifoTransaction(&request, 1));

    *device_out = std::move(device);
  }

 protected:
  BlobfsTestSetup setup_;
};

TEST_F(BlobfsCheckerTest, TestEmpty) {
  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestNonEmpty) {
  for (unsigned i = 0; i < 3; i++) {
    AddRandomBlob();
  }
  EXPECT_OK(Sync());

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestInodeWithUnallocatedBlock) {
  for (unsigned i = 0; i < 3; i++) {
    AddRandomBlob();
  }
  EXPECT_OK(Sync());

  Extent e(1, 1);
  blobfs()->GetAllocator()->FreeBlocks(e);

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

// TODO(https://fxbug.dev/45924): determine why running this test on an
// empty blobfs fails on ASAN QEMU bot.
TEST_F(BlobfsCheckerTest, TestAllocatedBlockCountTooHigh) {
  AddRandomBlob();
  EXPECT_OK(Sync());

  Superblock superblock = blobfs()->Info();
  superblock.alloc_block_count++;
  ASSERT_OK(UpdateSuperblock(superblock));

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestAllocatedBlockCountTooLow) {
  for (unsigned i = 0; i < 3; i++) {
    AddRandomBlob();
  }
  EXPECT_OK(Sync());

  Superblock superblock = blobfs()->Info();
  superblock.alloc_block_count = 2;
  UpdateSuperblock(superblock);

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestFewerThanMinimumBlocksAllocated) {
  Extent e(0, 1);
  blobfs()->GetAllocator()->FreeBlocks(e);
  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestAllocatedInodeCountTooHigh) {
  AddRandomBlob();
  EXPECT_OK(Sync());

  Superblock superblock = blobfs()->Info();
  superblock.alloc_inode_count++;
  UpdateSuperblock(superblock);

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestAllocatedInodeCountTooLow) {
  for (unsigned i = 0; i < 3; i++) {
    AddRandomBlob();
  }
  EXPECT_OK(Sync());

  Superblock superblock = blobfs()->Info();
  superblock.alloc_inode_count = 2;
  UpdateSuperblock(superblock);

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, TestCorruptBlobs) {
  for (unsigned i = 0; i < 5; i++) {
    if (i % 2 == 0) {
      AddRandomBlob();
    } else {
      AddCorruptBlob();
    }
  }
  EXPECT_OK(Sync());

  BlobfsChecker checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(checker.Check());
}

TEST_F(BlobfsCheckerTest, BadPreviousNode) {
  std::unique_ptr<BlockDevice> device;
  CorruptExtentContainer([](ExtentContainer& container) { ++container.previous_node; }, {},
                         &device);
  // At present, this issue gets caught at mount time.
  ASSERT_STATUS(setup_.Mount(std::move(device)), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_F(BlobfsCheckerTest, CorruptReserved) {
  std::unique_ptr<BlockDevice> device;
  CorruptNode([](Inode& node) { node.reserved = 1; }, &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptFlags) {
  std::unique_ptr<BlockDevice> device;
  CorruptNode([](Inode& node) { node.header.flags |= ~kBlobFlagMaskValid; }, &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptVersion) {
  std::unique_ptr<BlockDevice> device;
  CorruptNode([](Inode& node) { node.header.version = kBlobNodeVersion + 1; }, &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptFlagsInExtentContainer) {
  std::unique_ptr<BlockDevice> device;
  CorruptExtentContainer(
      [](ExtentContainer& container) { container.header.flags |= kBlobFlagChunkCompressed; }, {},
      &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptNextNodeInInode) {
  std::unique_ptr<BlockDevice> device;
  CorruptNode(
      [](Inode& node) {
        // This only works because corrupt node currently only creates one extent container.
        ASSERT_EQ(node.header.next_node, kMaxNodeId);
        node.header.next_node = 0;
      },
      &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptNextNodeInExtentContainer) {
  std::unique_ptr<BlockDevice> device;
  CorruptExtentContainer(
      [](ExtentContainer& container) {
        // This only works because CorruptExtentContainer currently only creates one extent
        // container, so this should be the last one.
        ASSERT_EQ(container.header.next_node, kMaxNodeId);
        container.header.next_node = 0;
      },
      {}, &device);
  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptUnallocatedNode) {
  const uint64_t node_map_start_block = NodeMapStartBlock(blobfs()->Info());

  // Unmount.
  auto device = setup_.Unmount();

  {
    storage::VmoBuffer buffer;
    ASSERT_OK(buffer.Initialize(device.get(), 1, kBlobfsBlockSize, "test_buffer"));
    BlockFifoRequest request{
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = buffer.vmoid(),
        .length = kBlobfsBlockSize / kBlockSize,
        .vmo_offset = 0,
        .dev_offset = node_map_start_block * kBlobfsBlockSize / kBlockSize,
    };
    ASSERT_OK(device->FifoTransaction(&request, 1));

    Inode* node = reinterpret_cast<Inode*>(buffer.Data(0));
    unsigned i;
    for (i = 0; i < kBlobfsInodesPerBlock && node[i].header.IsAllocated(); ++i) {
    }
    ASSERT_LT(i, kBlobfsInodesPerBlock);

    node[i].header.flags = kBlobFlagChunkCompressed;

    // Write the change back.
    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_OK(device->FifoTransaction(&request, 1));
  }

  ASSERT_OK(setup_.Mount(std::move(device)));

  BlobfsChecker strict_checker(blobfs(), BlobfsChecker::Options{.strict = true});
  EXPECT_FALSE(strict_checker.Check());

  BlobfsChecker checker(blobfs());
  EXPECT_TRUE(checker.Check());
}

TEST_F(BlobfsCheckerTest, CorruptExtentCountInExtentContainer) {
  std::unique_ptr<BlockDevice> device;
  CorruptExtentContainer([](ExtentContainer& container) { container.extent_count += 100; },
                         [](Inode& node) { node.extent_count += 100; }, &device);
  // At present, this issue gets caught at mount time.
  ASSERT_STATUS(setup_.Mount(std::move(device)), ZX_ERR_IO_DATA_INTEGRITY);
}

}  // namespace
}  // namespace blobfs

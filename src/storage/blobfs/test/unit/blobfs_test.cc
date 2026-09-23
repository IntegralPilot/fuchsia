// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blobfs.h"

#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.storage.blobfs/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/sync/completion.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_creator.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs_inspect_tree.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/compression/external_decompressor.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/service/overwrite_configuration.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/blobfs/transaction.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/vmo_buffer.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {
namespace {

using ::block_client::FakeBlockDevice;

constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = 400 * kBlobfsBlockSize / kBlockSize;
constexpr uint32_t kNumNodes = 128;

class MockBlockDevice : public FakeBlockDevice {
 public:
  MockBlockDevice(uint64_t block_count, uint32_t block_size)
      : FakeBlockDevice(block_count, block_size) {}

  static std::unique_ptr<MockBlockDevice> CreateAndFormat(const FilesystemOptions& options,
                                                          uint64_t num_blocks) {
    auto device = std::make_unique<MockBlockDevice>(num_blocks, kBlockSize);
    EXPECT_EQ(FormatFilesystem(device.get(), options), ZX_OK);
    return device;
  }

  bool saw_trim() const { return saw_trim_; }

  zx_status_t FifoTransaction(BlockFifoRequest* requests, size_t count) final;
  zx_status_t BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* info) const final;

 private:
  bool saw_trim_ = false;
};

zx_status_t MockBlockDevice::FifoTransaction(BlockFifoRequest* requests, size_t count) {
  for (size_t i = 0; i < count; i++) {
    if (requests[i].command.opcode == BLOCK_OPCODE_TRIM) {
      saw_trim_ = true;
      return ZX_OK;
    }
  }
  return FakeBlockDevice::FifoTransaction(requests, count);
}

zx_status_t MockBlockDevice::BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* info) const {
  zx_status_t status = FakeBlockDevice::BlockGetInfo(info);
  if (status == ZX_OK) {
    info->flags |= fuchsia_storage_block::wire::DeviceFlag::kTrimSupport;
  }
  return status;
}

template <uint64_t oldest_minor_version, uint64_t num_blocks = kNumBlocks,
          typename Device = MockBlockDevice>
class BlobfsTestAtRevision : public BlobfsTestSetup, public testing::Test {
 public:
  void SetUp() final {
    FilesystemOptions fs_options{.oldest_minor_version = oldest_minor_version};
    auto device = Device::CreateAndFormat(fs_options, num_blocks);
    ASSERT_TRUE(device);
    device_ = device.get();

    auto connector_or = GetDecompressorCreatorConnector();
    ASSERT_TRUE(connector_or.is_ok());
    connector_ = connector_or.value();

    ASSERT_EQ(ZX_OK, Mount(std::move(device), GetMountOptions()));

    srand(testing::UnitTest::GetInstance()->random_seed());
  }

  void TearDown() final {
    // Process any pending notifications before tearing down blobfs (necessary for paged vmos).
    loop().RunUntilIdle();
  }

 protected:
  virtual MountOptions GetMountOptions() const {
    return MountOptions{
        .decompression_connector = connector_,
    };
  }

  DecompressorCreatorConnector* connector_;
  Device* device_ = nullptr;
};

using BlobfsTest = BlobfsTestAtRevision<blobfs::kBlobfsCurrentMinorVersion>;

TEST_F(BlobfsTest, GetDevice) { ASSERT_EQ(device_, blobfs()->GetDevice()); }

TEST_F(BlobfsTest, BlockNumberToDevice) {
  ASSERT_EQ(42 * kBlobfsBlockSize / kBlockSize, blobfs()->BlockNumberToDevice(42));
}

TEST_F(BlobfsTest, CleanFlag) {
  // Scope all operations while the filesystem is alive to ensure they
  // don't have dangling references once it is destroyed.
  {
    storage::VmoBuffer buffer;
    ASSERT_EQ(buffer.Initialize(blobfs(), 1, kBlobfsBlockSize, "source"), ZX_OK);

    // Write the superblock with the clean flag unset on Blobfs::Create in Setup.
    storage::Operation operation = {};
    memcpy(buffer.Data(0), &blobfs()->Info(), sizeof(Superblock));
    operation.type = storage::OperationType::kWrite;
    operation.dev_offset = 0;
    operation.length = 1;

    ASSERT_EQ(blobfs()->RunOperation(operation, &buffer), ZX_OK);

    // Read the superblock with the clean flag unset.
    operation.type = storage::OperationType::kRead;
    ASSERT_EQ(blobfs()->RunOperation(operation, &buffer), ZX_OK);
    Superblock* info = reinterpret_cast<Superblock*>(buffer.Data(0));
    EXPECT_EQ(0u, (info->flags & kBlobFlagClean));
  }

  // Destroy the blobfs instance to force writing of the clean bit.
  auto device = Unmount();

  // Read the superblock, verify the clean flag is set.
  uint8_t block[kBlobfsBlockSize] = {};
  static_assert(sizeof(block) >= sizeof(Superblock));
  block_client::ReaderWriter reader(*device);
  ASSERT_EQ(reader.Read(0, kBlobfsBlockSize, &block), ZX_OK);
  Superblock* info = reinterpret_cast<Superblock*>(block);
  EXPECT_EQ(kBlobFlagClean, (info->flags & kBlobFlagClean));
}

// Tests reading a well known location.
TEST_F(BlobfsTest, RunOperationExpectedRead) {
  storage::VmoBuffer buffer;
  ASSERT_EQ(buffer.Initialize(blobfs(), 1, kBlobfsBlockSize, "source"), ZX_OK);

  // Read the first block.
  storage::Operation operation = {};
  operation.type = storage::OperationType::kRead;
  operation.length = 1;
  ASSERT_EQ(blobfs()->RunOperation(operation, &buffer), ZX_OK);

  uint64_t* data = reinterpret_cast<uint64_t*>(buffer.Data(0));
  EXPECT_EQ(kBlobfsMagic0, data[0]);
  EXPECT_EQ(kBlobfsMagic1, data[1]);
}

// Tests that we can read back what we write.
TEST_F(BlobfsTest, RunOperationReadWrite) {
  char data[kBlobfsBlockSize] = "something to test";

  storage::VmoBuffer buffer;
  ASSERT_EQ(buffer.Initialize(blobfs(), 1, kBlobfsBlockSize, "source"), ZX_OK);
  memcpy(buffer.Data(0), data, kBlobfsBlockSize);

  storage::Operation operation = {};
  operation.type = storage::OperationType::kWrite;
  operation.dev_offset = 1;
  operation.length = 1;

  ASSERT_EQ(blobfs()->RunOperation(operation, &buffer), ZX_OK);

  memset(buffer.Data(0), 'a', kBlobfsBlockSize);
  operation.type = storage::OperationType::kRead;
  ASSERT_EQ(blobfs()->RunOperation(operation, &buffer), ZX_OK);

  ASSERT_EQ(memcmp(data, buffer.Data(0), kBlobfsBlockSize), 0);
}

TEST_F(BlobfsTest, TrimsData) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(1024);
  auto blob = CreateBlob(*blobfs(), delivery_blob);
  ASSERT_OK(blob);

  EXPECT_FALSE(device_->saw_trim());
  ASSERT_OK(blob->QueueUnlink());

  sync_completion_t completion;
  blobfs()->Sync([&completion](zx_status_t status) { sync_completion_signal(&completion); });
  EXPECT_EQ(sync_completion_wait(&completion, zx::duration::infinite().get()), ZX_OK);

  ASSERT_TRUE(device_->saw_trim());
}

class BlobfsOverwriteStatusTest : public BlobfsTestSetupWithThread, public testing::Test {};

TEST_F(BlobfsOverwriteStatusTest, ChangeOverwriteConfig) {
  ASSERT_OK(CreateFormatMount(1024, kBlobfsBlockSize));

  auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fxfs::BlobCreator>,
                    fbl::MakeRefCounted<BlobCreator>(*blobfs()));
  svc_dir->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_storage_blobfs::OverwriteConfiguration>,
      fbl::MakeRefCounted<OverwriteConfigurationService>(loop().dispatcher(), *blobfs()));

  auto svc_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_OK(vfs()->ServeDirectory(std::move(svc_dir), std::move(svc_endpoints->server)));
  BlobCreatorWrapper creator = BlobCreatorWrapper::Connect(svc_endpoints->client.borrow());
  auto client_end =
      component::ConnectAt<fuchsia_storage_blobfs::OverwriteConfiguration>(svc_endpoints->client);
  ASSERT_OK(client_end);
  fidl::WireSyncClient client(std::move(*client_end));

  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToCompact);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  TestDeliveryBlob blob = TestDeliveryBlob::CreateUncompressed(16000, 7);
  ASSERT_OK(creator.CreateAndWriteBlob(blob));

  ASSERT_FALSE(creator.NeedsOverwrite(blob.digest()).value());
  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kNoOverwrite);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(blobfs()->OverwriteConfig(), BlobOverwriteConfig::kNoOverwrite);
  ASSERT_FALSE(creator.NeedsOverwrite(blob.digest()).value());

  // Shift to padded format. Now wants an overwrite.
  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToPadded);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(blobfs()->OverwriteConfig(), BlobOverwriteConfig::kOverwriteToPadded);
  ASSERT_TRUE(creator.NeedsOverwrite(blob.digest()).value());
  {
    BlobWriterWrapper writer = creator.CreateExisting(blob.digest()).value();
    ASSERT_OK(writer.WriteBlob(blob));
  }

  ASSERT_FALSE(creator.NeedsOverwrite(blob.digest()).value());
  // No overwrite never wants and overwrite.
  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kNoOverwrite);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(blobfs()->OverwriteConfig(), BlobOverwriteConfig::kNoOverwrite);
  ASSERT_FALSE(creator.NeedsOverwrite(blob.digest()).value());

  // Shift back to compact format, now it wants an overwrite.
  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToCompact);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  ASSERT_TRUE(creator.NeedsOverwrite(blob.digest()).value());
}

TEST_F(BlobfsOverwriteStatusTest, AlwaysReturnsNotFoundForUnfinishedBlob) {
  ASSERT_OK(CreateFormatMount(1024, kBlobfsBlockSize));

  auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fxfs::BlobCreator>,
                    fbl::MakeRefCounted<BlobCreator>(*blobfs()));
  svc_dir->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_storage_blobfs::OverwriteConfiguration>,
      fbl::MakeRefCounted<OverwriteConfigurationService>(loop().dispatcher(), *blobfs()));

  auto svc_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_OK(vfs()->ServeDirectory(std::move(svc_dir), std::move(svc_endpoints->server)));
  BlobCreatorWrapper creator = BlobCreatorWrapper::Connect(svc_endpoints->client.borrow());
  auto client_end =
      component::ConnectAt<fuchsia_storage_blobfs::OverwriteConfiguration>(svc_endpoints->client);
  ASSERT_OK(client_end);
  fidl::WireSyncClient client(std::move(*client_end));

  ASSERT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  TestDeliveryBlob blob = TestDeliveryBlob::CreateUncompressed(16000, 7);
  auto writer = creator.Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, blob.data().size() - 1));
  ASSERT_OK(writer->BytesReady(blob.data().size() - 1));

  // Should be NOT_FOUND regardless of overwrite config.
  ASSERT_EQ(creator.NeedsOverwrite(blob.digest()).error_value(), ZX_ERR_NOT_FOUND);

  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToCompact);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(creator.NeedsOverwrite(blob.digest()).error_value(), ZX_ERR_NOT_FOUND);

  {
    auto result = client->Set(fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToPadded);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
  }
  ASSERT_EQ(creator.NeedsOverwrite(blob.digest()).error_value(), ZX_ERR_NOT_FOUND);
}

TEST_F(BlobfsTest, GetNodeWithAnInvalidNodeIndexIsAnError) {
  uint32_t invalid_node_index = kMaxNodeId - 1;
  auto node = blobfs()->GetNode(invalid_node_index);
  EXPECT_EQ(node.status_value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(BlobfsTest, FreeInodeWithAnInvalidNodeIndexIsAnError) {
  BlobTransaction transaction;
  uint32_t invalid_node_index = kMaxNodeId - 1;
  EXPECT_EQ(blobfs()->FreeInode(invalid_node_index, transaction), ZX_ERR_INVALID_ARGS);
}

TEST_F(BlobfsTest, BlockIteratorByNodeIndexWithAnInvalidNodeIndexIsAnError) {
  uint32_t invalid_node_index = kMaxNodeId - 1;
  auto block_iterator = blobfs()->BlockIteratorByNodeIndex(invalid_node_index);
  EXPECT_EQ(block_iterator.status_value(), ZX_ERR_INVALID_ARGS);
}

using BlobfsTestWithLargeDevice =
    BlobfsTestAtRevision<blobfs::kBlobfsCurrentMinorVersion,
                         /*num_blocks=*/2560 * kBlobfsBlockSize / kBlockSize>;

TEST_F(BlobfsTestWithLargeDevice, WritingBlobLargerThanWritebackCapacitySucceeds) {
  fbl::RefPtr<fs::Vnode> root;
  ASSERT_OK(blobfs()->OpenRootNode(&root));

  auto blob_data = TestBlobData::Create((blobfs()->WriteBufferBlockCount() + 1) * kBlobfsBlockSize);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob_data);
  auto blob = CreateBlob(*blobfs(), delivery_blob);
  // If this starts to fail with an ERR_NO_SPACE error it could be because WriteBufferBlockCount()
  // has changed and is now returning something too big for the device we're using in this test.
  ASSERT_OK(blob);

  sync_completion_t sync;
  root->Sync([&](zx_status_t status) {
    EXPECT_OK(status);
    sync_completion_signal(&sync);
  });
  sync_completion_wait(&sync, ZX_TIME_INFINITE);

  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));
}

#ifndef NDEBUG

class FsckAtEndOfEveryTransactionTest : public BlobfsTest {
 protected:
  MountOptions GetMountOptions() const override {
    MountOptions options = BlobfsTest::GetMountOptions();
    options.fsck_at_end_of_every_transaction = true;
    return options;
  }
};

TEST_F(FsckAtEndOfEveryTransactionTest, FsckAtEndOfEveryTransaction) {
  auto blob_data = TestBlobData::CreateRealistic(500123);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob_data);
  auto blob = CreateBlob(*blobfs(), delivery_blob);
  ASSERT_OK(blob);
  blob->QueueUnlink();

  blobfs()->Sync([loop = &loop()](zx_status_t) { loop->Quit(); });
  loop().Run();
}

#endif  // !defined(NDEBUG)

/*
void VnodeSync(fs::Vnode* vnode) {
  // It's difficult to get a precise hook into the period between when data has been written and
  // when it has been flushed to disk.  The journal will delay flushing metadata, so the following
  // should test sync being called before metadata has been flushed, and then again afterwards.
  for (int i = 0; i < 2; ++i) {
    sync_completion_t sync;
    vnode->Sync([&](zx_status_t status) {
      EXPECT_EQ(ZX_OK, status);
      sync_completion_signal(&sync);
    });
    sync_completion_wait(&sync, ZX_TIME_INFINITE);
  }
}
*/

// In this test we try to simulate fragmentation and test fragmentation metrics. We create
// fragmentation by first creating few blobs, deleting a subset of those blobs and then finally
// creating a huge blob that occupies all the blocks freed by blob deletion. We measure/verify
// metrics at each stage.
// This test has an understanding about block allocation policy.

void FragmentationStatsEqual(const FragmentationStats& lhs, const FragmentationStats& rhs) {
  EXPECT_EQ(lhs.total_nodes, rhs.total_nodes);
  EXPECT_EQ(lhs.files_in_use, rhs.files_in_use);
  EXPECT_EQ(lhs.extent_containers_in_use, rhs.extent_containers_in_use);
  EXPECT_EQ(lhs.extents_per_file, rhs.extents_per_file);
  EXPECT_EQ(lhs.free_fragments, rhs.free_fragments);
  EXPECT_EQ(lhs.in_use_fragments, rhs.in_use_fragments);
}

TEST(BlobfsFragmentationTest, FragmentationMetrics) {
  FragmentationMetrics stub_metrics;
  auto device = MockBlockDevice::CreateAndFormat(
      {
          .oldest_minor_version = kBlobfsCurrentMinorVersion,
          .num_inodes = kNumNodes,
      },
      kNumBlocks);
  ASSERT_TRUE(device);

  BlobfsTestSetup setup;
  ASSERT_EQ(ZX_OK, setup.Mount(std::move(device), {}));

  srand(testing::UnitTest::GetInstance()->random_seed());

  {
    FragmentationStats expected{};
    expected.total_nodes = setup.blobfs()->Info().inode_count;
    // All fragments should be free since we didn't create any files yet.
    expected.free_fragments[setup.blobfs()->Info().data_block_count - 1] = 1;
    FragmentationStats actual;
    setup.blobfs()->CalculateFragmentationMetrics(stub_metrics, &actual);
    ASSERT_NO_FATAL_FAILURE(FragmentationStatsEqual(expected, actual));
  }

  fbl::RefPtr<fs::Vnode> root;
  ASSERT_EQ(setup.blobfs()->OpenRootNode(&root), ZX_OK);
  std::vector<Digest> digests;
  constexpr int kSmallBlobCount = 10;
  digests.reserve(kSmallBlobCount);
  // We create 10 blobs that occupy 1 block each. After these creation, data block bitmap should
  // look like (first 10 bits set and all other bits unset.)
  // 111111111100000000....
  for (uint8_t i = 0; i < kSmallBlobCount; i++) {
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(64, i);
    ASSERT_OK(CreateBlob(*setup.blobfs(), delivery_blob));
    digests.push_back(delivery_blob.digest());
  }

  // The last free fragment should reflect the number of blocks we allocated.
  uint64_t last_free_fragment = setup.blobfs()->Info().data_block_count - kSmallBlobCount;

  {
    FragmentationStats expected{};
    expected.total_nodes = setup.blobfs()->Info().inode_count;
    expected.files_in_use = kSmallBlobCount;
    // Each blob should only use a single extent.
    expected.extents_per_file[1] = kSmallBlobCount;
    expected.in_use_fragments[1] = kSmallBlobCount;
    expected.free_fragments[last_free_fragment - 1] = 1;
    FragmentationStats actual;
    setup.blobfs()->CalculateFragmentationMetrics(stub_metrics, &actual);
    ASSERT_NO_FATAL_FAILURE(FragmentationStatsEqual(expected, actual));
  }

  // Delete few blobs. Notice the pattern we delete. With these deletions free(0) and used(1)
  // block bitmap will look as follows 1010100111000000... This creates 4 free fragments. 6 used
  // fragments.
  constexpr uint64_t kBlobsDeleted = 4;
  for (size_t i : {1, 3, 5, 6}) {
    auto blob = GetBlob(*setup.blobfs(), digests[i]);
    ASSERT_OK(blob);
    ASSERT_OK(blob->QueueUnlink());
  }

  // Ensure that all reserved extents get returned.
  {
    sync_completion_t sync_done;
    root->Sync([&sync_done](zx_status_t) { sync_completion_signal(&sync_done); });
    sync_completion_wait(&sync_done, ZX_TIME_INFINITE);
  }

  {
    FragmentationStats expected{};
    expected.total_nodes = setup.blobfs()->Info().inode_count;
    expected.files_in_use = kSmallBlobCount - kBlobsDeleted;
    expected.free_fragments[1] = 2;
    expected.free_fragments[2] = 1;
    expected.free_fragments[last_free_fragment - 1] = 1;
    expected.extents_per_file[1] = kSmallBlobCount - kBlobsDeleted;
    expected.in_use_fragments[1] = kSmallBlobCount - kBlobsDeleted;
    FragmentationStats actual;
    setup.blobfs()->CalculateFragmentationMetrics(stub_metrics, &actual);
    ASSERT_NO_FATAL_FAILURE(FragmentationStatsEqual(expected, actual));
  }

  // Create a huge (20 blocks) blob that potentially fills at least three free fragments that we
  // created above.
  const uint64_t kLargeFileNumBlocks = 20;
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(kLargeFileNumBlocks * kBlobfsBlockSize);
  auto blob = CreateBlob(*setup.blobfs(), delivery_blob);
  ASSERT_OK(blob);
  auto node = setup.blobfs()->GetNode(blob->Ino());
  ASSERT_OK(node);
  uint64_t blocks = node->block_count;
  ASSERT_GT(blocks, kBlobsDeleted);

  {
    FragmentationStats expected{};
    expected.total_nodes = setup.blobfs()->Info().inode_count;
    expected.files_in_use = kSmallBlobCount - kBlobsDeleted + 1;
    expected.extent_containers_in_use = 1;
    // The end gets pushed out by the new blob minus the 4 blocks it took from the old blobs.
    expected.free_fragments[last_free_fragment - blocks + 4 - 1] = 1;
    expected.extents_per_file[1] = kSmallBlobCount - kBlobsDeleted;
    // The large file we create should span three extents.
    expected.extents_per_file[4] = 1;
    // 2 small blobs were deleted side-by-side. They merge into one fragment.
    expected.in_use_fragments[1] = kSmallBlobCount - 2;
    expected.in_use_fragments[2] = 1;
    expected.in_use_fragments[blocks - kBlobsDeleted] = 1;
    FragmentationStats actual;
    setup.blobfs()->CalculateFragmentationMetrics(stub_metrics, &actual);
    ASSERT_NO_FATAL_FAILURE(FragmentationStatsEqual(expected, actual));
  }
}

TEST_F(BlobfsTest, MemoryUse) {
  blobfs()->GetAllocator()->Decommit();

  zx_info_vmo_t info[128];
  size_t actual;
  ASSERT_EQ(
      zx::process::self()->get_info(ZX_INFO_PROCESS_VMOS, info, sizeof(info), &actual, nullptr),
      ZX_OK);

  for (size_t i = 0; i < actual; ++i) {
    if (!strcmp(info[i].name, "nodemap")) {
      // It's an empty blobfs, so it should have no committed bytes in the nodemap.
      ASSERT_EQ(info[i].committed_bytes, 0ul);
      return;
    }
  }
  ADD_FAILURE() << "Unable to find nodemap VMO";
}

}  // namespace
}  // namespace blobfs

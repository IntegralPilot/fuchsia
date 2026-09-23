// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob.h"

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <tuple>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/digest/digest.h"
#include "src/lib/digest/node-digest.h"
#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/blobfs/test/unit/utils.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/vmo_buffer.h"

namespace blobfs {

namespace {

constexpr uint32_t kTestDeviceBlockSize = 512;
constexpr uint32_t kTestDeviceNumBlocks = 400 * kBlobfsBlockSize / kTestDeviceBlockSize;

const size_t kPageSize = zx_system_get_page_size();

}  // namespace

class BlobTest : public BlobfsTestSetup,
                 public testing::TestWithParam<std::tuple<BlobLayoutFormat, CompressionAlgorithm>> {
 public:
  // Tests that need to test migration from a specific revision can override this method to
  // specify an older minor revision. See also blobfs_revision_test.cc for general migration tests.
  virtual uint64_t GetOldestMinorVersion() const { return kBlobfsCurrentMinorVersion; }

  void SetUp() override {
    auto device =
        std::make_unique<block_client::FakeBlockDevice>(kTestDeviceNumBlocks, kTestDeviceBlockSize);
    device->set_hook([this](const BlockFifoRequest& request, const zx::vmo* vmo) {
      std::lock_guard l(this->hook_lock_);
      if (hook_) {
        return hook_(request, vmo);
      }
      return ZX_OK;
    });

    FilesystemOptions filesystem_options{
        .blob_layout_format = std::get<0>(GetParam()),
        .oldest_minor_version = GetOldestMinorVersion(),
    };
    ASSERT_OK(FormatFilesystem(device.get(), filesystem_options));
    ASSERT_OK(Mount(std::move(device)));
  }

  static CompressionAlgorithm GetCompressionAlgorithm() { return std::get<1>(GetParam()); }

  static const zx::vmo& GetPagedVmo(Blob& blob) {
    std::lock_guard lock(blob.mutex_);
    return blob.paged_vmo();
  }

  static bool IsPurgeable(Blob& blob) {
    std::lock_guard lock(blob.mutex_);
    return blob.Purgeable();
  }

  static bool IsDeletionQueued(Blob& blob) { return blob.DeletionQueued(); }

  void set_hook(block_client::FakeBlockDevice::Hook hook) {
    std::lock_guard l(this->hook_lock_);
    hook_ = std::move(hook);
  }

  zx::result<fbl::RefPtr<Blob>> CreateBlob(const TestBlobData& blob_data) {
    return CreateBlob(
        TestDeliveryBlob::CreateWithCompressionAlgorithm(blob_data, GetCompressionAlgorithm()));
  }

  zx::result<fbl::RefPtr<Blob>> CreateBlob(const TestDeliveryBlob& delivery_blob) {
    return blobfs::CreateBlob(*blobfs(), delivery_blob);
  }

  zx::result<fbl::RefPtr<Blob>> GetBlob(const Digest& digest) {
    return blobfs::GetBlob(*blobfs(), digest);
  }

  zx::result<> WaitForZeroChildren(const Blob& blob) {
    constexpr auto kMaxWait = std::chrono::seconds(60);
    const auto start = std::chrono::steady_clock::now();
    while (std::chrono::steady_clock::now() <= start + kMaxWait) {
      loop().RunUntilIdle();
      std::lock_guard lock(blob.mutex_);
      if (!blob.HasReferences()) {
        return zx::ok();
      }
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
    return zx::error(ZX_ERR_TIMED_OUT);
  }

 private:
  std::mutex hook_lock_;
  block_client::FakeBlockDevice::Hook hook_ __TA_GUARDED(hook_lock_);
};

namespace {

TEST_P(BlobTest, ReadingBlobZerosTail) {
  // An uncompressed blob is used so the exact size of it in storage is known.
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(64);
  uint64_t block;
  // Create the blob and pull out of the block where the blob contents were written.
  {
    auto blob = CreateBlob(delivery_blob);
    ASSERT_OK(blob);
    block = blobfs()->GetNode(blob->Ino())->extents[0].Start() + DataStartBlock(blobfs()->Info());
  }

  auto block_device = Unmount();

  // Read the block that contains the blob.
  storage::VmoBuffer buffer;
  ASSERT_OK(buffer.Initialize(block_device.get(), 1, kBlobfsBlockSize, "test_buffer"));
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = buffer.vmoid(),
      .length = kBlobfsBlockSize / kTestDeviceBlockSize,
      .vmo_offset = 0,
      .dev_offset = block * kBlobfsBlockSize / kTestDeviceBlockSize,
  };
  ASSERT_OK(block_device->FifoTransaction(&request, 1));

  // Corrupt the end of the page.
  static_cast<uint8_t*>(buffer.Data(0))[kPageSize - 1] = 1;

  // Write the block back.
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  ASSERT_OK(block_device->FifoTransaction(&request, 1));

  // Remount and try and read the blob.
  ASSERT_OK(Mount(std::move(block_device)));

  auto blob = GetBlob(delivery_blob.digest());
  ASSERT_OK(blob);
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_EQ(GetVmoStreamSize(*vmo), 64ul);
  ASSERT_EQ(GetVmoSize(*vmo), kPageSize);

  uint8_t data;
  EXPECT_OK(vmo->read(&data, kPageSize - 1, 1));
  // The corrupted bit in the tail was zeroed when being read.
  EXPECT_EQ(data, 0);
}

TEST_P(BlobTest, WriteBlobWithSharedBlockInCompactFormat) {
  // Create a blob where the Merkle tree in the compact layout fits perfectly into the space
  // remaining at the end of the blob.
  ASSERT_EQ(blobfs()->Info().block_size, digest::kDefaultNodeSize);
  auto blob_data =
      TestBlobData::CreateRealistic((digest::kDefaultNodeSize - digest::kSha256Length) * 3);
  // An uncompressed blob is used so the exact size of it in storage is known.
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob_data);

  auto merkle_tree = TestMerkleTree::CreateCompact(blob_data);
  EXPECT_EQ(blob_data.data().size() + merkle_tree.merkle_tree().size(),
            digest::kDefaultNodeSize * 3);

  {
    auto blob = CreateBlob(delivery_blob);
    ASSERT_OK(blob);
  }

  // Remount to avoid caching.
  Remount();

  // Read back the blob
  auto blob = GetBlob(blob_data.digest());
  ASSERT_OK(blob);
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));
}

TEST_P(BlobTest, WriteErrorsAreFused) {
  // A uncompressed blob is used to ensure that blobfs will run out of space.
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(
      static_cast<size_t>(kTestDeviceBlockSize) * kTestDeviceNumBlocks);

  fbl::RefPtr blob = fbl::MakeRefCounted<Blob>(*blobfs(), delivery_blob.digest());
  ASSERT_OK(blobfs()->GetCache().Add(blob));
  ASSERT_OK(blob->Truncate(delivery_blob.data().size()));
  size_t out_actual = 0;
  ASSERT_STATUS(
      blob->Write(delivery_blob.data().data(), delivery_blob.data().size(), 0, &out_actual),
      ZX_ERR_NO_SPACE);
  // Writing just 1 byte now should see the same error returned.
  ASSERT_STATUS(blob->Write(delivery_blob.data().data(), 1, 0, &out_actual), ZX_ERR_NO_SPACE);

  // Whilst we have the failed file still open, we should be able to try again immediately.
  fbl::RefPtr blob2 = fbl::MakeRefCounted<Blob>(*blobfs(), delivery_blob.digest());
  ASSERT_OK(blobfs()->GetCache().Add(blob2));
  ASSERT_OK(blob2->Truncate(delivery_blob.data().size()));
  ASSERT_OK(blob2->Write(delivery_blob.data().data(), 1, 0, &out_actual));
  ASSERT_EQ(out_actual, 1ul);
}

TEST_P(BlobTest, UnlinkBlocksUntilNoVmoChildren) {
  auto blob_data = TestBlobData::CreateRealistic(1 << 16);

  // Write the blob.
  auto blob = CreateBlob(blob_data);
  ASSERT_OK(blob);

  // Get a clone of the VMO.
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);

  // Mark the blob to be purged.
  ASSERT_OK(blob->QueueUnlink());

  // The blob can still be read.
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));
  // The blob isn't purgeable because of `vmo`.
  ASSERT_FALSE(IsPurgeable(**blob));
  ASSERT_TRUE(IsDeletionQueued(**blob));
}

TEST_P(BlobTest, VmoChildDeletedTriggersPurging) {
  auto blob_data = TestBlobData::CreateRealistic(1 << 16);

  // Write the blob.
  auto blob = CreateBlob(blob_data);
  ASSERT_OK(blob);

  // Get a clone of the VMO.
  auto blob_vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(blob_vmo);
  zx::vmo vmo = std::move(*blob_vmo);

  // Mark the blob to be purged.
  ASSERT_OK(blob->QueueUnlink());
  ASSERT_EQ(GetVmoStreamSize(vmo), blob_data.data().size());

  // Drop the VMO. This should eventually trigger deletion of the blob.
  vmo.reset();

  // Unfortunately, polling the filesystem is the best option for checking if the blob is deleted.
  bool deleted = false;
  const auto start = std::chrono::steady_clock::now();
  constexpr auto kMaxWait = std::chrono::seconds(60);
  while (std::chrono::steady_clock::now() <= start + kMaxWait) {
    loop().RunUntilIdle();
    auto blob = GetBlob(blob_data.digest());
    if (blob.is_ok()) {
      // The blob still exists.
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
      continue;
    }
    ASSERT_STATUS(blob, ZX_ERR_NOT_FOUND);
    deleted = true;
    break;
  }
  EXPECT_TRUE(deleted);
}

// Some paging failures result in permanent failure. Failed block ops should not.
TEST_P(BlobTest, ReadErrorsTemporary) {
  auto blob_data = TestBlobData::CreateRealistic(1 << 16);

  // Write the blob.
  auto blob = CreateBlob(blob_data);
  ASSERT_OK(blob);

  // Get a clone of the VMO.
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);

  // Add a hook to toggle read failure.
  std::atomic<zx_status_t> fail_ops = ZX_OK;
  set_hook(
      [&fail_ops](const BlockFifoRequest& _req, const zx::vmo* _vmo) { return fail_ops.load(); });

  // Attempt a read with various failure modes.
  char buf;
  for (zx_status_t err :
       {ZX_ERR_IO, ZX_ERR_IO_DATA_INTEGRITY, ZX_ERR_IO_REFUSED, ZX_ERR_BAD_STATE}) {
    fail_ops.store(err);
    ASSERT_NE(vmo->read(&buf, 0, 1), ZX_OK);
  }

  // Now succeed.
  fail_ops.store(ZX_OK);
  ASSERT_OK(vmo->read(&buf, 0, 1));

  // Clear the hook to stop using the atomic.
  set_hook({});
}

std::string GetVmoName(const zx::vmo& vmo) {
  char buf[ZX_MAX_NAME_LEN + 1] = {'\0'};
  EXPECT_OK(vmo.get_property(ZX_PROP_NAME, buf, ZX_MAX_NAME_LEN));
  return std::string(buf, ::strlen(buf));
}

TEST_P(BlobTest, VmoNameMatchesStateOfBlob) {
  auto blob_data = TestBlobData::Create(64);
  auto blob = CreateBlob(blob_data);
  ASSERT_OK(blob);
  const auto active_blob_name = FormatBlobDataVmoName(blob_data.digest());
  const auto inactive_blob_name = FormatInactiveBlobDataVmoName(blob_data.digest());

  for (int i = 0; i < 2; ++i) {
    {
      auto vmo = blob->GetVmoForBlobReader();
      ASSERT_OK(vmo);
      ASSERT_EQ(GetVmoName(*vmo), std::string_view(active_blob_name));
    }
    ASSERT_OK(WaitForZeroChildren(**blob));
    ASSERT_EQ(GetVmoName(GetPagedVmo(**blob)), std::string_view(inactive_blob_name));
  }
}

TEST_P(BlobTest, WritesToArbitraryOffsetsFails) {
  auto blob_data = TestBlobData::Create(64);
  auto delivery_blob =
      TestDeliveryBlob::CreateWithCompressionAlgorithm(blob_data, GetCompressionAlgorithm());
  fbl::RefPtr blob = fbl::MakeRefCounted<Blob>(*blobfs(), delivery_blob.digest());
  ASSERT_OK(blobfs()->GetCache().Add(blob));
  ASSERT_OK(blob->Truncate(delivery_blob.data().size()));

  size_t out_actual;
  ASSERT_STATUS(blob->Write(delivery_blob.data().data(), 10, 10, &out_actual),
                ZX_ERR_NOT_SUPPORTED);
  ASSERT_OK(blob->Write(delivery_blob.data().data(), 10, 0, &out_actual));
  ASSERT_EQ(out_actual, 10u);
  ASSERT_STATUS(blob->Write(delivery_blob.data().data() + 10, delivery_blob.data().size() - 10, 20,
                            &out_actual),
                ZX_ERR_NOT_SUPPORTED);
  ASSERT_OK(blob->Write(delivery_blob.data().data() + 10, delivery_blob.data().size() - 10, 10,
                        &out_actual));
  ASSERT_EQ(out_actual, delivery_blob.data().size() - 10);
}

TEST_P(BlobTest, WrittenBlobsAreInitiallyPagedOut) {
  auto blob_data = TestBlobData::Create(64);
  auto blob = CreateBlob(blob_data);
  ASSERT_OK(blob);

  // The pager backed VMO isn't created until the blob is opened.
  ASSERT_FALSE(GetPagedVmo(**blob).is_valid());
}

TEST_P(BlobTest, GetVmoForBlobReaderOnNullBlobIsSupported) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(0);
  ASSERT_OK(CreateBlob(delivery_blob));

  // Make sure the async part of the write finishes.
  loop().RunUntilIdle();

  auto blob = GetBlob(delivery_blob.digest());
  ASSERT_OK(blob);
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_EQ(GetVmoStreamSize(*vmo), 0ul);
}

TEST_P(BlobTest, CheckerboardFragmentationExhaustsJournalOnBlobWrite) {
  // Create 200 small 1-block blobs to fill contiguous block map space, then
  // unlink every other one to create a checkboard pattern of holes in the data
  // area.
  constexpr size_t kSmallBlobSize = 1 * kBlobfsBlockSize;
  std::vector<fbl::RefPtr<Blob>> blobs;
  blobs.reserve(200);

  for (size_t i = 0; i < 200; i++) {
    auto delivery_blob =
        TestDeliveryBlob::CreateUncompressed(TestBlobData::CreatePrefixed(kSmallBlobSize, i));
    zx::result<fbl::RefPtr<Blob>> blob = CreateBlob(delivery_blob);
    ASSERT_OK(blob);
    blobs.push_back(std::move(*blob));
  }
  for (size_t i = 0; i < 200; i += 2) {
    ASSERT_OK(blobs[i]->QueueUnlink());
    blobs[i].reset();
  }

  // Attempt to write a single large 100-block blob to be fragmented into the holes.
  constexpr size_t kLargeBlobSize = 100 * kBlobfsBlockSize;
  auto large_delivery_blob = TestDeliveryBlob::CreateUncompressed(
      TestBlobData::CreatePrefixed(kLargeBlobSize, /*prefix=*/999999));

  // Each write to an extent container will write to the superblock. With a
  // highly fragmented blob, if the superblock writes aren't deduplicated the
  // number of writes will exceed the journal size.
  zx::result<fbl::RefPtr<Blob>> large_blob = CreateBlob(large_delivery_blob);
  ASSERT_OK(large_blob);
}

std::string GetTestParamName(
    const ::testing::TestParamInfo<std::tuple<BlobLayoutFormat, CompressionAlgorithm>>& param) {
  const auto& [layout, compression_algorithm] = param.param;
  return GetBlobLayoutFormatNameForTests(layout) +
         GetCompressionAlgorithmName(compression_algorithm);
}

INSTANTIATE_TEST_SUITE_P(
    /*no prefix*/, BlobTest,
    testing::Combine(testing::Values(BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart,
                                     BlobLayoutFormat::kCompactMerkleTreeAtEnd),
                     testing::Values(CompressionAlgorithm::kUncompressed,
                                     CompressionAlgorithm::kChunked)),
    GetTestParamName);

}  // namespace
}  // namespace blobfs

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/fd.h>

#include <vector>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <zxtest/zxtest.h>

#include "src/storage/fvm/format.h"
#include "src/storage/fvm/fvm_test_instance.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace fvm {
namespace {

// Shared constants for all resize tests.
constexpr uint64_t kTestBlockSize = 512;
constexpr uint64_t kSliceSize = 1 << 20;

constexpr uint64_t kDataSizeInBlocks = 10;
constexpr uint64_t kDataSize = kTestBlockSize * kDataSizeInBlocks;

constexpr char kPartitionName[] = "partition-name";
constexpr uuid::Uuid kPartitionUniqueGuid = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                                             0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f};
constexpr uuid::Uuid kPartitionTypeGuid = {0xAA, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99,
                                           0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17};
constexpr uint64_t kPartitionSliceCount = 1;

void CheckWrite(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t off,
                std::span<uint8_t> buf) {
  for (unsigned char& i : buf) {
    i = static_cast<uint8_t>(rand());
  }
  ASSERT_OK(block_client::SingleWriteBytes(device, buf.data(), buf.size(), off));
}

void CheckRead(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t off,
               std::span<const uint8_t> in) {
  std::vector<uint8_t> out(in.size());
  ASSERT_OK(block_client::SingleReadBytes(device, out.data(), out.size(), off));
  ASSERT_EQ(memcmp(in.data(), out.data(), in.size()), 0);
}

void CheckWriteReadBlock(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t block,
                         size_t count) {
  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  size_t len = block_info.block_size * count;
  size_t off = block_info.block_size * block;
  std::vector<uint8_t> in(len);
  ASSERT_NO_FATAL_FAILURE(CheckWrite(device, off, in));
  ASSERT_NO_FATAL_FAILURE(CheckRead(device, off, in));
}

class FvmResizeTest : public zxtest::Test {
 protected:
  void RestartFvmWithNewDiskSize(uint64_t block_count) {
    instance_->RestartFvmWithNewDiskSize(kTestBlockSize, block_count);
  }

  zx::result<std::unique_ptr<fvm::BlockConnector>> AllocatePartition(
      const fvm::AllocatePartitionRequest& request) {
    return instance_->AllocatePartition(request);
  }

  zx::result<std::unique_ptr<fvm::BlockConnector>> OpenPartition(std::string_view label) {
    return instance_->OpenPartition(label);
  }

  fuchsia_storage_block::wire::VolumeManagerInfo GetFvmInfo() { return instance_->GetFvmInfo(); }

  void CreateGrowableFvm(uint64_t initial_block_count, uint64_t max_block_count) {
    instance_->CreateRamdisk(kTestBlockSize, initial_block_count);
    ASSERT_OK(fs_management::FvmInitPreallocated(instance_->GetRamdiskPartition(),
                                                 initial_block_count * kTestBlockSize,
                                                 max_block_count * kTestBlockSize, kSliceSize));
    instance_->StartFvm();
  }

  static void ExtendVolume(fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume,
                           uint64_t start_slice, uint64_t slice_count) {
    const fidl::WireResult result = fidl::WireCall(volume)->Extend(start_slice, slice_count);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

 private:
  std::unique_ptr<fvm::FvmInstance> instance_ = std::make_unique<fvm::FvmInstance>();
};

TEST_F(FvmResizeTest, PreallocatedMetadataGrowsCorrectly) {
  constexpr uint64_t kInitialBlockCount = (50 * kSliceSize) / kTestBlockSize;
  constexpr uint64_t kMaxBlockCount = (4 << 10) * kSliceSize / kTestBlockSize;
  Header expected =
      Header::FromDiskSize(fvm::kMaxUsablePartitions, kMaxBlockCount * kTestBlockSize, kSliceSize);

  ASSERT_NO_FATAL_FAILURE(CreateGrowableFvm(kInitialBlockCount, kMaxBlockCount));
  zx::result vp = AllocatePartition({.slice_count = kPartitionSliceCount,
                                     .type = kPartitionTypeGuid,
                                     .guid = kPartitionUniqueGuid,
                                     .name = kPartitionName});
  ASSERT_OK(vp);

  {
    auto info = GetFvmInfo();
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The disk is smaller than our eventual target so it reports less possible slices.
    ASSERT_LT(info.slice_count, expected.pslice_count);
  }

  std::vector<uint8_t> buf(kDataSize);
  ASSERT_NO_FATAL_FAILURE(CheckWrite(vp->as_block(), 0, buf));

  vp = {};
  ASSERT_NO_FATAL_FAILURE(RestartFvmWithNewDiskSize(kMaxBlockCount));

  {
    auto info = GetFvmInfo();
    // Other info should be the same
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The new total possible slice count should be our expected slice count.
    ASSERT_EQ(info.slice_count, expected.pslice_count);
  }

  vp = OpenPartition(kPartitionName);
  ASSERT_OK(vp);
  // The original data we wrote is still there
  ASSERT_NO_FATAL_FAILURE(CheckRead(vp->as_block(), 0, buf));

  // Now we can extend to fill the rest of the available space, beyond our initial size.
  ASSERT_NO_FATAL_FAILURE(ExtendVolume(vp->as_block(), kPartitionSliceCount,
                                       expected.pslice_count - kPartitionSliceCount));
  size_t block_offset = (expected.pslice_count - 1) * kSliceSize / kBlockSize;
  ASSERT_NO_FATAL_FAILURE(CheckWriteReadBlock(vp->as_block(), block_offset, kDataSizeInBlocks));
}

TEST_F(FvmResizeTest, PreallocatedMetadataGrowsAsMuchAsPossible) {
  constexpr uint64_t kInitialBlockCount = (50 * kSliceSize) / kTestBlockSize;
  constexpr uint64_t kMaxBlockCount = (1 << 10) * kSliceSize / kTestBlockSize;
  // Compute the expected header information. This is the header computed for the original slice
  // size, expanded by as many slices as possible.
  Header expected =
      Header::FromDiskSize(kMaxUsablePartitions, kMaxBlockCount * kTestBlockSize, kSliceSize);
  expected.SetSliceCount(expected.GetAllocationTableAllocatedEntryCount());

  ASSERT_NO_FATAL_FAILURE(CreateGrowableFvm(kInitialBlockCount, kMaxBlockCount));
  zx::result vp = AllocatePartition({.slice_count = kPartitionSliceCount,
                                     .type = kPartitionTypeGuid,
                                     .guid = kPartitionUniqueGuid,
                                     .name = kPartitionName});
  ASSERT_OK(vp);

  {
    auto info = GetFvmInfo();
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The disk is smaller than our eventual target so it reports less possible slices.
    ASSERT_LT(info.slice_count, expected.pslice_count);
  }

  std::vector<uint8_t> buf(kDataSize);
  ASSERT_NO_FATAL_FAILURE(CheckWrite(vp->as_block(), 0, buf));

  vp = {};
  // This defines a ramdisk size much larger than our header could handle so the resize will max
  // out the slices in the header.
  constexpr uint64_t kLargeBlockCount = (2 << 10) * kSliceSize / kTestBlockSize;
  ASSERT_NO_FATAL_FAILURE(RestartFvmWithNewDiskSize(kLargeBlockCount));

  {
    auto info = GetFvmInfo();
    // Other info should be the same
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The new total possible slice count should be our expected slice count, despite the larger
    // disk space available.
    ASSERT_EQ(info.slice_count, expected.pslice_count);
    // In fact, it should be the maximum number of slices possible for the size of the allocation
    // table, which is also reported.
    ASSERT_EQ(info.slice_count, info.maximum_slice_count);
  }

  vp = OpenPartition(kPartitionName);
  ASSERT_OK(vp);
  // The original data we wrote is still there
  ASSERT_NO_FATAL_FAILURE(CheckRead(vp->as_block(), 0, buf));

  // Now we can extend to fill the rest of the available space, beyond our initial size.
  ASSERT_NO_FATAL_FAILURE(ExtendVolume(vp->as_block(), kPartitionSliceCount,
                                       expected.pslice_count - kPartitionSliceCount));
  size_t block_offset = (expected.pslice_count - 1) * kSliceSize / kBlockSize;
  ASSERT_NO_FATAL_FAILURE(CheckWriteReadBlock(vp->as_block(), block_offset, kDataSizeInBlocks));
}

TEST_F(FvmResizeTest, PreallocatedMetadataRemainsValidInPartialGrowths) {
  constexpr uint64_t kInitialBlockCount = (50 * kSliceSize) / kTestBlockSize;
  constexpr uint64_t kMidBlockCount = (4 << 10) * kSliceSize / kTestBlockSize;
  constexpr uint64_t kMaxBlockCount = (8 << 10) * kSliceSize / kTestBlockSize;
  Header expected_mid =
      Header::FromDiskSize(kMaxUsablePartitions, kMidBlockCount * kTestBlockSize, kSliceSize);
  Header expected_max =
      Header::FromDiskSize(kMaxUsablePartitions, kMaxBlockCount * kTestBlockSize, kSliceSize);

  ASSERT_NO_FATAL_FAILURE(CreateGrowableFvm(kInitialBlockCount, kMaxBlockCount));
  zx::result vp = AllocatePartition({.slice_count = kPartitionSliceCount,
                                     .type = kPartitionTypeGuid,
                                     .guid = kPartitionUniqueGuid,
                                     .name = kPartitionName});
  ASSERT_OK(vp);

  {
    auto info = GetFvmInfo();
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The disk is smaller than our eventual target so it reports less possible slices.
    ASSERT_LT(info.slice_count, expected_mid.pslice_count);
    ASSERT_LT(info.slice_count, expected_max.pslice_count);
  }

  std::vector<uint8_t> buf(kDataSize);
  ASSERT_NO_FATAL_FAILURE(CheckWrite(vp->as_block(), 0, buf));

  vp = {};
  ASSERT_NO_FATAL_FAILURE(RestartFvmWithNewDiskSize(kMidBlockCount));

  {
    auto info = GetFvmInfo();
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The disk is now the midpoint size, so it has the midpoint slice amount.
    ASSERT_EQ(info.slice_count, expected_mid.pslice_count);
    ASSERT_LT(info.slice_count, expected_max.pslice_count);
  }

  vp = OpenPartition(kPartitionName);
  ASSERT_OK(vp);
  // The original data we wrote is still there.
  ASSERT_NO_FATAL_FAILURE(CheckRead(vp->as_block(), 0, buf));

  vp = {};
  ASSERT_NO_FATAL_FAILURE(RestartFvmWithNewDiskSize(kMaxBlockCount));

  {
    auto info = GetFvmInfo();
    ASSERT_EQ(kSliceSize, info.slice_size);
    ASSERT_EQ(kPartitionSliceCount, info.assigned_slice_count);
    // The disk is now the maximum size, so it has the maximum slice amount.
    ASSERT_GT(info.slice_count, expected_mid.pslice_count);
    ASSERT_EQ(info.slice_count, expected_max.pslice_count);
  }

  vp = OpenPartition(kPartitionName);
  ASSERT_OK(vp);
  // The original data we wrote is still there.
  ASSERT_NO_FATAL_FAILURE(CheckRead(vp->as_block(), 0, buf));

  // Now we can extend to fill the rest of the available space, beyond our initial size.
  ASSERT_NO_FATAL_FAILURE(ExtendVolume(vp->as_block(), kPartitionSliceCount,
                                       expected_max.pslice_count - kPartitionSliceCount));
  size_t block_offset = (expected_max.pslice_count - 1) * kSliceSize / kBlockSize;
  ASSERT_NO_FATAL_FAILURE(CheckWriteReadBlock(vp->as_block(), block_offset, kDataSizeInBlocks));
}

}  // namespace
}  // namespace fvm

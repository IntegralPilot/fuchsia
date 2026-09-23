// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.fs/cpp/common_types.h>
#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fidl/fuchsia.update.verify/cpp/common_types.h>
#include <fidl/fuchsia.update.verify/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/diagnostics/reader/cpp/inspect.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <poll.h>
#include <sys/mman.h>
#include <unistd.h>
#include <utime.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"
#include "src/storage/blobfs/test/integration/fdio_test.h"
#include "src/storage/fs_test/fs_test.h"
#include "src/storage/fs_test/test_filesystem.h"
#include "src/storage/fvm/format.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/fs_management/cpp/options.h"
#include "src/storage/lib/vfs/cpp/inspect/inspect_data.h"
#include "src/storage/lib/vfs/cpp/inspect/inspect_tree.h"

namespace blobfs {
namespace {

using BlobfsIntegrationTest = ParameterizedBlobfsTest;

// Go over the parent device logic and test fixture.
TEST_P(BlobfsIntegrationTest, Trivial) {}

TEST_P(BlobfsIntegrationTest, Basics) {
  for (unsigned int i = 10; i < 16; i++) {
    auto blob = TestBlobData::CreatePrefixed(1 << i, i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

    // We can re-open and verify the Blob as read-only.
    ASSERT_OK(blob_reader().VerifyBlob(blob));

    // We cannot create the same blob twice.
    ASSERT_STATUS(blob_creator().CreateAndWriteBlob(delivery_blob), ZX_ERR_ALREADY_EXISTS);

    ASSERT_OK(Unlink(delivery_blob.digest()));
  }
}

TEST_P(BlobfsIntegrationTest, NullBlobCreateUnlink) {
  auto null_delivery_blob = TestDeliveryBlob::CreateUncompressed(0);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(null_delivery_blob));
  auto vmo = blob_reader().GetVmo(null_delivery_blob.digest());
  ASSERT_OK(vmo);
  ASSERT_EQ(GetVmoStreamSize(*vmo), 0lu);

  ASSERT_STATUS(blob_creator().CreateAndWriteBlob(null_delivery_blob), ZX_ERR_ALREADY_EXISTS)
      << "Null Blob should already exist";

  ASSERT_THAT(ListBlobs(), testing::ElementsAre(null_delivery_blob.digest()));
  ASSERT_OK(Unlink(null_delivery_blob.digest())) << "Null Blob should be unlinkable";
}

TEST_P(BlobfsIntegrationTest, NullBlobCreateRemount) {
  // Create the null blob.
  auto null_delivery_blob = TestDeliveryBlob::CreateUncompressed(0);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(null_delivery_blob));

  ASSERT_OK(Remount());

  ASSERT_OK(blob_reader().GetVmo(null_delivery_blob.digest())) << "Null blob lost after reboot";
  ASSERT_OK(Unlink(null_delivery_blob.digest())) << "Null Blob should be unlinkable";
}

TEST_P(BlobfsIntegrationTest, CompressibleBlob) {
  for (size_t i = 10; i < 22; i++) {
    // Create blobs which are trivially compressible.
    auto blob = TestBlobData::Create(1 << i);
    auto delivery_blob = TestDeliveryBlob::CreateCompressed(blob);
    // The blob should have compressed.
    ASSERT_GT(blob.data().size(), delivery_blob.data().size());

    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

    // We can re-open and verify the Blob as read-only.
    ASSERT_OK(blob_reader().VerifyBlob(blob));

    // Force decompression by remounting, re-accessing blob.
    ASSERT_OK(Remount());
    ASSERT_OK(blob_reader().VerifyBlob(blob));

    ASSERT_OK(Unlink(blob.digest()));
  }
}

TEST_P(BlobfsIntegrationTest, ReadDirectory) {
  constexpr size_t kMaxEntries = 50;
  constexpr size_t kBlobSize = 1 << 10;

  // Try to readdir on an empty directory.
  DIR* dir = opendir(fs().mount_path().c_str());
  ASSERT_NE(dir, nullptr);
  auto cleanup = fit::defer([dir]() { closedir(dir); });
  ASSERT_EQ(readdir(dir), nullptr) << "Expected blobfs to start empty";

  // Fill a directory with entries.
  std::set<Digest> blobs;
  for (size_t i = 0; i < kMaxEntries; ++i) {
    auto blob = TestBlobData::CreatePrefixed(kBlobSize, i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
    blobs.insert(blob.digest());
  }

  // Check that we see the expected number of entries
  size_t entries_seen = 0;
  struct dirent* dir_entry;
  while ((dir_entry = readdir(dir)) != nullptr) {
    entries_seen++;
  }
  ASSERT_EQ(kMaxEntries, entries_seen);
  entries_seen = 0;
  rewinddir(dir);

  // Readdir on a directory which contains entries, removing them as we go
  // along.
  while ((dir_entry = readdir(dir)) != nullptr) {
    Digest entry;
    ASSERT_OK(entry.Parse(dir_entry->d_name));
    ASSERT_EQ(blobs.erase(entry), 1lu);
    ASSERT_OK(Unlink(entry));
    entries_seen++;
  }
  ASSERT_THAT(blobs, testing::IsEmpty());
  ASSERT_EQ(kMaxEntries, entries_seen);

  ASSERT_EQ(readdir(dir), nullptr) << "Directory should be empty";
  cleanup.cancel();
  ASSERT_EQ(0, closedir(dir));
}

TEST_P(BlobfsIntegrationTest, DirectoryCreateBlobIsNotSupported) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(16);
  fbl::unique_fd fd(openat(fs().GetRootFd().get(), delivery_blob.digest().ToString().c_str(),
                           O_CREAT | O_EXCL | O_RDWR, S_IRUSR | S_IWUSR));
  ASSERT_FALSE(fd.is_valid());
  ASSERT_EQ(errno, ENOTSUP);
}

TEST_P(BlobfsIntegrationTest, DirectoryOpenBlobIsNotSupported) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(16);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
  fbl::unique_fd fd(
      openat(fs().GetRootFd().get(), delivery_blob.digest().ToString().c_str(), O_RDONLY));
  ASSERT_FALSE(fd.is_valid());
  ASSERT_EQ(errno, ENOTSUP);
}

TEST_P(BlobfsIntegrationTest, DirectoryOpenDotIsSupported) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(16);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  fbl::unique_fd fd(openat(fs().GetRootFd().get(), ".", O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(fd.is_valid());
  // List the directory entries to confirm that it's the blob directory.
  DIR* dir = fdopendir(fd.get());
  ASSERT_NE(dir, nullptr);
  fd.release();
  auto cleanup = fit::defer([dir]() { closedir(dir); });
  struct dirent* de = readdir(dir);
  ASSERT_EQ(de->d_name, delivery_blob.digest().ToString());
}

fs_test::TestFilesystemOptions MinimumDiskSizeOptions() {
  auto options = fs_test::TestFilesystemOptions::BlobfsWithoutFvm();
  Superblock info;
  info.data_block_count = kMinimumDataBlocks;
  info.journal_block_count = kMinimumJournalBlocks;
  info.flags = 0;
  info.inode_count = options.num_inodes;
  options.device_block_count = TotalBlocks(info) * kBlobfsBlockSize / options.device_block_size;
  return options;
}

TEST(SmallDiskTest, SmallestValidDisk) {
  auto options = MinimumDiskSizeOptions();
  EXPECT_OK(fs_test::TestFilesystem::Create(MinimumDiskSizeOptions()));
}

TEST(SmallDiskTest, DiskTooSmall) {
  auto options = MinimumDiskSizeOptions();
  options.device_block_count -= kBlobfsBlockSize / options.device_block_size;
  EXPECT_NE(fs_test::TestFilesystem::Create(options).status_value(), ZX_OK);
}

fs_test::TestFilesystemOptions MinimumFvmDiskSizeOptions() {
  auto options = fs_test::TestFilesystemOptions::DefaultBlobfs();
  size_t blocks_per_slice = options.fvm_slice_size / kBlobfsBlockSize;

  // Calculate slices required for data blocks based on minimum requirement and slice size.
  uint64_t required_data_slices =
      fbl::round_up(kMinimumDataBlocks, blocks_per_slice) / blocks_per_slice;
  uint64_t required_journal_slices =
      fbl::round_up(kMinimumJournalBlocks, blocks_per_slice) / blocks_per_slice;
  uint64_t required_inode_slices =
      fbl::round_up(BlocksRequiredForInode(options.num_inodes), blocks_per_slice) /
      blocks_per_slice;

  // Require an additional 1 slice each for super and block bitmaps.
  uint64_t blobfs_slices =
      required_journal_slices + required_inode_slices + required_data_slices + 2;
  fvm::Header header =
      fvm::Header::FromSliceCount(fvm::kMaxUsablePartitions, blobfs_slices, options.fvm_slice_size);
  options.device_block_count = header.fvm_partition_size / options.device_block_size;
  return options;
}

TEST(SmallDiskTest, SmallestValidFvmDisk) {
  EXPECT_OK(fs_test::TestFilesystem::Create(MinimumFvmDiskSizeOptions()));
}

TEST(SmallDiskTest, FvmDiskTooSmall) {
  auto options = MinimumFvmDiskSizeOptions();
  options.device_block_count -= kBlobfsBlockSize / options.device_block_size;
  EXPECT_NE(fs_test::TestFilesystem::Create(options).status_value(), ZX_OK);
}

void QueryInfo(fs_test::TestFilesystem& fs, size_t expected_nodes, size_t expected_bytes) {
  fbl::unique_fd root_fd;
  ASSERT_TRUE(root_fd = fbl::unique_fd(open(fs.mount_path().c_str(), O_RDONLY | O_DIRECTORY)))
      << strerror(errno);
  fdio_cpp::UnownedFdioCaller root_connection(root_fd);
  auto result = fidl::WireCall(root_connection.directory())->QueryFilesystem();
  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_OK(result.value().s);
  const auto& info = *result.value().info;

  constexpr std::string_view kFsName = "blobfs";
  const char* name = reinterpret_cast<const char*>(info.name.data());
  ASSERT_EQ(name, kFsName) << "Unexpected filesystem mounted";
  EXPECT_EQ(info.block_size, kBlobfsBlockSize);
  EXPECT_EQ(info.max_filename_size, 64U);
  EXPECT_EQ(info.fs_type, static_cast<uint32_t>(fuchsia_fs::VfsType::kBlobfs));
  EXPECT_NE(info.fs_id, 0ul);

  // Check that used_bytes are within a reasonable range
  EXPECT_GE(info.used_bytes, expected_bytes);
  EXPECT_LE(info.used_bytes, info.total_bytes);

  // Check that total_bytes are a multiple of slice_size
  const uint64_t slice_size = fs.options().fvm_slice_size;
  EXPECT_GE(info.total_bytes, slice_size);
  EXPECT_EQ(info.total_bytes % slice_size, 0ul);
  EXPECT_GE(info.total_nodes, fs.options().num_inodes);
  EXPECT_EQ((info.total_nodes * sizeof(Inode)) % slice_size, 0ul);
  EXPECT_EQ(info.used_nodes, expected_nodes);
}

TEST_F(BlobfsWithFvmTest, QueryInfo) {
  size_t total_bytes = 0;
  ASSERT_NO_FATAL_FAILURE(QueryInfo(fs(), 0, 0));
  for (size_t i = 10; i < 16; i++) {
    auto blob = TestBlobData::CreatePrefixed(1 << i, i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

    auto merkle_tree = TestMerkleTree::CreateCompact(blob);
    total_bytes +=
        fbl::round_up(merkle_tree.merkle_tree().size() + blob.data().size(), kBlobfsBlockSize);
  }

  ASSERT_NO_FATAL_FAILURE(QueryInfo(fs(), 6, total_bytes));
}

TEST_P(BlobfsIntegrationTest, UseAfterUnlink) {
  auto blob = TestBlobData::Create(64lu * 1024);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // Remount to guarantee that the blob is paged out.
  ASSERT_OK(Remount());

  auto vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(vmo);
  ASSERT_OK(Unlink(blob.digest()));

  ASSERT_TRUE(VerifyContents(*vmo, blob.data()));
}

TEST_P(BlobfsIntegrationTest, EdgeAllocation) {
  // Powers of two...
  for (ssize_t i = 1; i < 16; i++) {
    for (ssize_t j : {-1, 0, 1}) {
      auto blob = TestBlobData::CreateRandom((1 << i) + j);
      auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
      ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
      ASSERT_OK(Unlink(blob.digest()));
    }
  }
}

TEST_P(BlobfsIntegrationTest, UmountWhileWritingFile) {
  const size_t kBlobSize = 1 << 16;
  auto blob = TestBlobData::Create(kBlobSize);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  auto writer = blob_creator().Create(delivery_blob.digest());
  ASSERT_OK(writer);
  auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob);
  ASSERT_OK(incremental_writer->Write(kBlobSize / 2));

  // Remount blobfs while a blob writer is open.
  ASSERT_OK(Remount());

  // The serving end of the blob writer should be closed.
  ASSERT_STATUS(incremental_writer->Complete(), ZX_ERR_PEER_CLOSED);
  // The blob shouldn't exist.
  ASSERT_STATUS(blob_reader().GetVmo(delivery_blob.digest()), ZX_ERR_NOT_FOUND);

  // The blob can be created again.
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
  ASSERT_OK(blob_reader().VerifyBlob(blob));
  ASSERT_OK(Unlink(blob.digest()));
}

TEST_P(BlobfsIntegrationTest, UmountWithOpenBlob) {
  constexpr uint64_t kBlobSize = 1 << 16;
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(kBlobSize);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // Remount to ensure that the VMO is not paged in.
  ASSERT_OK(Remount());

  // Get a handle to the VMO that should have uncommitted pages.
  auto vmo = blob_reader().GetVmo(delivery_blob.digest());
  ASSERT_OK(vmo);

  // Remounting will remove the pager backing the VMO.
  ASSERT_OK(Remount());

  std::vector<uint8_t> data(kBlobSize);
  // Trying to read from the VMO fails because the pager that was backing it is gone.
  ASSERT_STATUS(vmo->read(data.data(), 0, kBlobSize), ZX_ERR_BAD_STATE);

  ASSERT_OK(Unlink(delivery_blob.digest()));
}

TEST_P(BlobfsIntegrationTest, CreateUmountRemountSmall) {
  for (size_t i = 10; i < 16; i++) {
    auto blob = TestBlobData::CreatePrefixed(1 << i, i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

    ASSERT_OK(Remount());
    ASSERT_OK(blob_reader().VerifyBlob(blob));
    ASSERT_OK(Unlink(blob.digest()));
  }
}

// Tests that we cannot read from the Blob until it has been fully written.
TEST_P(BlobfsIntegrationTest, EarlyRead) {
  auto blob = TestBlobData::Create(1 << 17);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  auto writer = blob_creator().Create(delivery_blob.digest());
  ASSERT_OK(writer);

  // The blob shouldn't be readable until it's been fully written.
  ASSERT_STATUS(blob_reader().GetVmo(delivery_blob.digest()), ZX_ERR_NOT_FOUND);

  auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob);
  ASSERT_OK(incremental_writer);
  ASSERT_OK(incremental_writer->Write(1 << 16));

  // The blob shouldn't be readable until it's been fully written.
  ASSERT_STATUS(blob_reader().GetVmo(delivery_blob.digest()), ZX_ERR_NOT_FOUND);

  ASSERT_OK(incremental_writer->Complete());

  // Check that attempting to read early didn't cause problems.
  ASSERT_OK(blob_reader().VerifyBlob(blob));
}

// Try unlinking while creating a blob.
TEST_P(BlobfsIntegrationTest, RestartCreation) {
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(1 << 17);
  {
    // Unlink after init.
    auto writer = blob_creator().Create(delivery_blob.digest());
    ASSERT_OK(writer);

    ASSERT_OK(Unlink(delivery_blob.digest()));
  }

  {
    // Unlink after first write.
    auto writer = blob_creator().Create(delivery_blob.digest());
    ASSERT_OK(writer);
    auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob);
    ASSERT_OK(incremental_writer);
    ASSERT_OK(incremental_writer->Write(1 << 16));

    ASSERT_OK(Unlink(delivery_blob.digest()));

    ASSERT_STATUS(incremental_writer->Write(1), ZX_ERR_BAD_STATE);
  }

  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
}

// Attempt using invalid operations.
TEST_P(BlobfsIntegrationTest, RenameIsInvalid) {
  // First off, make a valid blob.
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(64);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  auto blob_name = delivery_blob.digest().ToString();
  auto path = fs().mount_path() + "/" + std::string(blob_name);
  ASSERT_LT(rename(path.c_str(), path.c_str()), 0);
}

// Attempt operations on the root directory.
TEST_P(BlobfsIntegrationTest, RootDirectory) {
  std::string name(fs().mount_path());
  name.append("/.");
  fbl::unique_fd dirfd(open(name.c_str(), O_RDONLY));
  ASSERT_TRUE(dirfd) << "Cannot open root directory";

  char buf[8];
  ASSERT_LT(write(dirfd.get(), buf, 8), 0) << "Should not write to directory";
  ASSERT_LT(read(dirfd.get(), buf, 8), 0) << "Should not read from directory";

  // Should NOT be able to unlink root dir.
  ASSERT_LT(unlink(name.c_str()), 0);
}

TEST_P(BlobfsIntegrationTest, PartialWrite) {
  constexpr size_t kBloBsize = 1 << 20;
  auto blob_complete = TestBlobData::CreatePrefixed(kBloBsize, 1);
  auto delivery_blob_complete = TestDeliveryBlob::CreateUncompressed(blob_complete);
  auto blob_partial = TestBlobData::CreatePrefixed(kBloBsize, 2);
  auto delivery_blob_partial = TestDeliveryBlob::CreateUncompressed(blob_partial);

  // Partially write out first blob.
  auto writer = blob_creator().Create(delivery_blob_partial.digest());
  ASSERT_OK(writer);
  auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob_partial);
  ASSERT_OK(incremental_writer);
  ASSERT_OK(incremental_writer->Write(kBloBsize - 20));

  // Completely write out second blob.
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob_complete));
  ASSERT_OK(blob_reader().VerifyBlob(blob_complete));
}

TEST_P(BlobfsIntegrationTest, ReadOnly) {
  // Mount the filesystem as read-write. We can create new blobs.
  auto blob1 = TestBlobData::CreatePrefixed(1 << 10, 1);
  auto delivery_blob1 = TestDeliveryBlob::CreateUncompressed(blob1);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob1));
  ASSERT_OK(blob_reader().VerifyBlob(blob1));

  fs_management::MountOptions options = fs().DefaultMountOptions();
  options.readonly = true;
  ASSERT_OK(Remount(options));

  // We can read old blobs.
  ASSERT_OK(blob_reader().VerifyBlob(blob1));

  // We cannot create new blobs.
  auto blob2 = TestBlobData::CreatePrefixed(1 << 10, 2);
  ASSERT_NE(blob_creator().Create(blob2.digest()).status_value(), ZX_OK);
}

void OpenBlockDevice(const std::string& path,
                     std::unique_ptr<block_client::RemoteBlockDevice>* block_device) {
  zx::result channel = component::Connect<fuchsia_storage_block::Block>(path);
  ASSERT_TRUE(channel.is_ok()) << channel.status_string();
  zx::result device = block_client::RemoteBlockDevice::Create(std::move(channel.value()));
  ASSERT_TRUE(device.is_ok()) << device.status_string();
  *block_device = std::move(device.value());
}

using SliceRange = fuchsia_storage_block::wire::VsliceRange;

uint64_t BlobfsBlockToFvmSlice(fs_test::TestFilesystem& fs, uint64_t block) {
  const size_t blocks_per_slice = fs.options().fvm_slice_size / kBlobfsBlockSize;
  return block / blocks_per_slice;
}

// The test creates a blob with data of size disk_size. The data is
// compressible so needs less space on disk. This will test if we can persist
// a blob whose uncompressed data is larger than available free space.
// The test is expected to fail when compression is turned off.
TEST_P(BlobfsIntegrationTest, BlobLargerThanAvailableSpaceTest) {
  auto blob = TestBlobData::Create(
      (fs().options().device_block_count * fs().options().device_block_size) + 1);
  auto delivery_blob = TestDeliveryBlob::CreateCompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // We can re-open and verify the Blob.
  ASSERT_OK(blob_reader().VerifyBlob(blob));

  // Force decompression by remounting, re-accessing blob.
  ASSERT_OK(Remount());
  ASSERT_OK(blob_reader().VerifyBlob(blob));

  ASSERT_OK(Unlink(blob.digest()));
}

void GetSliceRange(const BlobfsWithFvmTest& test, const std::vector<uint64_t>& slices,
                   std::vector<SliceRange>* ranges) {
  std::unique_ptr<block_client::RemoteBlockDevice> block_device;
  ASSERT_NO_FATAL_FAILURE(OpenBlockDevice(test.fs().DevicePath().value(), &block_device));

  size_t ranges_count;
  SliceRange range_array[fuchsia_storage_block::wire::kMaxSliceRequests];
  ASSERT_OK(
      block_device->VolumeQuerySlices(slices.data(), slices.size(), range_array, &ranges_count));
  ranges->clear();
  for (size_t i = 0; i < ranges_count; i++) {
    ranges->push_back(range_array[i]);
  }
}

// This tests growing both additional inodes and data blocks.
TEST_F(BlobfsWithFvmTest, ResizePartition) {
  ASSERT_OK(fs().Unmount());
  std::vector<SliceRange> old_slices;
  std::vector<uint64_t> query = {BlobfsBlockToFvmSlice(fs(), kFVMNodeMapStart),
                                 BlobfsBlockToFvmSlice(fs(), kFVMDataStart)};
  ASSERT_NO_FATAL_FAILURE(GetSliceRange(*this, query, &old_slices));
  ASSERT_EQ(old_slices.size(), 2ul);
  ASSERT_OK(fs().Mount());
  ASSERT_OK(Reconnect());

  size_t required_nodes =
      ((old_slices[0].count * fs().options().fvm_slice_size) / kBlobfsInodeSize) + 2;
  for (size_t i = 0; i < required_nodes; i++) {
    auto blob = TestBlobData::CreatePrefixed(sizeof(i), i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
  }

  // Remount partition.
  ASSERT_OK(Remount());

  ASSERT_OK(fs().Unmount());
  std::vector<SliceRange> slices;
  ASSERT_NO_FATAL_FAILURE(GetSliceRange(*this, query, &slices));
  ASSERT_EQ(slices.size(), 2ul);
  EXPECT_EQ(slices[0].count, old_slices[0].count + 1);
  EXPECT_GT(slices[1].count, old_slices[1].count);
}

void FvmShrink(const std::string& path, uint64_t offset, uint64_t length) {
  std::unique_ptr<block_client::RemoteBlockDevice> block_device;
  ASSERT_NO_FATAL_FAILURE(OpenBlockDevice(path, &block_device));
  ASSERT_OK(block_device->VolumeShrink(offset, length));
}

void FvmExtend(const std::string& path, uint64_t offset, uint64_t length) {
  std::unique_ptr<block_client::RemoteBlockDevice> block_device;
  ASSERT_NO_FATAL_FAILURE(OpenBlockDevice(path, &block_device));
  ASSERT_OK(block_device->VolumeExtend(offset, length));
}

TEST_F(BlobfsWithFvmTest, CorruptAtMount) {
  ASSERT_OK(fs().Unmount());

  // Shrink slice so FVM will differ from Blobfs.
  uint64_t offset = BlobfsBlockToFvmSlice(fs(), kFVMNodeMapStart);
  std::vector<SliceRange> slices;
  std::vector<uint64_t> query = {BlobfsBlockToFvmSlice(fs(), kFVMNodeMapStart)};
  ASSERT_NO_FATAL_FAILURE(GetSliceRange(*this, query, &slices));
  ASSERT_EQ(slices.size(), 1ul);
  uint64_t len = slices[0].count;
  ASSERT_GT(len, 0ul);
  ASSERT_NO_FATAL_FAILURE(FvmShrink(fs().DevicePath().value(), offset + len - 1, 1));

  ASSERT_NE(fs().Mount().status_value(), ZX_OK);

  // Grow slice count with one extra slice.
  ASSERT_NO_FATAL_FAILURE(FvmExtend(fs().DevicePath().value(), offset + len - 1, 2));

  EXPECT_OK(fs().Mount());
  EXPECT_OK(fs().Unmount());

  // Verify that mount automatically removed the extra slice.
  ASSERT_NO_FATAL_FAILURE(GetSliceRange(*this, query, &slices));
  ASSERT_EQ(slices.size(), 1ul);
  EXPECT_TRUE(slices[0].allocated);
  EXPECT_EQ(slices[0].count, len);
}

TEST_P(BlobfsIntegrationTest, FailedWrite) {
  const uint64_t pages_per_block = kBlobfsBlockSize / fs().options().device_block_size;
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(kBlobfsBlockSize, 0x1);
  auto writer = blob_creator().Create(delivery_blob.digest());
  ASSERT_OK(writer);
  auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob);
  ASSERT_OK(incremental_writer);
  // Write at least the delivery blob header. This will cause blobfs to reserve blocks for the blob
  // which might touch FVM. We do this before sleeping the ramdisk to make the number of writes
  // after sleeping is deterministic.
  ASSERT_OK(incremental_writer->Write(4096));

  // Perform a Sync now to make sure they don't interfere with what follows.
  ASSERT_EQ(fsync(root_fd()), 0) << strerror(errno);

  // Journal:
  // - One Superblock block
  // - One Inode table block
  // - One Bitmap block
  //
  // Non-journal:
  // - One Inode table block
  // - One Data block
  constexpr uint64_t kBlockCountToWrite = 5;

  // Sleep after |kBlockCountToWrite - 1| blocks. This is 1 less than will be needed to write out
  // the entire blob. This ensures that writing the blob will ultimately fail, but the write
  // operation will return a successful response.
  ASSERT_OK(fs().GetRamDisk()->SleepAfter(pages_per_block * (kBlockCountToWrite - 1)));
  auto wake = fit::defer([&] { ASSERT_OK(fs().GetRamDisk()->Wake()); });

  ASSERT_OK(incremental_writer->Complete());

  // Since the write operation ultimately failed when going out to disk,
  // syncfs will return a failed response.
  ASSERT_LT(syncfs(root_fd()), 0);

  // With FVM, the write will fail early when trying to acquire more space. Without FVM, the write
  // will fail to commit at the end.
  auto delivery_blob2 = TestDeliveryBlob::CreateUncompressed(kBlobfsBlockSize * 5, 0x2);
  ASSERT_NE(blob_creator().CreateAndWriteBlob(delivery_blob2).status_value(), ZX_OK);
}

struct CloneThreadArgs {
  const Digest digest;
  const BlobReaderWrapper& blob_reader;
  std::atomic_bool done{false};
};

void CloneThread(CloneThreadArgs* args) {
  while (!args->done) {
    auto vmo = args->blob_reader.GetVmo(args->digest);
    ZX_ASSERT(vmo.is_ok());
    // Yielding before closing the VMO improves the ability for the main thread to race with this
    // one.
    zx_thread_legacy_yield(0);
  }
}

// This test ensures that blobfs' lifecycle management correctly deals with a highly volatile
// number of VMO clones (which blobfs has special logic to handle, preventing the in-memory
// blob from being discarded while there are active clones).
// See https://fxbug.dev/42131342 for background on this test case.
TEST_P(BlobfsIntegrationTest, VmoCloneWatchingTest) {
  auto blob = TestBlobData::Create(4096, 'A');
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  struct CloneThreadArgs thread_args{
      .digest = delivery_blob.digest(),
      .blob_reader = blob_reader(),
  };
  std::thread clone_thread(CloneThread, &thread_args);

  constexpr int kIterations = 1000;
  for (int i = 0; i < kIterations; ++i) {
    // Ensure that the contents read out from the VMO match expectations.
    // If the blob is destroyed while there are still active clones, and paging is enabled, future
    // reads for uncommitted sections of the VMO will be full of zeroes (this is the kernel's
    // behavior when the pager source is detached from a pager-backed VMO), which would fail this
    // assertion.
    ASSERT_OK(blob_reader().VerifyBlob(blob));
  }

  thread_args.done = true;
  clone_thread.join();
}

TEST_P(BlobfsIntegrationTest, ReaddirAfterUnlinkingFileWithOpenHandleShouldNotReturnFile) {
  auto blob = TestBlobData::Create(1 << 5);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // Make sure the blob can be listed.
  ASSERT_THAT(ListBlobs(), testing::ElementsAre(blob.digest()));

  // Remount to ensure the pages in the blob aren't already present.
  ASSERT_OK(Remount());

  auto vmo = blob_reader().GetVmo(blob.digest());

  // Unlink the blob while it's still open.
  ASSERT_OK(Unlink(blob.digest()));

  // Check that the blob is no longer included in readdir.
  ASSERT_THAT(ListBlobs(), testing::IsEmpty());

  // Verify that the blob is still open.
  ASSERT_TRUE(VerifyContents(*vmo, blob.data()));
}

TEST_P(BlobfsIntegrationTest, ComponentOtaHealthCheckDuringBlobWrite) {
  auto blob = TestBlobData::Create(8192);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);

  fidl::UnownedClientEnd export_dir = fs().ServiceDirectory();
  zx::result client_end =
      component::ConnectAt<fuchsia_update_verify::ComponentOtaHealthCheck>(export_dir);
  ASSERT_OK(client_end) << "Opening verify service failed";
  fidl::WireSyncClient health_checker(std::move(client_end.value()));
  auto is_healthy = [&health_checker]() {
    fidl::WireResult result = health_checker->GetHealthStatus();
    ASSERT_OK(result.status());
    ASSERT_EQ(result->health_status, fuchsia_update_verify::HealthStatus::kHealthy);
  };
  ASSERT_NO_FATAL_FAILURE(is_healthy());

  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  ASSERT_NO_FATAL_FAILURE(is_healthy());

  auto incremental_writer = writer->CreateIncrementalWriter(delivery_blob);
  ASSERT_OK(incremental_writer);
  ASSERT_NO_FATAL_FAILURE(is_healthy());

  ASSERT_OK(incremental_writer->Write(4096));
  ASSERT_NO_FATAL_FAILURE(is_healthy());

  ASSERT_OK(incremental_writer->Complete());
  ASSERT_NO_FATAL_FAILURE(is_healthy());

  ASSERT_OK(blob_reader().VerifyBlob(blob));
  ASSERT_NO_FATAL_FAILURE(is_healthy());
}

TEST(BlobfsComponentMetricsTest, BlobLayoutMetrics) {
  fs_test::TestFilesystemOptions options = BlobfsWithPaddedLayoutTestParam();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(options.device_block_count * options.device_block_size, 0, &vmo));
  options.vmo = vmo.borrow();
  auto fs_or = fs_test::TestFilesystem::Create(options);
  ASSERT_OK(fs_or);
  fs_test::TestFilesystem fs = std::move(fs_or.value());

  // Start empty.
  int64_t padded_blobs;
  int64_t compact_blobs;
  {
    std::optional<diagnostics::reader::InspectData> snapshot;
    fs.TakeSnapshot(&snapshot);
    auto node = snapshot.value().payload().value()->GetByPath({"blob_layout_stats"});
    padded_blobs =
        node->node().get_property<inspect::IntPropertyValue>("padded_layout_blobs")->value();
    compact_blobs =
        node->node().get_property<inspect::IntPropertyValue>("compact_layout_blobs")->value();
  }
  ASSERT_EQ(padded_blobs, 0l);
  ASSERT_EQ(compact_blobs, 0l);

  // Add one blob in compressed format.
  auto blob = TestDeliveryBlob::CreateCompressed(kBlobfsBlockSize * 4);
  {
    std::unique_ptr<BlobCreatorWrapper> creator;
    auto creator_channel = component::ConnectAt<fuchsia_fxfs::BlobCreator>(fs.ServiceDirectory());
    ASSERT_OK(creator_channel);
    creator = std::make_unique<BlobCreatorWrapper>(
        fidl::WireSyncClient<fuchsia_fxfs::BlobCreator>(std::move(*creator_channel)));
    EXPECT_OK(creator->CreateAndWriteBlob(blob));
  }
  EXPECT_EQ(syncfs(fs.GetRootFd().get()), 0);
  {
    std::optional<diagnostics::reader::InspectData> snapshot;
    fs.TakeSnapshot(&snapshot);
    auto node = snapshot.value().payload().value()->GetByPath({"blob_layout_stats"});
    padded_blobs =
        node->node().get_property<inspect::IntPropertyValue>("padded_layout_blobs")->value();
    compact_blobs =
        node->node().get_property<inspect::IntPropertyValue>("compact_layout_blobs")->value();
  }
  ASSERT_EQ(padded_blobs, 1l);
  ASSERT_EQ(compact_blobs, 0l);

  // Remount while changing the write format.
  EXPECT_OK(fs.Unmount());
  {
    Superblock superblock;
    EXPECT_OK(vmo.read(&superblock, 0, sizeof(Superblock)));
    superblock.flags &= ~kBlobWriteLegacyMerkle;
    EXPECT_OK(vmo.write(&superblock, 0, sizeof(Superblock)));
  }
  EXPECT_OK(fs.Mount());

  // Count the blobs at mount.
  {
    std::optional<diagnostics::reader::InspectData> snapshot;
    fs.TakeSnapshot(&snapshot);
    auto node = snapshot.value().payload().value()->GetByPath({"blob_layout_stats"});
    padded_blobs =
        node->node().get_property<inspect::IntPropertyValue>("padded_layout_blobs")->value();
    compact_blobs =
        node->node().get_property<inspect::IntPropertyValue>("compact_layout_blobs")->value();
  }
  ASSERT_EQ(padded_blobs, 1l);
  ASSERT_EQ(compact_blobs, 0l);

  // Rewrite as compact.
  {
    std::unique_ptr<BlobCreatorWrapper> creator;
    auto creator_channel = component::ConnectAt<fuchsia_fxfs::BlobCreator>(fs.ServiceDirectory());
    ASSERT_OK(creator_channel);
    creator = std::make_unique<BlobCreatorWrapper>(
        fidl::WireSyncClient<fuchsia_fxfs::BlobCreator>(std::move(*creator_channel)));
    auto writer = creator->CreateExisting(blob.digest());
    EXPECT_OK(writer);
    EXPECT_OK(writer->WriteBlob(blob));
  }
  EXPECT_EQ(syncfs(fs.GetRootFd().get()), 0);
  {
    std::optional<diagnostics::reader::InspectData> snapshot;
    fs.TakeSnapshot(&snapshot);
    auto node = snapshot.value().payload().value()->GetByPath({"blob_layout_stats"});
    padded_blobs =
        node->node().get_property<inspect::IntPropertyValue>("padded_layout_blobs")->value();
    compact_blobs =
        node->node().get_property<inspect::IntPropertyValue>("compact_layout_blobs")->value();
  }
  ASSERT_EQ(padded_blobs, 0l);
  ASSERT_EQ(compact_blobs, 1l);

  // Remove the blob.
  ASSERT_EQ(unlinkat(fs.GetRootFd().get(), blob.digest().ToString().c_str(), 0), 0);

  {
    std::optional<diagnostics::reader::InspectData> snapshot;
    fs.TakeSnapshot(&snapshot);
    auto node = snapshot.value().payload().value()->GetByPath({"blob_layout_stats"});
    padded_blobs =
        node->node().get_property<inspect::IntPropertyValue>("padded_layout_blobs")->value();
    compact_blobs =
        node->node().get_property<inspect::IntPropertyValue>("compact_layout_blobs")->value();
  }
  ASSERT_EQ(padded_blobs, 0l);
  ASSERT_EQ(compact_blobs, 0l);
}

class BlobfsMetricIntegrationTest : public FdioTest {
 protected:
  void GetReadBytes(uint64_t* total_read_bytes) {
    const std::array<std::string, 2> algorithms = {"uncompressed", "chunked"};
    const std::array<std::string, 2> read_methods = {"paged_read_stats", "unpaged_read_stats"};
    inspect::Hierarchy hierarchy;
    TakeSnapshot(&hierarchy);
    *total_read_bytes = 0;
    for (const std::string& algorithm : algorithms) {
      for (const std::string& stat : read_methods) {
        uint64_t read_bytes;
        ASSERT_NO_FATAL_FAILURE(
            GetUintMetricFromHierarchy(hierarchy, {stat, algorithm}, "read_bytes", &read_bytes));
        *total_read_bytes += read_bytes;
      }
    }
  }
};

TEST_F(BlobfsMetricIntegrationTest, CreateAndRead) {
  uint64_t blobs_created;
  ASSERT_NO_FATAL_FAILURE(GetUintMetric({"allocation_stats"}, "blobs_created", &blobs_created));
  ASSERT_EQ(blobs_created, 0ul);

  auto blob = TestBlobData::Create(1 << 10);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  ASSERT_NO_FATAL_FAILURE(GetUintMetric({"allocation_stats"}, "blobs_created", &blobs_created));
  ASSERT_EQ(blobs_created, 1ul);

  uint64_t read_bytes = 0;
  ASSERT_NO_FATAL_FAILURE(GetReadBytes(&read_bytes));
  ASSERT_EQ(read_bytes, 0ul);

  ASSERT_OK(blob_reader().VerifyBlob(blob));

  ASSERT_NO_FATAL_FAILURE(GetReadBytes(&read_bytes));
  ASSERT_EQ(read_bytes, fbl::round_up(blob.data().size(), kBlobfsBlockSize));
}

TEST_F(BlobfsMetricIntegrationTest, BlobfsInspectTree) {
  using namespace inspect::testing;
  using namespace ::testing;

  inspect::Hierarchy hierarchy;
  TakeSnapshot(&hierarchy);

  // Ensure that all nodes we expect exist.
  for (const char* name :
       {fs_inspect::kInfoNodeName, fs_inspect::kUsageNodeName, fs_inspect::kFvmNodeName}) {
    ASSERT_NE(hierarchy.GetByPath({name}), nullptr)
        << "Could not find expected node in Blobfs inspect hierarchy: " << name;
  }

  // Test known values specific to Blobfs.
  const inspect::Hierarchy* info_node = hierarchy.GetByPath({fs_inspect::kInfoNodeName});
  ASSERT_NE(info_node, nullptr);
  EXPECT_THAT(
      *info_node,
      NodeMatches(AllOf(
          NameMatches(fs_inspect::kInfoNodeName),
          PropertyList(IsSupersetOf({StringIs(fs_inspect::InfoData::kPropName, "blobfs"),
                                     IntIs(fs_inspect::InfoData::kPropMaxFilenameLength, 64),
                                     StringIs(fs_inspect::InfoData::kPropOldestVersion,
                                              ::testing::MatchesRegex("^[0-9]+\\/[0-9]+$"))})))));

  const inspect::Hierarchy* usage_node = hierarchy.GetByPath({fs_inspect::kUsageNodeName});
  ASSERT_NE(usage_node, nullptr);
  EXPECT_THAT(*usage_node,
              NodeMatches(AllOf(
                  NameMatches(fs_inspect::kUsageNodeName),
                  PropertyList(IsSupersetOf({IntIs(fs_inspect::UsageData::kPropUsedNodes, 0)})))));

  // Create a file to increase the used inode count.
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(1 << 10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // Take a new snapshot of the tree and check that the used node count went up.
  TakeSnapshot(&hierarchy);

  usage_node = hierarchy.GetByPath({fs_inspect::kUsageNodeName});
  ASSERT_NE(usage_node, nullptr);
  EXPECT_THAT(*usage_node,
              NodeMatches(AllOf(
                  NameMatches(fs_inspect::kUsageNodeName),
                  PropertyList(IsSupersetOf({IntIs(fs_inspect::UsageData::kPropUsedNodes, 1)})))));
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, BlobfsIntegrationTest,
                         testing::Values(BlobfsDefaultTestParam(), BlobfsWithFvmTestParam(),
                                         BlobfsWithPaddedLayoutTestParam()),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace blobfs

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/volume_image/fvm/fvm_sparse_image_reader.h"

#include <fidl/fuchsia.storage.block/cpp/markers.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fpromise/result.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <utility>

#include <gtest/gtest.h>
#include <ramdevice-client/ramdisk.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/fvm/format.h"
#include "src/storage/fvm/fvm_sparse.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/block_protocol/types.h"
#include "src/storage/lib/buffer/resizeable_vmo_buffer.h"
#include "src/storage/lib/fs_management/cpp/admin.h"
#include "src/storage/lib/fs_management/cpp/component.h"
#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/options.h"
#include "src/storage/testing/fvm.h"
#include "src/storage/volume_image/address_descriptor.h"
#include "src/storage/volume_image/ftl/ftl_image.h"
#include "src/storage/volume_image/ftl/options.h"
#include "src/storage/volume_image/utils/fd_reader.h"
#include "src/storage/volume_image/utils/writer.h"

namespace storage::volume_image {
namespace {

constexpr std::string_view sparse_image_path = "/pkg/data/test_fvm.sparse.blk";

// Create a ram-disk and copy the output directly into the ram-disk, and then see if FVM can read
// it and minfs Fsck passes.
TEST(FvmSparseImageReaderTest, PartitionsInImagePassFsck) {
  auto base_reader_or = FdReader::Create(sparse_image_path);
  ASSERT_TRUE(base_reader_or.is_ok()) << base_reader_or.error();
  auto sparse_image_or = OpenSparseImage(base_reader_or.value(), std::nullopt);
  ASSERT_TRUE(sparse_image_or.is_ok()) << sparse_image_or.error();

  // Create a ram-disk.
  constexpr int kDeviceBlockSize = 8192;
  const uint64_t disk_size = sparse_image_or.value().volume().size;
  auto ram_disk_or =
      ramdevice_client::Ramdisk::Create(kDeviceBlockSize, disk_size / kDeviceBlockSize);
  ASSERT_OK(ram_disk_or);

  // Open the ram disk
  zx::result channel = component::Connect<fuchsia_storage_block::Block>(ram_disk_or.value().path());
  ASSERT_OK(channel);

  zx::result device = block_client::RemoteBlockDevice::Create(std::move(channel.value()));
  ASSERT_OK(device);
  std::unique_ptr<block_client::RemoteBlockDevice> client = std::move(device.value());

  constexpr uint64_t kInitialVmoSize = 1048576;
  storage::ResizeableVmoBuffer vmo(kDeviceBlockSize);
  ASSERT_OK(vmo.Attach("test", *client));
  ASSERT_OK(vmo.Grow(kInitialVmoSize / kDeviceBlockSize));

  memset(vmo.Data(0), 0xaf, kInitialVmoSize);

  // Initialize the entire ramdisk with a filler (that isn't zero).
  for (uint64_t offset = 0; offset < disk_size; offset += kInitialVmoSize) {
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
        .vmoid = vmo.vmoid(),
        .length =
            static_cast<uint32_t>(std::min(disk_size - offset, kInitialVmoSize) / kDeviceBlockSize),
        .dev_offset = offset / kDeviceBlockSize};
    ASSERT_OK(client->FifoTransaction(&request, 1));
  }

  for (const AddressMap& map : sparse_image_or.value().address().mappings) {
    ASSERT_EQ(map.count % kDeviceBlockSize, 0u);
    ASSERT_EQ(map.target % kDeviceBlockSize, 0u);
    ASSERT_LT(map.target, disk_size);
    ASSERT_LE(map.target + map.count, disk_size);
    EXPECT_TRUE(map.options.empty());

    if (vmo.capacity() * kDeviceBlockSize < map.count) {
      ASSERT_OK(vmo.Grow(map.count / kDeviceBlockSize));
    }
    auto result = sparse_image_or.value().reader()->Read(
        map.source, std::span<uint8_t>(reinterpret_cast<uint8_t*>(vmo.Data(0)), map.count));
    ASSERT_TRUE(result.is_ok()) << result.error();

    // Write the mapping to the ram disk.
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
        .vmoid = vmo.vmoid(),
        .length = static_cast<uint32_t>(map.count / kDeviceBlockSize),
        .dev_offset = map.target / kDeviceBlockSize,
    };

    ASSERT_OK(client->FifoTransaction(&request, 1))
        << "length=" << request.length << ", dev_offset=" << request.dev_offset;
  }

  ASSERT_OK(vmo.Detach(*client));
  client.reset();

  // Attempt to fsck minfs.
  {
    zx::result instance = storage::OpenFvmPartition(ram_disk_or.value().path(), "data");
    ASSERT_OK(instance);

    // And finally run fsck on the volume.
    fs_management::FsckOptions options{
        .verbose = false,
        .never_modify = true,
        .always_modify = false,
        .force = true,
    };
    auto component = fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatMinfs);
    EXPECT_OK(fs_management::Fsck(instance->path(), component, options));
  }

  // Attempt to fsck blobfs.
  {
    zx::result instance = storage::OpenFvmPartition(ram_disk_or.value().path(), "blobfs");
    ASSERT_OK(instance);

    // And finally run fsck on the volume.
    fs_management::FsckOptions options{
        .verbose = false,
        .never_modify = true,
        .always_modify = false,
        .force = true,
    };
    auto component = fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatBlobfs);
    EXPECT_OK(fs_management::Fsck(instance->path(), component, options));
  }
}

class NullWriter : public Writer {
 public:
  fpromise::result<void, std::string> Write(uint64_t offset,
                                            std::span<const uint8_t> buffer) override {
    return fpromise::ok();
  }
};

TEST(FvmSparseImageReaderTest, ImageWithMaxSizeAllocatesEnoughMetadata) {
  auto base_reader_or = FdReader::Create(sparse_image_path);
  ASSERT_TRUE(base_reader_or.is_ok()) << base_reader_or.error();

  fvm::SparseImage image = {};
  auto image_stream = std::span<uint8_t>(reinterpret_cast<uint8_t*>(&image), sizeof(image));
  auto read_result = base_reader_or.value().Read(0, image_stream);
  ASSERT_TRUE(read_result.is_ok()) << read_result.error();
  ASSERT_EQ(image.magic, fvm::kSparseFormatMagic);

  auto sparse_image_or = OpenSparseImage(base_reader_or.value(), 300 << 20);
  ASSERT_TRUE(sparse_image_or.is_ok()) << sparse_image_or.error();
  auto sparse_image = sparse_image_or.take_value();

  auto expected_header =
      fvm::Header::FromDiskSize(fvm::kMaxUsablePartitions, 300 << 20, image.slice_size);

  fvm::Header header = {};
  auto header_stream = std::span<uint8_t>(reinterpret_cast<uint8_t*>(&header), sizeof(header));
  read_result = sparse_image.reader()->Read(
      sparse_image.reader()->length() - expected_header.GetMetadataAllocatedBytes(), header_stream);
  ASSERT_TRUE(read_result.is_ok()) << read_result.error();
  ASSERT_EQ(header.magic, fvm::kMagic);

  EXPECT_EQ(expected_header.GetMetadataAllocatedBytes(), header.GetMetadataAllocatedBytes());
}

// This doesn't test that the resulting image is valid, but it least tests that FtlImageWrite can
// consume the sparse image without complaining.
TEST(FvmSparseImageReaderTest, WriteFtlImageSucceeds) {
  auto base_reader_or = FdReader::Create(sparse_image_path);
  ASSERT_TRUE(base_reader_or.is_ok()) << base_reader_or.error();
  auto sparse_image_or = OpenSparseImage(base_reader_or.value(), std::nullopt);
  ASSERT_TRUE(sparse_image_or.is_ok()) << sparse_image_or.error();

  constexpr int kFtlPageSize = 8192;
  NullWriter writer;
  auto result = FtlImageWrite(
      RawNandOptions{
          .page_size = kFtlPageSize,
          .page_count = static_cast<uint32_t>(sparse_image_or.value().volume().size / kFtlPageSize),
          .pages_per_block = 32,
          .oob_bytes_size = 16,
      },
      sparse_image_or.value(), &writer);
  EXPECT_TRUE(result.is_ok()) << result.error();
}

}  // namespace
}  // namespace storage::volume_image

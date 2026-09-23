// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <lib/fidl/cpp/wire/array.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <zstd/zstd.h>

#include "src/lib/digest/digest.h"
#include "src/lib/files/file.h"
#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob_creator.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blob_reader.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace blobfs {
namespace {

constexpr std::string_view kBlobfsImageV8Rev4 = "/pkg/data/blobfs.8.4.img.zstd";
constexpr std::string_view kBlobfsImageV9Rev4 = "/pkg/data/blobfs.9.4.img.zstd";

// A 400 byte file. This blob won't have a merkle tree stored with it.
constexpr fidl::Array<uint8_t, digest::kSha256Length> kSmallBlobDigest = {
    0x0f, 0x2f, 0xa3, 0x9c, 0x6c, 0x0d, 0x78, 0x9f, 0x68, 0x00, 0x60, 0x9e, 0xa7, 0xec, 0x8c, 0xd6,
    0x67, 0xb5, 0x85, 0xb5, 0x8c, 0x12, 0x02, 0x0d, 0xa5, 0x9a, 0x88, 0xfe, 0x38, 0x81, 0x36, 0xfb};

// A 12KB file that compresses to well under 8KiB. The blob takes up 2 blocks in the padded blob
// layout format and only 1 block in the compact format.
constexpr fidl::Array<uint8_t, digest::kSha256Length> kLargeBlobDigest = {
    0x10, 0x03, 0x0a, 0x1d, 0x8f, 0xe9, 0xf8, 0x8b, 0xca, 0x39, 0xb3, 0x60, 0xcd, 0xf8, 0xe4, 0x4c,
    0xb3, 0xae, 0xb9, 0x17, 0xc1, 0x1c, 0x77, 0x1f, 0x62, 0x91, 0x36, 0x49, 0x8d, 0xe0, 0x30, 0x92};

constexpr uint32_t kBlockSize = 512;
constexpr MountOptions kMountReadOnly = {.writability = Writability::ReadOnlyFilesystem};
constexpr MountOptions kMountWritable = {.writability = Writability::Writable};

std::unique_ptr<block_client::FakeBlockDevice> LoadBlobfsImage(const std::string_view& image_file) {
  std::vector<uint8_t> compressed_data;
  files::ReadFileToVector(std::string(image_file), &compressed_data);
  auto expected_uncompressed_size =
      ZSTD_getFrameContentSize(compressed_data.data(), compressed_data.size());
  ZX_ASSERT(expected_uncompressed_size != ZSTD_CONTENTSIZE_ERROR);
  ZX_ASSERT(expected_uncompressed_size != ZSTD_CONTENTSIZE_UNKNOWN);
  std::vector<uint8_t> uncompressed_data(expected_uncompressed_size);
  auto uncompressed_size = ZSTD_decompress(uncompressed_data.data(), uncompressed_data.size(),
                                           compressed_data.data(), compressed_data.size());
  ZX_ASSERT(!ZSTD_isError(uncompressed_size));
  ZX_ASSERT(expected_uncompressed_size == uncompressed_size);

  uint64_t block_count = uncompressed_data.size() / kBlockSize;

  auto device = std::make_unique<block_client::FakeBlockDevice>(block_count, kBlockSize);
  auto vmo = device->VmoChildReference();
  ZX_ASSERT(vmo.is_ok());
  ZX_ASSERT(vmo->write(uncompressed_data.data(), 0, uncompressed_data.size()) == ZX_OK);

  return device;
}

fidl::Array<uint8_t, digest::kSha256Length> DigestToFidlArray(const Digest& digest) {
  fidl::Array<uint8_t, digest::kSha256Length> array;
  digest.CopyTo(array.data_);
  return array;
}

class BlobfsMigrationTest : public BlobfsTestSetupWithThread, public testing::Test {
 protected:
  void VerifyBlob(const fidl::Array<uint8_t, digest::kSha256Length>& blob_hash) {
    // The `fidl::ServerBindingGroup` inside of the `BlobReader` is not thread safe. All bindings
    // added and removed from it must happen on the dispatcher thread. A task is posted to the
    // dispatcher thread to create the connection binding and another task is posted to destroy the
    // BlobReader which will destroying the binding.
    auto reader = std::make_unique<BlobReader>(*blobfs());

    auto [client, server] = fidl::Endpoints<fuchsia_fxfs::BlobReader>::Create();
    RunOnDispatcherThread([&reader, &server]() { reader->ConnectService(server.TakeChannel()); });

    // Any mismatch in the blob layout format in a large blob should fail this call. Using the wrong
    // format will read the wrong blocks when initializing the decompressor and the blob will fail
    // to open.
    auto response = fidl::WireCall(client)->GetVmo(blob_hash);
    ZX_ASSERT(response.ok());
    ZX_ASSERT(response->is_ok());
    auto& vmo = response->value()->vmo;
    uint64_t size = GetVmoSize(vmo);
    std::vector<uint8_t> data(size);
    // Read the entire blob to force blobfs to page in and verify the blob's contents.
    ZX_ASSERT(vmo.read(data.data(), 0, size) == ZX_OK);

    RunOnDispatcherThread([reader = std::move(reader)]() mutable {});
  }

  void WriteBlob(const TestDeliveryBlob& blob) {
    // The `fidl::ServerBindingGroup`s inside of the `BlobCreator` are not thread safe. All bindings
    // added and removed from them must happen on the dispatcher thread. A task is posted to the
    // dispatcher thread to create the connection bindings and another task is posted to destroy the
    // `BlobCreator` which will destroying the bindings.
    auto creator = std::make_unique<BlobCreator>(*blobfs());
    auto [creator_client, creator_server] = fidl::Endpoints<fuchsia_fxfs::BlobCreator>::Create();
    RunOnDispatcherThread(
        [&creator, &creator_server]() { creator->ConnectService(creator_server.TakeChannel()); });

    BlobCreatorWrapper creator_wrapper =
        BlobCreatorWrapper(fidl::WireSyncClient(std::move(creator_client)));
    ZX_ASSERT(creator_wrapper.CreateAndWriteBlob(blob).status_value() == ZX_OK);

    RunOnDispatcherThread([creator = std::move(creator)]() mutable {});
  }

  BlobLayoutFormat GetInodeFormat(uint32_t node_index) {
    auto node = blobfs()->GetNode(node_index);
    ZX_ASSERT(node.status_value() == ZX_OK);
    ZX_ASSERT(node->header.IsInode());
    ZX_ASSERT(node->header.IsAllocated());
    return GetBlobLayoutFormat(blobfs()->Info(), **node);
  }
};

TEST_F(BlobfsMigrationTest, ValidateV8Rev4ImageAssumptions) {
  auto device = LoadBlobfsImage(kBlobfsImageV8Rev4);
  Mount(std::move(device), kMountReadOnly);

  EXPECT_EQ(blobfs()->Info().major_version, 0x8u);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);

  auto small_blob_inode = blobfs()->GetNode(0);
  ASSERT_OK(small_blob_inode.status_value());
  ASSERT_EQ(small_blob_inode->block_count, 1u);
  ASSERT_THAT(small_blob_inode->merkle_root_hash, testing::ElementsAreArray(kSmallBlobDigest));

  auto large_blob_inode = blobfs()->GetNode(1);
  ASSERT_OK(large_blob_inode.status_value());
  ASSERT_EQ(large_blob_inode->block_count, 2u);
  ASSERT_THAT(large_blob_inode->merkle_root_hash, testing::ElementsAreArray(kLargeBlobDigest));
}

TEST_F(BlobfsMigrationTest, ValidateV9Rev4ImageAssumptions) {
  auto device = LoadBlobfsImage(kBlobfsImageV9Rev4);
  Mount(std::move(device), kMountReadOnly);

  EXPECT_EQ(blobfs()->Info().major_version, 0x9u);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);

  auto small_blob_inode = blobfs()->GetNode(0);
  ASSERT_OK(small_blob_inode.status_value());
  ASSERT_EQ(small_blob_inode->block_count, 1u);
  ASSERT_THAT(small_blob_inode->merkle_root_hash, testing::ElementsAreArray(kSmallBlobDigest));

  auto large_blob_inode = blobfs()->GetNode(1);
  ASSERT_OK(large_blob_inode.status_value());
  ASSERT_EQ(large_blob_inode->block_count, 1u);
  ASSERT_THAT(large_blob_inode->merkle_root_hash, testing::ElementsAreArray(kLargeBlobDigest));
}

TEST_F(BlobfsMigrationTest, MigrateFromV8Rev4ToV10Rev4) {
  auto device = LoadBlobfsImage(kBlobfsImageV8Rev4);
  Mount(std::move(device), kMountWritable);

  // Blobfs was migrated
  EXPECT_EQ(blobfs()->Info().major_version, 0xAu);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_NE(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  // Blobs are still readable.
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);

  // New blobs in correct format.
  auto blob = TestDeliveryBlob::CreateCompressed(9000);
  WriteBlob(blob);

  VerifyBlob(DigestToFidlArray(blob.digest()));
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);

  Remount(kMountReadOnly);

  // Migration persisted.
  EXPECT_EQ(blobfs()->Info().major_version, 0xAu);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_NE(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  VerifyBlob(DigestToFidlArray(blob.digest()));
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
}

TEST_F(BlobfsMigrationTest, MigrateFromV9Rev4ToV10Rev4) {
  auto device = LoadBlobfsImage(kBlobfsImageV9Rev4);
  Mount(std::move(device), kMountWritable);

  // Blobfs was migrated
  EXPECT_EQ(blobfs()->Info().major_version, 0xAu);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_EQ(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  // Blobs are still readable.
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);

  // New blobs in correct format.
  auto blob = TestDeliveryBlob::CreateCompressed(9000);
  WriteBlob(blob);

  VerifyBlob(DigestToFidlArray(blob.digest()));
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kCompactMerkleTreeAtEnd);

  Remount(kMountReadOnly);

  // Migration persisted.
  EXPECT_EQ(blobfs()->Info().major_version, 0xAu);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_EQ(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  VerifyBlob(DigestToFidlArray(blob.digest()));
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
}

TEST_F(BlobfsMigrationTest, MountV8Rev4ReadOnly) {
  auto device = LoadBlobfsImage(kBlobfsImageV8Rev4);
  Mount(std::move(device), kMountReadOnly);

  // Blobfs wasn't migrated.
  EXPECT_EQ(blobfs()->Info().major_version, 0x8u);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_EQ(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  // Blobs are still readable based on the superblock version.
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
}

TEST_F(BlobfsMigrationTest, MountV9Rev4ReadOnly) {
  auto device = LoadBlobfsImage(kBlobfsImageV9Rev4);
  Mount(std::move(device), kMountReadOnly);

  // Blobfs wasn't migrated.
  EXPECT_EQ(blobfs()->Info().major_version, 0x9u);
  EXPECT_EQ(blobfs()->Info().oldest_minor_version, 0x4ul);
  EXPECT_EQ(blobfs()->Info().flags & kBlobWriteLegacyMerkle, 0u);
  // Blobs are still readable based on the superblock version.
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
}

TEST_F(BlobfsMigrationTest, MixedBlobLayoutFormatsFromV8) {
  auto device = LoadBlobfsImage(kBlobfsImageV8Rev4);
  Mount(std::move(device), kMountWritable);
  EXPECT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  // Force writing the other format to ensure that it can parse properly.
  blobfs()->SetOverwriteConfig(BlobOverwriteConfig::kOverwriteToCompact);
  EXPECT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kCompactMerkleTreeAtEnd);

  auto blob = TestDeliveryBlob::CreateCompressed(9000);
  WriteBlob(blob);

  // Blobfs V10Rev4 supports blobs in both formats.
  VerifyBlob(DigestToFidlArray(blob.digest()));
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  EXPECT_EQ(GetInodeFormat(0), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  EXPECT_EQ(GetInodeFormat(1), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kCompactMerkleTreeAtEnd);

  Remount(kMountWritable);

  VerifyBlob(DigestToFidlArray(blob.digest()));
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  EXPECT_EQ(GetInodeFormat(0), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  EXPECT_EQ(GetInodeFormat(1), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
}

TEST_F(BlobfsMigrationTest, MixedBlobLayoutFormatsFromV9) {
  auto device = LoadBlobfsImage(kBlobfsImageV9Rev4);
  Mount(std::move(device), kMountWritable);
  EXPECT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  // Force writing the other format to ensure that it can parse properly.
  blobfs()->SetOverwriteConfig(BlobOverwriteConfig::kOverwriteToPadded);
  EXPECT_EQ(blobfs()->BlobWriteFormat(), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);

  auto blob = TestDeliveryBlob::CreateCompressed(9000);
  WriteBlob(blob);

  // Blobfs V10Rev4 supports blobs in both formats.
  VerifyBlob(DigestToFidlArray(blob.digest()));
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  EXPECT_EQ(GetInodeFormat(0), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  EXPECT_EQ(GetInodeFormat(1), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);

  Remount(kMountWritable);

  VerifyBlob(DigestToFidlArray(blob.digest()));
  VerifyBlob(kSmallBlobDigest);
  VerifyBlob(kLargeBlobDigest);
  EXPECT_EQ(GetInodeFormat(0), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  EXPECT_EQ(GetInodeFormat(1), BlobLayoutFormat::kCompactMerkleTreeAtEnd);
  EXPECT_EQ(GetInodeFormat(2), BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
}

}  // namespace
}  // namespace blobfs

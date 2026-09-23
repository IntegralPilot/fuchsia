// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_loader.h"

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <span>
#include <string>
#include <tuple>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/digest/digest.h"
#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/blobfs/test/unit/local_decompressor_creator.h"
#include "src/storage/blobfs/test/unit/utils.h"
#include "src/storage/blobfs/transaction.h"
#include "src/storage/lib/operation/operation.h"

namespace blobfs {

namespace {
constexpr uint32_t kTestBlockSize = 512;
constexpr uint32_t kNumBlocks = 400 * kBlobfsBlockSize / kTestBlockSize;

using ::testing::Combine;
using ::testing::TestParamInfo;
using ::testing::TestWithParam;
using ::testing::ValuesIn;

using TestParamType = std::tuple<CompressionAlgorithm, BlobLayoutFormat>;

}  // namespace

// This class isn't in the anonymous namespace because it needs to be friended by the `Blob` class
// to access some of the private members.
class BlobLoaderTest : public TestWithParam<TestParamType> {
 public:
  void SetUp() override {
    std::tie(compression_algorithm_, blob_layout_format_) = GetParam();
    srand(testing::UnitTest::GetInstance()->random_seed());

    FilesystemOptions fs_options{
        .blob_layout_format = blob_layout_format_,
    };
    auto connector_or = LocalDecompressorCreator::Create();
    ASSERT_TRUE(connector_or.is_ok());
    decompressor_creator_ = std::move(connector_or.value());
    options_ = {
        .decompression_connector = &decompressor_creator_->GetDecompressorConnector(),
    };
    ASSERT_OK(setup_.CreateFormatMount(kNumBlocks, kTestBlockSize, fs_options, options_));

    // Pre-seed with some blobs.
    for (int i = 0; i < 3; i++) {
      AddBlob(1024, i);
    }
    ASSERT_OK(setup_.Remount(options_));
  }

  // AddBlob creates and writes a blob of a specified size to the file system. The contents of the
  // blob are compressible at a realistic level for a typical ELF binary.
  TestBlobData AddBlob(size_t sz, int prefix) {
    auto blob_data = TestBlobData::CreateRealistic(sz, prefix);
    auto delivery_blob =
        TestDeliveryBlob::CreateWithCompressionAlgorithm(blob_data, compression_algorithm_);
    ZX_ASSERT(CreateBlob(*setup_.blobfs(), delivery_blob).is_ok());
    return blob_data;
  }

  BlobLoader& loader() { return setup_.blobfs()->loader(); }

  CompressionAlgorithm ExpectedAlgorithm() const { return compression_algorithm_; }

  fbl::RefPtr<Blob> LookupBlob(const Digest& digest) {
    auto blob = GetBlob(*setup_.blobfs(), digest);
    ZX_ASSERT(blob.is_ok());
    return blob.value();
  }

  uint32_t LookupInode(const Digest& digest) { return LookupBlob(digest)->Ino(); }

  CompressionAlgorithm LookupCompression(const Digest& digest) {
    auto algorithm_or = AlgorithmForInode(*setup_.blobfs()->GetNode(LookupInode(digest)).value());
    EXPECT_TRUE(algorithm_or.is_ok());
    return algorithm_or.value();
  }

  // Used to access protected Blob/BlobVerifier members because this class is a friend.
  static std::span<const uint8_t> GetBlobMerkleData(const Blob& blob) {
    std::lock_guard lock(blob.mutex_);
    return blob.loader_info_.verifier->merkle_data();
  }

  void CheckMerkleTreeContents(std::span<const uint8_t> merkle_data,
                               std::span<const uint8_t> blob_data) {
    TestMerkleTree merkle_tree(blob_data, ShouldUseCompactMerkleTreeFormat(blob_layout_format_));
    ASSERT_TRUE(std::ranges::equal(merkle_data, merkle_tree.merkle_tree()));
  }

 protected:
  std::unique_ptr<LocalDecompressorCreator> decompressor_creator_;
  BlobfsTestSetup setup_;

  MountOptions options_;
  BlobLayoutFormat blob_layout_format_;
  CompressionAlgorithm compression_algorithm_;
};

namespace {

TEST_P(BlobLoaderTest, SmallBlob) {
  size_t blob_len = 1024;
  TestBlobData blob_data = AddBlob(blob_len, 5);
  ASSERT_OK(setup_.Remount(options_));
  // We explicitly don't check the compression algorithm was respected here, since files this small
  // don't need to be compressed.

  auto blob = LookupBlob(blob_data.digest());
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));

  // Verify there's no Merkle data for this small blob.
  const auto& merkle = GetBlobMerkleData(*blob);
  EXPECT_EQ(merkle.size(), 0ul);
}

TEST_P(BlobLoaderTest, LargeBlob) {
  size_t blob_len = 1 << 18;
  TestBlobData blob_data = AddBlob(blob_len, 5);
  ASSERT_OK(setup_.Remount(options_));
  ASSERT_EQ(LookupCompression(blob_data.digest()), ExpectedAlgorithm());

  auto blob = LookupBlob(blob_data.digest());
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));

  CheckMerkleTreeContents(GetBlobMerkleData(*blob), blob_data.data());
}

TEST_P(BlobLoaderTest, LargeBlobWithNonAlignedLength) {
  size_t blob_len = (1 << 18) - 1;
  TestBlobData blob_data = AddBlob(blob_len, 5);
  ASSERT_OK(setup_.Remount(options_));
  ASSERT_EQ(LookupCompression(blob_data.digest()), ExpectedAlgorithm());

  auto blob = LookupBlob(blob_data.digest());
  auto vmo = blob->GetVmoForBlobReader();
  ASSERT_OK(vmo);
  ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()));

  CheckMerkleTreeContents(GetBlobMerkleData(*blob), blob_data.data());
}

TEST_P(BlobLoaderTest, NullBlobWithCorruptedMerkleRootFailsToLoad) {
  TestBlobData blob_data = AddBlob(0, 0);

  // The added empty blob should be valid.
  auto blob = LookupBlob(blob_data.digest());
  ASSERT_OK(blob->Verify());

  uint8_t corrupt_merkle_root[digest::kSha256Length] = "-corrupt-null-blob-merkle-root-";
  Digest corrupt_digest(corrupt_merkle_root);
  {
    // Corrupt the null blob's merkle root.
    // |inode| holds a pointer into |blobfs()| and needs to be destroyed before remounting.
    auto inode = setup_.blobfs()->GetNode(blob->Ino());
    corrupt_digest.CopyTo(inode->merkle_root_hash);
    BlobTransaction transaction;
    const uint64_t block = (blob->Ino() * kBlobfsInodeSize) / kBlobfsBlockSize;
    transaction.AddOperation(
        {.vmo = zx::unowned_vmo(setup_.blobfs()->GetAllocator()->GetNodeMapVmo().get()),
         .op = {
             .type = storage::OperationType::kWrite,
             .vmo_offset = block,
             .dev_offset = NodeMapStartBlock(setup_.blobfs()->Info()) + block,
             .length = 1,
         }});
    transaction.Commit(*setup_.blobfs()->GetJournal());
  }

  // Remount the filesystem so the node cache will pickup the new name for the blob.
  blob.reset();  // Required for Remount() to succeed.
  ASSERT_OK(setup_.Remount(options_));

  // Verify the empty blob can be found by the corrupt name.
  // Loading the data should report corruption.
  auto corrupt_blob = LookupBlob(corrupt_digest);
  ASSERT_STATUS(corrupt_blob->Verify(), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_P(BlobLoaderTest, LoadBlobWithAnInvalidNodeIndexIsAnError) {
  uint32_t invalid_node_index = kMaxNodeId - 1;
  auto result = loader().LoadBlob(invalid_node_index);
  ASSERT_TRUE(result.is_error());
  EXPECT_STATUS(result.error_value(), ZX_ERR_INVALID_ARGS);
}

TEST_P(BlobLoaderTest, LoadBlobWithACorruptNextNodeIndexIsAnError) {
  TestBlobData blob_data = AddBlob(1 << 14, 5);
  ASSERT_OK(setup_.Remount(options_));

  // Corrupt the next node index of the inode.
  uint32_t invalid_node_index = kMaxNodeId - 1;
  uint32_t node_index = LookupInode(blob_data.digest());
  auto inode = setup_.blobfs()->GetAllocator()->GetNode(node_index);
  ASSERT_TRUE(inode.is_ok());
  inode->header.next_node = invalid_node_index;
  inode->extent_count = 2;

  auto result = loader().LoadBlob(node_index);
  ASSERT_TRUE(result.is_error());
  EXPECT_STATUS(result.error_value(), ZX_ERR_IO_DATA_INTEGRITY);
}

std::string GetTestParamName(const TestParamInfo<TestParamType>& param) {
  auto [compression_algorithm, blob_layout_format] = param.param;
  return GetBlobLayoutFormatNameForTests(blob_layout_format) +
         GetCompressionAlgorithmName(compression_algorithm);
}

constexpr std::array<CompressionAlgorithm, 2> kCompressionAlgorithms = {
    CompressionAlgorithm::kUncompressed,
    CompressionAlgorithm::kChunked,
};

constexpr std::array<BlobLayoutFormat, 2> kBlobLayouts = {
    BlobLayoutFormat::kCompactMerkleTreeAtEnd,
    BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart,
};

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, BlobLoaderTest,
                         Combine(ValuesIn(kCompressionAlgorithms), ValuesIn(kBlobLayouts)),
                         GetTestParamName);

}  // namespace
}  // namespace blobfs

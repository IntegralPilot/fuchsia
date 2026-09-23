// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_verifier.h"

#include <lib/fzl/owned-vmo-mapper.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <span>
#include <string>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/array.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs_metrics.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"

namespace blobfs {
namespace {

struct BlockMerkleTreeInfo {
  fzl::OwnedVmoMapper blocks;
  // Points into |blocks|, will be offset according to blob format.
  uint8_t* merkle_data = nullptr;
  Digest root;

  std::span<const uint8_t> GetMerkleDataBlocks() const {
    return std::span(static_cast<const uint8_t*>(blocks.start()), blocks.size());
  }
};

class BlobVerifierTest : public testing::TestWithParam<BlobLayoutFormat> {
 public:
  std::shared_ptr<BlobfsMetrics> GetMetrics() { return metrics_; }

  void SetUp() override { srand(testing::UnitTest::GetInstance()->random_seed()); }

  // Creates a default blob layout for an uncompressed file of the given size.
  static std::unique_ptr<BlobLayout> GetBlobLayout(size_t size) {
    auto layout_or = BlobLayout::CreateFromSizes(GetParam(), size, size, kBlobfsBlockSize);
    EXPECT_OK(layout_or);  // Should always succeed in this use.
    return std::move(*layout_or);
  }

  static TestMerkleTree GenerateTree(std::span<const uint8_t> data) {
    return TestMerkleTree(data, ShouldUseCompactMerkleTreeFormat(GetParam()));
  }

  // Like GenerateTree but puts the merkle data into a VMO that will have the merkle data aligned
  // inside of it as it would on disk according to the given layout.
  static BlockMerkleTreeInfo GenerateMerkleTreeBlocks(const BlobLayout& layout,
                                                      std::span<const uint8_t> data) {
    TestMerkleTree tree = GenerateTree(data);

    BlockMerkleTreeInfo block_info;
    EXPECT_OK(block_info.blocks.CreateAndMap(layout.MerkleTreeBlockAlignedSize(), "Merkle blocks"));

    auto start_offset = layout.MerkleTreeOffsetWithinBlockOffset();
    EXPECT_LE(tree.merkle_tree().size() + start_offset, block_info.blocks.size());

    block_info.merkle_data = &static_cast<uint8_t*>(block_info.blocks.start())[start_offset];
    memcpy(block_info.merkle_data, tree.merkle_tree().data(), tree.merkle_tree().size());

    block_info.root = tree.digest();
    return block_info;
  }

 private:
  std::shared_ptr<BlobfsMetrics> metrics_ = std::make_shared<BlobfsMetrics>(false);
};

void FillWithRandom(uint8_t* buf, size_t len) {
  for (unsigned i = 0; i < len; ++i) {
    buf[i] = static_cast<uint8_t>(rand());
  }
}

TEST_P(BlobVerifierTest, CreateAndVerifyNullBlob) {
  auto merkle_tree = GenerateTree(std::span<const uint8_t>());

  auto verifier_or = BlobVerifier::CreateWithoutTree(merkle_tree.digest(), GetMetrics(), 0ul);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_OK(verifier->Verify(nullptr, 0ul, 0ul));
  EXPECT_OK(verifier->VerifyPartial(nullptr, 0ul, 0ul, 0ul));
}

TEST_P(BlobVerifierTest, CreateAndVerifySmallBlob) {
  uint8_t buf[8192];
  FillWithRandom(buf, sizeof(buf));

  auto merkle_tree = GenerateTree(buf);

  auto verifier_or =
      BlobVerifier::CreateWithoutTree(merkle_tree.digest(), GetMetrics(), sizeof(buf));
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_OK(verifier->Verify(buf, sizeof(buf), sizeof(buf)));

  EXPECT_OK(verifier->VerifyPartial(buf, 8192, 0, 8192));

  // Partial ranges
  EXPECT_STATUS(verifier->VerifyPartial(buf, 8191, 0, 8191), ZX_ERR_INVALID_ARGS);

  // Verify past the end
  EXPECT_STATUS(
      verifier->VerifyPartial(buf, static_cast<size_t>(2) * 8192, 0, static_cast<size_t>(2) * 8192),
      ZX_ERR_INVALID_ARGS);
}

TEST_P(BlobVerifierTest, CreateAndVerifySmallBlobDataCorrupted) {
  uint8_t buf[8192];
  FillWithRandom(buf, sizeof(buf));

  auto merkle_tree = GenerateTree(buf);

  // Invert one character
  buf[42] = static_cast<uint8_t>(~(buf[42]));

  auto verifier_or =
      BlobVerifier::CreateWithoutTree(merkle_tree.digest(), GetMetrics(), sizeof(buf));
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_STATUS(verifier->Verify(buf, sizeof(buf), sizeof(buf)), ZX_ERR_IO_DATA_INTEGRITY);
  EXPECT_STATUS(verifier->VerifyPartial(buf, 8192, 0, 8192), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_P(BlobVerifierTest, CreateAndVerifyBigBlob) {
  size_t sz = 1 << 16;
  auto buf = fbl::MakeArray<uint8_t>(sz);
  FillWithRandom(buf.get(), sz);

  auto layout = GetBlobLayout(sz);
  BlockMerkleTreeInfo info = GenerateMerkleTreeBlocks(*layout, buf);

  auto verifier_or =
      BlobVerifier::Create(info.root, GetMetrics(), info.GetMerkleDataBlocks(), *layout);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_OK(verifier->Verify(buf.get(), sz, sz));
  EXPECT_OK(verifier->VerifyPartial(buf.get(), sz, 0, sz));

  // Block-by-block
  for (size_t i = 0; i < sz; i += 8192) {
    EXPECT_OK(verifier->VerifyPartial(buf.get() + i, 8192, i, 8192));
  }

  // Partial ranges
  EXPECT_STATUS(verifier->VerifyPartial(buf.data(), 8191, 0, 8191), ZX_ERR_INVALID_ARGS);

  // Verify past the end
  EXPECT_STATUS(verifier->VerifyPartial(buf.data() + (sz - 8192), static_cast<size_t>(2) * 8192,
                                        sz - 8192, static_cast<size_t>(2) * 8192),
                ZX_ERR_INVALID_ARGS);
}

TEST_P(BlobVerifierTest, CreateAndVerifyBigBlobDataCorrupted) {
  size_t sz = 1 << 16;
  auto buf = fbl::MakeArray<uint8_t>(sz);
  FillWithRandom(buf.get(), sz);

  auto layout = GetBlobLayout(sz);
  BlockMerkleTreeInfo info = GenerateMerkleTreeBlocks(*layout, buf);

  // Invert a char in the first block. All other blocks are still valid.
  buf.get()[42] = static_cast<uint8_t>(~(buf.get()[42]));

  auto verifier_or =
      BlobVerifier::Create(info.root, GetMetrics(), info.GetMerkleDataBlocks(), *layout);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_STATUS(verifier->Verify(buf.get(), sz, sz), ZX_ERR_IO_DATA_INTEGRITY);
  EXPECT_STATUS(verifier->VerifyPartial(buf.get(), sz, 0, sz), ZX_ERR_IO_DATA_INTEGRITY);

  // Block-by-block -- first block fails, rest succeed
  for (size_t i = 0; i < sz; i += 8192) {
    zx_status_t status = verifier->VerifyPartial(buf.get() + i, 8192, i, 8192);
    if (i == 0) {
      EXPECT_STATUS(status, ZX_ERR_IO_DATA_INTEGRITY);
    } else {
      EXPECT_OK(status);
    }
  }
}

TEST_P(BlobVerifierTest, CreateAndVerifyBigBlobMerkleCorrupted) {
  size_t sz = 1 << 16;
  auto buf = fbl::MakeArray<uint8_t>(sz);
  FillWithRandom(buf.get(), sz);

  auto layout = GetBlobLayout(sz);
  BlockMerkleTreeInfo info = GenerateMerkleTreeBlocks(*layout, buf);

  // Invert a char in the tree.
  info.merkle_data[0] ^= 0xff;

  auto verifier_or =
      BlobVerifier::Create(info.root, GetMetrics(), info.GetMerkleDataBlocks(), *layout);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_STATUS(verifier->Verify(buf.get(), sz, sz), ZX_ERR_IO_DATA_INTEGRITY);
  EXPECT_STATUS(verifier->VerifyPartial(buf.get(), sz, 0, sz), ZX_ERR_IO_DATA_INTEGRITY);

  // Block-by-block -- everything fails
  for (size_t i = 0; i < sz; i += 8192) {
    EXPECT_STATUS(verifier->VerifyPartial(buf.get() + i, 8192, i, 8192), ZX_ERR_IO_DATA_INTEGRITY);
  }
}

TEST_P(BlobVerifierTest, NonZeroTailCausesVerifyToFail) {
  constexpr int kBlobSize = 8000;
  uint8_t buf[kBlobfsBlockSize];
  FillWithRandom(buf, kBlobSize);
  // Zero the tail.
  memset(&buf[kBlobSize], 0, kBlobfsBlockSize - kBlobSize);

  auto merkle_tree = GenerateTree(std::span(buf).subspan(0, kBlobSize));

  auto verifier_or = BlobVerifier::CreateWithoutTree(merkle_tree.digest(), GetMetrics(), kBlobSize);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  EXPECT_OK(verifier->Verify(buf, kBlobSize, sizeof(buf)));

  buf[kBlobSize] = 1;
  EXPECT_STATUS(verifier->Verify(buf, kBlobSize, sizeof(buf)), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_P(BlobVerifierTest, NonZeroTailCausesVerifyPartialToFail) {
  constexpr unsigned kBlobSize = (1 << 16) - 100;
  std::vector<uint8_t> buf(fbl::round_up(kBlobSize, kBlobfsBlockSize));
  FillWithRandom(buf.data(), kBlobSize);

  auto layout = GetBlobLayout(kBlobSize);
  BlockMerkleTreeInfo info =
      GenerateMerkleTreeBlocks(*layout, std::span(buf).subspan(0, kBlobSize));

  auto verifier_or =
      BlobVerifier::Create(info.root, GetMetrics(), info.GetMerkleDataBlocks(), *layout);
  ASSERT_OK(verifier_or);
  BlobVerifier* verifier = verifier_or.value().get();

  constexpr int kVerifyOffset = kBlobSize - (kBlobSize % kBlobfsBlockSize);
  EXPECT_OK(verifier->VerifyPartial(&buf[kVerifyOffset], kBlobSize - kVerifyOffset, kVerifyOffset,
                                    buf.size() - kVerifyOffset));

  buf[kBlobSize] = 1;
  EXPECT_STATUS(verifier->VerifyPartial(&buf[kVerifyOffset], kBlobSize - kVerifyOffset,
                                        kVerifyOffset, buf.size() - kVerifyOffset),
                ZX_ERR_IO_DATA_INTEGRITY);
}

std::string GetTestName(const testing::TestParamInfo<BlobLayoutFormat>& param) {
  return GetBlobLayoutFormatNameForTests(param.param);
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, BlobVerifierTest,
                         ::testing::Values(BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart,
                                           BlobLayoutFormat::kCompactMerkleTreeAtEnd),
                         GetTestName);

}  // namespace
}  // namespace blobfs

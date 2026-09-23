// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/delivery_blob.h"

#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <string>
#include <tuple>
#include <utility>

#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/digest/digest.h"
#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/blobfs_test_setup.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {

namespace {

constexpr uint32_t kTestDeviceBlockSize = 512;
constexpr uint32_t kTestDeviceNumBlocks = 400 * kBlobfsBlockSize / kTestDeviceBlockSize;
constexpr size_t kSmallBlobSize = 1024;
constexpr size_t kLargeBlobSize = kBlobfsBlockSize * 20;

// Large blobs must cover at least two levels in the Merkle tree to cover all branches.
static_assert(kLargeBlobSize > kBlobfsBlockSize);

struct DeliveryBlobTestParams {
  // Blob layout format that the Blobfs instance should be formatted with.
  BlobLayoutFormat format;

  // If true, specify that the delivery blob should be compressed.
  bool compress;

  // Size of the blob the test case should write.
  size_t blob_size;

  DeliveryBlobType type;

  using ParamsAsTuple = std::tuple</*format*/ BlobLayoutFormat, /*compress*/ bool,
                                   /*blob_size*/ size_t, /*type*/ DeliveryBlobType>;

  explicit DeliveryBlobTestParams(ParamsAsTuple params)
      : format(std::get<0>(params)),
        compress(std::get<1>(params)),
        blob_size(std::get<2>(params)),
        type(std::get<3>(params)) {}

  static auto GetTestCombinations() {
    return testing::ConvertGenerator<ParamsAsTuple>(testing::Combine(
        /*format*/ testing::Values(BlobLayoutFormat::kCompactMerkleTreeAtEnd,
                                   BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart),
        /*compress*/ testing::Bool(),
        /*blob_size*/ testing::Values(0, kSmallBlobSize, kLargeBlobSize),
        /*type*/ testing::Values(DeliveryBlobType::kType1, DeliveryBlobType::kType2)));
  }

  static std::string GetTestParamName(const DeliveryBlobTestParams& params) {
    // These tests use rather large parameter names, so we use a more compact format when describing
    // which blob format the test case is using.
    std::string format_name;
    switch (params.format) {
      case blobfs::BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart:
        format_name = "DeprecatedFormat";
        break;
      case blobfs::BlobLayoutFormat::kCompactMerkleTreeAtEnd:
        format_name = "CompactFormat";
        break;
    }
    std::string type_name;
    switch (params.type) {
      case blobfs::DeliveryBlobType::kType1:
        type_name = "Type1";
        break;
      case blobfs::DeliveryBlobType::kType2:
        type_name = "Type2";
        break;
      default:
        type_name = "INVALID";
        break;
    }
    return format_name + type_name + std::string(params.compress ? "Compressed" : "Uncompressed") +
           std::string(params.blob_size > 0 ? std::to_string(params.blob_size) : "NullBlob");
  }
};

class DeliveryBlobTest : public BlobfsTestSetup,
                         public testing::TestWithParam<DeliveryBlobTestParams> {
 public:
  void SetUp() override {
    auto device =
        std::make_unique<block_client::FakeBlockDevice>(kTestDeviceNumBlocks, kTestDeviceBlockSize);

    const FilesystemOptions filesystem_options{
        .blob_layout_format = GetParam().format,
    };
    ASSERT_OK(FormatFilesystem(device.get(), filesystem_options));
    ASSERT_OK(Mount(std::move(device), {}));
    ASSERT_OK(blobfs()->OpenRootNode(&root_));
  }

  void TearDown() override {
    if (root_) {
      ASSERT_OK(root_->Close());
    }
  }

  const fbl::RefPtr<fs::Vnode>& root() const {
    ZX_ASSERT(root_);
    return root_;
  }

 private:
  fbl::RefPtr<fs::Vnode> root_ = nullptr;
};

TEST_P(DeliveryBlobTest, WriteAll) {
  auto blob_data = TestBlobData::CreateRandom(GetParam().blob_size);
  TestDeliveryBlob delivery_blob(blob_data, GetParam().compress, GetParam().type);

  auto blob = CreateBlob(*blobfs(), delivery_blob);
  ASSERT_OK(blob);

  // Validate file contents.
  if (GetParam().blob_size > 0) {
    auto vmo = blob->GetVmoForBlobReader();
    ASSERT_OK(vmo);
    ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()))
        << "Blob contents don't match after writing to disk.";
  }
}

TEST_P(DeliveryBlobTest, WriteChunked) {
  auto blob_data = TestBlobData::CreateRandom(GetParam().blob_size);
  TestDeliveryBlob delivery_blob(blob_data, GetParam().compress, GetParam().type);

  fbl::RefPtr blob = fbl::MakeRefCounted<Blob>(*blobfs(), blob_data.digest());
  ASSERT_OK(blobfs()->GetCache().Add(blob));
  ASSERT_OK(blob->Truncate(delivery_blob.data().size()));

  // Write the delivery blob in chunks. We use a very small chunk size to cover more edge cases.
  constexpr size_t kChunkSize = 4;
  size_t bytes_written = 0;
  while (bytes_written < delivery_blob.data().size()) {
    const size_t to_write = std::min(kChunkSize, delivery_blob.data().size() - bytes_written);
    size_t out_actual;
    ASSERT_OK(blob->Write(delivery_blob.data().data() + bytes_written, to_write, bytes_written,
                          &out_actual))
        << "Failed to write " << to_write << " bytes at offset " << bytes_written;
    ASSERT_EQ(out_actual, to_write);
    bytes_written += out_actual;
  }
  ASSERT_EQ(bytes_written, delivery_blob.data().size());

  // Validate file contents.
  if (GetParam().blob_size > 0) {
    auto vmo = blob->GetVmoForBlobReader();
    ASSERT_OK(vmo);
    ASSERT_TRUE(VerifyContents(*vmo, blob_data.data()))
        << "Blob contents don't match after writing to disk.";
  }
}

// Verify that CalculateDeliveryBlobDigest works with all types of generated delivery blobs.
TEST_P(DeliveryBlobTest, CalculateDeliveryBlobDigest) {
  auto blob_data = TestBlobData::CreateRandom(GetParam().blob_size);
  const fbl::Array<uint8_t> delivery_blob =
      GenerateDeliveryBlobWithType(GetParam().type, blob_data.data(), GetParam().compress).value();
  zx::result<digest::Digest> digest = CalculateDeliveryBlobDigest(delivery_blob);
  ASSERT_OK(digest) << "Data length: " << delivery_blob.size();
  ASSERT_EQ(*digest, blob_data.digest());
}

std::string GetTestParamName(const ::testing::TestParamInfo<DeliveryBlobTestParams>& p) {
  return DeliveryBlobTestParams::GetTestParamName(p.param);
}

INSTANTIATE_TEST_SUITE_P(
    /*no prefix*/, DeliveryBlobTest, DeliveryBlobTestParams::GetTestCombinations(),
    GetTestParamName);

}  // namespace
}  // namespace blobfs

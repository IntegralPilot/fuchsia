// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/compression/external_decompressor.h"

#include <fcntl.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <span>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/compression/chunked.h"
#include "src/storage/blobfs/compression/compressor.h"
#include "src/storage/blobfs/compression/seekable_decompressor.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/fdio_test.h"

namespace blobfs {
namespace {

// These settings currently achieve about 60% compression.
constexpr int kCompressionLevel = 5;
constexpr double kDataRandomnessRatio = 0.25;

constexpr size_t kDataSize{static_cast<size_t>(500) * 1024};  // 500KiB
constexpr size_t kMapSize{kDataSize * static_cast<size_t>(2)};

// Generates a data set of size with sequences of the same bytes and random
// values appearing with frequency kDataRandomnessRatio.
void GenerateData(size_t size, uint8_t* dst) {
  srand(testing::UnitTest::GetInstance()->random_seed());
  for (size_t i = 0; i < size; i++) {
    if ((rand() % 1000) / 1000.0l >= kDataRandomnessRatio) {
      dst[i] = 12;
    } else {
      dst[i] = static_cast<uint8_t>(rand() % 256);
    }
  }
}

void CompressData(std::unique_ptr<Compressor> compressor, void* input_data, size_t* size) {
  ASSERT_OK(compressor->Update(input_data, kDataSize));
  ASSERT_OK(compressor->End());
  *size = compressor->Size();
}

TEST(ExternalDecompressorSetUpTest, DecompressedVmoMissingWrite) {
  zx::vmo compressed_vmo;
  ASSERT_OK(zx::vmo::create(kMapSize, 0, &compressed_vmo));
  zx::vmo decompressed_vmo;
  ASSERT_OK(compressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & (~ZX_RIGHT_WRITE), &decompressed_vmo));

  DecompressorCreatorConnector& connector = DecompressorCreatorConnector::DefaultServiceConnector();
  zx::result<std::unique_ptr<ExternalDecompressorClient>> client_or =
      ExternalDecompressorClient::Create(&connector, decompressed_vmo, compressed_vmo);
  ASSERT_STATUS(client_or, ZX_ERR_INVALID_ARGS);
}

TEST(ExternalDecompressorSetUpTest, CompressedVmoMissingDuplicate) {
  zx::vmo decompressed_vmo;
  ASSERT_OK(zx::vmo::create(kMapSize, 0, &decompressed_vmo));
  zx::vmo compressed_vmo;
  ASSERT_OK(
      decompressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & (~ZX_RIGHT_DUPLICATE), &compressed_vmo));

  DecompressorCreatorConnector& connector = DecompressorCreatorConnector::DefaultServiceConnector();
  zx::result<std::unique_ptr<ExternalDecompressorClient>> client_or =
      ExternalDecompressorClient::Create(&connector, decompressed_vmo, compressed_vmo);
  ASSERT_STATUS(client_or, ZX_ERR_ACCESS_DENIED);
}

class ExternalDecompressorTest : public ::testing::Test {
 public:
  void SetUp() override {
    GenerateData(kDataSize, input_data_);

    zx::vmo compressed_vmo;
    ASSERT_OK(zx::vmo::create(kMapSize, 0, &compressed_vmo));
    zx::vmo remote_compressed_vmo;
    ASSERT_OK(compressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & (~ZX_RIGHT_WRITE),
                                       &remote_compressed_vmo));
    ASSERT_OK(compressed_mapper_.Map(std::move(compressed_vmo), 0, kMapSize,
                                     ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));

    zx::vmo decompressed_vmo;
    ASSERT_OK(zx::vmo::create(kMapSize, 0, &decompressed_vmo));
    zx::vmo remote_decompressed_vmo;
    ASSERT_OK(decompressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS, &remote_decompressed_vmo));
    ASSERT_OK(decompressed_mapper_.Map(std::move(decompressed_vmo), 0, kMapSize,
                                       ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));

    DecompressorCreatorConnector& connector =
        DecompressorCreatorConnector::DefaultServiceConnector();
    zx::result<std::unique_ptr<ExternalDecompressorClient>> client_or =
        ExternalDecompressorClient::Create(&connector, remote_decompressed_vmo,
                                           remote_compressed_vmo);
    ASSERT_OK(client_or.status_value());
    client_ = std::move(client_or.value());
  }

 protected:
  uint8_t input_data_[kDataSize];
  fzl::OwnedVmoMapper compressed_mapper_;
  fzl::OwnedVmoMapper decompressed_mapper_;
  std::unique_ptr<ExternalDecompressorClient> client_;
};

// Get a full range mapping for a SeekableDecompressor.
zx::result<std::vector<CompressionMapping>> GetMappings(SeekableDecompressor* decompressor,
                                                        size_t length) {
  std::vector<CompressionMapping> mappings;
  size_t current = 0;
  while (current < length) {
    zx::result<CompressionMapping> mapping_or =
        decompressor->MappingForDecompressedRange(current, 1, std::numeric_limits<size_t>::max());
    if (!mapping_or.is_ok()) {
      return mapping_or.take_error();
    }
    current += mapping_or.value().decompressed_length;
    mappings.push_back(mapping_or.value());
  }
  return zx::ok(std::move(mappings));
}

// Simple success case for chunked decompression, but done on each chunk just
// to verify success.
TEST_F(ExternalDecompressorTest, ChunkedPartialDecompression) {
  size_t compressed_size;
  std::unique_ptr<ChunkedCompressor> compressor = nullptr;
  ASSERT_OK(ChunkedCompressor::Create({CompressionAlgorithm::kChunked, kCompressionLevel},
                                      kDataSize, &compressed_size, &compressor));
  ASSERT_OK(compressor->SetOutput(compressed_mapper_.start(), kMapSize));
  CompressData(std::move(compressor), input_data_, &compressed_size);

  std::unique_ptr<SeekableDecompressor> local_decompressor;
  ASSERT_OK(SeekableChunkedDecompressor::CreateDecompressor(
      std::span(static_cast<const uint8_t*>(compressed_mapper_.start()), compressed_size),
      compressed_size, &local_decompressor));

  ExternalSeekableDecompressor decompressor(client_.get(), local_decompressor->algorithm());

  auto mappings_or = GetMappings(local_decompressor.get(), kDataSize);
  ASSERT_TRUE(mappings_or.is_ok());
  std::vector<CompressionMapping> mappings = mappings_or.value();
  // Ensure that we're testing multiple chunks and not one large chunk.
  ASSERT_GT(mappings.size(), 1ul);
  for (CompressionMapping mapping : mappings) {
    ASSERT_OK(decompressor.DecompressRange(mapping.compressed_offset, mapping.compressed_length,
                                           mapping.decompressed_length));
    ASSERT_EQ(0, memcmp(static_cast<uint8_t*>(input_data_) + mapping.decompressed_offset,
                        decompressed_mapper_.start(), mapping.decompressed_length));
  }
}

using ExternalDecompressorE2ePagedTest = FdioTest;

TEST_F(ExternalDecompressorE2ePagedTest, VerifyRemoteDecompression) {
  // Create a new blob on the mounted filesystem.
  auto blob = TestBlobData::CreateRealistic(kDataSize);
  auto delivery_blob = TestDeliveryBlob::CreateCompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  uint64_t before_decompressions;
  ASSERT_NO_FATAL_FAILURE(
      GetUintMetric({"paged_read_stats"}, "remote_decompressions", &before_decompressions));

  ASSERT_OK(blob_reader().VerifyBlob(blob));

  uint64_t after_decompressions;
  ASSERT_NO_FATAL_FAILURE(
      GetUintMetric({"paged_read_stats"}, "remote_decompressions", &after_decompressions));
  ASSERT_GT(after_decompressions, before_decompressions);
}

TEST_F(ExternalDecompressorE2ePagedTest, MultiframeDecompression) {
  auto blob = TestBlobData::CreateRealistic(kDataSize);
  auto delivery_blob = TestDeliveryBlob::CreateCompressed(blob);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  uint64_t decompressions;
  ASSERT_NO_FATAL_FAILURE(
      GetUintMetric({"paged_read_stats"}, "remote_decompressions", &decompressions));
  ASSERT_EQ(decompressions, 0ul);

  {
    // Retrieve a read-only COW child of the pager-backed VMO.
    auto parent = blob_reader().GetVmo(blob.digest());
    ASSERT_OK(parent);

    // Can't call ZX_VMO_OP_COMMIT on a readonly vmo. Creating a writeable COW
    // child of the COW child.
    zx::vmo vmo;
    ASSERT_OK(parent->create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, kDataSize, &vmo));
    ASSERT_TRUE(vmo.is_valid());

    ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, kDataSize, nullptr, 0));
  }

  // Decompressed it all in a single decompression instead of many 32K chunks.
  ASSERT_NO_FATAL_FAILURE(
      GetUintMetric({"paged_read_stats"}, "remote_decompressions", &decompressions));
  ASSERT_EQ(decompressions, 1ul);
}

}  // namespace
}  // namespace blobfs

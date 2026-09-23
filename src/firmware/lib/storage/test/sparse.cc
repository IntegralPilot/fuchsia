// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/storage/gpt_utils.h>
#include <lib/storage/sparse.h>
#include <lib/storage/storage.h>
#include <sparse_format.h>

#include <algorithm>
#include <array>
#include <numeric>
#include <optional>
#include <span>
#include <vector>

#include <gtest/gtest.h>

#include "src/firmware/lib/zircon_boot/test/test_data/test_images.h"
#include "test_utils.h"

// In the generated gpt data, the block size is 512
// and the vbmeta_{a,b,r} partitions all have 6 blocks.
// It's easier to test using a nonstandard (but completely valid) chunk size
// to stay under the maximum partition size than to work around this in other ways.
constexpr size_t kBlkSize = 512;

struct SparseDataDescriptor {
  enum class SparseChunkType {
    kUnknown,
    kRaw = CHUNK_TYPE_RAW,
    kFill = CHUNK_TYPE_FILL,
    kDontCare = CHUNK_TYPE_DONT_CARE,
    kCrC32 = CHUNK_TYPE_CRC32,
  };

  SparseChunkType type;
  size_t output_blocks = 0;
  uint32_t payload = 0;

  template <size_t ChunkHeaderSize>
  constexpr size_t total_sz() const {
    static_assert(ChunkHeaderSize >= sizeof(chunk_header_t));
    switch (type) {
      case SparseChunkType::kRaw:
        return output_blocks * kBlkSize + ChunkHeaderSize;
      case SparseChunkType::kFill:
      case SparseChunkType::kCrC32:
        return ChunkHeaderSize + sizeof(payload);
      case SparseChunkType::kDontCare:
      default:
        return ChunkHeaderSize;
    }
  }

  constexpr uint32_t ExpectedImageWord(size_t word_offset, uint32_t base_fill) const {
    switch (type) {
      case SparseChunkType::kRaw:
        return payload + static_cast<uint32_t>(word_offset);
      case SparseChunkType::kFill:
        return payload;
      case SparseChunkType::kDontCare:
        return base_fill;
      default:
        return 0;
    }
  }
};

template <typename T>
void AddBytes(std::vector<uint8_t> &vec, const T &data) {
  const uint8_t *data_bytes = reinterpret_cast<const uint8_t *>(&data);
  vec.insert(vec.end(), data_bytes, data_bytes + sizeof(data));
}

class GptDataHolder {
 public:
  explicit GptDataHolder(FuchsiaFirmwareStorage &storage) : storage_(storage) {}

  ~GptDataHolder() { FuchsiaFirmwareStorageFreeGptData(&storage_, &data_); }
  GptData *gpt_data() { return &data_; }

 private:
  GptData data_;
  FuchsiaFirmwareStorage &storage_;
};

template <size_t FileHeaderSize, size_t ChunkHeaderSize>
std::vector<uint8_t> MakeSparseImage(std::span<const SparseDataDescriptor> descriptors) {
  static_assert(FileHeaderSize >= sizeof(sparse_header_t));
  static_assert(ChunkHeaderSize >= sizeof(chunk_header_t));

  std::vector<uint8_t> data;

  size_t output_blocks = std::reduce(
      descriptors.begin(), descriptors.end(), 0,
      [](size_t sum, const SparseDataDescriptor &d) -> size_t { return sum + d.output_blocks; });

  sparse_header_t header = {
      .magic = SPARSE_HEADER_MAGIC,
      .major_version = 1,
      .file_hdr_sz = FileHeaderSize,
      .chunk_hdr_sz = ChunkHeaderSize,
      .blk_sz = kBlkSize,
      .total_blks = static_cast<uint32_t>(output_blocks),
      .total_chunks = static_cast<uint32_t>(descriptors.size()),
      .image_checksum = 0xDEADBEEF  // We don't do crc validation as of 2023-04-19
  };

  AddBytes(data, header);

  constexpr size_t file_header_pad_bytes = FileHeaderSize - sizeof(sparse_header_t);
  if constexpr (file_header_pad_bytes) {
    std::array<uint8_t, file_header_pad_bytes> file_header_pad;
    AddBytes(data, file_header_pad);
  }

  for (const auto &chunk : descriptors) {
    chunk_header_t hdr = {
        .chunk_type = static_cast<uint16_t>(chunk.type),
        .reserved1 = 0,
        .chunk_sz = static_cast<uint32_t>(chunk.output_blocks),
        .total_sz = static_cast<uint32_t>(chunk.total_sz<ChunkHeaderSize>()),
    };

    AddBytes(data, hdr);

    constexpr size_t chunk_header_pad_bytes = ChunkHeaderSize - sizeof(chunk_header_t);
    if constexpr (chunk_header_pad_bytes) {
      std::array<uint8_t, chunk_header_pad_bytes> chunk_header_pad;
      AddBytes(data, chunk_header_pad);
    }

    // Add payload + i to differentiate raw and fill chunks.
    size_t payload_words = (chunk.total_sz<ChunkHeaderSize>() - ChunkHeaderSize) / sizeof(uint32_t);
    for (size_t i = 0; i < payload_words; i++) {
      AddBytes<uint32_t>(data, chunk.payload + static_cast<uint32_t>(i));
    }
  }

  return data;
}

std::vector<uint8_t> GenerateExpectedData(std::span<const SparseDataDescriptor> descriptors,
                                          uint32_t base_fill) {
  std::vector<uint8_t> data;
  for (const auto &chunk : descriptors) {
    for (size_t i = 0; i < (chunk.output_blocks * kBlkSize) / sizeof(chunk.payload); i++) {
      AddBytes<uint32_t>(data, chunk.ExpectedImageWord(i, base_fill));
    }
  }

  return data;
}

bool FillPartition(FuchsiaFirmwareStorage &ops, const GptData *data, const char *name,
                   uint32_t payload) {
  size_t partition_size;
  if (!FuchsiaFirmwareStorageGetPartitionSize(&ops, data, name, &partition_size)) {
    return false;
  }

  std::vector<uint32_t> fill(partition_size / sizeof(uint32_t), payload);
  return FuchsiaFirmwareStorageGptWrite(&ops, data, name, 0, fill.size() * sizeof(uint32_t),
                                        fill.data());
}

TEST(FuchsiaSparseWriterTest, TestEmptyImage) {
  // The serialized gpt header expects 512 byte blocks. We don't actually care very much,
  // but it's easier to play along.
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), 512);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  FuchsiaFirmwareStorage storage = test_storage.GetFuchsiaFirmwareStorage();

  GptDataHolder gpt_data(storage);
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, gpt_data.gpt_data()));

  // We initialize the partition to a non-zero value to make sure that we aren't
  // writing zeroes.
  const char *partition = "zircon_a";
  ASSERT_TRUE(FillPartition(storage, gpt_data.gpt_data(), partition, 0x55555555));

  std::vector<uint8_t> sparse_image =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({});

  std::vector<uint8_t> before_data(kBlkSize);
  ASSERT_TRUE(FuchsiaFirmwareStorageGptRead(&storage, gpt_data.gpt_data(), partition, 0,
                                            before_data.size(), before_data.data()));

  ASSERT_TRUE(FuchsiaWriteSparseImage(&storage, gpt_data.gpt_data(), partition, sparse_image.data(),
                                      sparse_image.size()));

  std::vector<uint8_t> after_data(before_data.size());
  ASSERT_TRUE(FuchsiaFirmwareStorageGptRead(&storage, gpt_data.gpt_data(), partition, 0,
                                            after_data.size(), after_data.data()));

  ASSERT_EQ(before_data, after_data);
}

template <size_t FileHeaderSize, size_t ChunkHeaderSize>
void RunBasicSparseTest() {
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), 512);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  FuchsiaFirmwareStorage storage = test_storage.GetFuchsiaFirmwareStorage();

  GptDataHolder gpt_data(storage);
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, gpt_data.gpt_data()));

  // Same as above: we particularly want to make sure DONT_CARE isn't writing zeroes.
  const char *partition = "vbmeta_a";
  constexpr uint32_t base_fill = 0x55555555;
  ASSERT_TRUE(FillPartition(storage, gpt_data.gpt_data(), partition, base_fill));

  constexpr SparseDataDescriptor chunks[] = {
      {SparseDataDescriptor::SparseChunkType::kRaw, 1, 0xCAFED00D},
      {SparseDataDescriptor::SparseChunkType::kDontCare, 1},
      {SparseDataDescriptor::SparseChunkType::kFill, 1, 0x8BADF00D},
      {SparseDataDescriptor::SparseChunkType::kCrC32, 0, 0xCAB00D1E},
      {SparseDataDescriptor::SparseChunkType::kFill, 2, 0xCABBA6E5},
      {SparseDataDescriptor::SparseChunkType::kRaw, 2, 0xFEEDC0DE},
      {SparseDataDescriptor::SparseChunkType::kDontCare, 1},
  };
  std::vector<uint8_t> sparse_image = MakeSparseImage<FileHeaderSize, ChunkHeaderSize>(chunks);

  std::vector<uint8_t> expected = GenerateExpectedData(chunks, base_fill);

  ASSERT_TRUE(FuchsiaWriteSparseImage(&storage, gpt_data.gpt_data(), partition, sparse_image.data(),
                                      sparse_image.size()));

  std::vector<uint8_t> actual(expected.size());
  ASSERT_TRUE(FuchsiaFirmwareStorageGptRead(&storage, gpt_data.gpt_data(), partition, 0,
                                            actual.size(), actual.data()));

  if (expected != actual) {
    auto mismatch = std::mismatch(expected.begin(), expected.end(), actual.begin(), actual.end());
    FAIL() << "Mismatch at index " << std::distance(expected.begin(), mismatch.first) << " of "
           << expected.size() << " (wanted '" << *mismatch.first << "', got '" << *mismatch.second
           << "')";
  }
}

TEST(FuchsiaSparseWriterTest, TestBasic) {
  RunBasicSparseTest<sizeof(sparse_header_t), sizeof(chunk_header_t)>();
}
TEST(FuchsiaSparseWriterTest, TestBasicLargeHeaders) {
  RunBasicSparseTest<sizeof(sparse_header_t) * 2, sizeof(chunk_header_t) * 2>();
}

struct FuchsiaSparseWriterBadHeaderTestCase {
  std::string_view name;
  void (*corruption_func)(sparse_header_t &);
};

using FuchsiaSparseWriterBadHeaderTest =
    ::testing::TestWithParam<FuchsiaSparseWriterBadHeaderTestCase>;

TEST_P(FuchsiaSparseWriterBadHeaderTest, TestBadHeader) {
  const FuchsiaSparseWriterBadHeaderTestCase &test_case = GetParam();

  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), 512);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  FuchsiaFirmwareStorage storage = test_storage.GetFuchsiaFirmwareStorage();

  GptDataHolder gpt_data(storage);
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, gpt_data.gpt_data()));

  constexpr SparseDataDescriptor chunks[] = {
      {SparseDataDescriptor::SparseChunkType::kRaw, 1, 0x8BADF00D},
  };

  std::vector<uint8_t> sparse_image =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>(chunks);
  sparse_header_t *header = reinterpret_cast<sparse_header_t *>(sparse_image.data());
  test_case.corruption_func(*header);

  ASSERT_FALSE(FuchsiaWriteSparseImage(&storage, gpt_data.gpt_data(), "vbmeta_a",
                                       sparse_image.data(), sparse_image.size()));
}

INSTANTIATE_TEST_SUITE_P(
    FuchsiaSparseWriterBadHeaderTests, FuchsiaSparseWriterBadHeaderTest,
    testing::ValuesIn<FuchsiaSparseWriterBadHeaderTestCase>({
        {"bad_magic", [](sparse_header_t &h) { h.magic = 0xBEEF; }},
        {"major_too_high", [](sparse_header_t &h) { h.major_version = 16; }},
        {"bad_hdr_size", [](sparse_header_t &h) { h.file_hdr_sz = 0x6; }},
        {"bad_chunk_size", [](sparse_header_t &h) { h.chunk_hdr_sz = 0x7; }},
        {"bad_blk_size", [](sparse_header_t &h) { h.blk_sz = 511; }},
        {"big_blk_size", [](sparse_header_t &h) { h.blk_sz = 8192; }},
    }),
    [](const testing::TestParamInfo<FuchsiaSparseWriterBadHeaderTest::ParamType> &info) {
      return std::string(info.param.name.begin(), info.param.name.end());
    });

struct FuchsiaSparseWriterBadChunkTestCase {
  std::string_view name;
  uint16_t chunk_type;
  size_t payload_size;
};

using FuchsiaSparseWriterBadChunkTest =
    ::testing::TestWithParam<FuchsiaSparseWriterBadChunkTestCase>;

TEST_P(FuchsiaSparseWriterBadChunkTest, TestBadChunk) {
  const FuchsiaSparseWriterBadChunkTestCase &test_case = GetParam();

  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), 512);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  FuchsiaFirmwareStorage storage = test_storage.GetFuchsiaFirmwareStorage();

  GptDataHolder gpt_data(storage);
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, gpt_data.gpt_data()));
  std::vector<uint8_t> sparse_image =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({});
  chunk_header_t chunk = {
      .chunk_type = test_case.chunk_type,
      .reserved1 = 0,
      .chunk_sz = 1,
      .total_sz = static_cast<uint32_t>(sizeof(chunk_header_t) + test_case.payload_size),
  };
  AddBytes(sparse_image, chunk);

  // Add a payload for raw chunk tests
  for (size_t i = 0; i < kBlkSize; i++) {
    sparse_image.push_back(0);
  }
  reinterpret_cast<sparse_header_t *>(sparse_image.data())->total_chunks = 1;

  ASSERT_FALSE(FuchsiaWriteSparseImage(&storage, gpt_data.gpt_data(), "vbmeta_a",
                                       sparse_image.data(), sparse_image.size()));
}

INSTANTIATE_TEST_SUITE_P(
    FuchsiaSparseWriterBadChunkTests, FuchsiaSparseWriterBadChunkTest,
    testing::ValuesIn<FuchsiaSparseWriterBadChunkTestCase>({
        {"chunk_too_big_for_image", CHUNK_TYPE_FILL, 0x40000},
        {"inconsistent_raw_chunk", CHUNK_TYPE_RAW, 15},
        {"inconsistent_dont_care_chunk", CHUNK_TYPE_DONT_CARE, 1},
        {"inconsistent_fill_chunk", CHUNK_TYPE_FILL, 0},
        {"inconsistent_crc32_chunk", CHUNK_TYPE_CRC32, 0},
        {"unexpected_chunk_type", 0xBEEF, 0},
    }),
    [](const testing::TestParamInfo<FuchsiaSparseWriterBadChunkTest::ParamType> &info) {
      return std::string(info.param.name.begin(), info.param.name.end());
    });

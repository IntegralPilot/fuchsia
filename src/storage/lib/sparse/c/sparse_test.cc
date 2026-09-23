// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/sparse/c/sparse.h"

#include <sparse_format.h>

#include <algorithm>
#include <array>
#include <numeric>
#include <optional>
#include <span>
#include <vector>

#include <gtest/gtest.h>

constexpr const size_t kBlkSize = 512;
constexpr const size_t kDiskSize = 128 * 1024;
constexpr const size_t kScratchBufferSize = 4096;

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

struct TestSparseIoBuffer {
  std::vector<uint8_t> data;

  static TestSparseIoBuffer Create(size_t size) {
    std::vector<uint8_t> data(size, 0);
    return TestSparseIoBuffer(std::move(data));
  }

  explicit TestSparseIoBuffer(std::vector<uint8_t> data) : data(std::move(data)) {}

  static size_t Size(SparseIoBufferHandle handle) {
    auto me = static_cast<TestSparseIoBuffer *>(handle);
    return me->data.size();
  }

  static bool Read(SparseIoBufferHandle handle, uint64_t offset, uint8_t *dst, size_t size) {
    auto me = static_cast<TestSparseIoBuffer *>(handle);
    if (offset + size > me->data.size())
      return false;
    std::copy(std::next(me->data.cbegin(), offset), std::next(me->data.cbegin(), offset + size),
              dst);
    return true;
  }

  static bool Write(SparseIoBufferHandle handle, uint64_t offset, const uint8_t *src, size_t size) {
    auto me = static_cast<TestSparseIoBuffer *>(handle);
    if (offset + size > me->data.size())
      return false;
    std::copy(src, src + size, std::next(me->data.begin(), offset));
    return true;
  }

  static bool Fill(SparseIoBufferHandle handle, uint32_t payload) {
    auto me = static_cast<TestSparseIoBuffer *>(handle);
    if (me->data.size() % sizeof(payload) != 0) {
      return false;
    }

    // Can't just call std::transform because the vector's
    // allocated memory may not be 4 byte aligned.
    for (size_t i = 0; i < me->data.size(); i += sizeof(payload)) {
      memcpy(me->data.data() + i, &payload, sizeof(payload));
    }
    return true;
  }

  static SparseIoBufferOps Interface() {
    return SparseIoBufferOps{
        .size = Size,
        .read = Read,
        .fill = Fill,
    };
  }
};

struct TestSparseIo {
 public:
  explicit TestSparseIo(size_t size)
      : buffer_(std::make_unique<uint8_t[]>((size))),
        buffer_size_(size),
        fill_buffer_(TestSparseIoBuffer::Create(kScratchBufferSize)) {}

  void *data() { return buffer_.get(); }
  const void *data() const { return buffer_.get(); }

  void Fill(size_t size, uint32_t payload) {
    std::vector<uint32_t> fill(size / sizeof(uint32_t), payload);
    ASSERT_TRUE(Write(0, reinterpret_cast<uint8_t *>(fill.data()), size));
  }

  SparseIoInterface Interface() {
    return SparseIoInterface{
        .ctx = this,
        .fill_handle = &fill_buffer_,
        .handle_ops = TestSparseIoBuffer::Interface(),
        .write = WriteRaw,
    };
  }

  bool Read(uint64_t dev_offset, uint8_t *dst, size_t size) {
    if (size + dev_offset > buffer_size_)
      return false;
    std::copy(buffer_.get() + dev_offset, buffer_.get() + dev_offset + size, dst);
    return true;
  }

  bool Write(uint64_t dev_offset, uint8_t *src, size_t size) {
    if (size + dev_offset > buffer_size_)
      return false;
    std::copy(src, src + size, buffer_.get() + dev_offset);
    return true;
  }

 private:
  static bool WriteRaw(void *ctx, uint64_t dev_offset, SparseIoBufferHandle src,
                       uint64_t src_offset, size_t size) {
    auto me = static_cast<TestSparseIo *>(ctx);
    auto src_buffer = static_cast<TestSparseIoBuffer *>(src);
    if (src_offset + size > src_buffer->data.size())
      return false;
    return me->Write(dev_offset, &src_buffer->data[src_offset], size);
  }

  std::unique_ptr<uint8_t[]> buffer_;
  size_t buffer_size_;
  TestSparseIoBuffer fill_buffer_;
};

TEST(FuchsiaSparseWriterTest, TestEmptyImage) {
  TestSparseIo test_storage(kDiskSize);

  // Initialize the disk to a non-zero value to make sure that we aren't writing zeroes.
  test_storage.Fill(kDiskSize, 0x55555555);

  TestSparseIoBuffer sparse_image(
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({}));

  std::vector<uint8_t> before_data(kBlkSize);
  ASSERT_TRUE(test_storage.Read(0, before_data.data(), before_data.size()));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_TRUE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));

  std::vector<uint8_t> after_data(before_data.size());
  ASSERT_TRUE(test_storage.Read(0, after_data.data(), after_data.size()));

  ASSERT_EQ(before_data, after_data);
}

template <size_t FileHeaderSize, size_t ChunkHeaderSize>
void RunBasicSparseTest() {
  TestSparseIo test_storage(kDiskSize);

  // Initialize the disk to a non-zero value to make sure that we aren't writing zeroes.
  const uint32_t kFill = 0x55555555;
  test_storage.Fill(kDiskSize, kFill);

  constexpr SparseDataDescriptor chunks[] = {
      {SparseDataDescriptor::SparseChunkType::kRaw, 1, 0xCAFED00D},
      {SparseDataDescriptor::SparseChunkType::kDontCare, 1},
      {SparseDataDescriptor::SparseChunkType::kFill, 1, 0x8BADF00D},
      {SparseDataDescriptor::SparseChunkType::kCrC32, 0, 0xCAB00D1E},
      {SparseDataDescriptor::SparseChunkType::kFill, 2, 0xCABBA6E5},
      {SparseDataDescriptor::SparseChunkType::kRaw, 2, 0xFEEDC0DE},
      {SparseDataDescriptor::SparseChunkType::kDontCare, 1},
  };
  TestSparseIoBuffer sparse_image(MakeSparseImage<FileHeaderSize, ChunkHeaderSize>(chunks));

  std::vector<uint8_t> expected = GenerateExpectedData(chunks, kFill);

  SparseIoInterface io = test_storage.Interface();
  ASSERT_TRUE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));

  std::vector<uint8_t> actual(expected.size());
  ASSERT_TRUE(test_storage.Read(0, actual.data(), actual.size()));

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

  TestSparseIo test_storage(kDiskSize);

  constexpr SparseDataDescriptor chunks[] = {
      {SparseDataDescriptor::SparseChunkType::kRaw, 1, 0x8BADF00D},
  };

  TestSparseIoBuffer sparse_image(
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>(chunks));
  sparse_header_t *header = reinterpret_cast<sparse_header_t *>(sparse_image.data.data());
  test_case.corruption_func(*header);

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
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

  TestSparseIo test_storage(kDiskSize);

  TestSparseIoBuffer sparse_image(
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({}));
  chunk_header_t chunk = {
      .chunk_type = test_case.chunk_type,
      .reserved1 = 0,
      .chunk_sz = 1,
      .total_sz = static_cast<uint32_t>(sizeof(chunk_header_t) + test_case.payload_size),
  };
  AddBytes(sparse_image.data, chunk);

  // Add a payload for raw chunk tests
  for (size_t i = 0; i < kBlkSize; i++) {
    sparse_image.data.push_back(0);
  }
  reinterpret_cast<sparse_header_t *>(sparse_image.data.data())->total_chunks = 1;

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
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

TEST(FuchsiaSparseWriterSecurityTest, TestBadMagicNumber) {
  TestSparseIo test_storage(kDiskSize);
  std::vector<uint8_t> image_data =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({});
  // Corrupt magic
  reinterpret_cast<sparse_header_t *>(image_data.data())->magic = 0xDEADC0DE;
  TestSparseIoBuffer sparse_image(std::move(image_data));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
}

TEST(FuchsiaSparseWriterSecurityTest, TestUnpackedSizeMismatch) {
  TestSparseIo test_storage(kDiskSize);
  // Create image with total_blks = 10 but no chunks (unpacked blocks = 0)
  std::vector<uint8_t> image_data =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>({});
  reinterpret_cast<sparse_header_t *>(image_data.data())->total_blks = 10;
  TestSparseIoBuffer sparse_image(std::move(image_data));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
}

TEST(FuchsiaSparseWriterSecurityTest, TestOutputOffsetOverflowRawChunk) {
  TestSparseIo test_storage(kDiskSize);
  // Set total_blks to a small value, say 2.
  // Add a RAW chunk that attempts to write 5 blocks (exceeding total_blks).
  const SparseDataDescriptor descriptors[] = {{.type = SparseDataDescriptor::SparseChunkType::kRaw,
                                               .output_blocks = 5,
                                               .payload = 0x11111111}};
  std::vector<uint8_t> image_data =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>(descriptors);
  reinterpret_cast<sparse_header_t *>(image_data.data())->total_blks = 2;
  TestSparseIoBuffer sparse_image(std::move(image_data));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
}

TEST(FuchsiaSparseWriterSecurityTest, TestOutputOffsetOverflowFillChunk) {
  TestSparseIo test_storage(kDiskSize);
  // Set total_blks to a small value, say 2.
  // Add a FILL chunk that attempts to write 5 blocks (exceeding total_blks).
  const SparseDataDescriptor descriptors[] = {{.type = SparseDataDescriptor::SparseChunkType::kFill,
                                               .output_blocks = 5,
                                               .payload = 0x11111111}};
  std::vector<uint8_t> image_data =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>(descriptors);
  reinterpret_cast<sparse_header_t *>(image_data.data())->total_blks = 2;
  TestSparseIoBuffer sparse_image(std::move(image_data));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
}

TEST(FuchsiaSparseWriterSecurityTest, TestOutputOffsetOverflowDontCareChunk) {
  TestSparseIo test_storage(kDiskSize);
  // Set total_blks to a small value, say 2.
  // Add a DONT_CARE chunk that attempts to skip 5 blocks (exceeding total_blks).
  const SparseDataDescriptor descriptors[] = {
      {.type = SparseDataDescriptor::SparseChunkType::kDontCare, .output_blocks = 5}};
  std::vector<uint8_t> image_data =
      MakeSparseImage<sizeof(sparse_header_t), sizeof(chunk_header_t)>(descriptors);
  reinterpret_cast<sparse_header_t *>(image_data.data())->total_blks = 2;
  TestSparseIoBuffer sparse_image(std::move(image_data));

  SparseIoInterface io = test_storage.Interface();
  ASSERT_FALSE(sparse_unpack_image(&io, sparse_nop_logger, &sparse_image));
}

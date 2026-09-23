// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_data_producer.h"

#include <zircon/assert.h>

#include <cstdint>
#include <cstring>
#include <span>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/storage/blobfs/format.h"

namespace blobfs {
namespace {

TEST(SimpleBlobDataProducerTest, EmptyProducer) {
  SimpleBlobDataProducer producer({});
  ASSERT_THAT(producer.Consume(0), testing::IsEmpty());
}

TEST(SimpleBlobDataProducerTest, ConsumeZero) {
  std::vector<uint8_t> data(kBlobfsBlockSize, 0x01);
  SimpleBlobDataProducer producer(data);
  ASSERT_THAT(producer.Consume(0), testing::IsEmpty());
}

TEST(SimpleBlobDataProducerTest, ConsumeExactlyAvailable) {
  std::vector<uint8_t> data(kBlobfsBlockSize, 0x01);
  SimpleBlobDataProducer producer(data);
  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::ElementsAreArray(data));
}

TEST(SimpleBlobDataProducerTest, ConsumeMoreThanAvailable) {
  std::vector<uint8_t> data(kBlobfsBlockSize, 0x01);
  SimpleBlobDataProducer producer(data);
  ASSERT_THAT(producer.Consume(kBlobfsBlockSize * 2), testing::ElementsAreArray(data));
  ASSERT_THAT(producer.Consume(kBlobfsBlockSize * 2), testing::IsEmpty());
}

constexpr uint8_t kPaddingValue = 0x00;
constexpr uint8_t kDataValue = 0x01;
constexpr uint8_t kMerkleValue = 0x02;

struct TestMergedData {
  explicit TestMergedData(size_t data_size, size_t padding, size_t merkle_size)
      : data_buffer(data_size + kBlobfsBlockSize, kDataValue),
        merkle_buffer(merkle_size + kBlobfsBlockSize, kMerkleValue),
        producer(MergeBlobDataProducer({data_buffer.data(), data_size}, padding,
                                       {merkle_buffer.data() + kBlobfsBlockSize, merkle_size})) {
    ZX_ASSERT((data_size + padding + merkle_size) % kBlobfsBlockSize == 0);
    memset(data_buffer.data() + data_size, 0x03, kBlobfsBlockSize);
    memset(merkle_buffer.data(), 0x04, kBlobfsBlockSize);
  }

  std::span<const uint8_t> Consume(uint64_t max) { return producer.Consume(max); }

  std::vector<uint8_t> data_buffer;
  std::vector<uint8_t> merkle_buffer;
  MergeBlobDataProducer producer;
};

TEST(MergeBlobDataProducerTest, EmptyProducer) {
  TestMergedData producer(0, 0, 0);

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

TEST(MergeBlobDataProducerTest, OnlyDataBlock) {
  TestMergedData producer(kBlobfsBlockSize, 0, 0);

  auto result = producer.Consume(kBlobfsBlockSize);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_THAT(result, testing::Each(kDataValue));

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

TEST(MergeBlobDataProducerTest, OnlyMerkleBlock) {
  TestMergedData producer(0, 0, kBlobfsBlockSize);

  auto result = producer.Consume(kBlobfsBlockSize);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_THAT(result, testing::Each(kMerkleValue));

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

TEST(MergeBlobDataProducerTest, SingleSharedBlock) {
  TestMergedData producer(1, kBlobfsBlockSize - 2, 1);

  auto result = producer.Consume(kBlobfsBlockSize);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_EQ(result[0], kDataValue);
  ASSERT_THAT(result.subspan(1, kBlobfsBlockSize - 2), testing::Each(kPaddingValue));
  ASSERT_EQ(result[kBlobfsBlockSize - 1], kMerkleValue);

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

TEST(MergeBlobDataProducerTest, IndependentBlocks) {
  TestMergedData producer(4097, kBlobfsBlockSize - 2, 4097);

  auto result = producer.Consume(kBlobfsBlockSize * 2);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_THAT(result.subspan(0, 4097), testing::Each(kDataValue));
  ASSERT_THAT(result.subspan(4097), testing::Each(kPaddingValue));

  result = producer.Consume(kBlobfsBlockSize * 2);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_THAT(result.subspan(0, 4095), testing::Each(kPaddingValue));
  ASSERT_THAT(result.subspan(4095), testing::Each(kMerkleValue));

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

TEST(MergeBlobDataProducerTest, SharedBlockWithMultipleMerkleBlocks) {
  TestMergedData producer(1, kBlobfsBlockSize - 2, 8193);

  auto result = producer.Consume(kBlobfsBlockSize * 2);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_EQ(result[0], kDataValue);
  ASSERT_THAT(result.subspan(1, kBlobfsBlockSize - 2), testing::Each(kPaddingValue));
  ASSERT_EQ(result[kBlobfsBlockSize - 1], kMerkleValue);

  result = producer.Consume(kBlobfsBlockSize * 2);
  ASSERT_THAT(result, testing::SizeIs(kBlobfsBlockSize));
  ASSERT_THAT(result, testing::Each(kMerkleValue));

  ASSERT_THAT(producer.Consume(kBlobfsBlockSize), testing::IsEmpty());
}

}  // namespace
}  // namespace blobfs

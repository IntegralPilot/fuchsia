// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_data_producer.h"

#include <zircon/assert.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <span>
#include <tuple>

#include "src/storage/blobfs/format.h"

namespace blobfs {
namespace {
template <typename T>
std::tuple<std::span<T>, std::span<T>> SplitSpan(std::span<T> span, size_t split_point) {
  auto start = span.subspan(0, std::min(split_point, span.size()));
  auto end = span.subspan(start.size());
  return std::make_tuple(start, end);
}
}  // namespace

SimpleBlobDataProducer::SimpleBlobDataProducer(std::span<const uint8_t> data) : data_(data) {
  ZX_DEBUG_ASSERT_MSG(data_.size() % kBlobfsBlockSize == 0,
                      "The span is not a multiple of the block size: %lu", data_.size());
}

std::span<const uint8_t> SimpleBlobDataProducer::Consume(uint64_t max) {
  auto [result, remaining] = SplitSpan(data_, max);
  data_ = remaining;
  return result;
}

MergeBlobDataProducer::MergeBlobDataProducer(std::span<uint8_t> first, size_t padding,
                                             std::span<uint8_t> second)
    : first_(first), second_(second), padding_(padding) {
  ZX_DEBUG_ASSERT_MSG((first_.size() + second_.size() + padding_) % kBlobfsBlockSize == 0,
                      "The arguments do not add up to a multiple of "
                      "the block size: first:%lu, second:%lu, padding:%lu",
                      first_.size(), second_.size(), padding_);
  ZX_ASSERT_MSG(padding_ < kBlobfsBlockSize, "Padding size:%lu more than blobfs block size: %lu",
                padding_, kBlobfsBlockSize);
}

std::span<const uint8_t> MergeBlobDataProducer::Consume(uint64_t max) {
  if (!first_.empty()) {
    auto [data, first_remaining] = SplitSpan(first_, max);
    first_ = first_remaining;

    // |max| must be a multiple of the block size. If the split above returned a span that wasn't a
    // multiple of the block size then all of the first span must have been consumed. The buffer
    // backing the first span must have room after the first span to fill out the rest of the block.
    // Start consuming the padding and the second span to fill out the block.
    const size_t partial_block = data.size() % kBlobfsBlockSize;
    if (partial_block > 0) {
      ZX_DEBUG_ASSERT(first_.empty());
      // First, add any padding that might be required.
      const size_t to_pad = std::min(padding_, kBlobfsBlockSize - partial_block);
      memset(data.data() + data.size(), 0, to_pad);
      data = std::span(data.data(), data.size() + to_pad);
      padding_ -= to_pad;

      // If we still don't have a full block, fill the block with data from the second span.
      const size_t partial_block = data.size() % kBlobfsBlockSize;
      if (partial_block > 0) {
        auto [data2, second_remaining] = SplitSpan(second_, kBlobfsBlockSize - partial_block);
        second_ = second_remaining;
        memcpy(data.data() + data.size(), data2.data(), data2.size());
        data = std::span(data.data(), data.size() + data2.size());
      }
    }
    return data;
  }

  auto [data, second_remaining] = SplitSpan(second_, max - padding_);
  second_ = second_remaining;
  // If we have some padding, prepend zeros.
  if (padding_ > 0) {
    data = std::span(data.data() - padding_, data.size() + padding_);
    memset(data.data(), 0, padding_);
    padding_ = 0;
  }
  return data;
}

}  // namespace blobfs

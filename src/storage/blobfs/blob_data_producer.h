// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_BLOB_DATA_PRODUCER_H_
#define SRC_STORAGE_BLOBFS_BLOB_DATA_PRODUCER_H_

#include <cstddef>
#include <cstdint>
#include <span>

namespace blobfs {

// BlobDataProducer is an abstract class that is used when writing blobs. It produces data (see the
// Consume method) which is then to be written to the device.
class BlobDataProducer {
 public:
  // Consumes up to |max| bytes from the producer. |max| must be a multiple of |kBlobfsBlockSize|.
  // Producers must always produce a multiple of |kBlobfsBlockSize|. An empty span is returned when
  // the producer has run out of data.
  virtual std::span<const uint8_t> Consume(uint64_t max) = 0;
};

// A simple producer that vends data from a supplied span. The span's size must be a multiple of
// |kBlobfsBlockSize|.
class SimpleBlobDataProducer : public BlobDataProducer {
 public:
  explicit SimpleBlobDataProducer(std::span<const uint8_t> data);

  // BlobDataProducer implementation:
  std::span<const uint8_t> Consume(uint64_t max) final;

 private:
  std::span<const uint8_t> data_;
};

// Merges two spans together with optional padding between them.
//
//  - The length of the spans plus the padding must add up to a multiple of |kBlobfsBlockSize|.
//  - The padding must not be greater than or equal to |kBlobfsBlockSize|.
//  - The first span must point to a buffer that can accommodate up to |kBlobfsBlockSize| - 1 bytes
//    written after the span.
//  - The second span must point to a buffer than can accommodate up to |kBlobfsBlockSize| - 1 bytes
//    written before the span.
class MergeBlobDataProducer : public BlobDataProducer {
 public:
  MergeBlobDataProducer(std::span<uint8_t> first, size_t padding, std::span<uint8_t> second);

  // BlobDataProducer implementation:
  std::span<const uint8_t> Consume(uint64_t max) final;

 private:
  std::span<uint8_t> first_;
  std::span<uint8_t> second_;
  size_t padding_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_BLOB_DATA_PRODUCER_H_

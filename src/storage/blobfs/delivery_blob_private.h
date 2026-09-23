// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file includes implementation details specific to offline compression that should not
// be relied on (i.e. everything defined outside of RFC 0207).

#ifndef SRC_STORAGE_BLOBFS_DELIVERY_BLOB_PRIVATE_H_
#define SRC_STORAGE_BLOBFS_DELIVERY_BLOB_PRIVATE_H_

#include <lib/zx/result.h>

#include <cstddef>
#include <cstdint>
#include <span>

#include "src/storage/blobfs/delivery_blob.h"

namespace blobfs {

/// Metadata format corresponding to a delivery blob of type `DeliveryBlobType::kType1` and
/// `DeliveryBlobType::kType2`.
struct MetadataType1 {
  /// Flag indicating if the payload is compressed with the zstd-chunked format. If not set, the
  /// payload is assumed to be an uncompressed blob.
  static constexpr uint32_t kIsCompressed = 1ul << 0;
  /// Mask of all valid flags.
  static constexpr uint32_t kValidFlagsMask = kIsCompressed;

  /// The size of the header including metadata and checksum.
  static const size_t kHeaderSize;

  // Fields:

  /// Expected length of payload, in bytes. May be compressed or uncompressed depending on `flags`.
  uint64_t payload_length;
  /// Checksum (CRC32) covering both header + metadata. Should be set to zero when calculating.
  uint32_t checksum;
  /// Flags indicating format and how to decode payload.
  uint32_t flags;

  /// Check if the metadata is valid. Requires `header` for checksum validation.
  bool IsValid(const DeliveryBlobHeader& header) const;

  bool IsCompressed() const { return flags & kIsCompressed; }

  /// Calculate the checksum over the header and this metadata.
  uint32_t Checksum(const DeliveryBlobHeader& header) const;

  /// Create new `MetadataType1` with the specified `payload_length`. If `is_compressed` is
  /// true, the payload is assumed to be in the zstd-chunked format. Otherwise, the payload
  /// following the metadata ia assumed to be uncompressed.
  static MetadataType1 Create(const DeliveryBlobHeader& header, size_t payload_length,
                              bool is_compressed);

  /// Parse and return a `MetadataType1` from a byte `buffer` and a parsed `header`.
  static zx::result<MetadataType1> FromBuffer(std::span<const uint8_t> buffer,
                                              const blobfs::DeliveryBlobHeader& header);
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_DELIVERY_BLOB_PRIVATE_H_

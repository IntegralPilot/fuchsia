// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file describes the format of delivery blobs as specified in RFC 0207.
// It includes high-level types for interfacing with a delivery blob.

#ifndef SRC_STORAGE_BLOBFS_DELIVERY_BLOB_H_
#define SRC_STORAGE_BLOBFS_DELIVERY_BLOB_H_

#include <lib/zx/result.h>

#include <cstdint>
#include <cstdlib>
#include <optional>
#include <span>

#include <fbl/array.h>

#include "src/lib/chunked-compression/compression-params.h"
#include "src/lib/digest/digest.h"

namespace blobfs {

// Type of the delivery blob's metadata/payload. Corresponds to the `--type` argument used to
// generate the blob with the `blobfs-compression` tool.
//
// *WARNING*: The underlying values of these fields form are used in external tools when generating
// delivery blobs as per RFC 0207. Use caution when changing or re-using the values specified here.
enum class DeliveryBlobType : uint32_t {
  // Reserved for future use.
  kReserved = 0,
  // Type-1 blobs support the zstd-chunked compression format or are uncompressed.
  kType1 = 1,
  // Type-2 blobs are the same as Type-1 except that they specifically use zstd compression level 21
  // and chunk sizes of 128KiB.
  kType2 = 2,
};

// The default delivery blob type used by Blobfs, this will be used internally by Blobfs when
// compressing blobs for image generation.
constexpr DeliveryBlobType kDefaultBlobfsDeliveryBlobType = DeliveryBlobType::kType1;

// Returns the chunked compression params used for the given blob type and blob size. If the type
// does not use ChunkedCompression, this will return an error.
zx::result<chunked_compression::CompressionParams> GetChunkedCompressionParamsForType(
    DeliveryBlobType type, size_t input_size);

// Header of a delivery blob as specified in RFC 0207.
struct DeliveryBlobHeader {
  // Header Fields:

  // 32-bit magic number.
  uint8_t magic[4];
  // Type of blob the metadata/payload represent.
  DeliveryBlobType type;
  // Total header length, including metadata associated with `type`. For a given delivery blob,
  // the metadata starts after `sizeof(DeliveryBlobHeader)` bytes, and the payload section starts
  // after `header_length` bytes.
  uint32_t header_length;

  // Methods:

  // Check if the header is valid (i.e. `magic` is correct, `type` is a valid value).
  bool IsValid() const;

  // Create a new `DeliveryBlobHeader` with the specified `metadata_length`.
  static DeliveryBlobHeader Create(DeliveryBlobType type, size_t metadata_length);

  // Parse and return a `DeliveryBlobHeader` from a byte `buffer`.
  static zx::result<DeliveryBlobHeader> FromBuffer(std::span<const uint8_t> buffer);
};

// Generate a specific type of delivery blob payload from the given blob `data`.
//
// If `compress` is not specified, the result will be uncompressed if the compressed data is larger
// than the input. If `compress` is true, the result will always be compressed, and if false, will
// always be uncompressed.
//
// *WARNING*: Modifying the compression parameters used by this function can cause a mismatch
// between the calculated on-disk size used for size checking. This function is used to calculate
// `compressed_file_size` in the blob info JSON file.
zx::result<fbl::Array<uint8_t>> GenerateDeliveryBlobWithType(DeliveryBlobType type,
                                                             std::span<const uint8_t> data,
                                                             std::optional<bool> compress);

// Calculate the Merkle root of an RFC 0207 compliant delivery blob. `data` must be a complete
// delivery blob and cannot include any trailing data.
//
// *WARNING*: Aside from checksum verification and basic validity checks provided by the
// chunked_compression library, this function makes no security guarantees. Decompression is
// performed in the thread/address space of the caller.
zx::result<digest::Digest> CalculateDeliveryBlobDigest(std::span<const uint8_t> data);

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_DELIVERY_BLOB_H_

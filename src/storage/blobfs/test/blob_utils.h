// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_TEST_BLOB_UTILS_H_
#define SRC_STORAGE_BLOBFS_TEST_BLOB_UTILS_H_

#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/object_traits.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <zircon/assert.h>

#include <concepts>
#include <cstdint>
#include <cstring>
#include <optional>
#include <span>
#include <string>
#include <utility>

#include <fbl/array.h>
#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/delivery_blob.h"
#include "src/storage/blobfs/format.h"

namespace blobfs {

template <typename T, typename U>
int StreamAll(T func, int fd, U* buf, size_t max) {
  size_t n = 0;
  while (n != max) {
    ssize_t d = func(fd, &buf[n], max - n);
    if (d < 0) {
      return -1;
    }
    n += d;
  }
  return 0;
}

// Verifies that a vmo contains the data in the provided buffer.
bool VerifyContents(const zx::vmo& blob_vmo, std::span<const uint8_t> expected_data);

// The the stream size of a VMO.
uint64_t GetVmoSize(const zx::vmo& vmo);

// The the stream size of a VMO.
uint64_t GetVmoStreamSize(const zx::vmo& vmo);

// Returns the name of |format| for use in parameterized tests.
std::string GetBlobLayoutFormatNameForTests(BlobLayoutFormat format);

constexpr uint64_t kRingBufferSize = 256ul * 1024;

// Class for creating blob data and generating its merkle root.
class TestBlobData {
 public:
  explicit TestBlobData(fbl::Array<uint8_t> data);

  // Generates a blob containing `fill` repeatedly.
  static TestBlobData Create(size_t size, uint8_t fill = 0xAB);

  // Generates a blob starting with the bytes from |prefix|. The remainder of the blob is filled
  // with 0xAB. |size| must be at least as large as sizeof(prefix).
  template <std::integral T>
  static TestBlobData CreatePrefixed(size_t size, T prefix) {
    ZX_ASSERT(size >= sizeof(T));
    auto data = fbl::MakeArray<uint8_t>(size);
    memcpy(data.data(), &prefix, sizeof(T));
    memset(data.data() + sizeof(T), 0xAB, size - sizeof(T));
    return TestBlobData(std::move(data));
  }

  // Generates a blob with realistic data (derived from, for example, an ELF binary). `prefix` is
  // placed at the start of the blob which allows for the same realistic data to be used to create
  // multiple distinct blobs.
  static TestBlobData CreateRealistic(size_t size, int prefix = 0);

  // Generates a blob with bytes generated from `rand`.
  static TestBlobData CreateRandom(size_t size);

  std::span<const uint8_t> data() const { return {data_.data(), data_.size()}; }
  const Digest& digest() const { return digest_; }

  TestBlobData(TestBlobData&&) = default;
  TestBlobData& operator=(TestBlobData&&) = default;

 private:
  fbl::Array<uint8_t> data_;
  Digest digest_;
};

// Class for creating a merkle tree and its root in either compact or padded format.
class TestMerkleTree {
 public:
  explicit TestMerkleTree(std::span<const uint8_t> data, bool use_compact_format);

  static TestMerkleTree CreatePadded(const TestBlobData& blob);
  static TestMerkleTree CreateCompact(const TestBlobData& blob);

  std::span<const uint8_t> merkle_tree() const { return merkle_tree_; }
  const Digest& digest() const { return digest_; }

  TestMerkleTree(TestMerkleTree&&) = default;
  TestMerkleTree& operator=(TestMerkleTree&&) = default;

 private:
  fbl::Array<uint8_t> merkle_tree_;
  Digest digest_;
};

// Class for constructing a delivery blob data from `TestBlobData`.
class TestDeliveryBlob {
 public:
  explicit TestDeliveryBlob(const TestBlobData& blob_info,
                            std::optional<bool> compress = std::nullopt,
                            DeliveryBlobType type = kDefaultBlobfsDeliveryBlobType);

  // Creates a compressed delivery blob from `blob_data`.
  static TestDeliveryBlob CreateCompressed(const TestBlobData& blob_data);

  // Wrapper for TestDeliveryBlob::CreateCompressed(TestBlobData::Create(size, fill)).
  static TestDeliveryBlob CreateCompressed(size_t size, uint8_t fill = 0xAB);

  // Create an uncompressed delivery blob from `blob_data`.
  static TestDeliveryBlob CreateUncompressed(const TestBlobData& blob_data);

  // Wrapper for TestDeliveryBlob::CreateUncompressed(TestBlobData::Create(size, fill)).
  static TestDeliveryBlob CreateUncompressed(size_t size, uint8_t fill = 0xAB);

  // Creates a delivery blob for `blob_data` using the compression specified by
  // `compression_algorithm`.
  static TestDeliveryBlob CreateWithCompressionAlgorithm(
      const TestBlobData& blob_data, CompressionAlgorithm compression_algorithm);

  std::span<const uint8_t> data() const { return data_; }
  const Digest& digest() const { return digest_; }

  TestDeliveryBlob(TestDeliveryBlob&&) = default;
  TestDeliveryBlob& operator=(TestDeliveryBlob&&) = default;

 private:
  fbl::Array<uint8_t> data_;
  Digest digest_;
};

class BlobReaderWrapper {
 public:
  explicit BlobReaderWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobReader> reader);
  static BlobReaderWrapper Connect(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir);

  zx::result<zx::vmo> GetVmo(const Digest& digest) const;

  zx::result<> VerifyBlob(const TestBlobData&) const;

 private:
  fidl::WireSyncClient<fuchsia_fxfs::BlobReader> reader_;
};

class IncrementalWriter;
class BlobWriterWrapper {
 public:
  explicit BlobWriterWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobWriter> writer);

  zx::result<> BytesReady(uint64_t bytes_written);

  zx::result<zx::vmo> GetVmo(uint64_t size);

  zx::result<IncrementalWriter> CreateIncrementalWriter(const TestDeliveryBlob& blob);
  zx::result<> WriteBlob(const TestDeliveryBlob& blob);

 private:
  fidl::WireSyncClient<fuchsia_fxfs::BlobWriter> writer_;
};

// A convenience class that manages sending data to a `BlobWriter`.
class IncrementalWriter {
 public:
  // `writer` and `data` must outlive this class.
  IncrementalWriter(BlobWriterWrapper& writer, std::span<const uint8_t> data, zx::vmo vmo);

  // Writes |amount| of bytes from the stored data into the vmo and calls BytesReady. |amount| must
  // not be zero and must not exceed the number of remaining bytes.
  zx::result<> Write(uint64_t amount);

  // Writes all remaining stored bytes. There must be some remaining bytes.
  zx::result<> Complete();

 private:
  BlobWriterWrapper& writer_;
  std::span<const uint8_t> data_;
  zx::vmo vmo_;
  uint64_t vmo_offset_ = 0;
};

class BlobCreatorWrapper {
 public:
  explicit BlobCreatorWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobCreator> creator);
  static BlobCreatorWrapper Connect(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir);

  zx::result<BlobWriterWrapper> Create(const Digest& digest) const;

  zx::result<BlobWriterWrapper> CreateExisting(const Digest& digest) const;

  zx::result<bool> NeedsOverwrite(const Digest& digest) const;

  zx::result<> CreateAndWriteBlob(const TestDeliveryBlob& blob) const;

 private:
  zx::result<BlobWriterWrapper> Create(const Digest& digest, bool allow_existing) const;

  fidl::WireSyncClient<fuchsia_fxfs::BlobCreator> creator_;
};

// Creates a new blob in `blobfs` with the contents of `delivery_blob`. The returned blob will be in
// the same state as if it had just been written by `BlobWriter`.
zx::result<fbl::RefPtr<Blob>> CreateBlob(Blobfs& blobfs, const TestDeliveryBlob& delivery_blob);

// Retrieve a blob in `blobfs` with the digest `digest`.
zx::result<fbl::RefPtr<Blob>> GetBlob(Blobfs& blobfs, const Digest& digest);

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_TEST_BLOB_UTILS_H_

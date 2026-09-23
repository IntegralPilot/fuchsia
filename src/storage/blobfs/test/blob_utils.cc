// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/test/blob_utils.h"

#include <fcntl.h>
#include <fidl/fuchsia.fxfs/cpp/common_types.h>
#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.fxfs/cpp/wire_messaging.h>
#include <fidl/fuchsia.fxfs/cpp/wire_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/array.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <optional>
#include <span>
#include <string>
#include <utility>
#include <vector>

#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <zstd/zstd.h>

#include "src/lib/digest/digest.h"
#include "src/lib/digest/merkle-tree.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/delivery_blob.h"
#include "src/storage/blobfs/format.h"

namespace blobfs {
namespace {

std::vector<uint8_t> LoadTemplateData() {
  constexpr char kDataFile[] = "/pkg/data/test_binary.zstd";
  fbl::unique_fd fd(open(kDataFile, O_RDONLY));
  EXPECT_TRUE(fd.is_valid());
  if (!fd) {
    fprintf(stderr, "blob_utils.cc: Failed to load template data file %s: %s\n", kDataFile,
            strerror(errno));
    return {};
  }
  struct stat s;
  EXPECT_EQ(fstat(fd.get(), &s), 0);
  size_t sz = s.st_size;

  std::vector<uint8_t> compressed(sz);
  EXPECT_EQ(StreamAll(read, fd.get(), compressed.data(), sz), 0);

  constexpr size_t kUncompressedReserveSize{static_cast<size_t>(128) * 1024};
  std::vector<uint8_t> uncompressed(kUncompressedReserveSize);
  uncompressed.resize(ZSTD_decompress(uncompressed.data(), uncompressed.size(), compressed.data(),
                                      compressed.size()));
  return uncompressed;
}

}  // namespace

bool VerifyContents(const zx::vmo& blob_vmo, std::span<const uint8_t> expected_data) {
  uint64_t blob_size = GetVmoStreamSize(blob_vmo);
  if (blob_size != expected_data.size()) {
    return false;
  }
  if (blob_size == 0) {
    return true;
  }
  fzl::VmoMapper mapper;
  ZX_ASSERT(mapper.Map(blob_vmo, 0, blob_size, ZX_VM_PERM_READ) == ZX_OK);
  return memcmp(mapper.start(), expected_data.data(), expected_data.size()) == 0;
}

uint64_t GetVmoSize(const zx::vmo& vmo) {
  uint64_t size;
  ZX_ASSERT(vmo.get_size(&size) == ZX_OK);
  return size;
}

uint64_t GetVmoStreamSize(const zx::vmo& vmo) {
  uint64_t stream_size;
  ZX_ASSERT(vmo.get_stream_size(&stream_size) == ZX_OK);
  return stream_size;
}

std::string GetBlobLayoutFormatNameForTests(BlobLayoutFormat format) {
  switch (format) {
    case BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart:
      return "PaddedMerkleTreeAtStartLayout";
    case BlobLayoutFormat::kCompactMerkleTreeAtEnd:
      return "CompactMerkleTreeAtEndLayout";
  }
}

TestMerkleTree::TestMerkleTree(std::span<const uint8_t> data, bool use_compact_format) {
  digest::MerkleTreeCreator mtc;
  mtc.SetUseCompactFormat(use_compact_format);
  zx_status_t status = mtc.SetDataLength(data.size());
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to set data length: %s", zx_status_get_string(status));
  size_t merkle_tree_size = mtc.GetTreeLength();
  if (merkle_tree_size) {
    merkle_tree_ = fbl::MakeArray<uint8_t>(merkle_tree_size);
  }
  uint8_t merkle_tree_root[digest::kSha256Length];
  status =
      mtc.SetTree(merkle_tree_.data(), merkle_tree_size, merkle_tree_root, digest::kSha256Length);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to set Merkle tree: %s", zx_status_get_string(status));
  status = mtc.Append(data.data(), data.size());
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to add data to Merkle tree: %s",
                zx_status_get_string(status));
  digest_ = merkle_tree_root;
}

TestMerkleTree TestMerkleTree::CreatePadded(const TestBlobData& blob) {
  TestMerkleTree merkle_tree(blob.data(), /*use_compact_format=*/false);
  ZX_ASSERT(merkle_tree.digest() == blob.digest());
  return merkle_tree;
}
TestMerkleTree TestMerkleTree::CreateCompact(const TestBlobData& blob) {
  TestMerkleTree merkle_tree(blob.data(), /*use_compact_format=*/true);
  ZX_ASSERT(merkle_tree.digest() == blob.digest());
  return merkle_tree;
}

TestBlobData::TestBlobData(fbl::Array<uint8_t> data)
    : data_(std::move(data)), digest_(TestMerkleTree(data_, true).digest()) {}

TestBlobData TestBlobData::Create(size_t size, uint8_t fill) {
  if (size == 0) {
    return TestBlobData(fbl::Array<uint8_t>());
  }
  auto data = fbl::MakeArray<uint8_t>(size);
  memset(data.data(), fill, size);
  return TestBlobData(std::move(data));
}

TestBlobData TestBlobData::CreateRealistic(size_t size, int prefix) {
  static auto* template_data = [] {
    auto* data = new std::vector(LoadTemplateData());
    ZX_ASSERT_MSG(data->size() > 0ul, "Failed to load realistic data");
    return data;
  }();
  if (size == 0) {
    return TestBlobData(fbl::Array<uint8_t>());
  }

  auto data = fbl::MakeArray<uint8_t>(size);
  size_t prefix_size = std::min(sizeof(prefix), data.size());
  memcpy(data.data(), &prefix, prefix_size);
  size_t offset = prefix_size;

  while (offset < data.size()) {
    size_t to_copy = std::min(template_data->size(), data.size() - offset);
    memcpy(&data[offset], template_data->data(), to_copy);
    offset += to_copy;
  }
  return TestBlobData(std::move(data));
}

TestBlobData TestBlobData::CreateRandom(size_t size) {
  if (size == 0) {
    return TestBlobData(fbl::Array<uint8_t>());
  }
  auto data = fbl::MakeArray<uint8_t>(size);
  for (uint8_t& e : data) {
    e = static_cast<uint8_t>(rand());
  }
  return TestBlobData(std::move(data));
}

TestDeliveryBlob::TestDeliveryBlob(const TestBlobData& blob_info, std::optional<bool> compress,
                                   DeliveryBlobType type)
    : digest_(blob_info.digest()) {
  auto delivery_blob = GenerateDeliveryBlobWithType(type, blob_info.data(), compress);
  ZX_ASSERT(delivery_blob.is_ok());
  data_ = *std::move(delivery_blob);
}

TestDeliveryBlob TestDeliveryBlob::CreateCompressed(const TestBlobData& blob_data) {
  return TestDeliveryBlob(blob_data, /*compress=*/true);
}

TestDeliveryBlob TestDeliveryBlob::CreateCompressed(size_t size, uint8_t fill) {
  return TestDeliveryBlob(TestBlobData::Create(size, fill), /*compress=*/true);
}

TestDeliveryBlob TestDeliveryBlob::CreateUncompressed(const TestBlobData& blob_data) {
  return TestDeliveryBlob(blob_data, /*compress=*/false);
}

TestDeliveryBlob TestDeliveryBlob::CreateUncompressed(size_t size, uint8_t fill) {
  return TestDeliveryBlob(TestBlobData::Create(size, fill), /*compress=*/false);
}

TestDeliveryBlob TestDeliveryBlob::CreateWithCompressionAlgorithm(
    const TestBlobData& blob_data, CompressionAlgorithm compression_algorithm) {
  switch (compression_algorithm) {
    case CompressionAlgorithm::kUncompressed:
      return TestDeliveryBlob(blob_data, /*compress=*/false);
    case CompressionAlgorithm::kChunked:
      return TestDeliveryBlob(blob_data, /*compress=*/true);
  }
}

BlobReaderWrapper::BlobReaderWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobReader> reader)
    : reader_(std::move(reader)) {}

BlobReaderWrapper BlobReaderWrapper::Connect(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir) {
  auto client_end = component::ConnectAt<fuchsia_fxfs::BlobReader>(svc_dir);
  ZX_ASSERT(client_end.is_ok());
  return BlobReaderWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobReader>(std::move(*client_end)));
}

zx::result<zx::vmo> BlobReaderWrapper::GetVmo(const Digest& digest) const {
  fidl::Array<uint8_t, 32> hash;
  digest.CopyTo(hash.data_);
  auto result = reader_->GetVmo(hash);
  ZX_ASSERT(result.ok());
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move((*result)->vmo));
}

zx::result<> BlobReaderWrapper::VerifyBlob(const TestBlobData& blob) const {
  auto vmo = GetVmo(blob.digest());
  if (vmo.is_error()) {
    return vmo.take_error();
  }
  if (!VerifyContents(*vmo, blob.data())) {
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }
  return zx::ok();
}

BlobWriterWrapper::BlobWriterWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobWriter> writer)
    : writer_(std::move(writer)) {}

zx::result<> BlobWriterWrapper::BytesReady(uint64_t bytes_written) {
  auto result = writer_->BytesReady(bytes_written);
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  return zx::ok();
}

zx::result<zx::vmo> BlobWriterWrapper::GetVmo(uint64_t size) {
  auto result = writer_->GetVmo(size);
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move((*result)->vmo));
}

zx::result<IncrementalWriter> BlobWriterWrapper::CreateIncrementalWriter(
    const TestDeliveryBlob& blob) {
  auto vmo = GetVmo(blob.data().size());
  if (vmo.is_error()) {
    return vmo.take_error();
  }
  return zx::ok(IncrementalWriter(*this, blob.data(), std::move(*vmo)));
}

zx::result<> BlobWriterWrapper::WriteBlob(const TestDeliveryBlob& blob) {
  auto incremental_writer = CreateIncrementalWriter(blob);
  if (incremental_writer.is_error()) {
    return incremental_writer.take_error();
  }
  return incremental_writer->Complete();
}

IncrementalWriter::IncrementalWriter(BlobWriterWrapper& writer, std::span<const uint8_t> data,
                                     zx::vmo vmo)
    : writer_(writer), data_(data), vmo_(std::move(vmo)) {}

zx::result<> IncrementalWriter::Write(uint64_t amount) {
  ZX_ASSERT(amount > 0);
  ZX_ASSERT(amount <= data_.size());
  while (amount > 0) {
    uint64_t bytes_to_write = std::min(kRingBufferSize - vmo_offset_, amount);
    if (zx_status_t status = vmo_.write(data_.data(), vmo_offset_, bytes_to_write);
        status != ZX_OK) {
      return zx::error(status);
    }
    if (zx::result result = writer_.BytesReady(bytes_to_write); result.is_error()) {
      return result.take_error();
    }
    data_ = data_.subspan(bytes_to_write);
    amount -= bytes_to_write;
    vmo_offset_ = (vmo_offset_ + bytes_to_write) % kRingBufferSize;
  }
  return zx::ok();
}

zx::result<> IncrementalWriter::Complete() { return Write(data_.size()); }

BlobCreatorWrapper::BlobCreatorWrapper(fidl::WireSyncClient<fuchsia_fxfs::BlobCreator> creator)
    : creator_(std::move(creator)) {}

BlobCreatorWrapper BlobCreatorWrapper::Connect(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir) {
  auto client_end = component::ConnectAt<fuchsia_fxfs::BlobCreator>(svc_dir);
  ZX_ASSERT(client_end.is_ok());
  return BlobCreatorWrapper(
      fidl::WireSyncClient<fuchsia_fxfs::BlobCreator>(std::move(*client_end)));
}

zx::result<BlobWriterWrapper> BlobCreatorWrapper::Create(const Digest& digest) const {
  return Create(digest, false);
}

zx::result<BlobWriterWrapper> BlobCreatorWrapper::CreateExisting(const Digest& digest) const {
  return Create(digest, true);
}

zx::result<> BlobCreatorWrapper::CreateAndWriteBlob(const TestDeliveryBlob& blob) const {
  auto writer = Create(blob.digest());
  if (writer.is_error()) {
    return writer.take_error();
  }
  return writer->WriteBlob(blob);
}

zx::result<BlobWriterWrapper> BlobCreatorWrapper::Create(const Digest& digest,
                                                         bool allow_existing) const {
  fidl::Array<uint8_t, 32> hash;
  digest.CopyTo(hash.data_);
  auto result = creator_->Create(hash, allow_existing);
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    switch (result->error_value()) {
      case fuchsia_fxfs::CreateBlobError::kAlreadyExists:
        return zx::error(ZX_ERR_ALREADY_EXISTS);
      case fuchsia_fxfs::CreateBlobError::kInternal:
        return zx::error(ZX_ERR_INTERNAL);
    }
  }
  return zx::ok(BlobWriterWrapper(
      fidl::WireSyncClient<fuchsia_fxfs::BlobWriter>(std::move((*result)->writer))));
}

zx::result<bool> BlobCreatorWrapper::NeedsOverwrite(const Digest& digest) const {
  fidl::Array<uint8_t, 32> hash;
  digest.CopyTo(hash.data_);
  auto result = creator_->NeedsOverwrite(hash);
  ZX_ASSERT(result.ok());
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  return zx::ok(result->value()->needs_overwrite);
}

zx::result<fbl::RefPtr<Blob>> CreateBlob(Blobfs& blobfs, const TestDeliveryBlob& delivery_blob) {
  fbl::RefPtr blob = fbl::MakeRefCounted<Blob>(blobfs, delivery_blob.digest());
  if (zx_status_t status = blobfs.GetCache().Add(blob); status != ZX_OK) {
    return zx::error(status);
  }
  if (zx_status_t status = blob->Truncate(delivery_blob.data().size()); status != ZX_OK) {
    return zx::error(status);
  }
  size_t actual = 0;
  if (zx_status_t status =
          blob->Write(delivery_blob.data().data(), delivery_blob.data().size(), 0, &actual);
      status != ZX_OK) {
    return zx::error(status);
  }
  if (actual != delivery_blob.data().size()) {
    return zx::error(ZX_ERR_IO);
  }
  if (!blob->IsReadable()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok(blob);
}

zx::result<fbl::RefPtr<Blob>> GetBlob(Blobfs& blobfs, const Digest& digest) {
  fbl::RefPtr<CacheNode> node;
  if (zx_status_t status = blobfs.GetCache().Lookup(digest, &node); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(fbl::RefPtr<Blob>::Downcast(std::move(node)));
}

}  // namespace blobfs

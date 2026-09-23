// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_creator.h"

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/delivery_blob_private.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/node_finder.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {
namespace {
constexpr uint64_t kRingBufferSize = 256ul * 1024;
}  // namespace

class BlobWriter final : public fidl::WireServer<fuchsia_fxfs::BlobWriter> {
 public:
  explicit BlobWriter(fbl::RefPtr<Blob> blob, zx::unowned_channel server_channel)
      : blob_(std::move(blob)), server_channel_(std::move(server_channel)) {
    blob_->SetBlobWriterHandler(this);
  }

  ~BlobWriter() {
    if (blob_) {
      blob_->SetBlobWriterHandler(nullptr);
    }
  }

  void GetVmo(fuchsia_fxfs::wire::BlobWriterGetVmoRequest* request,
              GetVmoCompleter::Sync& completer) final {
    if (!blob_) {
      completer.ReplyError(ZX_ERR_BAD_STATE);
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }
    auto result = GetVmoImpl(request->size);
    if (result.is_error()) {
      completer.ReplyError(result.status_value());
    } else {
      completer.ReplySuccess(std::move(result).value());
    }
  }

  void BytesReady(fuchsia_fxfs::wire::BlobWriterBytesReadyRequest* request,
                  BytesReadyCompleter::Sync& completer) final {
    if (!blob_) {
      completer.ReplyError(ZX_ERR_BAD_STATE);
      completer.Close(ZX_ERR_BAD_STATE);
    } else if (auto result = BytesReadyImpl(request->bytes_written); result.is_error()) {
      completer.ReplyError(result.status_value());
      completer.Close(result.status_value());
    } else {
      completer.ReplySuccess();
    }
  }

  // Checks if the server channel is still active. If the client side has closed the associated
  // blob will be dropped synchronously, failing any future requests.
  bool ChannelActive() {
    if (!blob_) {
      return false;
    }
    zx_status_t status = server_channel_->signal_peer(0, 0);
    FX_LOGS(INFO) << "Checking BlobWriter channel active: " << zx_status_get_string(status);
    if (status != ZX_OK) {
      blob_.reset();
      return false;
    }
    return true;
  }

 private:
  zx::result<zx::vmo> GetVmoImpl(uint64_t size) {
    if (expected_size_.has_value() || vmo_.is_valid()) {
      // GetVmo was already called.
      return zx::error(ZX_ERR_BAD_STATE);
    }
    if (size < MetadataType1::kHeaderSize) {
      // The number of bytes being transferred is less than the size of the delivery blob header.
      return zx::error(ZX_ERR_BAD_STATE);
    }
    expected_size_ = size;
    if (zx_status_t status = blob_->Truncate(size); status != ZX_OK) {
      return zx::error(status);
    }
    zx::vmo vmo;
    if (zx_status_t status = zx::vmo::create(kRingBufferSize, 0, &vmo); status != ZX_OK) {
      return zx::error(status);
    }
    zx::vmo vmo_dup;
    if (zx_status_t status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup); status != ZX_OK) {
      return zx::error(status);
    }
    if (zx_status_t status = vmo_mapper_.Map(vmo); status != ZX_OK) {
      return zx::error(status);
    }
    buffer_ = static_cast<uint8_t*>(vmo_mapper_.start());
    vmo_ = std::move(vmo);
    return zx::ok(std::move(vmo_dup));
  }

  zx::result<> WriteBufferToBlob(uint64_t buffer_offset, uint64_t len) {
    size_t actual = 0;
    if (zx_status_t status = blob_->Write(buffer_ + buffer_offset, len, total_written_, &actual);
        status != ZX_OK) {
      return zx::error(status);
    }
    // BlobWriter doesn't support partial writes.
    ZX_DEBUG_ASSERT(actual == len);
    total_written_ += len;
    return zx::ok();
  }

  zx::result<> BytesReadyImpl(uint64_t bytes_written) {
    if (bytes_written > kRingBufferSize) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    if (!vmo_.is_valid()) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    if (!expected_size_.has_value()) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    uint64_t expected_size = *expected_size_;
    if (total_written_ + bytes_written > expected_size) {
      return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
    }
    uint64_t vmo_offset = total_written_ % kRingBufferSize;

    // |bytes_ready| may wrap around the end of ring buffer.
    uint64_t to_write = std::min(bytes_written, kRingBufferSize - vmo_offset);
    if (zx::result result = WriteBufferToBlob(vmo_offset, to_write); result.is_error()) {
      return result.take_error();
    }
    if (to_write < bytes_written) {
      // Write the wrapped bytes.
      if (zx::result result = WriteBufferToBlob(0, bytes_written - to_write); result.is_error()) {
        return result.take_error();
      }
    }
    if (total_written_ == expected_size) {
      // The blob should now be fully written and in the readable state.
      ZX_DEBUG_ASSERT(blob_->IsReadable());
    }
    return zx::ok();
  }

  static zx_status_t CleanupInactiveBlobWritersInternal(Blob* blob) { return ZX_OK; }

  fbl::RefPtr<Blob> blob_;
  // Holds a borrow of the server_channel, it should be valid as long as the binding is. Used to
  // internally check if the client end is still open or not.
  zx::unowned_channel server_channel_;
  std::optional<uint64_t> expected_size_;
  uint64_t total_written_ = 0;
  zx::vmo vmo_;
  fzl::VmoMapper vmo_mapper_;
  uint8_t* buffer_ = nullptr;
};  // class BlobWriter

BlobCreator::BlobCreator(Blobfs& blobfs)
    : fs::Service([this](fidl::ServerEnd<fuchsia_fxfs::BlobCreator> server_end) {
        bindings_.AddBinding(blobfs_.dispatcher(), std::move(server_end), this,
                             fidl::kIgnoreBindingClosure);
        return ZX_OK;
      }),
      blobfs_(blobfs) {}

void BlobCreator::Create(fuchsia_fxfs::wire::BlobCreatorCreateRequest* request,
                         CreateCompleter::Sync& completer) {
  Digest digest(request->hash.data_);
  auto result = CreateImpl(digest, request->allow_existing);
  if (result.is_error()) {
    if (result.status_value() == ZX_ERR_ALREADY_EXISTS) {
      completer.ReplyError(fuchsia_fxfs::wire::CreateBlobError::kAlreadyExists);
    } else {
      completer.ReplyError(fuchsia_fxfs::wire::CreateBlobError::kInternal);
    }
  } else {
    completer.ReplySuccess(std::move(result).value());
  }
}

zx::result<fidl::ClientEnd<fuchsia_fxfs::BlobWriter>> BlobCreator::CreateImpl(const Digest& digest,
                                                                              bool allow_existing) {
  // Return an error early if blobfs is mounted readonly.
  if (blobfs_.writability() != Writability::Writable) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::optional<fbl::RefPtr<Blob>> to_overwrite;
  fbl::RefPtr<CacheNode> found;
  if (zx_status_t status = blobfs_.GetCache().Lookup(digest, &found); status == ZX_OK) {
    auto blob = fbl::RefPtr<Blob>::Downcast(std::move(found));

    if (allow_existing && blob->IsReadable()) {
      // Check the cache if a previous version is here. If so, save a ref to it to replace it. To
      // keep the number of states low, only allow this if the existing version is readable, and
      // there is not another overwrite already in-flight. Also blocks if the blob is already marked
      // deleted and has not yet been purged. It would be confusing to allow overwrite to start
      // after a purge has been queued, but will later purge both versions.
      Blob* overwriting_by = blob->GetOverwritingBy();
      if (overwriting_by) {
        if (BlobWriter* handler = overwriting_by->GetBlobWriterHandler(); handler) {
          // If there is an active writer, check if it is still open.
          if (handler->ChannelActive()) {
            return zx::error(ZX_ERR_ALREADY_EXISTS);
          }
        }
      }

      if (blob->GetOverwritingBy() || blob->DeletionQueued()) {
        return zx::error(ZX_ERR_ALREADY_EXISTS);
      }
      to_overwrite = std::move(blob);
    } else if (blob->IsReadable()) {
      // The blob has finished writing and is persistent, but they didn't set allow_existing.
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    } else if (BlobWriter* handler = blob->GetBlobWriterHandler(); handler) {
      // Drop the local reference, either we're exiting because a write is in-flight or we're going
      // let the blob get cleaned up by removing the binding.
      blob.reset();
      // If there is an active writer, check if it is still open.
      if (handler->ChannelActive()) {
        return zx::error(ZX_ERR_ALREADY_EXISTS);
      }
    }
  }

  fbl::RefPtr new_blob = fbl::MakeRefCounted<Blob>(blobfs_, digest);
  if (to_overwrite.has_value()) {
    // Don't put it in the cache if it is not the canonical version yet.
    if (zx_status_t status = to_overwrite.value()->SetOverwritingBy(new_blob.get());
        status != ZX_OK) {
      // This should never happen due to earlier checks.
      ZX_DEBUG_ASSERT(status == ZX_OK);
      return zx::error(status);
    }
    new_blob->SetBlobToOverwrite(std::move(to_overwrite.value()));
  } else if (zx_status_t status = blobfs_.GetCache().Add(new_blob); status != ZX_OK) {
    // This should never happen due to earlier checks.
    ZX_DEBUG_ASSERT(status == ZX_OK);
    return zx::error(status);
  }

  auto endpoints = fidl::CreateEndpoints<fuchsia_fxfs::BlobWriter>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto writer =
      std::make_unique<BlobWriter>(std::move(new_blob), endpoints->server.handle()->borrow());
  writer_bindings_.AddBinding(blobfs_.dispatcher(), std::move(endpoints->server), writer.get(),
                              [writer = std::move(writer)](fidl::UnbindInfo info) {
                                // The lambda owns the writer. When the binding is closed, the
                                // writer will be destroyed.
                              });
  return zx::ok(std::move(endpoints->client));
}

void BlobCreator::NeedsOverwrite(fuchsia_fxfs::wire::BlobCreatorNeedsOverwriteRequest* request,
                                 NeedsOverwriteCompleter::Sync& completer) {
  Digest digest(request->blob_hash.data_);
  fbl::RefPtr<CacheNode> found;
  if (zx_status_t status = blobfs_.GetCache().Lookup(digest, &found); status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    auto blob = fbl::RefPtr<Blob>::Downcast(std::move(found));
    // Check Readable. Until it is marked readable there is no valid ino number yet, and no format
    // layout has been committed to disk.
    if (!blob->IsReadable()) {
      // If it is in the process of being written, but it is not readable yet,
      // then it should be considered to not exist. This will make it match the blob listing and the
      // behaviour of `GetVmo`.
      completer.ReplyError(ZX_ERR_NOT_FOUND);
      return;
    }
    BlobOverwriteConfig config = blobfs_.OverwriteConfig();
    if (config == BlobOverwriteConfig::kNoOverwrite) {
      completer.ReplySuccess(false);
      return;
    }
    auto ino_or = blobfs_.GetNode(blob->Ino());
    if (ino_or.is_error()) {
      completer.ReplyError(ino_or.error_value());
      return;
    }
    InodePtr ino = std::move(ino_or.value());
    // If the intended format is not the same as the current format, return true.
    completer.ReplySuccess(ino->IsLegacyPaddedFormat() !=
                           (config == BlobOverwriteConfig::kOverwriteToPadded));
  }
}

}  // namespace blobfs

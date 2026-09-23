// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob_cache.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blob_verifier.h"
#include "src/storage/blobfs/blob_writer.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/loader_info.h"
#include "src/storage/blobfs/page_loader.h"
#include "src/storage/blobfs/transaction.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {

zx::result<> VerifyNullBlob(Blobfs& blobfs, const Digest& digest) {
  zx::result verifier = BlobVerifier::CreateWithoutTree(digest, blobfs.GetMetrics(), 0);
  if (verifier.is_error()) {
    return verifier.take_error();
  }
  if (zx_status_t status = verifier->Verify(nullptr, 0, 0); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

uint32_t Blob::Ino() const {
  std::lock_guard lock(mutex_);
  // This value isn't trustworthy if the blob has not yet been written out, or has been purged.
  ZX_ASSERT(state_ == BlobState::kReadable);
  return map_index_;
}

uint64_t Blob::FileSize() const {
  std::lock_guard lock(mutex_);
  if (state_ == BlobState::kReadable)
    return blob_size_;
  return 0;
}

Blob::Blob(Blobfs& blobfs, const Digest& digest)
    : CacheNode(*blobfs.vfs(), digest), blobfs_(blobfs) {
  writer_ = std::make_unique<Blob::Writer>(*this);
}

Blob::Blob(Blobfs& blobfs, uint32_t node_index, const Inode& inode)
    : CacheNode(*blobfs.vfs(), Digest(inode.merkle_root_hash)),
      blobfs_(blobfs),
      state_(BlobState::kReadable),
      map_index_(node_index),
      blob_size_(inode.blob_size),
      block_count_(inode.block_count) {}

bool Blob::IsDataLoaded() const {
  // Data is served out of the paged_vmo().
  return paged_vmo().is_valid();
}

zx_status_t Blob::MarkReadable(const WrittenBlob& written_blob) {
  map_index_ = written_blob.map_index;
  blob_size_ = written_blob.layout->FileSize();
  block_count_ = written_blob.layout->TotalBlockCount();
  state_ = BlobState::kReadable;
  writer_.reset();
  return ZX_OK;
}

zx_status_t Blob::LoadPagedVmosFromDisk() {
  ZX_ASSERT_MSG(!IsDataLoaded(), "Data VMO is not loaded.");

  // Do not attempt to load anything from disk if this blob is marked corrupted, as it may be the
  // metadata that is corrupted and we may start taking unexpected paths.
  if (is_corrupt_) {
    return ZX_ERR_IO_DATA_INTEGRITY;
  }

  zx::result<LoaderInfo> load_info_or = blobfs_.loader().LoadBlob(map_index_);
  if (load_info_or.is_error())
    return load_info_or.error_value();

  // Make the vmo.
  if (auto status = EnsureCreatePagedVmo(load_info_or->layout->FileBlockAlignedSize());
      status.is_error())
    return status.error_value();

  // Commit the other load information.
  loader_info_ = std::move(*load_info_or);

  return ZX_OK;
}

zx_status_t Blob::LoadVmosFromDisk() {
  if (IsDataLoaded())
    return ZX_OK;

  zx_status_t status;
  if (blob_size_ == 0) {
    // Null blobs don't need any loading, just verification that they're correct.
    status = VerifyNullBlob(blobfs_, digest()).status_value();
    if (status == ZX_OK) {
      status = EnsureCreatePagedVmo(0).status_value();
    }
  } else {
    status = LoadPagedVmosFromDisk();
  }

  return status;
}

zx_status_t Blob::QueueUnlink() {
  std::lock_guard lock(mutex_);

  deletable_ = true;
  // Attempt to purge in case the blob has been unlinked with no open fds
  return TryPurge();
}

zx_status_t Blob::Verify() {
  {
    std::lock_guard lock(mutex_);
    if (auto status = LoadVmosFromDisk(); status != ZX_OK)
      return status;
  }

  // For non-pager-backed blobs, commit the entire blob in memory. This will cause all of the pages
  // to be verified as they are read in (or for the null bob we just verify immediately). If the
  // commit operation fails due to a verification failure, we do propagate the error back via the
  // return status.
  //
  // This is a read-only operation on the blob so can be done with the shared lock. Since it will
  // reenter the Blob object on the pager thread to satisfy this request, it actually MUST be done
  // with only the shared lock or the reentrance on the pager thread will deadlock us.
  {
    fs::SharedLock lock(mutex_);

    // There is a race condition if somehow this blob was unloaded in between the above exclusive
    // lock and the shared lock in this block. Currently this is not possible because there is only
    // one thread processing fidl messages and paging events on the pager threads can't unload the
    // blob.
    //
    // But in the future certain changes might make this theoretically possible (though very
    // difficult to imagine in practice). If this were to happen, we would prefer to err on the side
    // of reporting a blob valid rather than mistakenly reporting errors that might cause a valid
    // blob to be deleted.
    if (state_ != BlobState::kReadable)
      return ZX_OK;

    if (blob_size_ == 0) {
      // It's the null blob, so just verify.
      return VerifyNullBlob(blobfs_, digest()).status_value();
    }
    return paged_vmo().op_range(ZX_VMO_OP_COMMIT, 0, blob_size_, nullptr, 0);
  }
}

void Blob::OnNoPagedVmoClones() {
  // Override the default behavior of PagedVnode to avoid clearing the paged_vmo. We keep this
  // alive for caching purposes as long as this object is alive, and this object's lifetime is
  // managed by the BlobCache.
  if (!HasReferences()) {
    // Mark the name to help identify the VMO is unused.
    SetPagedVmoName(false);
    // Hint that the VMO's pages are no longer needed, and can be evicted under memory pressure. If
    // a page is accessed again, it will lose the hint.
    zx_status_t status = paged_vmo().op_range(ZX_VMO_OP_DONT_NEED, 0, blob_size_, nullptr, 0);
    if (status != ZX_OK) {
      FX_LOGS(WARNING) << "Hinting DONT_NEED on blob " << digest()
                       << " failed: " << zx_status_get_string(status);
    }

    // This might have been the last reference to a deleted blob, so try purging it.
    if (zx_status_t status = TryPurge(); status != ZX_OK) {
      FX_LOGS(WARNING) << "Purging blob " << digest()
                       << " failed: " << zx_status_get_string(status);
    }
  }
}

BlobCache& Blob::GetCache() { return blobfs_.GetCache(); }

bool Blob::ShouldCache() const {
  std::lock_guard lock(mutex_);
  return state_ == BlobState::kReadable;
}

Blob::~Blob() {
  // The PagedVfs holds a strong reference to the vnode when there are outstanding VMO clones and a
  // weak reference when there are no clones. Since this is the destructor, the PagedVfs shouldn't
  // be holding a strong reference.
  //
  // When Blobfs is shutting down, it calls |WillTeardownFilesystem| which will remove the strong
  // reference from the PagedVfs if VMO clones did still exist.
  ZX_ASSERT(FreePagedVmo() == nullptr);
}

fuchsia_io::NodeProtocolKinds Blob::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kFile;
}

zx_status_t Blob::Write(const void* data, size_t len, size_t offset, size_t* out_actual) {
  TRACE_DURATION("blobfs", "Blob::Write", "len", len, "off", offset);
  return blobfs_.node_operations().write.Track([&]() -> zx_status_t {
    std::lock_guard lock(mutex_);
    *out_actual = 0;
    if (state_ == BlobState::kError) {
      ZX_DEBUG_ASSERT(writer_);
      return writer_->status().error_value();
    }
    if (len == 0) {
      return ZX_OK;
    }
    if (!writer_ || state_ != BlobState::kDataWrite) {
      return ZX_ERR_BAD_STATE;
    }
    if (offset != writer_->total_written()) {
      FX_LOGS(ERROR) << "only append is currently supported (requested_offset: " << offset
                     << ", expected: " << writer_->total_written() << ")";
      return ZX_ERR_NOT_SUPPORTED;
    }

    // Perform the actual write.
    ZX_DEBUG_ASSERT(state_ == BlobState::kDataWrite);
    zx::result written_blob = writer_->Write(*this, data, len, out_actual);
    if (written_blob.is_error()) {
      return OnWriteError(written_blob.take_error());
    }

    if ((*written_blob).has_value()) {
      return MarkReadable(*written_blob.value());
    }

    return ZX_OK;  // More data to write.
  });
}

zx_status_t Blob::Truncate(size_t len) {
  TRACE_DURATION("blobfs", "Blob::Truncate", "len", len);
  return blobfs_.node_operations().truncate.Track([this, len]() -> zx_status_t {
    std::lock_guard lock(mutex_);
    if (state_ != BlobState::kEmpty || writer_ == nullptr) {
      return ZX_ERR_BAD_STATE;
    }

    // Special case: If this is the null blob, we skip the write phase.
    if (len == 0) {
      zx::result written_blob = writer_->WriteNullBlob(*this);
      if (written_blob.is_error()) {
        return OnWriteError(written_blob.take_error());
      }
      return MarkReadable(*written_blob);
    }

    // Prepare writer_ to accept `len` bytes of data in total.
    zx::result status = writer_->Prepare(*this, len);
    if (status.is_error()) {
      return OnWriteError(status.take_error());
    }

    // Indicate that the blob is in the writable state now.
    state_ = BlobState::kDataWrite;
    return ZX_OK;
  });
}

// This function will get called on an arbitrary pager worker thread.
void Blob::VmoRead(uint64_t offset, uint64_t length) {
  TRACE_DURATION("blobfs", "Blob::VmoRead", "offset", offset, "length", length);

  // It's important that this function use only a shared read lock. This is for performance (to
  // allow multiple page requests to be run in parallel) and to prevent deadlock with the
  // non-paged Read() path. The non-paged path is implemented by reading from the vmo which will
  // recursively call into this code and taking an exclusive lock would deadlock.
  fs::SharedLock lock(mutex_);

  if (!paged_vmo()) {
    // Races with calling FreePagedVmo() on another thread can result in stale read requests.
    // Ignore them if the VMO is gone.
    return;
  }

  ZX_DEBUG_ASSERT(IsDataLoaded());

  std::optional vfs_opt = vfs();
  ZX_ASSERT(vfs_opt.has_value());
  fs::PagedVfs& vfs = vfs_opt.value().get();

  if (is_corrupt_) {
    FX_LOGS(ERROR) << "Blobfs failing page request because blob was previously found corrupt: "
                   << digest();
    if (auto error_result = vfs.ReportPagerError(paged_vmo(), offset, length, ZX_ERR_BAD_STATE);
        error_result.is_error()) {
      FX_LOGS(ERROR) << "Failed to report pager error to kernel: " << error_result.status_string();
    }
    return;
  }

  auto page_supplier = PageLoader::PageSupplier(
      [&vfs, &dest_vmo = paged_vmo()](uint64_t offset, uint64_t length, const zx::vmo& aux_vmo,
                                      uint64_t aux_offset) {
        return vfs.SupplyPages(dest_vmo, offset, length, aux_vmo, aux_offset);
      });
  PagerErrorStatus pager_error_status =
      blobfs_.page_loader().TransferPages(page_supplier, offset, length, loader_info_);
  if (pager_error_status != PagerErrorStatus::kOK) {
    FX_LOGS(ERROR) << "Pager failed to transfer pages to the blob, error: "
                   << zx_status_get_string(static_cast<zx_status_t>(pager_error_status));
    if (auto error_result = vfs.ReportPagerError(paged_vmo(), offset, length,
                                                 static_cast<zx_status_t>(pager_error_status));
        error_result.is_error()) {
      FX_LOGS(ERROR) << "Failed to report pager error to kernel: " << error_result.status_string();
    }

    // We've signaled a failure and unblocked outstanding page requests for this range. If the
    // pager error was a verification error, fail future requests as well - we should not service
    // further page requests on a corrupt blob.
    //
    // Note that we cannot simply detach the VMO from the pager here. There might be outstanding
    // page requests which have been queued but are yet to be serviced. These need to be handled
    // correctly - if the VMO is detached, there will be no way for us to communicate failure to
    // the kernel, since zx_pager_op_range() requires a valid pager VMO handle. Without being able
    // to make a call to zx_pager_op_range() to indicate a failed page request, the faulting
    // thread would hang indefinitely.
    if (pager_error_status == PagerErrorStatus::kErrDataIntegrity)
      is_corrupt_ = true;
  }
}

bool Blob::HasReferences() const { return has_clones(); }

void Blob::WillTeardownFilesystem() {
  // Be careful to release the pager reference outside the lock.
  fbl::RefPtr<fs::Vnode> pager_reference;
  {
    std::lock_guard lock(mutex_);
    pager_reference = FreePagedVmo();
  }
  // When pager_reference goes out of scope here, it could cause |this| to be deleted.
}

zx_status_t Blob::OpenNode(fbl::RefPtr<Vnode>* out_redirect) { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t Blob::CloseNode() { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t Blob::TryPurge() {
  if (Purgeable()) {
    return Purge();
  }
  return ZX_OK;
}

zx_status_t Blob::Purge() {
  ZX_DEBUG_ASSERT(Purgeable());

  if (state_ == BlobState::kReadable) {
    // A readable blob should only be purged if it has been unlinked.
    ZX_ASSERT_MSG(deletable_, "Should not purge blob which is not unlinked.");

    BlobTransaction transaction;
    if (zx_status_t status = blobfs_.FreeInode(map_index_, transaction); status != ZX_OK)
      return status;
    transaction.Commit(*blobfs_.GetJournal());
    blobfs_.GetAllocator()->Decommit();
  }

  // If the blob is in the error state, it should have already been evicted from
  // the cache (see Blob::OnWriteError).
  if (state_ != BlobState::kError) {
    if (zx_status_t status = GetCache().Evict(fbl::RefPtr(this)); status != ZX_OK)
      return status;
  }

  state_ = BlobState::kPurged;
  return ZX_OK;
}

uint64_t Blob::GetBlockSize() const { return blobfs_.Info().block_size; }

void Blob::SetPagedVmoName(bool active) {
  VmoNameBuffer name =
      active ? FormatBlobDataVmoName(digest()) : FormatInactiveBlobDataVmoName(digest());
  // Ignore failures, the name is for informational purposes only.
  paged_vmo().set_property(ZX_PROP_NAME, name.data(), name.size());
}

zx_status_t Blob::OnWriteError(zx::error_result error) {
  ZX_DEBUG_ASSERT(writer_);
  writer_->set_status(error);
  state_ = BlobState::kError;
  // Evict this blob from the cache now that we're placing it in an error state.
  if (zx_status_t status = GetCache().Evict(fbl::RefPtr(this)); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to evict blob from cache: " << zx_status_get_string(status);
  }
  // Return the now latched write error.
  return writer_->status().error_value();
}

zx::result<zx::vmo> Blob::GetVmoForBlobReader() {
  std::lock_guard lock(mutex_);
  if (state_ != BlobState::kReadable) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  if (zx_status_t status = LoadVmosFromDisk(); status != ZX_OK) {
    return zx::error(status);
  }
  if (!HasReferences()) {
    SetPagedVmoName(true);
  }

  zx::vmo child_vmo;
  if (zx_status_t status =
          paged_vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_NO_WRITE,
                                   0, blob_size_, &child_vmo);
      status != ZX_OK) {
    return zx::error(status);
  }
  DidClonePagedVmo();

  if (zx_status_t status = child_vmo.replace(
          ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_READ, &child_vmo);
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(child_vmo));
}

bool Blob::IsReadable() {
  std::lock_guard lock(mutex_);
  return state_ == BlobState::kReadable;
}

zx::result<> Blob::ReplaceBlob(BlobTransaction& transaction, uint32_t map_index,
                               uint64_t block_count,
                               const std::function<zx::result<>()>& transaction_completion) {
  std::lock_guard l(mutex_);

  if (zx_status_t status = blobfs_.FreeInode(map_index_, transaction); status != ZX_OK) {
    FailedReplacement();
    return zx::error(status);
  }

  if (zx::result<> result = transaction_completion(); result.is_error()) {
    FailedReplacement();
    return result.take_error();
  }

  if (zx::result<> result = CompleteReplacement(map_index, block_count); result.is_error()) {
    FailedReplacement();
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> Blob::CompleteReplacement(uint32_t map_index, uint64_t block_count) {
  // Don't keep the loader info in memory if this blob isn't currently loaded.
  if (loader_info_.layout) {
    zx::result<LoaderInfo> load_info_or = blobfs_.loader().LoadBlob(map_index);
    if (load_info_or.is_error()) {
      return load_info_or.take_error();
    }
    loader_info_ = std::move(load_info_or.value());
  }
  block_count_ = block_count;
  map_index_ = map_index;
  return zx::ok();
}

void Blob::FailedReplacement() { is_corrupt_.store(true, std::memory_order_relaxed); }

zx_status_t Blob::SetOverwritingBy(Blob* overwriter) {
  std::lock_guard lock(mutex_);
  if (overwritten_by_) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  overwritten_by_ = overwriter;
  return ZX_OK;
}

zx_status_t Blob::ClearOverwritingBy() {
  std::lock_guard lock(mutex_);
  if (!overwritten_by_) {
    return ZX_ERR_BAD_STATE;
  }
  overwritten_by_ = nullptr;
  // This may be queued for deletion that was previously blocked by `being_overwritten_`.
  return TryPurge();
}

Blob* Blob::GetOverwritingBy() {
  std::lock_guard lock(mutex_);
  return overwritten_by_;
}

void Blob::SetBlobToOverwrite(fbl::RefPtr<Blob> to_overwrite) {
  std::lock_guard lock(mutex_);
  ZX_DEBUG_ASSERT(writer_);
  if (writer_) {
    writer_->SetBlobToOverwrite(std::move(to_overwrite));
  }
}

void Blob::SetBlobWriterHandler(BlobWriter* writer_handler) {
  std::lock_guard lock(mutex_);
  blob_writer_handler_ = writer_handler;
}

BlobWriter* Blob::GetBlobWriterHandler() {
  std::lock_guard lock(mutex_);
  return blob_writer_handler_;
}

}  // namespace blobfs

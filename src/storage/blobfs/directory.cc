// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/directory.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cassert>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {

Directory::Directory(Blobfs* bs) : blobfs_(bs) {}

Directory::~Directory() = default;

fuchsia_io::NodeProtocolKinds Directory::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kDirectory;
}

zx_status_t Directory::Readdir(fs::VdirCookie* cookie, void* dirents, size_t len,
                               size_t* out_actual) {
  return blobfs_->Readdir(cookie, dirents, len, out_actual);
}

zx_status_t Directory::Read(void* data, size_t len, size_t off, size_t* out_actual) {
  return ZX_ERR_NOT_FILE;
}

zx_status_t Directory::Write(const void* data, size_t len, size_t offset, size_t* out_actual) {
  return ZX_ERR_NOT_FILE;
}

zx_status_t Directory::Append(const void* data, size_t len, size_t* out_end, size_t* out_actual) {
  return ZX_ERR_NOT_FILE;
}

zx_status_t Directory::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) {
  TRACE_DURATION("blobfs", "Directory::Lookup", "name", name);
  assert(memchr(name.data(), '/', name.length()) == nullptr);

  return blobfs_->node_operations().lookup.Track([&] {
    if (name == ".") {
      *out = fbl::RefPtr<Directory>(this);
      return ZX_OK;
    }
    return ZX_ERR_NOT_SUPPORTED;
  });
}

zx_status_t Directory::Unlink(std::string_view name, bool must_be_dir) {
  TRACE_DURATION("blobfs", "Directory::Unlink", "name", name, "must_be_dir", must_be_dir);
  assert(memchr(name.data(), '/', name.length()) == nullptr);

  return blobfs_->node_operations().unlink.Track([&] {
    Digest digest;
    if (zx_status_t status = digest.Parse(name.data(), name.length()); status != ZX_OK) {
      return status;
    }
    fbl::RefPtr<CacheNode> cache_node;
    if (zx_status_t status = blobfs_->GetCache().Lookup(digest, &cache_node); status != ZX_OK) {
      return status;
    }
    auto vnode = fbl::RefPtr<Blob>::Downcast(std::move(cache_node));
    blobfs_->GetMetrics()->UpdateLookup(vnode->FileSize());
    return vnode->QueueUnlink();
  });
}

void Directory::Sync(SyncCallback closure) {
  auto event = blobfs_->node_operations().sync.NewEvent();
  blobfs_->Sync(
      [this, cb = std::move(closure), event = std::move(event)](zx_status_t status) mutable {
        // This callback will be issued on the journal thread in the normal case. This is important
        // because the flush must happen there or it will block the main thread which would block
        // processing other requests.
        //
        // If called during shutdown this may get issued on the main thread but then the flush
        // transaction should be a no-op.
        if (status == ZX_OK) {
          status = blobfs_->Flush();
        }
        cb(status);
        event.SetStatus(status);
      });
}

}  // namespace blobfs

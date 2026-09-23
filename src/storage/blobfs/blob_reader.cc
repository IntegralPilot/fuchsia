// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_reader.h"

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/cache_node.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {
namespace {

zx::result<zx::vmo> GetVmoImpl(Blobfs& blobfs, const Digest& digest) {
  fbl::RefPtr<CacheNode> cache_node;
  if (zx_status_t status = blobfs.GetCache().Lookup(digest, &cache_node); status != ZX_OK) {
    return zx::error(status);
  }
  auto blob = fbl::RefPtr<Blob>::Downcast(std::move(cache_node));
  return blob->GetVmoForBlobReader();
}

}  // namespace

BlobReader::BlobReader(Blobfs& blobfs)
    : fs::Service([this](fidl::ServerEnd<fuchsia_fxfs::BlobReader> server_end) {
        bindings_.AddBinding(blobfs_.dispatcher(), std::move(server_end), this,
                             fidl::kIgnoreBindingClosure);
        return ZX_OK;
      }),
      blobfs_(blobfs) {}

void BlobReader::GetVmo(fuchsia_fxfs::wire::BlobReaderGetVmoRequest* request,
                        GetVmoCompleter::Sync& completer) {
  Digest digest(request->blob_hash.data_);
  zx::result<zx::vmo> result = GetVmoImpl(blobfs_, digest);
  if (result.is_error()) {
    completer.ReplyError(result.error_value());
  } else {
    completer.ReplySuccess(std::move(result).value());
  }
}

}  // namespace blobfs

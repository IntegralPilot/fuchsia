// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_BLOB_CREATOR_H_
#define SRC_STORAGE_BLOBFS_BLOB_CREATOR_H_

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

class BlobCreator final : public fidl::WireServer<fuchsia_fxfs::BlobCreator>, public fs::Service {
 public:
  explicit BlobCreator(Blobfs& blobfs);

  void Create(fuchsia_fxfs::wire::BlobCreatorCreateRequest* request,
              CreateCompleter::Sync& completer) final;

  void NeedsOverwrite(fuchsia_fxfs::wire::BlobCreatorNeedsOverwriteRequest* request,
                      NeedsOverwriteCompleter::Sync& completer) final;

  using fs::Service::Create;

 private:
  zx::result<fidl::ClientEnd<fuchsia_fxfs::BlobWriter>> CreateImpl(const Digest& digest,
                                                                   bool allow_existing);

  Blobfs& blobfs_;

  // The ComponentRunner shuts down the VFS hosting this service before destroying Blobfs. All of
  // the bindings need to be owned by this service to ensure they get destroyed before Blobfs is
  // destroyed.
  fidl::ServerBindingGroup<fuchsia_fxfs::BlobWriter> writer_bindings_;
  fidl::ServerBindingGroup<fuchsia_fxfs::BlobCreator> bindings_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_BLOB_CREATOR_H_

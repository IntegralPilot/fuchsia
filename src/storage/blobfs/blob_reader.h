// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_BLOB_READER_H_
#define SRC_STORAGE_BLOBFS_BLOB_READER_H_

#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

class BlobReader final : public fidl::WireServer<fuchsia_fxfs::BlobReader>, public fs::Service {
 public:
  explicit BlobReader(Blobfs& blobfs);

  void GetVmo(fuchsia_fxfs::wire::BlobReaderGetVmoRequest* request,
              GetVmoCompleter::Sync& completer) final;

  using fs::Service::GetVmo;

 private:
  Blobfs& blobfs_;

  // The ComponentRunner shuts down the VFS hosting this service before destroying Blobfs. All of
  // the bindings need to be owned by this service to ensure they get destroyed before Blobfs is
  // destroyed.
  fidl::ServerBindingGroup<fuchsia_fxfs::BlobReader> bindings_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_BLOB_READER_H_

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_SERVICE_ADMIN_H_
#define SRC_STORAGE_BLOBFS_SERVICE_ADMIN_H_

#include <fidl/fuchsia.fs/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

class AdminService : public fidl::WireServer<fuchsia_fs::Admin>, public fs::Service {
 public:
  using ShutdownRequester = fit::callback<void(fs::FuchsiaVfs::ShutdownCallback)>;
  AdminService(async_dispatcher_t* dispatcher, ShutdownRequester shutdown);

  void Shutdown(ShutdownCompleter::Sync& completer) override;

 private:
  ShutdownRequester shutdown_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_SERVICE_ADMIN_H_

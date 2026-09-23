// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/service/admin.h"

#include <fidl/fuchsia.fs/cpp/markers.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

AdminService::AdminService(async_dispatcher_t* dispatcher, ShutdownRequester shutdown)
    : fs::Service([dispatcher, this](fidl::ServerEnd<fuchsia_fs::Admin> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), this);
        return ZX_OK;
      }),
      shutdown_(std::move(shutdown)) {}

void AdminService::Shutdown(ShutdownCompleter::Sync& completer) {
  shutdown_([completer = completer.ToAsync()](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "filesystem shutdown failed";
    }
    completer.Reply();
  });
}

}  // namespace blobfs

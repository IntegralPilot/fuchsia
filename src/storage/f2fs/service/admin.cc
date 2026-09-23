// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/service/admin.h"

#include <lib/syslog/cpp/macros.h>

namespace f2fs {

AdminService::AdminService(async_dispatcher_t* dispatcher, ShutdownRequester shutdown)
    : fs::Service([dispatcher, this](fidl::ServerEnd<fuchsia_fs::Admin> server_end) {
        fidl::BindServer(
            dispatcher, std::move(server_end), this,
            [self = fbl::RefPtr<AdminService>(this)](AdminService*, fidl::UnbindInfo,
                                                     fidl::ServerEnd<fuchsia_fs::Admin>) {});
        return ZX_OK;
      }),
      shutdown_(std::move(shutdown)) {}

void AdminService::Shutdown(ShutdownCompleter::Sync& completer) {
  FX_LOGS(INFO) << "received shutdown command over admin interface";
  shutdown_([completer = completer.ToAsync()](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "filesystem shutdown failed: " << zx_status_get_string(status);
    }
    completer.Reply();
  });
}

}  // namespace f2fs

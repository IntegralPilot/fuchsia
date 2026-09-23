// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/service/lifecycle.h"

#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

namespace blobfs {

void LifecycleServer::Create(async_dispatcher_t* dispatcher, ShutdownCallback shutdown,
                             fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> request) {
  if (!request.is_valid()) {
    FX_LOGS(INFO)
        << "Invalid handle for lifecycle events, assuming test environment and continuing";
    return;
  }
  fidl::BindServer(dispatcher, std::move(request),
                   std::make_unique<LifecycleServer>(std::move(shutdown)));
}

void LifecycleServer::Stop(StopCompleter::Sync& completer) {
  FX_LOGS(INFO) << "received shutdown command over lifecycle interface";
  shutdown_([completer = completer.ToAsync()](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "shutdown failed";
    } else {
      FX_LOGS(INFO) << "blobfs shutdown complete";
    }
    completer.Close(status);
  });
}

}  // namespace blobfs

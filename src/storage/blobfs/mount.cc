// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/mount.h"

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>

#include <memory>
#include <utility>

#include "src/storage/blobfs/component_runner.h"

namespace blobfs {

zx::result<> StartComponent(ComponentOptions options, fidl::ServerEnd<fuchsia_io::Directory> root,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle) {
  // When the loop is destroyed, it can make calls into runner, so runner *must* be destroyed after
  // the loop.
  std::unique_ptr<ComponentRunner> runner;
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  trace::TraceProviderWithFdio provider(loop.dispatcher());

  runner = std::make_unique<ComponentRunner>(loop, options);
  auto status = runner->ServeRoot(std::move(root), std::move(lifecycle));
  if (status.is_error()) {
    return status;
  }

  loop.Run();

  return zx::ok();
}

}  // namespace blobfs

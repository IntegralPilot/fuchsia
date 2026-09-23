// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/mount.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include "src/storage/f2fs/component_runner.h"

namespace f2fs {

zx::result<> StartComponent(fidl::ServerEnd<fuchsia_io::Directory> root,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  trace::TraceProviderWithFdio trace_provider(loop.dispatcher());

  std::unique_ptr<ComponentRunner> runner(new ComponentRunner(loop.dispatcher()));
  runner->SetUnmountCallback([&loop]() {
    loop.Quit();
    FX_LOGS(INFO) << "unmounted successfully";
  });
  auto status = runner->ServeRoot(std::move(root), std::move(lifecycle));
  if (status.is_error()) {
    return status;
  }

  // |ZX_ERR_CANCELED| is returned when the loop is cancelled via |loop.Quit()|.
  ZX_ASSERT(loop.Run() == ZX_ERR_CANCELED);
  return zx::ok();
}

zx::result<size_t> MountOptions::GetValue(const MountOption option) const {
  if (option >= MountOption::kMaxNum)
    return zx::error(ZX_ERR_INVALID_ARGS);
  return zx::ok(opt_[static_cast<size_t>(option)]);
}

zx_status_t MountOptions::SetValue(const MountOption option, const size_t value) {
  if (option == MountOption::kActiveLogs && value != 2 && value != 4 && value != 6) {
    FX_LOGS(WARNING) << " active_logs can be set only to 2, 4, or 6.";
    return ZX_ERR_INVALID_ARGS;
  };
  opt_[static_cast<size_t>(option)] = value;
  return ZX_OK;
}

}  // namespace f2fs

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <cstdlib>
#include <utility>

#include "src/storage/blobfs/bin/blobfs_component_config.h"
#include "src/storage/blobfs/mount.h"

namespace {

zx_status_t StartComponent() {
  FX_LOGS(INFO) << "starting blobfs component";

  zx::channel outgoing_server = zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST));
  if (!outgoing_server.is_valid()) {
    FX_LOGS(ERROR) << "PA_DIRECTORY_REQUEST startup handle is required.";
    return ZX_ERR_INTERNAL;
  }
  fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir(std::move(outgoing_server));

  zx::channel lifecycle_channel = zx::channel(zx_take_startup_handle(PA_LIFECYCLE));
  if (!lifecycle_channel.is_valid()) {
    FX_LOGS(ERROR) << "PA_LIFECYCLE startup handle is required.";
    return ZX_ERR_INTERNAL;
  }
  fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle_request(
      std::move(lifecycle_channel));

  auto config = blobfs_component_config::Config::TakeFromStartupHandle();
  const blobfs::ComponentOptions options{
      .pager_threads = config.pager_threads(),
  };
  // blocks until blobfs exits
  zx::result status =
      blobfs::StartComponent(options, std::move(outgoing_dir), std::move(lifecycle_request));
  if (status.is_error()) {
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

}  // namespace

int main(int argc, char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"blobfs"}).BuildAndInitialize();

  if (zx_status_t status = StartComponent(); status != ZX_OK) {
    return EXIT_FAILURE;
  }

  return EXIT_SUCCESS;
}

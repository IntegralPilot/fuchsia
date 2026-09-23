// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/service/startup.h"

#include <lib/syslog/cpp/macros.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/fsck.h"
#include "src/storage/f2fs/mkfs.h"

namespace f2fs {

StartupService::StartupService(async_dispatcher_t* dispatcher, ConfigureCallback cb)
    : fs::Service([dispatcher, this](fidl::ServerEnd<fuchsia_fs_startup::Startup> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), this,
                         [self = fbl::RefPtr<StartupService>(this)](
                             StartupService*, fidl::UnbindInfo,
                             fidl::ServerEnd<fuchsia_fs_startup::Startup>) {});
        return ZX_OK;
      }),
      configure_(std::move(cb)) {}

void StartupService::Start(StartRequestView request, StartCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    if (!configure_)
      return zx::error(ZX_ERR_BAD_STATE);

    zx::result bc = f2fs::CreateBcacheMapper(std::move(request->device));
    if (bc.is_error()) {
      return bc.take_error();
    }

    // TODO: parse option from request->options.
    MountOptions mount_options;
    return configure_(*std::move(bc), mount_options);
  }());
}

void StartupService::Format(FormatRequestView request, FormatCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    zx::result bc = f2fs::CreateBcacheMapper(std::move(request->device), true);
    if (bc.is_error()) {
      return bc.take_error();
    }

    f2fs::MkfsOptions mkfs_options;
    // TODO: parse option from request->options.
    if (auto status = f2fs::Mkfs(mkfs_options, *std::move(bc)); status.is_error()) {
      FX_LOGS(ERROR) << "failed to format f2fs: " << status.status_string();
      return status.take_error();
    }
    return zx::ok();
  }());
}

void StartupService::Check(CheckRequestView request, CheckCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    zx::result bc = f2fs::CreateBcacheMapper(std::move(request->device));
    if (bc.is_error()) {
      return bc.take_error();
    }

    // TODO: parse option from request->options.
    FsckOptions fsck_options;
    fsck_options.repair = bc->IsWritable();

    if (zx_status_t status = Fsck(*std::move(bc), fsck_options); status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Fsck failed";
      return zx::error(status);
    }
    return zx::ok();
  }());
}

}  // namespace f2fs

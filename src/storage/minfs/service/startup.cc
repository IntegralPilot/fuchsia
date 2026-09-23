// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/service/startup.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/minfs/fsck.h"
#include "src/storage/minfs/minfs.h"

namespace minfs {

namespace {

MountOptions ParseMountOptions(fuchsia_fs_startup::wire::StartOptions start_options,
                               bool bcache_read_only) {
  MountOptions options;

  options.verbose = start_options.has_verbose() && start_options.verbose();
  options.fsck_after_every_transaction = start_options.has_fsck_after_every_transaction() &&
                                         start_options.fsck_after_every_transaction();

  if (bcache_read_only) {
    options.writability = Writability::ReadOnlyDisk;
    options.repair_filesystem = false;
  } else if (start_options.has_read_only() && start_options.read_only()) {
    options.writability = Writability::ReadOnlyFilesystem;
  } else {
    options.writability = Writability::Writable;
  }

  return options;
}

MountOptions ParseFormatOptions(const fuchsia_fs_startup::wire::FormatOptions& format_options) {
  MountOptions options;

  options.verbose = format_options.has_verbose() && format_options.verbose();
  if (format_options.has_fvm_data_slices() &&
      format_options.fvm_data_slices() > options.fvm_data_slices) {
    options.fvm_data_slices = format_options.fvm_data_slices();
  }
  // We _need_ a writable filesystem to meaningfully format it.
  options.writability = Writability::Writable;

  return options;
}

}  // namespace

StartupService::StartupService(async_dispatcher_t* dispatcher, ConfigureCallback cb)
    : fs::Service([dispatcher, this](fidl::ServerEnd<fuchsia_fs_startup::Startup> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), this);
        return ZX_OK;
      }),
      configure_(std::move(cb)) {}

void StartupService::Start(StartRequestView request, StartCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    if (!configure_)
      return zx::error(ZX_ERR_BAD_STATE);

    zx::result device = block_client::RemoteBlockDevice::Create(
        fidl::ClientEnd<fuchsia_storage_block::Block>(request->device.TakeChannel()));
    if (device.is_error()) {
      FX_PLOGS(ERROR, device.error_value()) << "Could not initialize block device";
      return device.take_error();
    }

    auto bcache_res = CreateBcache(std::move(device.value()));
    if (bcache_res.is_error()) {
      FX_PLOGS(ERROR, bcache_res.error_value()) << "Could not initialize bcache";
      return bcache_res.take_error();
    }
    auto [bcache, bcache_read_only] = *std::move(bcache_res);

    return configure_(std::move(bcache), ParseMountOptions(request->options, bcache_read_only));
  }());
}

void StartupService::Format(FormatRequestView request, FormatCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    zx::result device = block_client::RemoteBlockDevice::Create(
        fidl::ClientEnd<fuchsia_storage_block::Block>(request->device.TakeChannel()));
    if (device.is_error()) {
      FX_PLOGS(ERROR, device.error_value()) << "Could not initialize block device";
      return device.take_error();
    }
    auto bcache_res = CreateBcache(std::move(device.value()));
    if (bcache_res.is_error()) {
      FX_PLOGS(ERROR, bcache_res.error_value()) << "Could not initialize bcache";
      return bcache_res.take_error();
    }
    auto [bcache, bcache_read_only] = *std::move(bcache_res);
    if (bcache_read_only) {
      FX_LOGS(ERROR) << "Failed to format minfs: read only block device";
      return zx::error(ZX_ERR_BAD_STATE);
    }

    zx::result mkfs_res = Mkfs(ParseFormatOptions(request->options), bcache.get());
    if (mkfs_res.is_error()) {
      FX_PLOGS(ERROR, mkfs_res.error_value()) << "Failed to format minfs";
      return mkfs_res.take_error();
    }
    return zx::ok();
  }());
}

void StartupService::Check(CheckRequestView request, CheckCompleter::Sync& completer) {
  completer.Reply([&]() -> zx::result<> {
    zx::result device = block_client::RemoteBlockDevice::Create(
        fidl::ClientEnd<fuchsia_storage_block::Block>(request->device.TakeChannel()));
    if (device.is_error()) {
      FX_PLOGS(ERROR, device.error_value()) << "Could not initialize block device";
      return device.take_error();
    }
    auto bcache_res = CreateBcache(std::move(device.value()));
    if (bcache_res.is_error()) {
      FX_PLOGS(ERROR, bcache_res.error_value()) << "Could not initialize bcache";
      return bcache_res.take_error();
    }
    auto [bcache, bcache_read_only] = *std::move(bcache_res);

    FsckOptions fsck_options;
    fsck_options.read_only = bcache_read_only;
    fsck_options.repair = !bcache_read_only;
    auto bcache_or = Fsck(std::move(bcache), fsck_options);
    if (bcache_or.is_error()) {
      FX_PLOGS(ERROR, bcache_or.error_value()) << "Consistency check failed for minfs";
      return bcache_or.take_error();
    }
    return zx::ok();
  }());
}

}  // namespace minfs

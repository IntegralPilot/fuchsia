// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/component_runner.h"

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.fs/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/minfs/bcache.h"
#include "src/storage/minfs/minfs_private.h"
#include "src/storage/minfs/mount.h"
#include "src/storage/minfs/service/admin.h"
#include "src/storage/minfs/service/lifecycle.h"
#include "src/storage/minfs/service/startup.h"

namespace minfs {

ComponentRunner::ComponentRunner(async_dispatcher_t* dispatcher, bool die_on_mutation_failure)
    : fs::ManagedVfs(dispatcher),
      dispatcher_(dispatcher),
      die_on_mutation_failure_(die_on_mutation_failure) {
  outgoing_ = fbl::MakeRefCounted<fs::PseudoDir>();
  auto startup = fbl::MakeRefCounted<fs::PseudoDir>();
  outgoing_->AddEntry("startup", startup);

  FX_LOGS(INFO) << "setting up startup service";
  auto startup_svc = fbl::MakeRefCounted<StartupService>(
      dispatcher_, [this](std::unique_ptr<Bcache> device, const MountOptions& options) {
        FX_LOGS(INFO) << "configure callback is called";
        MountOptions modified_options = options;
        modified_options.die_on_mutation_failure = die_on_mutation_failure_;
        zx::result<> status = Configure(std::move(device), modified_options);
        if (status.is_error()) {
          FX_PLOGS(ERROR, status.status_value()) << "Could not configure minfs";
        }
        return status;
      });
  startup->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fs_startup::Startup>, startup_svc);
}

zx::result<> ComponentRunner::ServeRoot(
    fidl::ServerEnd<fuchsia_io::Directory> root,
    fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle) {
  LifecycleServer::Create(
      dispatcher_, [this](fs::FuchsiaVfs::ShutdownCallback cb) { this->Shutdown(std::move(cb)); },
      std::move(lifecycle));

  // Make dangling endpoints for the root directory and the service directory. Creating the
  // endpoints and putting them into the filesystem tree has the effect of queuing incoming
  // requests until the server end of the endpoints is bound.
  auto svc_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (svc_endpoints.is_error()) {
    FX_PLOGS(ERROR, svc_endpoints.status_value())
        << "mount failed; could not create service directory endpoints";
    return svc_endpoints.take_error();
  }
  outgoing_->AddEntry("svc", fbl::MakeRefCounted<fs::RemoteDir>(std::move(svc_endpoints->client)));
  svc_server_end_ = std::move(svc_endpoints->server);
  auto root_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (root_endpoints.is_error()) {
    FX_PLOGS(ERROR, root_endpoints.status_value())
        << "mount failed; could not create root directory endpoints";
    return root_endpoints.take_error();
  }
  outgoing_->AddEntry("root",
                      fbl::MakeRefCounted<fs::RemoteDir>(std::move(root_endpoints->client)));
  root_server_end_ = std::move(root_endpoints->server);

  zx_status_t status = ServeDirectory(outgoing_, std::move(root));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "mount failed; could not serve root directory";
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<> ComponentRunner::Configure(std::unique_ptr<Bcache> bcache,
                                        const MountOptions& options) {
  auto minfs = Minfs::Create(dispatcher_, std::move(bcache), options, this);
  if (minfs.is_error()) {
    FX_PLOGS(ERROR, minfs.status_value()) << "configure failed; could not create minfs";
    return minfs.take_error();
  }
  minfs_ = *std::move(minfs);
  SetReadonly(options.writability != Writability::Writable);

  auto root = minfs_->OpenRootNode();
  if (root.is_error()) {
    FX_PLOGS(ERROR, root.status_value()) << "cannot find root inode";
    return root.take_error();
  }

  zx_status_t status = ServeDirectory(*std::move(root), std::move(root_server_end_));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "configure failed; could not serve root directory";
    return zx::error(status);
  }

  auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  svc_dir->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_fs::Admin>,
      fbl::MakeRefCounted<AdminService>(dispatcher_, [this](fs::FuchsiaVfs::ShutdownCallback cb) {
        this->Shutdown(std::move(cb));
      }));

  status = ServeDirectory(std::move(svc_dir), std::move(svc_server_end_));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "configure failed; could not serve svc dir";
    return zx::error(status);
  }

  return zx::ok();
}

void ComponentRunner::Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) {
  TRACE_DURATION("minfs", "ComponentRunner::Shutdown");
  FX_LOGS(INFO) << "Shutting down";
  {
    std::scoped_lock lock(shutdown_lock_);
    // If the shutdown has already completed, just report it and be done.
    if (shutdown_result_.has_value()) {
      cb(shutdown_result_.value());
      return;
    }
    // Queue up any callbacks to be run at the end.
    shutdown_callbacks_.push_back(std::move(cb));
    // Only if this is the first entry should it actually perform the shutdown.
    if (shutdown_callbacks_.size() > 1) {
      return;
    }
  }

  fs::FuchsiaVfs::ShutdownCallback final_cb = [this](zx_status_t status) {
    std::scoped_lock lock(shutdown_lock_);
    this->shutdown_result_ = status;
    for (auto& cb : this->shutdown_callbacks_) {
      cb(status);
    }
  };

  ManagedVfs::Shutdown([this, cb = std::move(final_cb)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Managed VFS shutdown failed";
    }
    if (minfs_) {
      minfs_->Sync([this, cb = std::move(cb)](zx_status_t sync_status) mutable {
        if (sync_status != ZX_OK) {
          FX_PLOGS(ERROR, sync_status) << "Sync at unmount failed";
        }
        async::PostTask(dispatcher_, [this, cb = std::move(cb)]() mutable {
          std::unique_ptr<Bcache> bc = Minfs::Destroy(std::move(minfs_));
          bc.reset();

          if (on_unmount_) {
            on_unmount_();
          }

          // Tell the unmounting channel that we've completed teardown. This *must* be the last
          // thing we do because after this, the caller can assume that it's safe to destroy the
          // runner.
          cb(ZX_OK);
        });
      });
    } else {
      async::PostTask(dispatcher(), [this, cb = std::move(cb)]() mutable {
        if (on_unmount_) {
          on_unmount_();
        }

        cb(ZX_OK);
      });
    }
  });
}

zx::result<fs::FilesystemInfo> ComponentRunner::GetFilesystemInfo() {
  return minfs_->GetFilesystemInfo();
}

void ComponentRunner::OnNoConnections() {
  if (IsTerminating()) {
    return;
  }
  Shutdown([](zx_status_t status) mutable {
    ZX_ASSERT_MSG(status == ZX_OK, "Filesystem shutdown failed on OnNoConnections(): %s",
                  zx_status_get_string(status));
  });
}

}  // namespace minfs

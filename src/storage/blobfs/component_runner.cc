// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/component_runner.h"

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.fs/cpp/wire.h>
#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <fidl/fuchsia.storage.blobfs/cpp/wire.h>
#include <fidl/fuchsia.update.verify/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/component/cpp/tree_handler_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blob_creator.h"
#include "src/storage/blobfs/blob_reader.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/page_loader.h"
#include "src/storage/blobfs/service/admin.h"
#include "src/storage/blobfs/service/lifecycle.h"
#include "src/storage/blobfs/service/ota_health_check.h"
#include "src/storage/blobfs/service/overwrite_configuration.h"
#include "src/storage/blobfs/service/startup.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {

ComponentRunner::ComponentRunner(async::Loop& loop, ComponentOptions config)
    : fs::PagedVfs(loop.dispatcher(), config.pager_threads), loop_(loop), config_(config) {
  outgoing_ = fbl::MakeRefCounted<fs::PseudoDir>();
  auto startup = fbl::MakeRefCounted<fs::PseudoDir>();
  outgoing_->AddEntry("startup", startup);

  FX_LOGS(INFO) << "setting up services";

  auto startup_svc = fbl::MakeRefCounted<StartupService>(
      loop_.dispatcher(), config_,
      [this](std::unique_ptr<BlockDevice> device, const MountOptions& options) {
        FX_LOGS(INFO) << "configure callback is called";
        zx::result<> status = Configure(std::move(device), options);
        if (status.is_error()) {
          FX_LOGS(ERROR) << "Could not configure blobfs: " << status.status_string();
        }
        return status;
      });
  startup->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fs_startup::Startup>, startup_svc);
}

ComponentRunner::~ComponentRunner() {
  // Inform PagedVfs so that it can stop threads that might call out to blobfs.
  TearDown();
}

void ComponentRunner::Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) {
  {
    std::lock_guard l(shutdown_lock_);
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
    std::lock_guard l(this->shutdown_lock_);
    this->shutdown_result_ = status;
    for (auto& cb : this->shutdown_callbacks_) {
      cb(status);
    }
  };

  // Not including above book keeping to avoid tracing shutdown calls that don't actually do any
  // work.
  TRACE_DURATION("blobfs", "ComponentRunner::Shutdown");
  // Shutdown all external connections to blobfs.
  ManagedVfs::Shutdown([this, cb = std::move(final_cb)](zx_status_t status) mutable {
    async::PostTask(dispatcher(), [this, status, cb = std::move(cb)]() mutable {
      // The threads in the paged vfs' thread pool reference data owned by blobfs. The threads must
      // be stopped before blobfs is destroyed.
      TearDown();
      // Manually destroy the filesystem. The promise of Shutdown is that no
      // connections are active, and destroying the Runner object
      // should terminate all background workers.
      blobfs_ = nullptr;

      // Tell the mounting thread that the filesystem has terminated.
      loop_.Quit();

      // Tell the unmounting channel that we've completed teardown. This *must* be the last thing
      // we do because after this, the caller can assume that it's safe to destroy the runner.
      cb(status);
    });
  });
}

zx::result<fs::FilesystemInfo> ComponentRunner::GetFilesystemInfo() {
  return blobfs_->GetFilesystemInfo();
}

zx::result<> ComponentRunner::ServeRoot(
    fidl::ServerEnd<fuchsia_io::Directory> root,
    fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle) {
  LifecycleServer::Create(
      loop_.dispatcher(),
      [this](fs::FuchsiaVfs::ShutdownCallback cb) {
        FX_LOGS(INFO) << "Lifecycle stop request received.";
        this->Shutdown(std::move(cb));
      },
      std::move(lifecycle));

  // Make dangling endpoints for the root directory and the service directory. Creating the
  // endpoints and putting them into the filesystem tree has the effect of queuing incoming
  // requests until the server end of the endpoints is bound.
  auto svc_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (svc_endpoints.is_error()) {
    FX_LOGS(ERROR) << "mount failed; could not create service directory endpoints";
    return svc_endpoints.take_error();
  }
  outgoing_->AddEntry("svc", fbl::MakeRefCounted<fs::RemoteDir>(std::move(svc_endpoints->client)));
  svc_server_end_ = std::move(svc_endpoints->server);
  auto root_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (root_endpoints.is_error()) {
    FX_LOGS(ERROR) << "mount failed; could not create root directory endpoints";
    return root_endpoints.take_error();
  }
  outgoing_->AddEntry("root",
                      fbl::MakeRefCounted<fs::RemoteDir>(std::move(root_endpoints->client)));
  root_server_end_ = std::move(root_endpoints->server);

  zx_status_t status = ServeDirectory(outgoing_, std::move(root));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "mount failed; could not serve root directory";
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<> ComponentRunner::Configure(std::unique_ptr<BlockDevice> device,
                                        const MountOptions& options) {
  if (auto status = Init(); status.is_error()) {
    FX_LOGS(ERROR) << "configure failed; vfs init failed";
    return status.take_error();
  }

  // All of our pager threads get the deadline profile for scheduling.
  SetDeadlineProfile(GetPagerThreads());

  auto blobfs_or = Blobfs::Create(loop_.dispatcher(), std::move(device), this, options);
  if (blobfs_or.is_error()) {
    FX_LOGS(ERROR) << "configure failed; could not create blobfs: " << blobfs_or.status_string();
    return blobfs_or.take_error();
  }
  blobfs_ = std::move(blobfs_or.value());

  // Decommit memory we don't need committed.
  blobfs_->GetAllocator()->Decommit();

  SetReadonly(blobfs_->writability() != Writability::Writable);

  fbl::RefPtr<fs::Vnode> root;
  zx_status_t status = blobfs_->OpenRootNode(&root);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "configure failed; could not get root blob";
    return zx::error(status);
  }

  status = ServeDirectory(std::move(root), std::move(root_server_end_));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "configure failed; could not serve root directory";
    return zx::error(status);
  }

  // Specify to fall back to DeepCopy mode instead of Live mode (the default) on failures to send
  // a Frozen copy of the tree (e.g. if we could not create a child copy of the backing VMO).
  // This helps prevent any issues with querying the inspect tree while the filesystem is under
  // load, since snapshots at the receiving end must be consistent. See https://fxbug.dev/42135165
  // for details.
  exposed_inspector_.emplace(inspect::ComponentInspector{
      loop_.dispatcher(),
      {.inspector = *blobfs_->GetMetrics()->inspector(),
       .tree_handler_settings = {.snapshot_behavior = inspect::TreeServerSendPreference::Frozen(
                                     inspect::TreeServerSendPreference::Type::DeepCopy)}}});

  auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();

  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_update_verify::ComponentOtaHealthCheck>,
                    fbl::MakeRefCounted<OtaHealthCheckService>(loop_.dispatcher(), *blobfs_));
  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fs::Admin>,
                    fbl::MakeRefCounted<AdminService>(
                        blobfs_->dispatcher(), [this](fs::FuchsiaVfs::ShutdownCallback cb) {
                          FX_LOGS(INFO) << "fs_admin shutdown received.";
                          this->Shutdown(std::move(cb));
                        }));

  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fxfs::BlobReader>,
                    fbl::MakeRefCounted<BlobReader>(*blobfs_));

  svc_dir->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fxfs::BlobCreator>,
                    fbl::MakeRefCounted<BlobCreator>(*blobfs_));

  svc_dir->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_storage_blobfs::OverwriteConfiguration>,
      fbl::MakeRefCounted<OverwriteConfigurationService>(loop_.dispatcher(), *blobfs_));

  status = ServeDirectory(std::move(svc_dir), std::move(svc_server_end_));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "configure failed; could not serve svc dir";
    return zx::error(status);
  }

  return zx::ok();
}

}  // namespace blobfs

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/component_runner.h"

#include <fidl/fuchsia.fs.startup/cpp/wire.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/inspect.h"
#include "src/storage/f2fs/mount.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/reader.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/service/admin.h"
#include "src/storage/f2fs/service/lifecycle.h"
#include "src/storage/f2fs/service/startup.h"
#include "src/storage/f2fs/storage_buffer.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/f2fs/vnode_cache.h"
#include "src/storage/f2fs/writeback.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"

namespace f2fs {

zx::result<std::unique_ptr<ComponentRunner>> ComponentRunner::CreateRunner(
    FuchsiaDispatcher dispatcher) {
  std::unique_ptr<ComponentRunner> runner(new ComponentRunner(dispatcher));
  // Create Pager and PagerPool
  if (auto status = runner->Init(); status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(runner));
}

ComponentRunner::ComponentRunner(async_dispatcher_t* dispatcher)
    : fs::PagedVfs(dispatcher), dispatcher_(dispatcher) {
  outgoing_ = fbl::MakeRefCounted<fs::PseudoDir>();
  auto startup = fbl::MakeRefCounted<fs::PseudoDir>();
  outgoing_->AddEntry("startup", startup);

  FX_LOGS(INFO) << "setting up startup service";
  auto startup_svc = fbl::MakeRefCounted<StartupService>(
      dispatcher_, [this](std::unique_ptr<BcacheMapper> device, const MountOptions& options) {
        return Configure(std::move(device), options);
      });
  startup->AddEntry(fidl::DiscoverableProtocolName<fuchsia_fs_startup::Startup>, startup_svc);
}

ComponentRunner::~ComponentRunner() {
  // Inform PagedVfs so that it can stop threads that might call out to f2fs.
  TearDown();
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

zx::result<> ComponentRunner::Configure(std::unique_ptr<BcacheMapper> bcache,
                                        const MountOptions& options) {
  // Create Pager and PagerPool
  if (auto status = Init(); status.is_error()) {
    return status.take_error();
  }

  zx::result readonly = options.GetValue(MountOption::kReadOnly);
  SetReadonly(*readonly != 0);

  auto f2fs = F2fs::Create(dispatcher_, std::move(bcache), options, this);
  if (f2fs.is_error()) {
    FX_LOGS(ERROR) << "configure failed; could not create f2fs: " << f2fs.status_string();
    return f2fs.take_error();
  }
  f2fs_ = *std::move(f2fs);

  auto root_vnode = f2fs_->GetRootVnode();
  ZX_DEBUG_ASSERT(root_vnode.is_ok());

  zx_status_t status = ServeDirectory(std::move(*root_vnode), std::move(root_server_end_));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "configure failed; could not serve root directory: "
                   << zx_status_get_string(status);
    return zx::error(status);
  }

  f2fs_->GetInspectTree().Initialize();

  // Specify to fall back to DeepCopy mode instead of Live mode (the default) on failures to send
  // a Frozen copy of the tree (e.g. if we could not create a child copy of the backing VMO).
  // This helps prevent any issues with querying the inspect tree while the filesystem is under
  // load, since snapshots at the receiving end must be consistent. See https://fxbug.dev/42135165
  // for details.
  exposed_inspector_.emplace(inspect::ComponentInspector{
      dispatcher_,
      {
          .inspector = f2fs_->GetInspectTree().GetInspector(),
          .tree_handler_settings = {.snapshot_behavior = inspect::TreeServerSendPreference::Frozen(
                                        inspect::TreeServerSendPreference::Type::DeepCopy)},
      }});

  auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  svc_dir->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_fs::Admin>,
      fbl::MakeRefCounted<AdminService>(dispatcher_, [this](fs::FuchsiaVfs::ShutdownCallback cb) {
        this->Shutdown(std::move(cb));
      }));

  status = ServeDirectory(std::move(svc_dir), std::move(svc_server_end_));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "configure failed; could not serve svc dir: " << zx_status_get_string(status);
    return zx::error(status);
  }

  return zx::ok();
}

void ComponentRunner::Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) {
  TRACE_DURATION("f2fs", "ComponentRunner::Shutdown");
  fs::FuchsiaVfs::ShutdownCallback to_call;
  zx_status_t cached_result = ZX_OK;
  {
    std::scoped_lock lock(shutdown_lock_);
    // If the shutdown has already completed, just report it and be done.
    if (is_shutdown_) {
      to_call = std::move(cb);
      cached_result = shutdown_result_.status_value();
    } else {
      // Queue up any callbacks to be run at the end.
      shutdown_callbacks_.push_back(std::move(cb));
      // Only if this is the first entry should it actually perform the shutdown.
      if (shutdown_callbacks_.size() > 1) {
        return;
      }
    }
  }
  if (to_call) {
    to_call(cached_result);
    return;
  }

  fs::FuchsiaVfs::ShutdownCallback final_cb = [this](zx_status_t status) {
    std::vector<fs::FuchsiaVfs::ShutdownCallback> callbacks;
    {
      std::scoped_lock lock(shutdown_lock_);
      is_shutdown_ = true;
      shutdown_result_ = zx::make_result(status);
      callbacks = std::move(shutdown_callbacks_);
    }
    for (auto& cb : callbacks) {
      cb(status);
    }
  };

  ManagedVfs::Shutdown([this, cb = std::move(final_cb)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Managed VFS shutdown failed";
    }
    async::PostTask(dispatcher(), [this, status, cb = std::move(cb)]() mutable {
      if (f2fs_) {
        f2fs_->Sync([this](zx_status_t) mutable {
          TearDown();
          f2fs_->PutSuper();
        });
        // F2fs has a BcacheMapper which has a VmoBuffer which has a Vmoid. When the VmoBuffer is
        // destructed it tries to detach the Vmoid from the block device. F2fs must be destructed
        // before responding to the Shutdown request to ensure that the block device still exists
        // for these operations.
        f2fs_.reset();
      }
      if (on_unmount_) {
        on_unmount_();
      }

      // Tell the unmounting channel that we've completed teardown. This *must* be the last
      // thing we do because after this, the caller can assume that it's safe to destroy the
      // runner.
      cb(status);
    });
  });
}

zx::result<fs::FilesystemInfo> ComponentRunner::GetFilesystemInfo() {
  return f2fs_->GetFilesystemInfo();
}

void ComponentRunner::OnNoConnections() {
  if (IsTerminating()) {
    return;
  }
  Shutdown([](zx_status_t status) mutable {
    ZX_ASSERT_MSG(status == ZX_OK, "shutdown failed on OnNoConnections(): %s",
                  zx_status_get_string(status));
  });
}

}  // namespace f2fs

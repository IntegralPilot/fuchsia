// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MINFS_COMPONENT_RUNNER_H_
#define SRC_STORAGE_MINFS_COMPONENT_RUNNER_H_

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <optional>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/minfs/bcache.h"
#include "src/storage/minfs/minfs_private.h"
#include "src/storage/minfs/mount.h"

namespace minfs {

class ComponentRunner final : public fs::ManagedVfs {
 public:
  explicit ComponentRunner(async_dispatcher_t* dispatcher, bool die_on_mutation_failure = true);

  ComponentRunner(const ComponentRunner&) = delete;
  ComponentRunner& operator=(const ComponentRunner&) = delete;

  zx::result<> ServeRoot(fidl::ServerEnd<fuchsia_io::Directory> root,
                         fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle);
  zx::result<> Configure(std::unique_ptr<Bcache> bcache, const MountOptions& options);

  // fs::ManagedVfs interface
  void Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) final;
  zx::result<fs::FilesystemInfo> GetFilesystemInfo() final;
  void OnNoConnections() final;

  void SetUnmountCallback(fit::closure on_unmount) { on_unmount_ = std::move(on_unmount); }

 private:
  async_dispatcher_t* dispatcher_;
  fit::closure on_unmount_;
  bool die_on_mutation_failure_;

  // These are initialized when ServeRoot is called.
  fbl::RefPtr<fs::PseudoDir> outgoing_;

  // These are created when ServeRoot is called, and are consumed by a successful call to
  // Configure. This causes any incoming requests to queue in the channel pair until we start
  // serving the directories, after we start the filesystem and the services.
  fidl::ServerEnd<fuchsia_io::Directory> svc_server_end_;
  fidl::ServerEnd<fuchsia_io::Directory> root_server_end_;

  // These are only initialized by configure after a call to the startup service.
  std::unique_ptr<Minfs> minfs_;

  std::mutex shutdown_lock_;
  // The result of the attempted shutdown, to be presented to any late shutdown request arrivals.
  std::optional<zx_status_t> shutdown_result_ __TA_GUARDED(shutdown_lock_);
  // A queue of callbacks for shutdown requests that arrive while shutdown is running.
  std::vector<fs::FuchsiaVfs::ShutdownCallback> shutdown_callbacks_ __TA_GUARDED(shutdown_lock_);
};

}  // namespace minfs

#endif  // SRC_STORAGE_MINFS_COMPONENT_RUNNER_H_

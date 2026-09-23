// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_COMPONENT_RUNNER_H_
#define SRC_STORAGE_BLOBFS_COMPONENT_RUNNER_H_

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <optional>
#include <vector>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

namespace blobfs {

// The Runner class *has* to be final because it calls PagedVfs::TearDown from
// its destructor which is required to ensure thread-safety at destruction time.
class ComponentRunner final : public fs::PagedVfs {
 public:
  ComponentRunner(async::Loop& loop, ComponentOptions config);

  ComponentRunner(const ComponentRunner&) = delete;
  ComponentRunner& operator=(const ComponentRunner&) = delete;

  ~ComponentRunner();

  // fs::PagedVfs interface.
  void Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) final;
  zx::result<fs::FilesystemInfo> GetFilesystemInfo() final;

  zx::result<> ServeRoot(fidl::ServerEnd<fuchsia_io::Directory> root,
                         fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle);
  zx::result<> Configure(std::unique_ptr<BlockDevice> device, const MountOptions& options);

 private:
  async::Loop& loop_;
  ComponentOptions config_;

  // These are initialized when ServeRoot is called.
  fbl::RefPtr<fs::PseudoDir> outgoing_;

  // These are created when ServeRoot is called, and are consumed by a successful call to
  // Configure. This causes any incoming requests to queue in the channel pair until we start
  // serving the directories, after we start the filesystem and the services.
  fidl::ServerEnd<fuchsia_io::Directory> svc_server_end_;
  fidl::ServerEnd<fuchsia_io::Directory> root_server_end_;

  // These are only initialized by configure after a call to the startup service.
  std::unique_ptr<Blobfs> blobfs_;

  std::mutex shutdown_lock_;
  // The result of the attempted shutdown, to be presented to any late shutdown request arrivals.
  std::optional<zx_status_t> shutdown_result_ __TA_GUARDED(shutdown_lock_);
  // A queue of callbacks for shutdown requests that arrive while shutdown is running.
  std::vector<fs::FuchsiaVfs::ShutdownCallback> shutdown_callbacks_ __TA_GUARDED(shutdown_lock_);

  std::optional<inspect::ComponentInspector> exposed_inspector_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_COMPONENT_RUNNER_H_

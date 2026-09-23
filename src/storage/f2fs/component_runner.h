// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_COMPONENT_RUNNER_H_
#define SRC_STORAGE_F2FS_COMPONENT_RUNNER_H_

#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/inspect/component/cpp/component.h>
#include <zircon/compiler.h>

#include <mutex>
#include <vector>

#include "src/storage/f2fs/common.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

namespace f2fs {

class F2fs;
class BcacheMapper;
class MountOptions;

class ComponentRunner final : public PlatformVfs {
 public:
  explicit ComponentRunner(async_dispatcher_t* dispatcher);

  ComponentRunner(const ComponentRunner&) = delete;
  ComponentRunner& operator=(const ComponentRunner&) = delete;

  virtual ~ComponentRunner();

  // for unit tests
  static zx::result<std::unique_ptr<ComponentRunner>> CreateRunner(FuchsiaDispatcher dispatcher);

  zx::result<> ServeRoot(fidl::ServerEnd<fuchsia_io::Directory> root,
                         fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle);
  zx::result<> Configure(std::unique_ptr<BcacheMapper> bcache, const MountOptions& options);

  // fs::PagedVfs interface
  void Shutdown(fs::FuchsiaVfs::ShutdownCallback cb) final;
  zx::result<fs::FilesystemInfo> GetFilesystemInfo() final;
  void OnNoConnections() final;

  void SetUnmountCallback(fit::closure on_unmount) { on_unmount_ = std::move(on_unmount); }

 private:
  async_dispatcher_t* dispatcher_;
  fit::closure on_unmount_;

  // These are initialized when ServeRoot is called.
  fbl::RefPtr<fs::PseudoDir> outgoing_;

  // These are created when ServeRoot is called, and are consumed by a successful call to
  // Configure. This causes any incoming requests to queue in the channel pair until we start
  // serving the directories, after we start the filesystem and the services.
  fidl::ServerEnd<fuchsia_io::Directory> svc_server_end_;
  fidl::ServerEnd<fuchsia_io::Directory> root_server_end_;

  // These are only initialized by configure after a call to the startup service.
  std::unique_ptr<F2fs> f2fs_;

  std::optional<inspect::ComponentInspector> exposed_inspector_ = {};

  std::mutex shutdown_lock_;
  bool is_shutdown_ __TA_GUARDED(shutdown_lock_) = false;
  zx::result<> shutdown_result_ __TA_GUARDED(shutdown_lock_) = zx::ok();
  // A queue of callbacks for shutdown requests that arrive while shutdown is running.
  std::vector<fs::FuchsiaVfs::ShutdownCallback> shutdown_callbacks_ __TA_GUARDED(shutdown_lock_);
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_COMPONENT_RUNNER_H_

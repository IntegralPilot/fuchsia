// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <string>
#include <utility>

#include "src/storage/fs_test/fs_test.h"
#include "src/storage/lib/fs_management/cpp/mount.h"
#include "src/storage/lib/fs_management/cpp/options.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_dir.h"  // IWYU pragma: keep

namespace {

class MemfsInstance : public fs_test::FilesystemInstance {
 public:
  MemfsInstance() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}
  ~MemfsInstance() override { Shutdown(); }

  zx::result<> Format(const fs_test::TestFilesystemOptions&) override {
    Shutdown();

    zx::result result = memfs::Memfs::Create(loop_.dispatcher(), "<tmp>");
    if (result.is_error()) {
      return result.take_error();
    }
    auto& [memfs, root] = result.value();

    zx::result server = fidl::CreateEndpoints(&root_);
    if (server.is_error()) {
      return server.take_error();
    }

    if (zx_status_t status = memfs->ServeDirectory(std::move(root), std::move(server.value()));
        status != ZX_OK) {
      return zx::error(status);
    }
    if (zx_status_t status = loop_.StartThread(); status != ZX_OK) {
      return zx::error(status);
    }
    memfs_ = std::move(memfs);
    return zx::ok();
  }

  zx::result<> Mount(const std::string& mount_path,
                     const fs_management::MountOptions& options) override {
    if (!root_) {
      // Already mounted.
      return zx::error(ZX_ERR_BAD_STATE);
    }

    fdio_ns_t* ns;
    if (auto status = zx::make_result(fdio_ns_get_installed(&ns)); status.is_error()) {
      return status;
    }
    return zx::make_result(fdio_ns_bind(ns, fs_test::StripTrailingSlash(mount_path).c_str(),
                                        root_.TakeChannel().release()));
  }

  zx::result<> Unmount(const std::string& mount_path) override {
    return fs_test::FsUnbind(mount_path);
  }

  zx::result<> Fsck() override { return zx::ok(); }

  fs_management::SingleVolumeFilesystemInterface* fs() override { return nullptr; }

  std::string GetMoniker() const override {
    ZX_ASSERT_MSG(false, "GetMoniker unimplemented for MemfsInstance");
    return "";
  }

 private:
  void Shutdown() {
    loop_.Quit();
    loop_.JoinThreads();
    zx_status_t status = loop_.ResetQuit();
    ZX_ASSERT_MSG(status == ZX_OK, "%s", zx_status_get_string(status));
  }

  fidl::ClientEnd<fuchsia_io::Directory> root_;
  std::unique_ptr<memfs::Memfs> memfs_;
  async::Loop loop_;
};

class MemfsFilesystem : public fs_test::FilesystemImpl<MemfsFilesystem> {
 public:
  zx::result<std::unique_ptr<fs_test::FilesystemInstance>> Make(
      const fs_test::TestFilesystemOptions& options) const override {
    auto instance = std::make_unique<MemfsInstance>();
    zx::result<> status = instance->Format(options);
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(std::move(instance));
  }
  const Traits& GetTraits() const override {
    static Traits traits{
        .in_memory = true,
        .is_case_sensitive = true,
        .is_journaled = false,
        .name = "memfs",
        .supports_hard_links = true,
        .supports_mmap = true,
        .supports_mmap_shared_write = true,
        .supports_resize = false,
        .supports_sparse_files = true,
        .supports_watch_event_deleted = false,
        .timestamp_granularity = zx::nsec(1),
    };
    return traits;
  }
};

}  // namespace

__EXPORT std::unique_ptr<fs_test::Filesystem> GetFilesystem() {
  return std::make_unique<MemfsFilesystem>();
}

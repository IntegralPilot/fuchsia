// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_SCOPED_MEMFS_MANAGER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_SCOPED_MEMFS_MANAGER_H_

#include <fcntl.h>
#include <lib/fdio/namespace.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <string>
#include <unordered_map>

#include <fbl/unique_fd.h>

#include "src/lib/files/scoped_temp_dir.h"

namespace forensics::testing {

// Manages creating and destroying directories in the calling processes namespace.
class ScopedMemFsManager {
 public:
  ScopedMemFsManager() {
    tmp_storage_.reset(open("/tmp", O_RDONLY | O_DIRECTORY));
    FX_CHECK(tmp_storage_.is_valid());
    FX_CHECK(fdio_ns_get_installed(&ns_) == ZX_OK);
  }

  ~ScopedMemFsManager() {
    for (auto& [path, binding] : bindings_) {
      FX_CHECK(fdio_ns_unbind(ns_, path.c_str()) == ZX_OK);
      binding.reset();
      if (path == "/tmp") {
        // Rebind the real /tmp if it was unbound.
        FX_CHECK(fdio_ns_bind_fd(ns_, "/tmp", tmp_storage_.get()) == ZX_OK);
      }
    }
  }

  bool Contains(const std::string& path) const { return bindings_.contains(path); }

  // Create a directory at |path| in the component's namespace.
  void Create(const std::string& path) {
    FX_CHECK(!Contains(path));

    if (path == "/tmp") {
      // /tmp already exists in the namespace and backs all of the temporary directories. In order
      // to provide a scoped /tmp, the main /tmp needs to be unbound from the namespace.
      // `tmp_storage_` preserves a handle to the main /tmp and allows it to be bound again in the
      // destructor.
      FX_CHECK(fdio_ns_unbind(ns_, "/tmp") == ZX_OK);
    }

    auto temp_dir = std::make_unique<files::ScopedTempDirAt>(tmp_storage_.get());
    fbl::unique_fd temp_dir_fd(
        openat(temp_dir->root_fd(), temp_dir->path().c_str(), O_RDONLY | O_DIRECTORY));
    FX_CHECK(temp_dir_fd.is_valid());

    FX_CHECK(fdio_ns_bind_fd(ns_, path.c_str(), temp_dir_fd.get()) == ZX_OK);

    bindings_.emplace(path, std::move(temp_dir));
  }

 private:
  fbl::unique_fd tmp_storage_;
  fdio_ns_t* ns_;
  std::unordered_map<std::string, std::unique_ptr<files::ScopedTempDirAt>> bindings_;
};

}  // namespace forensics::testing

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_SCOPED_MEMFS_MANAGER_H_

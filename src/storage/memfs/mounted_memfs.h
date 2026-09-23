// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_MOUNTED_MEMFS_H_
#define SRC_STORAGE_MEMFS_MOUNTED_MEMFS_H_

#include <lib/async/dispatcher.h>
#include <lib/fdio/namespace.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <memory>
#include <string>
#include <utility>

#include "src/storage/memfs/memfs.h"

// A wrapper around memfs that automatically mounts and unmounts itself at a given path.
class MountedMemfs {
 public:
  MountedMemfs(MountedMemfs&&) = default;
  ~MountedMemfs() { [[maybe_unused]] zx_status_t status = fdio_ns_unbind(ns_, path_.c_str()); }

  const std::unique_ptr<memfs::Memfs>& operator->() const { return memfs_; }

  // Create and automatically mount a memfs instance at the given path in the local namespace. This
  // will be unmounted on cleanup.
  //
  // Memfs will run on the given dispatcher.
  static zx::result<MountedMemfs> Create(async_dispatcher_t* dispatcher, const char* path);

 private:
  MountedMemfs(std::unique_ptr<memfs::Memfs> memfs, fdio_ns_t* ns, const char* path)
      : memfs_(std::move(memfs)), ns_(ns), path_(path) {}

  std::unique_ptr<memfs::Memfs> memfs_;
  fdio_ns_t* ns_;
  std::string path_;
};

#endif  // SRC_STORAGE_MEMFS_MOUNTED_MEMFS_H_

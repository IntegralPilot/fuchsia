// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_REMOTE_DIR_H_
#define LIB_VFS_CPP_REMOTE_DIR_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/vfs/cpp/node.h>
#include <lib/zx/channel.h>

namespace vfs {

// A remote directory holds a channel to a remotely hosted directory to which requests are delegated
// when opened.
//
// This class is designed to allow programs to publish remote filesystems as directories without
// requiring a separate "mount" step.  In effect, a remote directory is "mounted" at creation time.
//
// It is not possible for the client to detach the remote directory or to mount a new one in its
// place.
//
// This class is thread-safe.
class RemoteDir final : public Node {
 public:
  // Binds to a remotely hosted directory channel via `remote_dir`. The channel must be valid and
  // must be compatible with the `fuchsia.io.Directory` protocol.
  explicit RemoteDir(zx::channel remote_dir) : Node(CreateRemoteDir(std::move(remote_dir))) {}

  // Binds to a remotely hosted directory using the specified `directory`. The handle must be valid.
  explicit RemoteDir(fidl::ClientEnd<fuchsia_io::Directory> directory)
      : RemoteDir(directory.TakeChannel()) {}

  // * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * *
  // Deprecated HLCPP Signatures
  // * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * *

  // Binds to a remotely hosted directory using the specified `dir`. The `dir` handle must be valid.
  // TODO(https://fxbug.dev/408237874): Mark this as removed at NEXT when we ship API level 29.
  explicit RemoteDir(fidl::InterfaceHandle<fuchsia::io::Directory> dir)
      ZX_DEPRECATED_SINCE(1, 29, "Use LLCPP signature instead.")
      : RemoteDir(dir.TakeChannel()) {}

 private:
  static vfs_internal_node_t* CreateRemoteDir(zx::channel dir) {
    vfs_internal_node_t* remote;
    ZX_ASSERT(vfs_internal_remote_directory_create(dir.release(), &remote) == ZX_OK);
    return remote;
  }
};

}  // namespace vfs

#endif  // LIB_VFS_CPP_REMOTE_DIR_H_

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_REMOTE_DIR_H_
#define SRC_STORAGE_LIB_VFS_CPP_REMOTE_DIR_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <zircon/availability.h>

#include <fbl/macros.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

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
class RemoteDir : public Vnode {
 public:
  // Construct with fbl::MakeRefCounted.

  fidl::UnownedClientEnd<fuchsia_io::Directory> client_end() const { return remote_client_; }

  // |Vnode| implementation:
  fuchsia_io::NodeProtocolKinds GetProtocols() const final;
  bool IsRemote() const final;
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  void DeprecatedOpenRemote(fuchsia_io::OpenFlags, fuchsia_io::ModeType, fidl::StringView,
                            fidl::ServerEnd<fuchsia_io::Node>) const final;
#endif
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  void OpenRemote(fuchsia_io::wire::OpenableOpenRequest request) const final;
#else
  void OpenRemote(fuchsia_io::wire::DirectoryOpenRequest request) const final;
#endif

 protected:
  friend fbl::internal::MakeRefCountedHelper<RemoteDir>;
  friend fbl::RefPtr<RemoteDir>;

  // Binds to a remotely hosted directory using the specified FIDL client channel endpoint.  The
  // channel must be valid.
  explicit RemoteDir(fidl::ClientEnd<fuchsia_io::Directory> remote_dir_client);

  // Releases the remotely hosted directory.
  ~RemoteDir() override;

 private:
  fidl::ClientEnd<fuchsia_io::Directory> const remote_client_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(RemoteDir);
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_REMOTE_DIR_H_

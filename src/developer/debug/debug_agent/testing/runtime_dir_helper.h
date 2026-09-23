// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TESTING_RUNTIME_DIR_HELPER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TESTING_RUNTIME_DIR_HELPER_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include "fbl/ref_ptr.h"
#include "fidl/fuchsia.io/cpp/fidl.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

namespace testing {

// A class that can emulate multiple component runtime directories by constructing a flat namespace
// of directories named by given job koids, which will each contain a nested file at the relative
// path elf/job_id, which contains the same job koid as the parent directory, but is readable by
// entities expecting to have a handle to the "root" of a component's namespaced runtime directory.
//
// Tests may use this to emulate any number of mocked component runtime directories. All directories
// that are to be serviced by this helper must be created before calling |Start|.
class RuntimeDirHelper {
 public:
  RuntimeDirHelper()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        vfs_(loop_.dispatcher()),
        root_dir_(fbl::MakeRefCounted<fs::PseudoDir>()) {}
  ~RuntimeDirHelper();

  // Starts serving the VFS instance on a separate thread. |client_dispatcher| will service the
  // client end of the connection. Any calls that attempt to add a new directory after calling this
  // method will assert.
  void Start(async_dispatcher_t* client_dispatcher);

  // Cleans up the VFS instance and shuts down the dedicated message loop thread.
  void Cleanup();

  // Adds a new top-level directory for |job|, containing a file with a path "elf/job_id" containing
  // the same job koid.
  void AddJobIdFile(zx_koid_t job);

  fidl::ClientEnd<fuchsia_io::Directory> GetScopedDirectoryHandle(zx_koid_t job);

 private:
  async::Loop loop_;
  fs::ManagedVfs vfs_;

  // The root directory served by |vfs_|. Consumed when |Start| is called.
  fbl::RefPtr<fs::PseudoDir> root_dir_;
  fidl::Client<fuchsia_io::Directory> root_dir_client_;
};

}  // namespace testing

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TESTING_RUNTIME_DIR_HELPER_H_

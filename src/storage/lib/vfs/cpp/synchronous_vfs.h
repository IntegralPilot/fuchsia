// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_SYNCHRONOUS_VFS_H_
#define SRC_STORAGE_LIB_VFS_CPP_SYNCHRONOUS_VFS_H_

#ifndef __Fuchsia__
#error "Fuchsia-only header"
#endif

#include <lib/async/dispatcher.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <memory>

#include <fbl/intrusive_double_list.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

// A specialization of |FuchsiaVfs| which can be dropped.
//
// During destruction connections are destroyed asynchronously, so they will outlast the VFS.
//
// This class is NOT thread-safe and it must be used with a single-threaded asynchronous dispatcher.
//
// This class is final because of its destructor and the semi-complex shutdown behaviour it relies
// on that might not work if the instance is partially destroyed (which would be the case if when
// the SynchronousVfs destructor is running for a derived instance).
class SynchronousVfs final : public FuchsiaVfs {
 public:
  explicit SynchronousVfs(async_dispatcher_t* dispatcher = nullptr);
  ~SynchronousVfs() override;

  // FuchsiaVfs overrides.
  void CloseAllConnectionsForVnode(const Vnode& node,
                                   CloseAllConnectionsForVnodeCallback callback) final;

 private:
  void Shutdown(ShutdownCallback handler) override;

  // On success, consumes |channel|.
  zx::result<> RegisterConnection(std::unique_ptr<internal::Connection> connection,
                                  zx::channel& channel) final;

  fbl::DoublyLinkedList<internal::Connection*> connections_ __TA_GUARDED(vfs_lock_);
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_SYNCHRONOUS_VFS_H_

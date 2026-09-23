// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

#include <lib/async/dispatcher.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <memory>
#include <mutex>
#include <utility>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

SynchronousVfs::SynchronousVfs(async_dispatcher_t* dispatcher) : FuchsiaVfs(dispatcher) {}

SynchronousVfs::~SynchronousVfs() { Shutdown(nullptr); }

void SynchronousVfs::Shutdown(ShutdownCallback handler) {
  // Arrange for connections to be unbound asynchronously.
  {
    std::lock_guard lock(vfs_lock_);
    for (internal::Connection& connection : connections_) {
      connection.Unbind();
    }
    connections_.clear();
    WillDestroy();
  }

  WaitTillDone();

  if (handler)
    handler(ZX_OK);
}

void SynchronousVfs::CloseAllConnectionsForVnode(const Vnode& node,
                                                 CloseAllConnectionsForVnodeCallback callback) {
  {
    std::lock_guard lock(vfs_lock_);
    for (internal::Connection& connection : connections_) {
      if (connection.vnode().get() == &node) {
        connection.Unbind();
      }
    }
  }
  if (callback) {
    callback();
  }
}

zx::result<> SynchronousVfs::RegisterConnection(std::unique_ptr<internal::Connection> connection,
                                                zx::channel& channel) {
  std::lock_guard lock(vfs_lock_);
  if (IsTerminating()) {
    return zx::error(ZX_ERR_CANCELED);
  }
  connections_.push_back(connection.release());
  connections_.back().Bind(std::move(channel), [](internal::Connection* connection) {
    auto vfs = connection->vfs();
    if (vfs) {
      SynchronousVfs* sync_vfs = static_cast<SynchronousVfs*>(vfs.get());
      std::lock_guard lock(sync_vfs->vfs_lock_);
      if (!vfs->IsTerminating())
        sync_vfs->connections_.erase(*connection);
    }
    delete connection;
  });

  return zx::ok();
}

}  // namespace fs

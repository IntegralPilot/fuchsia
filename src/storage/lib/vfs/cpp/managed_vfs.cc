// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/managed_vfs.h"

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <functional>
#include <memory>
#include <mutex>
#include <utility>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

ManagedVfs::ManagedVfs(async_dispatcher_t* dispatcher) : FuchsiaVfs(dispatcher) {
  ZX_DEBUG_ASSERT(dispatcher);
}

ManagedVfs::~ManagedVfs() { ZX_DEBUG_ASSERT(connections_.is_empty()); }

bool ManagedVfs::NoMoreClients() const { return IsTerminating() && connections_.is_empty(); }

// Asynchronously drop all connections.
void ManagedVfs::Shutdown(ShutdownCallback handler) {
  ZX_DEBUG_ASSERT(handler);
  ZX_DEBUG_ASSERT(!IsTerminating());
  zx_status_t status =
      async::PostTask(dispatcher(), [this, closure = std::move(handler)]() mutable {
        std::lock_guard lock(lock_);
        ZX_DEBUG_ASSERT(!shutdown_handler_);
        shutdown_handler_ = std::move(closure);
        WillDestroy();

        // Signal the teardown on channels in a way that doesn't potentially pull them out from
        // underneath async callbacks.
        std::for_each(connections_.begin(), connections_.end(),
                      std::mem_fn(&internal::Connection::Unbind));

        MaybeAsyncFinishShutdown();
      });
  ZX_DEBUG_ASSERT_MSG(status == ZX_OK, "status = %s", zx_status_get_string(status));
}

void ManagedVfs::CloseAllConnectionsForVnode(const Vnode& node,
                                             CloseAllConnectionsForVnodeCallback callback) {
  async::PostTask(dispatcher(), [this, &node, callback = std::move(callback)]() mutable {
    // Each connection to the Vnode takes a copy of the shared pointer of the AutoCall.  When a
    // connection is finished closing, |UnregisterConnection| will drop the copy and when the last
    // copy is dropped the AutoCall will call |callback|.
    auto closer = std::make_shared<fit::deferred_action<fit::callback<void()>>>(
        [callback = std::move(callback)]() mutable {
          if (callback) {
            callback();
          }
        });  // Must go before |lock|.
    std::lock_guard lock(lock_);
    for (internal::Connection& connection : connections_) {
      if (connection.vnode().get() == &node) {
        connection.Unbind();
        closing_connections_.emplace(&connection, closer);
      }
    }
    // |closer| will call the callback here if no connections needed closing.
  });
}

// Trigger "OnShutdownComplete" if all preconditions have been met.
void ManagedVfs::MaybeAsyncFinishShutdown() {
  if (NoMoreClients()) {
    shutdown_task_.Post(dispatcher());
  }
}

void ManagedVfs::FinishShutdown(async_dispatcher_t*, async::TaskBase*,
                                zx_status_t dispatcher_status) {
  // Call the shutdown function outside of the lock since it can cause |this| to be deleted which
  // will in turn delete the lock object itself.
  ShutdownCallback handler;
  {
    std::lock_guard lock(lock_);
    ZX_ASSERT_MSG(NoMoreClients(), "Failed to complete VFS shutdown: dispatcher status = %d\n",
                  dispatcher_status);
    ZX_DEBUG_ASSERT(shutdown_handler_);
    handler = std::move(shutdown_handler_);
  }

  handler(ZX_OK);
  // |this| can be deleted at this point!
}

zx::result<> ManagedVfs::RegisterConnection(std::unique_ptr<internal::Connection> connection,
                                            zx::channel& channel) {
  std::lock_guard lock(lock_);
  if (IsTerminating()) {
    return zx::error(ZX_ERR_CANCELED);
  }
  connections_.push_back(connection.release());
  connections_.back().Bind(std::move(channel), [this](internal::Connection* connection) {
    std::shared_ptr<fit::deferred_action<fit::callback<void()>>> closer;  // Must go before lock.
    std::lock_guard lock(lock_);

    auto iter = closing_connections_.find(connection);
    if (iter != closing_connections_.end()) {
      closer = iter->second;
      closing_connections_.erase(iter);
    }

    connections_.erase(*connection);
    delete connection;

    MaybeAsyncFinishShutdown();

    if (connections_.is_empty()) {
      OnNoConnections();
    }

    // |closer| will call the callback here if it's the last connection to be closed.
  });
  return zx::ok();
}

}  // namespace fs

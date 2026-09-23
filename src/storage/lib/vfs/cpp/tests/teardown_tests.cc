// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_messaging.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace {

class FdCountVnode : public fs::Vnode {
 public:
  FdCountVnode() = default;
  ~FdCountVnode() override {
    std::lock_guard lock(mutex_);
    EXPECT_EQ(open_count(), 0u);
  }

  size_t fds() const {
    std::lock_guard lock(mutex_);
    return open_count();
  }

  fuchsia_io::NodeProtocolKinds GetProtocols() const final {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }
};

struct AsyncTearDownSync {
  libsync::Completion sync_thread_start;
  libsync::Completion sync_may_proceed;
  libsync::Completion sync_vnode_destroyed;
};

class AsyncTearDownVnode : public FdCountVnode {
 public:
  explicit AsyncTearDownVnode(AsyncTearDownSync& completions, zx_status_t status_for_sync = ZX_OK)
      : completions_(completions), status_for_sync_(status_for_sync) {}

  ~AsyncTearDownVnode() override {
    // C) Tear down the Vnode.
    EXPECT_EQ(fds(), 0u);
    if (thread_.has_value()) {
      thread_.value().join();
    }
    completions_.sync_vnode_destroyed.Signal();
  }

 private:
  void Sync(fs::Vnode::SyncCallback callback) final {
    thread_.emplace([&completions = this->completions_, callback = std::move(callback),
                     status_for_sync = status_for_sync_]() mutable {
      // A) Identify when the sync has started being processed.
      completions.sync_thread_start.Signal();
      // B) Wait until the connection has been closed.
      completions.sync_may_proceed.Wait();
      callback(status_for_sync);
    });
  }

  std::optional<std::thread> thread_;
  AsyncTearDownSync& completions_;
  zx_status_t status_for_sync_;
};

// Helper function which creates a VFS with a served Vnode, starts a sync request, and then closes
// the connection to the client in the middle of the async callback.
//
// This helps tests get ready to try handling a tricky teardown.
void SyncStart(AsyncTearDownSync& completions, async::Loop* loop,
               std::unique_ptr<fs::ManagedVfs>* vfs, zx_status_t status_for_sync = ZX_OK) {
  *vfs = std::make_unique<fs::ManagedVfs>(loop->dispatcher());
  ASSERT_EQ(loop->StartThread(), ZX_OK);

  auto vn = fbl::AdoptRef(new AsyncTearDownVnode(completions, status_for_sync));
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Node>::Create();
  ASSERT_EQ((*vfs)->Serve(vn, server_end.TakeChannel(), {}), ZX_OK);
  vn = nullptr;

  fidl::WireClient client(std::move(client_end), loop->dispatcher());
  client->Sync().ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_io::Node::Sync>& result) {
    ASSERT_FALSE(result.ok());
    ASSERT_TRUE(result.is_canceled());
  });

  // A) Wait for sync to begin.
  completions.sync_thread_start.Wait();
}

void CommonTestUnpostedTeardown(zx_status_t status_for_sync) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  AsyncTearDownSync completions;
  std::unique_ptr<fs::ManagedVfs> vfs;

  ASSERT_NO_FATAL_FAILURE(SyncStart(completions, &loop, &vfs, status_for_sync));

  // B) Let sync complete.
  completions.sync_may_proceed.Signal();

  libsync::Completion shutdown_done;
  vfs->Shutdown([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_OK);
    // C) Issue an explicit shutdown, check that the Vnode has
    // already torn down.
    EXPECT_EQ(completions.sync_vnode_destroyed.Wait(zx::duration::infinite_past()), ZX_OK);
    shutdown_done.Signal();
  });
  ASSERT_EQ(shutdown_done.Wait(zx::sec(3)), ZX_OK);
}

// Test a case where the VFS object is shut down outside the dispatch loop.
TEST(Teardown, UnpostedTeardown) { CommonTestUnpostedTeardown(ZX_OK); }

// Test a case where the VFS object is shut down outside the dispatch loop, where the |Vnode::Sync|
// operation also failed causing the connection to be closed.
TEST(Teardown, UnpostedTeardownSyncError) { CommonTestUnpostedTeardown(ZX_ERR_INVALID_ARGS); }

void CommonTestPostedTeardown(zx_status_t status_for_sync) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  AsyncTearDownSync completions;
  std::unique_ptr<fs::ManagedVfs> vfs;

  ASSERT_NO_FATAL_FAILURE(SyncStart(completions, &loop, &vfs, status_for_sync));

  // B) Let sync complete.
  completions.sync_may_proceed.Signal();

  libsync::Completion shutdown_done;
  ASSERT_EQ(async::PostTask(
                loop.dispatcher(),
                [&]() {
                  vfs->Shutdown([&](zx_status_t status) {
                    EXPECT_EQ(status, ZX_OK);
                    // C) Issue an explicit shutdown, check that the Vnode has
                    // already torn down.
                    EXPECT_EQ(completions.sync_vnode_destroyed.Wait(zx::duration::infinite_past()),
                              ZX_OK);
                    shutdown_done.Signal();
                  });
                }),
            ZX_OK);
  ASSERT_EQ(shutdown_done.Wait(zx::sec(3)), ZX_OK);
}

// Test a case where the VFS object is shut down as a posted request to the dispatch loop.
TEST(Teardown, PostedTeardown) { ASSERT_NO_FATAL_FAILURE(CommonTestPostedTeardown(ZX_OK)); }

// Test a case where the VFS object is shut down as a posted request to the dispatch loop, where the
// |Vnode::Sync| operation also failed causing the connection to be closed.
TEST(Teardown, PostedTeardownSyncError) {
  ASSERT_NO_FATAL_FAILURE(CommonTestPostedTeardown(ZX_ERR_INVALID_ARGS));
}

// Test a case where the VFS object destroyed inside the callback to Shutdown.
TEST(Teardown, TeardownDeleteThis) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  AsyncTearDownSync completions;
  std::unique_ptr<fs::ManagedVfs> vfs;

  ASSERT_NO_FATAL_FAILURE(SyncStart(completions, &loop, &vfs));

  // B) Let sync complete.
  completions.sync_may_proceed.Signal();

  libsync::Completion shutdown_done;
  fs::ManagedVfs* raw_vfs = vfs.release();
  raw_vfs->Shutdown([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_OK);
    // C) Issue an explicit shutdown, check that the Vnode has already torn down.
    EXPECT_EQ(completions.sync_vnode_destroyed.Wait(zx::duration::infinite_past()), ZX_OK);
    delete raw_vfs;
    shutdown_done.Signal();
  });
  ASSERT_EQ(shutdown_done.Wait(zx::sec(3)), ZX_OK);
}

TEST(Teardown, SynchronousTeardown) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);
  zx::channel client;

  {
    // Tear down the VFS while the async loop is running.
    auto vfs = std::make_unique<fs::SynchronousVfs>(loop.dispatcher());
    auto vn = fbl::AdoptRef(new FdCountVnode());
    zx::channel server;
    ASSERT_EQ(zx::channel::create(0, &client, &server), ZX_OK);
    ASSERT_EQ(vfs->Serve(vn, std::move(server), {}), ZX_OK);
  }

  loop.Quit();

  {
    // Tear down the VFS while the async loop is not running.
    auto vfs = std::make_unique<fs::SynchronousVfs>(loop.dispatcher());
    auto vn = fbl::AdoptRef(new FdCountVnode());
    zx::channel server;
    ASSERT_EQ(zx::channel::create(0, &client, &server), ZX_OK);
    ASSERT_EQ(vfs->Serve(vn, std::move(server), {}), ZX_OK);

    loop.Shutdown();
  }

  {
    // Tear down the VFS with no active connections.
    auto vfs = std::make_unique<fs::SynchronousVfs>(loop.dispatcher());
  }
}

}  // namespace

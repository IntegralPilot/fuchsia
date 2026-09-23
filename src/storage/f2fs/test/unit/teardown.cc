// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/sync/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <sys/stat.h>
#include <zircon/errors.h>
#include <zircon/time.h>

#include <memory>
#include <thread>
#include <tuple>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/vnode.h"
#include "unit_lib.h"

namespace f2fs {
namespace {
using Runner = ComponentRunner;

class AsyncTearDownVnode : public Dir {
 public:
  AsyncTearDownVnode(F2fs* fs, ino_t ino, sync_completion_t* completions)
      : Dir(fs, ino, 0), callback_(nullptr), completions_(completions) {}

  ~AsyncTearDownVnode() {
    // C) Tear down the Vnode.
    sync_completion_signal(&completions_[2]);
  }

 private:
  void Sync(fs::Vnode::SyncCallback callback) final {
    callback_ = std::move(callback);
    std::thread thrd(&AsyncTearDownVnode::SyncThread, this);
    thrd.detach();
  }

  static void SyncThread(AsyncTearDownVnode* arg) {
    fs::Vnode::SyncCallback callback;
    {
      fbl::RefPtr<AsyncTearDownVnode> async_vn = fbl::RefPtr(arg);
      // A) Identify when the sync has started being processed.
      sync_completion_signal(&async_vn->completions_[0]);
      // B) Wait until the connection has been closed.
      sync_completion_wait(&async_vn->completions_[1], ZX_TIME_INFINITE);
      callback = std::move(async_vn->callback_);
    }
    callback(ZX_OK);
  }

  fs::Vnode::SyncCallback callback_;
  sync_completion_t* completions_;
};

TEST(Teardown, ShutdownOnNoConnections) {
  std::unique_ptr<f2fs::BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  MountOptions options{};
  ASSERT_EQ(options.SetValue(MountOption::kDiscard, 1), ZX_OK);
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ASSERT_TRUE(vfs_or.is_ok());
  auto fs_or = F2fs::Create(nullptr, std::move(bc), options, (*vfs_or).get());
  ASSERT_TRUE(fs_or.is_ok());
  auto fs = (*fs_or).get();
  auto on_unmount = []() { FX_LOGS(INFO) << "shutdown complete"; };
  vfs_or->SetUnmountCallback(std::move(on_unmount));
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  sync_completion_t root_completions[3], child_completions[3];

  // Create root directory connection.
  nid_t root_nid;
  auto nid_or = fs->GetNodeManager().AllocNid();
  ASSERT_TRUE(nid_or.is_ok());
  root_nid = *nid_or;
  auto root_dir = fbl::AdoptRef(new AsyncTearDownVnode(fs, root_nid, root_completions));
  root_dir->SetMode(S_IFDIR);

  async::Loop clients_loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto root_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  fidl::WireClient<fuchsia_io::Directory> root_client(std::move(root_endpoints.client),
                                                      loop.dispatcher());
  ASSERT_EQ(vfs_or->ServeDirectory(std::move(root_dir), std::move(root_endpoints.server)), ZX_OK);

  // A) Wait for root directory sync to begin.
  root_client->Sync().Then([](auto& result) {});
  sync_completion_wait(&root_completions[0], ZX_TIME_INFINITE);

  // Create child vnode connection.
  nid_t child_nid;
  nid_or = fs->GetNodeManager().AllocNid();
  ASSERT_TRUE(nid_or.is_ok());
  child_nid = *nid_or;
  auto child_dir = fbl::AdoptRef(new AsyncTearDownVnode(fs, child_nid, child_completions));
  child_dir->SetMode(S_IFDIR);

  auto child_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  fidl::WireClient<fuchsia_io::Directory> child_client(std::move(child_endpoints.client),
                                                       clients_loop.dispatcher());
  ASSERT_EQ(vfs_or->ServeDirectory(std::move(child_dir), std::move(child_endpoints.server)), ZX_OK);

  // A) Wait for child vnode sync to begin.
  child_client->Sync().Then([](auto& result) {});
  sync_completion_wait(&child_completions[0], ZX_TIME_INFINITE);

  // Terminate root directory connection.
  std::ignore = root_client.UnbindMaybeGetEndpoint();

  // B) Let complete sync.
  sync_completion_signal(&root_completions[1]);

  // C) Tear down root directory.
  sync_completion_wait(&root_completions[2], ZX_TIME_INFINITE);

  // Sleep for a while until filesystem shutdown completes.
  zx::nanosleep(zx::deadline_after(zx::msec(100)));
  ASSERT_FALSE(vfs_or->IsTerminating());

  // Terminate child vnode connection.
  std::ignore = child_client.UnbindMaybeGetEndpoint().is_ok();

  // B) Let complete sync.
  sync_completion_signal(&child_completions[1]);

  // C) Tear down child vnode.
  sync_completion_wait(&child_completions[2], ZX_TIME_INFINITE);

  // Sleep for a while until filesystem shutdown completes.
  zx::nanosleep(zx::deadline_after(zx::msec(100)));
  ASSERT_TRUE(vfs_or->IsTerminating());
  fs->Sync();
  fs->PutSuper();
}

}  // namespace
}  // namespace f2fs

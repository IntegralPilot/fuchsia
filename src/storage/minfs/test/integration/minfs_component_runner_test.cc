// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <utility>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/minfs/bcache.h"
#include "src/storage/minfs/component_runner.h"
#include "src/storage/minfs/format.h"
#include "src/storage/minfs/minfs.h"
#include "src/storage/minfs/mount.h"

namespace minfs {
namespace {

class MinfsComponentRunnerTest : public testing::Test {
 public:
  MinfsComponentRunnerTest() : loop_(&kAsyncLoopConfigAttachToCurrentThread) {}

  void SetUp() override {
    constexpr uint64_t kBlockCount = 1 << 17;
    auto device = std::make_unique<block_client::FakeBlockDevice>(kBlockCount, kMinfsBlockSize);
    auto bcache = Bcache::Create(std::move(device), kBlockCount);
    ASSERT_OK(bcache);
    bcache_ = *std::move(bcache);
    ASSERT_OK(Mkfs(bcache_.get()));

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    root_ = std::move(endpoints.client);
    server_end_ = std::move(endpoints.server);
  }

  void StartServe() {
    runner_ = std::make_unique<ComponentRunner>(loop_.dispatcher());
    runner_->SetUnmountCallback([this]() { loop_.Quit(); });
    zx::result status = runner_->ServeRoot(std::move(server_end_), {});
    ASSERT_OK(status);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetSvcDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open(
        "svc", fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory, {},
        server.TakeChannel());
    EXPECT_OK(status.status());
    return std::move(client);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetRootDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open("root",
                                              fuchsia_io::wire::kPermReadable |
                                                  fuchsia_io::wire::kPermWritable |
                                                  fuchsia_io::wire::Flags::kProtocolDirectory,
                                              {}, server.TakeChannel());
    EXPECT_OK(status.status());
    return std::move(client);
  }

 protected:
  async::Loop loop_;
  std::unique_ptr<Bcache> bcache_;
  std::unique_ptr<ComponentRunner> runner_;
  fidl::ClientEnd<fuchsia_io::Directory> root_;
  fidl::ServerEnd<fuchsia_io::Directory> server_end_;
};

TEST_F(MinfsComponentRunnerTest, ServeAndConfigureStartsMinfs) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  auto client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_OK(client_end);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_OK(status);

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_OK(status);
    *callback_called = true;
  });
  // Shutdown quits the loop.
  ASSERT_STATUS(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(MinfsComponentRunnerTest, RequestsBeforeStartupAreQueuedAndServicedAfter) {
  // Start a call to the filesystem. We expect that this request will be queued and won't return
  // until Configure is called on the runner. Initially, GetRootDir will fire off an open call on
  // the root_ connection, but as the server end isn't serving anything yet, the request is queued
  // there. Once root_ starts serving requests, and the svc dir exists, (which is done by
  // StartServe below) that open call succeeds, but the root itself should be waiting to serve any
  // open calls it gets, queuing any requests. Once Configure is called, the root should start
  // servicing requests, and the request will succeed.
  auto root_dir = GetRootDir();
  fidl::WireSharedClient<fuchsia_io::Directory> root_client(std::move(root_dir),
                                                            loop_.dispatcher());

  std::atomic<bool> query_complete = false;
  root_client->QueryFilesystem().ThenExactlyOnce(
      [query_complete =
           &query_complete](fidl::WireUnownedResult<fuchsia_io::Directory::QueryFilesystem>& info) {
        EXPECT_OK(info.status());
        EXPECT_OK(info->s);
        *query_complete = true;
      });
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_FALSE(query_complete);

  ASSERT_NO_FATAL_FAILURE(StartServe());
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_FALSE(query_complete);

  auto svc_dir = GetSvcDir();
  auto client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_OK(client_end);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_OK(status);
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_TRUE(query_complete);

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_OK(status);
    *callback_called = true;
  });
  ASSERT_STATUS(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(MinfsComponentRunnerTest, LifecycleChannelShutsDownRunner) {
  auto lifecycle_endpoints = fidl::Endpoints<fuchsia_process_lifecycle::Lifecycle>::Create();
  auto lifecycle = std::move(lifecycle_endpoints.client);

  runner_ = std::make_unique<ComponentRunner>(loop_.dispatcher());
  std::atomic<bool> unmount_callback_called = false;
  runner_->SetUnmountCallback([this, &unmount_callback_called]() {
    EXPECT_FALSE(unmount_callback_called);
    loop_.Quit();
    unmount_callback_called = true;
  });
  zx::result status =
      runner_->ServeRoot(std::move(server_end_), std::move(lifecycle_endpoints.server));
  ASSERT_OK(status);
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_FALSE(unmount_callback_called);

  MountOptions options;
  status = runner_->Configure(std::move(bcache_), options);
  ASSERT_OK(status.status_value());
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_FALSE(unmount_callback_called);

  auto lifecycle_stop_res = fidl::WireCall(lifecycle)->Stop();
  ASSERT_OK(lifecycle_stop_res.status());

  ASSERT_STATUS(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(unmount_callback_called);
}

TEST_F(MinfsComponentRunnerTest, DoubleShutdown) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  ASSERT_OK(component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow()));

  MountOptions options;
  ASSERT_OK(runner_->Configure(std::move(bcache_), options));
  ASSERT_OK(loop_.RunUntilIdle());

  // ManagedVfs::Shutdown doesn't support being called twice. Lifecycle.Stop and Admin.Shutdown
  // could race and both call ComponentRunner::Shutdown. ComponentRunner::Shutdown needs to handle
  // being called twice, only calling ManagedVfs::Shutdown once and calling both callbacks with the
  // result.
  std::atomic<bool> callback_called = false;
  async::PostTask(loop_.dispatcher(), [this, callback_called = &callback_called]() {
    runner_->Shutdown([callback_called](zx_status_t status) {
      EXPECT_OK(status);
      *callback_called = true;
    });
  });
  std::atomic<bool> callback2_called = false;
  async::PostTask(loop_.dispatcher(), [this, callback_called = &callback2_called]() {
    runner_->Shutdown([callback_called](zx_status_t status) {
      EXPECT_OK(status);
      *callback_called = true;
    });
  });

  // Shutdown quits the loop, but not before it is able to run the callbacks.
  ASSERT_STATUS(loop_.Run(), ZX_ERR_CANCELED);
  // Both callbacks were completed.
  ASSERT_TRUE(callback_called);
  ASSERT_TRUE(callback2_called);
}

}  // namespace
}  // namespace minfs

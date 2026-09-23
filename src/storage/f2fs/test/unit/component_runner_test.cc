// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/component_runner.h"

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.fs/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/resource.h>

#include <gtest/gtest.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/mkfs.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace f2fs {
namespace {

constexpr uint64_t kDefaultBlockCount = kMinVolumeSegments * kDefaultBlocksPerSegment;

class F2fsComponentRunnerTest : public testing::Test {
 public:
  F2fsComponentRunnerTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

  void SetUp() override {
    auto device = std::make_unique<block_client::FakeBlockDevice>(kDefaultBlockCount, kBlockSize);
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    bc_or = Mkfs(MkfsOptions{}, std::move(*bc_or));
    ASSERT_TRUE(bc_or.is_ok());
    bcache_ = std::move(*bc_or);

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    root_ = std::move(endpoints.client);
    server_end_ = std::move(endpoints.server);
  }

  void StartServe() {
    runner_ = std::make_unique<ComponentRunner>(loop_.dispatcher());
    runner_->SetUnmountCallback([this]() { loop_.Quit(); });
    zx::result status = runner_->ServeRoot(std::move(server_end_), {});
    ASSERT_EQ(status.status_value(), ZX_OK);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetSvcDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open(
        "svc", fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory, {},
        server.TakeChannel());
    EXPECT_EQ(status.status(), ZX_OK);
    return std::move(client);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetRootDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open("root",
                                              fuchsia_io::wire::kPermReadable |
                                                  fuchsia_io::wire::kPermWritable |
                                                  fuchsia_io::wire::Flags::kProtocolDirectory,
                                              {}, server.TakeChannel());
    EXPECT_EQ(status.status(), ZX_OK);
    return std::move(client);
  }

  void ResetRootDir() { root_.reset(); }

 protected:
  async::Loop loop_;
  std::unique_ptr<BcacheMapper> bcache_;
  std::unique_ptr<ComponentRunner> runner_;
  fidl::ClientEnd<fuchsia_io::Directory> root_;
  fidl::ServerEnd<fuchsia_io::Directory> server_end_;
};

TEST_F(F2fsComponentRunnerTest, ServeAndConfigureStartsF2fs) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  auto client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_EQ(status, ZX_OK);
    *callback_called = true;
  });
  // Shutdown quits the loop.
  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(F2fsComponentRunnerTest, ShutdownWithoutF2fs) {
  ASSERT_NO_FATAL_FAILURE(StartServe());
  runner_->SetUnmountCallback([]() {});
  runner_->Shutdown([](zx_status_t status) { EXPECT_EQ(status, ZX_OK); });
  loop_.RunUntilIdle();
}

TEST_F(F2fsComponentRunnerTest, OnNoConnections) {
  ASSERT_NO_FATAL_FAILURE(StartServe());
  ResetRootDir();
  loop_.RunUntilIdle();
  zx::nanosleep(zx::deadline_after(zx::msec(100)));
  ASSERT_TRUE(runner_->IsTerminating());
}

TEST_F(F2fsComponentRunnerTest, RequestsBeforeStartupAreQueuedAndServicedAfter) {
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
        EXPECT_EQ(info.status(), ZX_OK);
        EXPECT_EQ(info->s, ZX_OK);
        *query_complete = true;
      });
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);
  ASSERT_FALSE(query_complete);

  ASSERT_NO_FATAL_FAILURE(StartServe());
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);
  ASSERT_FALSE(query_complete);

  auto svc_dir = GetSvcDir();
  auto client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);
  ASSERT_TRUE(query_complete);

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_EQ(status, ZX_OK);
    *callback_called = true;
  });
  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(F2fsComponentRunnerTest, LifecycleChannelShutsDownRunner) {
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
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);
  ASSERT_FALSE(unmount_callback_called);

  MountOptions options;
  status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);
  ASSERT_FALSE(unmount_callback_called);

  auto lifecycle_stop_res = fidl::WireCall(lifecycle)->Stop();
  ASSERT_EQ(lifecycle_stop_res.status(), ZX_OK);

  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(unmount_callback_called);
}

TEST_F(F2fsComponentRunnerTest, DoubleShutdown) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  auto client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  // ManagedVfs::Shutdown doesn't support being called twice. Lifecycle.Stop and Admin.Shutdown
  // could race and both call ComponentRunner::Shutdown. ComponentRunner::Shutdown needs to handle
  // being called twice, only calling ManagedVfs::Shutdown once and calling both callbacks with the
  // result.
  std::atomic<bool> callback_called = false;
  async::PostTask(loop_.dispatcher(), [this, callback_called = &callback_called]() {
    runner_->Shutdown([callback_called](zx_status_t status) {
      EXPECT_EQ(status, ZX_OK);
      *callback_called = true;
    });
  });
  std::atomic<bool> callback2_called = false;
  async::PostTask(loop_.dispatcher(), [this, callback_called = &callback2_called]() {
    runner_->Shutdown([callback_called](zx_status_t status) {
      EXPECT_EQ(status, ZX_OK);
      *callback_called = true;
    });
  });

  // Shutdown quits the loop, but not before running the callbacks.
  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  // Both callbacks were completed.
  ASSERT_TRUE(callback_called);
  ASSERT_TRUE(callback2_called);
}

TEST_F(F2fsComponentRunnerTest, AdminShutdownCanBeCalledMultipleTimes) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  auto startup_client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow());
  ASSERT_EQ(startup_client_end.status_value(), ZX_OK);

  MountOptions options;
  zx::result status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  auto admin_client_end1 = component::ConnectAt<fuchsia_fs::Admin>(svc_dir.borrow());
  ASSERT_EQ(admin_client_end1.status_value(), ZX_OK);
  auto admin_client_end2 = component::ConnectAt<fuchsia_fs::Admin>(svc_dir.borrow());
  ASSERT_EQ(admin_client_end2.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  fidl::WireSharedClient<fuchsia_fs::Admin> admin_client1(std::move(*admin_client_end1),
                                                          loop_.dispatcher());
  fidl::WireSharedClient<fuchsia_fs::Admin> admin_client2(std::move(*admin_client_end2),
                                                          loop_.dispatcher());

  runner_->SetUnmountCallback(
      [this]() { async::PostTask(loop_.dispatcher(), [this]() { loop_.Quit(); }); });

  std::atomic<bool> shutdown1_called = false;
  admin_client1->Shutdown().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_fs::Admin::Shutdown>& result) {
        EXPECT_EQ(result.status(), ZX_OK);
        shutdown1_called = true;
      });

  std::atomic<bool> shutdown2_called = false;
  admin_client2->Shutdown().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_fs::Admin::Shutdown>& result) {
        EXPECT_EQ(result.status(), ZX_OK);
        shutdown2_called = true;
      });

  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(shutdown1_called);
  ASSERT_TRUE(shutdown2_called);
}

TEST_F(F2fsComponentRunnerTest, AdminShutdownAndLifecycleStopRace) {
  auto lifecycle_endpoints = fidl::Endpoints<fuchsia_process_lifecycle::Lifecycle>::Create();
  auto lifecycle = std::move(lifecycle_endpoints.client);

  runner_ = std::make_unique<ComponentRunner>(loop_.dispatcher());
  std::atomic<bool> unmount_callback_called = false;
  runner_->SetUnmountCallback([this, &unmount_callback_called]() {
    EXPECT_FALSE(unmount_callback_called);
    unmount_callback_called = true;
    async::PostTask(loop_.dispatcher(), [this]() { loop_.Quit(); });
  });
  zx::result status =
      runner_->ServeRoot(std::move(server_end_), std::move(lifecycle_endpoints.server));
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  MountOptions options;
  status = runner_->Configure(std::move(bcache_), options);
  ASSERT_EQ(status.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  auto svc_dir = GetSvcDir();
  auto admin_client_end = component::ConnectAt<fuchsia_fs::Admin>(svc_dir.borrow());
  ASSERT_EQ(admin_client_end.status_value(), ZX_OK);
  ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK);

  fidl::WireSharedClient<fuchsia_fs::Admin> admin_client(std::move(*admin_client_end),
                                                         loop_.dispatcher());

  std::atomic<bool> admin_shutdown_called = false;
  admin_client->Shutdown().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_fs::Admin::Shutdown>& result) {
        EXPECT_EQ(result.status(), ZX_OK);
        admin_shutdown_called = true;
      });

  auto lifecycle_stop_res = fidl::WireCall(lifecycle)->Stop();
  ASSERT_EQ(lifecycle_stop_res.status(), ZX_OK);

  ASSERT_EQ(loop_.Run(), ZX_ERR_CANCELED);
  ASSERT_TRUE(unmount_callback_called);
  ASSERT_TRUE(admin_shutdown_called);
}

}  // namespace
}  // namespace f2fs

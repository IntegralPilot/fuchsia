// Copyright 2021 The Fuchsia Authors. All rights reserved.
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
#include <lib/zx/resource.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <utility>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/component_runner.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace blobfs {
namespace {

constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = 8192;

class BlobfsComponentRunnerTest : public testing::Test {
 public:
  BlobfsComponentRunnerTest()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        config_(ComponentOptions{.pager_threads = 1}) {}

  void SetUp() override {
    device_ = std::make_unique<block_client::FakeBlockDevice>(kNumBlocks, kBlockSize);
    ASSERT_OK(FormatFilesystem(device_.get(), FilesystemOptions{}));

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    root_ = std::move(endpoints.client);
    server_end_ = std::move(endpoints.server);
  }
  void TearDown() override {}

  void StartServe() {
    runner_ = std::make_unique<ComponentRunner>(loop_, config_);
    auto status = runner_->ServeRoot(std::move(server_end_),
                                     fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>());
    ASSERT_OK(status.status_value());
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetSvcDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open(
        "svc", fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory, {},
        server.TakeChannel());
    ZX_ASSERT(status.status() == ZX_OK);
    return std::move(client);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetRootDir() const {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto status = fidl::WireCall(root_)->Open("root",
                                              fuchsia_io::wire::kPermReadable |
                                                  fuchsia_io::wire::kPermWritable |
                                                  fuchsia_io::wire::Flags::kProtocolDirectory,
                                              {}, server.TakeChannel());
    ZX_ASSERT(status.status() == ZX_OK);
    return std::move(client);
  }

  async::Loop loop_;
  ComponentOptions config_;
  std::unique_ptr<block_client::FakeBlockDevice> device_;
  std::unique_ptr<ComponentRunner> runner_;
  fidl::ClientEnd<fuchsia_io::Directory> root_;
  fidl::ServerEnd<fuchsia_io::Directory> server_end_;
};

TEST_F(BlobfsComponentRunnerTest, ServeAndConfigureStartsBlobfs) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  ASSERT_OK(component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow()));

  MountOptions options;
  ASSERT_OK(runner_->Configure(std::move(device_), options));

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_OK(status);
    *callback_called = true;
  });
  // Shutdown quits the loop.
  ASSERT_STATUS(loop_.RunUntilIdle(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(BlobfsComponentRunnerTest, RequestsBeforeStartupAreQueuedAndServicedAfter) {
  // Start a call to the filesystem. We expect that this request will be queued and won't return
  // until Configure is called on the runner. Initially, GetSvcDir will fire off an open call on
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
  ASSERT_OK(runner_->Configure(std::move(device_), options));
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_TRUE(query_complete);

  std::atomic<bool> callback_called = false;
  runner_->Shutdown([callback_called = &callback_called](zx_status_t status) {
    EXPECT_OK(status);
    *callback_called = true;
  });
  ASSERT_STATUS(loop_.RunUntilIdle(), ZX_ERR_CANCELED);
  ASSERT_TRUE(callback_called);
}

TEST_F(BlobfsComponentRunnerTest, DoubleShutdown) {
  ASSERT_NO_FATAL_FAILURE(StartServe());

  auto svc_dir = GetSvcDir();
  ASSERT_OK(component::ConnectAt<fuchsia_fs_startup::Startup>(svc_dir.borrow()));

  MountOptions options;
  ASSERT_OK(runner_->Configure(std::move(device_), options));
  ASSERT_OK(loop_.RunUntilIdle());

  // It would be more accurate to call Lifecycle::Stop() somehow, to reproduce this but that isn't
  // easily injected here. Calling fs_admin::Shutdown() doesn't have the same effect because it
  // runs on the blobfs dispatcher instead of the loop dispatcher, which is shut down differently.
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

  // Shutdown quits the loop, but not before it is able to run the
  ASSERT_STATUS(loop_.RunUntilIdle(), ZX_ERR_CANCELED);
  // Both callbacks were completed.
  ASSERT_TRUE(callback_called);
  ASSERT_TRUE(callback2_called);
}

}  // namespace
}  // namespace blobfs

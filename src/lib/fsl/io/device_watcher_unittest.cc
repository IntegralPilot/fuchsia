// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fsl/io/device_watcher.h"

#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/defer.h>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/fxl/macros.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace fsl {
namespace {

class DeviceWatcher : public gtest::RealLoopFixture {};

// Test that the callback is never called with ".".
TEST_F(DeviceWatcher, IgnoreDot) {
  async::Loop fs_loop{&kAsyncLoopConfigNeverAttachToThread};

  fs_loop.StartThread("fs-loop");
  auto empty_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  fs::SynchronousVfs vfs(fs_loop.dispatcher());

  auto [dir_client, dir_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  async::PostTask(fs_loop.dispatcher(), [&, server_end = std::move(dir_server)]() mutable {
    vfs.ServeDirectory(empty_dir, std::move(server_end));
  });

  fdio_ns_t* ns;
  const char* kDevicePath = "/test-device-path";
  EXPECT_EQ(ZX_OK, fdio_ns_get_installed(&ns));
  EXPECT_EQ(ZX_OK, fdio_ns_bind(ns, kDevicePath, dir_client.TakeChannel().release()));
  auto defer_unbind = fit::defer([&]() { fdio_ns_unbind(ns, kDevicePath); });
  auto device_watcher = fsl::DeviceWatcher::CreateWithIdleCallback(
      kDevicePath,
      [](const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename) {
        // The pseudo-directory is empty, so this callback should never be called.
        ADD_FAILURE() << filename;
      },
      [this]() { QuitLoop(); });
  ASSERT_TRUE(device_watcher);
  // Wait until the idle callback has run.
  RunLoop();

  fs_loop.Shutdown();
}

}  // namespace
}  // namespace fsl

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <zircon/errors.h>

#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

namespace {

namespace fio = fuchsia_io;

class WatcherTest : public testing::Test {
 public:
  WatcherTest()
      : loop_(&kAsyncLoopConfigAttachToCurrentThread),
        vfs_(loop_.dispatcher()),
        root_(fbl::MakeRefCounted<fs::PseudoDir>()) {}

  async::Loop& loop() { return loop_; }
  fbl::RefPtr<fs::PseudoDir>& root() { return root_; }

  fidl::ClientEnd<fio::DirectoryWatcher> WatchRootDir(fio::WatchMask mask) {
    auto [client, server] = fidl::Endpoints<fio::DirectoryWatcher>::Create();
    EXPECT_EQ(root_->WatchDir(&vfs_, mask, 0, std::move(server)), ZX_OK);
    return std::move(client);
  }

 protected:
  void TearDown() override { ASSERT_EQ(loop_.RunUntilIdle(), ZX_OK); }

 private:
  async::Loop loop_;
  fs::ManagedVfs vfs_;
  fbl::RefPtr<fs::PseudoDir> root_;
};

TEST_F(WatcherTest, WatchersDroppedOnChannelClosed) {
  ASSERT_FALSE(root()->HasWatchers());
  {
    fidl::ClientEnd client = WatchRootDir(fio::WatchMask::kAdded);
    ASSERT_TRUE(root()->HasWatchers());
  }
  ASSERT_EQ(loop().RunUntilIdle(), ZX_OK);
  ASSERT_FALSE(root()->HasWatchers());
}

}  // namespace

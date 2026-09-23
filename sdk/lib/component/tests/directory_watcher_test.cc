// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.examples/cpp/wire.h>
#include <lib/async-testing/test_loop.h>
#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/fdio/namespace.h>
#include <stdlib.h>
#include <zircon/types.h>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace component {
namespace {

const char* kFakeServicePath = "/svc_test";

namespace testing {

class TestBase : public ::testing::Test {
 protected:
  int MkDir(std::string dir) {
    std::string path = svc_ + dir;
    return mkdir(path.data(), 066);
  }

 private:
  std::string svc_;

  void SetUp() override {
    char buf[] = "/tmp/svc.XXXXXX";
    svc_ = mkdtemp(buf);

    int ret = MkDir("/fuchsia.examples.EchoService");
    ASSERT_EQ(0, ret);
    ret = MkDir("/fuchsia.examples.EchoService/default");
    ASSERT_EQ(0, ret);
    ret = MkDir("/fuchsia.examples.EchoService/my_instance");
    ASSERT_EQ(0, ret);

    fbl::unique_fd fd(open(svc_.data(), O_RDONLY | O_DIRECTORY));
    ASSERT_TRUE(fd.is_valid());
    fdio_ns_t* ns;
    ASSERT_EQ(ZX_OK, fdio_ns_get_installed(&ns));

    zx_status_t status = fdio_ns_bind_fd(ns, kFakeServicePath, fd.get());
    ASSERT_EQ(ZX_OK, status);
  }
  void TearDown() override {
    fdio_ns_t* ns;
    ASSERT_EQ(ZX_OK, fdio_ns_get_installed(&ns));
    ASSERT_EQ(ZX_OK, fdio_ns_unbind(ns, kFakeServicePath));
  }
};

}  // namespace testing

class DirectoryWatcherTest : public testing::TestBase {
 protected:
  async::TestLoop& loop() { return loop_; }

 private:
  async::TestLoop loop_;
};

TEST_F(DirectoryWatcherTest, Begin) {
  std::vector<std::pair<fuchsia_io::wire::WatchEvent, std::string>> instances;
  DirectoryWatcher watcher;
  auto callback = ([&instances](fuchsia_io::wire::WatchEvent event, std::string instance) {
    instances.emplace_back(std::make_pair(event, std::move(instance)));
  });
  std::string service_dir = std::string(kFakeServicePath) + "/fuchsia.examples.EchoService";
  zx::result dir_result = OpenDirectory(service_dir);
  ASSERT_TRUE(dir_result.is_ok());
  zx_status_t status = watcher.Begin(dir_result.value(), callback, loop().dispatcher());
  ASSERT_EQ(ZX_OK, status);

  ASSERT_TRUE(loop().RunUntilIdle());
  // These instances are added by TestBase:
  EXPECT_THAT(instances, ::testing::UnorderedElementsAre(
                             std::make_pair(fuchsia_io::wire::WatchEvent::kExisting, "default"),
                             std::make_pair(fuchsia_io::wire::WatchEvent::kExisting, "my_instance"),
                             std::make_pair(fuchsia_io::wire::WatchEvent::kIdle, "")));

  instances.clear();
  int ret = MkDir("/fuchsia.examples.EchoService/added");
  ASSERT_EQ(0, ret);
  ASSERT_TRUE(loop().RunUntilIdle());
  EXPECT_THAT(instances, ::testing::UnorderedElementsAre(
                             std::make_pair(fuchsia_io::wire::WatchEvent::kAdded, "added")));

  status = watcher.Cancel();
  ASSERT_EQ(ZX_OK, status);

  instances.clear();
  ret = MkDir("/fuchsia.examples.EchoService/added-after");
  ASSERT_EQ(0, ret);
  ASSERT_FALSE(loop().RunUntilIdle());
  ASSERT_TRUE(instances.empty());
}

TEST_F(DirectoryWatcherTest, SyncDirectoryWatcher) {
  std::vector<std::string> instances;
  std::string service_dir = std::string(kFakeServicePath) + "/fuchsia.examples.EchoService";
  SyncDirectoryWatcher watcher(service_dir);

  zx::result watch_result = watcher.GetNextEntry(true);
  ASSERT_TRUE(watch_result.is_ok());
  instances.push_back(watch_result.value());
  watch_result = watcher.GetNextEntry(true);
  ASSERT_TRUE(watch_result.is_ok());
  instances.push_back(watch_result.value());
  watch_result = watcher.GetNextEntry(true);
  ASSERT_EQ(watch_result.status_value(), ZX_ERR_STOP);
  // These instances are added by TestBase:
  EXPECT_THAT(instances, ::testing::UnorderedElementsAre("default", "my_instance"));
}

}  // namespace
}  // namespace component

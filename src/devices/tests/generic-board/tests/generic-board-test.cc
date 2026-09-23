// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/macros.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <gtest/gtest.h>

namespace generic_board_test {

using namespace component_testing;
namespace fdt = fuchsia_driver_test;

void ListDir(int dir_fd, const char* path) {
  int fd = openat(dir_fd, path, O_RDONLY | O_DIRECTORY);
  if (fd < 0) {
    FX_LOGS(INFO) << "ListDir: Failed to open dir " << path << ": " << strerror(errno);
    return;
  }
  DIR* dir = fdopendir(fd);
  if (!dir) {
    FX_LOGS(INFO) << "ListDir: Failed to fdopendir " << path << ": " << strerror(errno);
    close(fd);
    return;
  }
  struct dirent* entry;
  FX_LOGS(INFO) << "ListDir: Listing " << path << ":";
  while ((entry = readdir(dir)) != nullptr) {
    if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
      continue;
    }
    FX_LOGS(INFO) << "  " << entry->d_name;
  }
  closedir(dir);
}

class GenericBoardTest : public testing::Test {
 public:
  GenericBoardTest() {
    loop_.StartThread("test-realm");
    SetupRealm();
  }

  void SetupRealm() {
    auto realm_builder = RealmBuilder::Create();

    fidl::Arena arena;
    auto args = fdt::wire::RealmArgs::Builder(arena)
                    .root_driver("fuchsia-boot:///platform-bus#meta/platform-bus.cm")
                    .platform_vid(9)
                    .platform_pid(9)
                    .board_name("generic-board")
                    .Build();

    driver_test_realm::Setup(realm_builder, loop_.dispatcher(),
                             driver_test_realm::OptionsBuilder().using_subpackage(false).Build(),
                             fidl::ToNatural(args));

    realm_ = std::make_unique<RealmRoot>(realm_builder.Build(loop_.dispatcher()));
  }

  void StartRealm() {
    zx::result<> bootup_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_TRUE(bootup_result.is_ok()) << bootup_result.status_string();
  }

  void TearDown() override {
    if (realm_) {
      driver_test_realm::ShutdownRealm(*realm_);
    }
  }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::unique_ptr<RealmRoot> realm_;
};

TEST_F(GenericBoardTest, DriversEnumerate) {
  StartRealm();

  // Open dev-class directory from the realm
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto result = realm_->component().exposed()->Open("dev-class", fuchsia::io::PERM_READABLE, {},
                                                    server_end.TakeChannel());
  ASSERT_EQ(result, ZX_OK);

  int dev_fd;
  result = fdio_fd_create(client_end.TakeChannel().release(), &dev_fd);
  ASSERT_EQ(result, ZX_OK);

  // Wait for fake-device in devfs
  FX_LOGS(INFO) << "Waiting for fake-device in devfs";
  bool found = false;
  std::string device_name;
  for (int i = 0; i < 30; ++i) {
    ListDir(dev_fd, ".");
    ListDir(dev_fd, "test");

    int fd = openat(dev_fd, "test", O_RDONLY | O_DIRECTORY);
    if (fd >= 0) {
      DIR* dir = fdopendir(fd);
      if (dir) {
        struct dirent* entry;
        while ((entry = readdir(dir)) != nullptr) {
          if (strcmp(entry->d_name, ".") != 0 && strcmp(entry->d_name, "..") != 0) {
            device_name = entry->d_name;
            found = true;
            break;
          }
        }
        closedir(dir);
      } else {
        close(fd);
      }
    }
    if (found) {
      break;
    }
    sleep(1);
  }
  ASSERT_TRUE(found) << "fake-device did not appear in devfs";
  FX_LOGS(INFO) << "Found fake-device: " << device_name;
  close(dev_fd);
}

}  // namespace generic_board_test

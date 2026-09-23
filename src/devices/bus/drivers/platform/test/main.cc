// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/platform-defs.h>
#include <lib/devmgr-integration-test/fixture.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/watcher.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/status.h>

#include <format>

#include <src/lib/files/directory.h>
#include <zxtest/zxtest.h>

namespace {

using device_watcher::RecursiveWaitForFile;
using devmgr_integration_test::IsolatedDevmgr;
namespace fio = fuchsia_io;

TEST(PbusTest, Enumeration) {
  zx::result devmgr = IsolatedDevmgr::Create({
      .root_device_driver = "fuchsia-boot:///platform-bus#meta/platform-bus.cm",
  });
  ASSERT_OK(devmgr.status_value());

  const int dirfd = devmgr.value().devfs_root().get();

  ASSERT_OK(RecursiveWaitForFile(dirfd, "sys/platform").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/pt/test-board").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/test-parent").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/test-parent/child-1").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/test-parent/child-1/child-2").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/test-parent/child-1/child-2/child-4")
                .status_value());
  EXPECT_OK(
      RecursiveWaitForFile(dirfd, "sys/platform/test-parent/child-1/child-3-top").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/test-parent/child-1/child-3-top/child-3")
                .status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/gpio/test-gpio/gpio/gpio-3").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd, "sys/platform/spi/test-spi/spi/spi-0-0").status_value());
  EXPECT_OK(RecursiveWaitForFile(dirfd,
                                 "sys/platform/test-interrupt-controller/test-interrupt-controller")
                .status_value());
  EXPECT_EQ(RecursiveWaitForFile(dirfd, "sys/platform/node_a/composite_node_spec").status_value(),
            ZX_OK);

  struct stat st;
  EXPECT_EQ(fstatat(dirfd, "sys/platform/pt/test-board", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent", &st, 0), 0);
  EXPECT_EQ(
      fstatat(dirfd, "sys/platform/test-interrupt-controller/test-interrupt-controller", &st, 0),
      0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent/child-1", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent/child-1/child-2", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent/child-1/child-3-top", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent/child-1/child-2/child-4", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/test-parent/child-1/child-3-top/child-3", &st, 0), 0);
  EXPECT_EQ(fstatat(dirfd, "sys/platform/gpio/test-gpio/gpio/gpio-3", &st, 0), 0);
  EXPECT_EQ(
      fstatat(dirfd, "sys/platform/node_a/composite_node_spec/test-composite-node-spec", &st, 0),
      0);

  auto svc_dir = devmgr->svc_dir();
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto flags =
      static_cast<uint64_t>(fio::wire::kPermReadable | fio::wire::Flags::kProtocolDirectory);
  ASSERT_OK(fdio_open3_at(svc_dir.channel().get(), "fuchsia.sysinfo.Service", flags,
                          server_end.TakeChannel().release()));
  fbl::unique_fd dir_fd;
  ASSERT_OK(fdio_fd_create(client_end.TakeChannel().release(), dir_fd.reset_and_get_address()));
  std::vector<std::string> entries;
  files::ReadDirContentsAt(dir_fd.get(), ".", &entries);
  std::string instance;
  for (const auto& entry : entries) {
    if (entry != ".") {
      instance = entry;
    }
  }
  std::string path = std::format("fuchsia.sysinfo.Service/{}/device", instance);
  fprintf(stderr, "path: %s", path.c_str());

  auto sysinfo_client_end = component::ConnectAt<fuchsia_sysinfo::SysInfo>(svc_dir, path);
  ASSERT_OK(sysinfo_client_end);
  const fidl::WireSyncClient client(*std::move(sysinfo_client_end));

  // Get board name.
  [&client]() {
    const fidl::WireResult result = client->GetBoardName();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    const std::string board_info{response.name.get()};
    EXPECT_STREQ(board_info, "driver-integration-test");
  }();

  // Get interrupt controller information.
  [&client]() {
    const fidl::WireResult result = client->GetInterruptControllerInfo();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    ASSERT_NOT_NULL(response.info.get());
  }();

  // Get board revision information.
  [&client]() {
    const fidl::WireResult result = client->GetBoardRevision();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    ASSERT_NE(response.revision, 0);
  }();
}

}  // namespace

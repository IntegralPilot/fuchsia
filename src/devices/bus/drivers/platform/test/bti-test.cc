// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.btitest/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/time.h>
#include <zircon/status.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace {

using device_watcher::RecursiveWaitForFile;

using namespace component_testing;

constexpr char kParentPath[] = "dev-topological/sys/platform/bti-test";
constexpr char kDeviceName[] = "test-bti";

TEST(PbusBtiTest, BtiIsSameAfterCrash) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread();

  auto realm_builder = component_testing::RealmBuilder::Create();

  auto options =
      driver_test_realm::OptionsBuilder()
          .add_extra_realm_capability(
              fuchsia_component_test::Capability::WithProtocol(
                  fuchsia_component_test::Protocol{{.name = "fuchsia.kernel.IommuResource"}}),
              ParentRef{})
          .Build();
  driver_test_realm::Setup(realm_builder, loop.dispatcher(), options,
                           fuchsia_driver_test::RealmArgs{{
                               .root_driver = "fuchsia-boot:///platform-bus#meta/platform-bus.cm",
                           }});

  auto realm = realm_builder.Build(loop.dispatcher());
  auto boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  auto node = driver_test_realm::WaitForNode(realm, "bti-test");
  ASSERT_TRUE(node.is_ok());

  // Connect to the parent directory.
  // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
  fbl::unique_fd parent_dir;
  {
    fbl::unique_fd fd;
    auto exposed = realm.component().CloneExposedDir();
    ASSERT_OK(fdio_fd_create(exposed.TakeChannel().release(), fd.reset_and_get_address()));
    ASSERT_OK(RecursiveWaitForFile(fd.get(), kParentPath));
    ASSERT_OK(fdio_open3_fd_at(
        fd.get(), kParentPath,
        uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory},
        parent_dir.reset_and_get_address()));
  }

  node = driver_test_realm::WaitForNode(realm, "bti-test.test-bti");
  ASSERT_TRUE(node.is_ok());

  uint64_t koid1;
  {
    fidl::WireSyncClient<fuchsia_hardware_btitest::BtiDevice> client;
    {
      zx::result channel = RecursiveWaitForFile(parent_dir.get(), kDeviceName);
      ASSERT_OK(channel);
      client.Bind(fidl::ClientEnd<fuchsia_hardware_btitest::BtiDevice>(std::move(channel.value())));
    }
    {
      const fidl::WireResult result = client->GetKoid();
      ASSERT_OK(result.status());
      koid1 = result.value().koid;
    }

    zx::result dir_watcher =
        device_watcher::DirWatcher::Create(fdio_cpp::UnownedFdioCaller(parent_dir).directory());
    ASSERT_OK(dir_watcher);

    ASSERT_OK(client->Crash());
    // We have to wait for both the entry to be removed in devfs and for the channel to be
    // closed. The channel closes before the device is removed from devfs so only waiting for
    // one could result in a race.
    ASSERT_OK(dir_watcher->WaitForRemoval(kDeviceName, zx::duration::infinite()));
    ASSERT_OK(client.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(),
                                                     nullptr));
  }

  node = driver_test_realm::WaitForNode(realm, "bti-test.test-bti");
  ASSERT_TRUE(node.is_ok());

  // We implicitly rely on driver host being rebound in the event of a crash.
  uint64_t koid2;
  {
    fidl::WireSyncClient<fuchsia_hardware_btitest::BtiDevice> client;
    {
      zx::result channel = RecursiveWaitForFile(parent_dir.get(), kDeviceName);
      ASSERT_OK(channel);
      client.Bind(fidl::ClientEnd<fuchsia_hardware_btitest::BtiDevice>(std::move(channel.value())));
    }
    {
      const fidl::WireResult result = client->GetKoid();
      ASSERT_OK(result.status());
      koid2 = result.value().koid;
    }
  }

  ASSERT_EQ(koid1, koid2);

  driver_test_realm::ShutdownRealm(realm);
}

}  // namespace

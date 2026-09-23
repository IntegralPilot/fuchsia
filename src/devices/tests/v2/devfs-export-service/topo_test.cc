// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device.fs/cpp/fidl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/watcher.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <gtest/gtest.h>
#include <src/devices/lib/client/device_topology.h>

namespace devfs {
namespace {

void CheckTopoPath(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed, const std::string& path) {
  zx::result dir = component::OpenDirectoryAt(exposed, path);
  ASSERT_EQ(dir.status_value(), ZX_OK);

  auto watch_result = device_watcher::WatchDirectoryForItems<std::string>(
      *dir, [](std::string_view name) -> std::optional<std::string> { return std::string(name); });
  ASSERT_EQ(watch_result.status_value(), ZX_OK);
  std::string instance_name = std::move(watch_result.value());

  zx::result<std::string> path_result = fdf_topology::GetTopologicalPath(*dir, instance_name);
  ASSERT_TRUE(path_result.is_ok());
  ASSERT_TRUE(path_result.value() == "/root");
}
}  // anonymous namespace

TEST(DevfsPath, GetTopologicalPath) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread();

  auto realm_args = fuchsia_driver_test::RealmArgs();
  realm_args.root_driver("fuchsia-boot:///dtr#meta/root.cm");
  std::vector<fuchsia_component_test::Capability> exposes = {{
      fuchsia_component_test::Capability::WithService(
          fuchsia_component_test::Service{{.name = "fuchsia.services.test.Device"}}),
  }};

  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder, loop.dispatcher(),
                           driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
                           std::move(realm_args));

  auto realm = realm_builder.Build(loop.dispatcher());

  zx::result<> boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  fidl::UnownedClientEnd<fuchsia_io::Directory> exposed{
      realm.component().exposed().unowned_channel()};
  // Open dev-class/devfs_service_test and wait for the topological path.
  CheckTopoPath(exposed, "dev-class/devfs_service_test");
  // Open Service directory and access the topological_path
  CheckTopoPath(exposed, "fuchsia.services.test.Device");

  driver_test_realm::ShutdownRealm(realm);
}

}  // namespace devfs

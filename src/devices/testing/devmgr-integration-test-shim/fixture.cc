// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "include/lib/devmgr-integration-test/fixture.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <stdint.h>
#include <zircon/status.h>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/ref_ptr.h>

namespace devmgr_integration_test {

namespace {

constexpr std::string_view kBootPath = "/boot/";
constexpr std::string_view kBootUrlPrefix = "fuchsia-boot:///";

std::string PathToUrl(std::string_view path) {
  if (path.find(kBootUrlPrefix) == 0) {
    return std::string(path);
  }
  if (path.find(kBootPath) != 0) {
    ZX_ASSERT_MSG(false, "Driver path to devmgr-launcher must start with /boot/!");
  }
  return std::string(kBootUrlPrefix) + "#" + path.substr(kBootPath.size()).data();
}

}  // namespace

__EXPORT
devmgr_launcher::Args IsolatedDevmgr::DefaultArgs() {
  devmgr_launcher::Args args;
  args.root_device_driver = "/boot/meta/sysdev.cm";
  return args;
}

__EXPORT
IsolatedDevmgr::IsolatedDevmgr() = default;

__EXPORT
zx::result<IsolatedDevmgr> IsolatedDevmgr::Create(devmgr_launcher::Args args) {
  IsolatedDevmgr devmgr;
  devmgr.loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
  devmgr.loop_->StartThread();

  std::vector<fuchsia_component_test::Capability> exposes = {{
      fuchsia_component_test::Capability::WithService(
          fuchsia_component_test::Service{{.name = "fuchsia.sysinfo.Service"}}),
  }};

  // Create and build the realm.
  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(
      realm_builder, devmgr.loop_->dispatcher(),
      driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
      fuchsia_driver_test::RealmArgs{{
          .root_driver = PathToUrl(args.root_device_driver),
          .driver_tests_enable = std::move(args.driver_tests_enable),
          .driver_tests_disable = std::move(args.driver_tests_disable),
          .software_devices =
              std::vector{
                  fuchsia_driver_test::SoftwareDevice{{
                      .device_name = "ram-disk",
                      .device_id = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_DISK,
                  }},
              },
      }});

  devmgr.realm_ = std::make_unique<component_testing::RealmRoot>(
      realm_builder.Build(devmgr.loop_->dispatcher()));

  auto boot_result = driver_test_realm::WaitForBootup(*devmgr.realm_);
  if (boot_result.status_value() != ZX_OK) {
    return zx::error(boot_result.status_value());
  }

  // Connect to dev.
  // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
  fidl::InterfaceHandle<fuchsia::io::Directory> dev;
  if (zx_status_t status = devmgr.realm_->component().exposed()->Open(
          "dev-topological", fuchsia::io::PERM_READABLE, {}, dev.NewRequest().TakeChannel());
      status != ZX_OK) {
    return zx::error(status);
  }

  if (zx_status_t status =
          fdio_fd_create(dev.TakeChannel().release(), devmgr.devfs_root_.reset_and_get_address());
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(devmgr));
}

__EXPORT
IsolatedDevmgr::IsolatedDevmgr(IsolatedDevmgr&& other) noexcept = default;

__EXPORT
IsolatedDevmgr& IsolatedDevmgr::operator=(IsolatedDevmgr&& other) noexcept = default;

__EXPORT
IsolatedDevmgr::~IsolatedDevmgr() {
  if (realm_) {
    driver_test_realm::ShutdownRealm(*realm_);
  }
}

}  // namespace devmgr_integration_test

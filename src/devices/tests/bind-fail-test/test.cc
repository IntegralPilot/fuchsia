// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/devmgr-integration-test/fixture.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <vector>

#include <zxtest/zxtest.h>

namespace {

using devmgr_integration_test::IsolatedDevmgr;

TEST(BindFailTest, BindFail) {
  const char kDriver[] = "bind-fail-test-driver.cm";
  auto args = IsolatedDevmgr::DefaultArgs();

  args.root_device_driver = "/boot/meta/test-parent-sys.cm";

  zx::result devmgr = IsolatedDevmgr::Create(std::move(args));
  ASSERT_OK(devmgr.status_value());

  fidl::ClientEnd<fuchsia_device::Controller> client_end;
  {
    zx::result channel = device_watcher::RecursiveWaitForFile(devmgr.value().devfs_root().get(),
                                                              "sys/test/test2/device_controller");
    ASSERT_OK(channel);
    client_end.channel() = std::move(channel.value());
  }
  fidl::WireSyncClient controller(std::move(client_end));

  const fidl::WireResult result = controller->Bind(fidl::StringView{kDriver});
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  // The driver will run its tests in its bind routine, and return ZX_OK on success.
  ASSERT_TRUE(response.is_error());
  ASSERT_STATUS(ZX_ERR_NOT_SUPPORTED, response.error_value());
}

}  // namespace

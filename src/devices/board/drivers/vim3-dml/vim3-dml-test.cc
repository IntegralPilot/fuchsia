// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zbi-format/board.h>

#include <gtest/gtest.h>

namespace vim3_dml {

namespace {

const zbi_platform_id_t kPlatformId = {
    .vid = PDEV_VID_KHADAS,
    .pid = PDEV_PID_VIM3,
    .board_name = "vim3-dml",
};

}  // namespace

class Vim3DmlTest : public testing::Test {
 public:
  Vim3DmlTest() { loop_.StartThread("test-realm"); }

  ~Vim3DmlTest() override {
    if (realm_) {
      libsync::Completion teardown_complete;
      realm_->Teardown(
          [&](fit::result<fuchsia::component::Error> result) { teardown_complete.Signal(); });
      teardown_complete.Wait();
    }
  }

  void SetupRealm() {
    auto realm_builder = component_testing::RealmBuilder::Create();

    fidl::Arena arena;
    auto args = fuchsia_driver_test::wire::RealmArgs::Builder(arena)
                    .root_driver("fuchsia-boot:///platform-bus#meta/platform-bus.cm")
                    .platform_vid(kPlatformId.vid)
                    .platform_pid(kPlatformId.pid)
                    .board_name(kPlatformId.board_name)
                    .Build();

    driver_test_realm::Setup(realm_builder, loop_.dispatcher(),
                             driver_test_realm::OptionsBuilder().using_subpackage(true).Build(),
                             fidl::ToNatural(args));
    realm_ =
        std::make_unique<component_testing::RealmRoot>(realm_builder.Build(loop_.dispatcher()));
    ASSERT_TRUE(driver_test_realm::WaitForBootup(*realm_).is_ok());
  }

  zx::result<> WaitOnDevices(const std::vector<std::string>& device_paths) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto result = realm_->component().exposed()->Open("dev-topological", fuchsia::io::PERM_READABLE,
                                                      {}, server_end.TakeChannel());
    if (result != ZX_OK) {
      return zx::error(result);
    }

    int dev_fd;
    result = fdio_fd_create(client_end.TakeChannel().release(), &dev_fd);
    if (result != ZX_OK) {
      return zx::error(result);
    }

    auto close_fd = fit::defer([&dev_fd]() { close(dev_fd); });

    for (const auto& path : device_paths) {
      auto wait_result = device_watcher::RecursiveWaitForFile(dev_fd, path.c_str());
      if (wait_result.is_error()) {
        return wait_result.take_error();
      }
    }
    return zx::ok();
  }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::unique_ptr<component_testing::RealmRoot> realm_;
};

TEST_F(Vim3DmlTest, DmlEnumeration) {
  std::vector<std::string> device_node_paths = {
      "sys/platform/adc-9000",
      "sys/platform/adc-buttons",
      "sys/platform/arm-mali-0",
      "sys/platform/audio-controller-ff642000",
      "sys/platform/bt-uart-ffd24000",
      "sys/platform/canvas-ff638000",
      "sys/platform/clock-controller-ff63c000",
      "sys/platform/cpu-controller-0",
      "sys/platform/dwmac-ff3f0000",
      "sys/platform/ethernet-phy-ff634000",
      "sys/platform/gpio-buttons",
      "sys/platform/gpio-controller-ff634400",
      "sys/platform/gpio-controller-20",
      "sys/platform/gpu-ffe40000",
      "sys/platform/display-ff900000",
      "sys/platform/hrtimer-0",
      "sys/platform/i2c-1c000",
      "sys/platform/i2c-5000",
      "sys/platform/interrupt-controller-ffc01000",
      "sys/platform/mmc-ffe03000",
      "sys/platform/mmc-ffe05000",
      "sys/platform/mmc-ffe07000",
      "sys/platform/nna-ff100000",
      "sys/platform/power-controller",
      "sys/platform/khadas-mcu-18",
      "sys/platform/rtc-51",
      "sys/platform/pwm_a-regulator",
      "sys/platform/pwm_a0_d-regulator",
      "sys/platform/pwm-ffd1b000",
      "sys/platform/register-controller-1000",
      "sys/platform/suspend",
      "sys/platform/temperature-sensor-ff634800",
      "sys/platform/temperature-sensor-ff634c00",
      "sys/platform/usb-ff400000",
      "sys/platform/usb-ff500000",
      "sys/platform/usb-phy-ffe09000",
      "sys/platform/video-decoder-ffd00000",
      "sys/platform/wifi",
  };
  SetupRealm();
  ASSERT_TRUE(WaitOnDevices(device_node_paths).is_ok());
}

}  // namespace vim3_dml

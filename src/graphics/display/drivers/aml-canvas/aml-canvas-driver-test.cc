// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/aml-canvas-driver.h"

#include <fidl/fuchsia.hardware.amlogiccanvas/cpp/wire.h>
#include <lib/driver/compat/cpp/logging.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace aml_canvas {

namespace {

class AmlCanvasDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    zx::vmo mmio_vmo;
    static constexpr uint64_t kMmioVmoSize = 0x2000;
    zx_status_t status = zx::vmo::create(kMmioVmoSize, 0, &mmio_vmo);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    fdf_fake::FakePDev::Config config;
    config.use_fake_bti = true;
    config.mmios[0] = fdf::PDev::MmioInfo{
        .offset = 0,
        .size = kMmioVmoSize,
        .vmo = std::move(mmio_vmo),
    };
    fake_pdev_.SetConfig(std::move(config));

    auto instance_handler = fake_pdev_.GetInstanceHandler(dispatcher);
    return to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        std::move(instance_handler));
  }

  fdf_fake::FakePDev& fake_pdev() { return fake_pdev_; }

 private:
  fdf_fake::FakePDev fake_pdev_;
};

class TestConfig final {
 public:
  using DriverType = AmlCanvasDriver;
  using EnvironmentType = AmlCanvasDriverTestEnvironment;
};

class AmlCanvasDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_OK(result);
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
};

TEST_F(AmlCanvasDriverTest, Lifecycle) {
  // SetUp and TearDown handle start and stop.
}

TEST_F(AmlCanvasDriverTest, ServesAmlogicCanvasDeviceProtocol) {
  zx::result canvas_client_end =
      driver_test().Connect<fuchsia_hardware_amlogiccanvas::Service::Device>();
  ASSERT_OK(canvas_client_end);
}

}  // namespace

}  // namespace aml_canvas

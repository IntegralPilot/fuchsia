// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/board-resources.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace aml_canvas {

namespace {

class FakePdevTest : public ::testing::Test, public loop_fixture::RealLoop {
 public:
  FakePdevTest() = default;
  ~FakePdevTest() = default;

  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> ConnectToFakePdev() {
    auto [pdev_client, pdev_server] =
        fidl::Endpoints<fuchsia_hardware_platform_device::Device>::Create();
    fake_pdev_server_.Connect(std::move(pdev_server));
    return std::move(pdev_client);
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_fake::FakePDev fake_pdev_server_;
};

TEST_F(FakePdevTest, MapMmioSuccess) {
  zx::vmo vmo;
  const uint64_t vmo_size = 0x2000;
  ASSERT_OK(zx::vmo::create(vmo_size, /*options=*/0, &vmo));

  fdf_fake::FakePDev::Config config;
  config.mmios[0] = fdf::PDev::MmioInfo{
      .offset = 0,
      .size = vmo_size,
      .vmo = std::move(vmo),
  };
  fake_pdev_server_.SetConfig(std::move(config));

  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev = ConnectToFakePdev();
  PerformBlockingWork([&] {
    zx::result<fdf::MmioBuffer> mmio_result = MapMmio(MmioResourceIndex::kDmc, pdev);
    ASSERT_OK(mmio_result);
  });
}

TEST_F(FakePdevTest, MapMmioPdevFailure) {
  fdf_fake::FakePDev::Config config;
  fake_pdev_server_.SetConfig(std::move(config));

  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev = ConnectToFakePdev();
  PerformBlockingWork([&] {
    zx::result<fdf::MmioBuffer> mmio_result = MapMmio(MmioResourceIndex::kDmc, pdev);
    ASSERT_NE(mmio_result.status_value(), ZX_OK);
  });
}

TEST_F(FakePdevTest, GetBtiSuccess) {
  zx::result fake_bti = fake_bti::CreateFakeBti();
  ASSERT_OK(fake_bti);

  fdf_fake::FakePDev::Config config;
  config.btis[0] = std::move(fake_bti.value());
  fake_pdev_server_.SetConfig(std::move(config));

  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev = ConnectToFakePdev();
  PerformBlockingWork([&] {
    zx::result<zx::bti> bti_result = GetBti(BtiResourceIndex::kCanvas, pdev);
    ASSERT_OK(bti_result);
  });
}

TEST_F(FakePdevTest, GetBtiPdevFailure) {
  fdf_fake::FakePDev::Config config;
  config.use_fake_bti = false;
  fake_pdev_server_.SetConfig(std::move(config));

  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev = ConnectToFakePdev();
  PerformBlockingWork([&] {
    zx::result<zx::bti> bti_result = GetBti(BtiResourceIndex::kCanvas, pdev);
    ASSERT_NE(bti_result.status_value(), ZX_OK);
  });
}

}  // namespace

}  // namespace aml_canvas

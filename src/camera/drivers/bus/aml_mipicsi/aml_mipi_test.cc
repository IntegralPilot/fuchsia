// Copyright 2024 The Fuchsia Authors. All rights reserved.  Use of
// this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/bus/aml_mipicsi/aml_mipi.h"

#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace camera::test {

class IncomingNamespace {
 public:
  void Init(fidl::ServerEnd<fuchsia_io::Directory> outgoing_server) {
    fdf_fake::FakePDev::Config pdev_config{.use_fake_bti = true, .use_fake_irq = true};
    pdev_config.mmios[AmlMipiDevice::kCsiPhy0] = fake_mmio_.GetMmioBuffer();
    pdev_config.mmios[AmlMipiDevice::kAphy0] = fake_mmio_.GetMmioBuffer();
    pdev_config.mmios[AmlMipiDevice::kCsiHost0] = fake_mmio_.GetMmioBuffer();
    pdev_config.mmios[AmlMipiDevice::kMipiAdap] = fake_mmio_.GetMmioBuffer();
    pdev_config.mmios[AmlMipiDevice::kHiu] = fake_mmio_.GetMmioBuffer();
    ASSERT_OK(pdev_.SetConfig(std::move(pdev_config)));
    ASSERT_OK(outgoing_.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler()));
    ASSERT_OK(outgoing_.Serve(std::move(outgoing_server)));
  }

 private:
  fdf_fake::FakePDev pdev_;
  component::OutgoingDirectory outgoing_{async_get_default_dispatcher()};
  ddk_fake::FakeMmioRegRegion fake_mmio_{1, 1};
};

class AmlMipiTest : public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(incoming_loop_.StartThread("incoming-namespace"));
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_OK(endpoints);
    incoming_namespace_.SyncCall(&IncomingNamespace::Init, std::move(endpoints->server));
    parent_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                            std::move(endpoints->client), "pdev");
    ASSERT_OK(fdf::RunOnDispatcherSync(driver_dispatcher_->async_dispatcher(), [&]() {
      ASSERT_OK(AmlMipiDevice::Bind(nullptr, parent_.get()));
    }));
  }

 private:
  std::shared_ptr<MockDevice> parent_ = MockDevice::FakeRootParent();
  fdf::UnownedSynchronizedDispatcher driver_dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();

  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      incoming_loop_.dispatcher(), std::in_place};
};

// Verify that the aml-mipi driver can bind.
TEST_F(AmlMipiTest, CanBind) { ASSERT_NO_FATAL_FAILURE(); }

}  // namespace camera::test

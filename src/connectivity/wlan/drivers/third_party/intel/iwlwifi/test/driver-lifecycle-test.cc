// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/async/default.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-bti/bti.h>

#include <gtest/gtest.h>

#include "third_party/iwlwifi/platform/pcie-iwlwifi-driver.h"

namespace {

constexpr int kTestDeviceId = 0x095a;
constexpr int kTestSubsysDeviceId = 0x9e10;

// Implement all the WireServer handlers of fuchsia_hardware_pci::Device as protocol as required by
// FIDL.
class FakePciParent : public fidl::WireServer<fuchsia_hardware_pci::Device> {
 public:
  fuchsia_hardware_pci::Service::InstanceHandler GetInstanceHandler(async_dispatcher_t* dispatcher) {
    return fuchsia_hardware_pci::Service::InstanceHandler({
        .device = binding_group_.CreateHandler(
            this, dispatcher,
            fidl::kIgnoreBindingClosure),
    });
  }

  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override {
    fuchsia_hardware_pci::wire::DeviceInfo info;
    info.device_id = kTestDeviceId;
    completer.Reply(info);
  }
  void GetBar(GetBarRequestView request, GetBarCompleter::Sync& completer) override {
    fuchsia_hardware_pci::wire::Bar bar;
    completer.ReplySuccess(std::move(bar));
  }

  void SetBusMastering(SetBusMasteringRequestView request,
                       SetBusMasteringCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }

  void ResetDevice(ResetDeviceCompleter::Sync& completer) override { completer.ReplySuccess(); }

  void AckInterrupt(AckInterruptCompleter::Sync& completer) override { completer.ReplySuccess(); }

  void MapInterrupt(MapInterruptRequestView request,
                    MapInterruptCompleter::Sync& completer) override {
    zx::interrupt interrupt;
    completer.ReplySuccess(std::move(interrupt));
  }

  void GetInterruptModes(GetInterruptModesCompleter::Sync& completer) override {
    fuchsia_hardware_pci::wire::InterruptModes modes;
    completer.Reply(modes);
  }

  void SetInterruptMode(SetInterruptModeRequestView request,
                        SetInterruptModeCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }

  void ReadConfig8(ReadConfig8RequestView request, ReadConfig8Completer::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void ReadConfig16(ReadConfig16RequestView request,
                    ReadConfig16Completer::Sync& completer) override {
    // Always return the fake sub-system device id to pass the initialization.
    completer.ReplySuccess(kTestSubsysDeviceId);
  }

  void ReadConfig32(ReadConfig32RequestView request,
                    ReadConfig32Completer::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void WriteConfig8(WriteConfig8RequestView request,
                    WriteConfig8Completer::Sync& completer) override {
    completer.ReplySuccess();
  }

  void WriteConfig16(WriteConfig16RequestView request,
                     WriteConfig16Completer::Sync& completer) override {
    completer.ReplySuccess();
  }

  void WriteConfig32(WriteConfig32RequestView request,
                     WriteConfig32Completer::Sync& completer) override {
    completer.ReplySuccess();
  }

  void GetCapabilities(GetCapabilitiesRequestView request,
                       GetCapabilitiesCompleter::Sync& completer) override {
    std::vector<uint8_t> empty_vec;
    auto empty_vec_view = fidl::VectorView<uint8_t>::FromExternal(empty_vec);
    completer.Reply(empty_vec_view);
  }

  void GetExtendedCapabilities(GetExtendedCapabilitiesRequestView request,
                               GetExtendedCapabilitiesCompleter::Sync& completer) override {
    std::vector<uint16_t> empty_vec;
    auto empty_vec_view = fidl::VectorView<uint16_t>::FromExternal(empty_vec);
    completer.Reply(empty_vec_view);
  }

  void GetBti(GetBtiRequestView request, GetBtiCompleter::Sync& completer) override {
    zx_handle_t fake_handle;
    fake_bti_create(&fake_handle);
    zx::bti bti(fake_handle);
    completer.ReplySuccess(std::move(bti));
  }

  fidl::ServerBindingGroup<fuchsia_hardware_pci::Device> binding_group_;
};

class IwlwifiTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    auto handler = fake_pci_parent_.GetInstanceHandler(dispatcher);
    return to_driver_vfs.AddService<fuchsia_hardware_pci::Service>(std::move(handler));
  }

 private:
  FakePciParent fake_pci_parent_;
};

class TestConfig final {
 public:
  using DriverType = wlan::iwlwifi::PcieIwlwifiDriver;
  using EnvironmentType = IwlwifiTestEnvironment;
};

class DriverLifeCycleTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
    driver_stopped_ = false;
  }

  void TearDown() override {
    if (!driver_stopped_) {
      zx::result<> result = driver_test().StopDriver();
      ASSERT_EQ(ZX_OK, result.status_value());
      driver_test().ShutdownAndDestroyDriver();
    }
  }

  size_t GetNodeNumber() {
    size_t count = 0;
    driver_test().RunInNodeContext([&count](fdf_testing::TestNode& node) {
      count = node.children().size();
    });
    return count;
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  bool driver_stopped_ = false;

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_F(DriverLifeCycleTest, DeviceLifeCycle) {
  // Starting PcieIwlwifiDriver will trigger AddNode for wlanphy virtual device. Start() hook is
  // called in Setup().
  EXPECT_EQ(GetNodeNumber(), (size_t)1);

  // TODO(b/290283534): Add more operations here like AddWlansoftmacDevice() and
  // RemoveWlansoftmacDevice().

  zx::result<> stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  // We don't shutdown any of the children devices during PrepareStop(), this will be handled by
  // driver framework.
  EXPECT_EQ(GetNodeNumber(), (size_t)1);

  driver_test().ShutdownAndDestroyDriver();
  driver_stopped_ = true;

  while (GetNodeNumber() > 0) {
    driver_test().runtime().RunUntilIdle();
  }

  EXPECT_EQ(GetNodeNumber(), 0UL);
}

}  // namespace

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/aml-usb-phy/aml-usb-phy.h"

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>

#include <string>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gtest/gtest.h>
#include <soc/aml-common/aml-registers.h>

#include "src/devices/registers/testing/mock-registers/mock-registers.h"
#include "src/devices/usb/drivers/aml-usb-phy/aml-usb-phy-device.h"
#include "src/devices/usb/drivers/aml-usb-phy/usb-phy-regs.h"
#include "src/devices/usb/drivers/aml-usb-phy/usb-phy3-regs.h"
#include "src/lib/testing/predicates/status.h"

namespace aml_usb_phy {

constexpr auto kRegisterBanks = 4;
constexpr auto kRegisterCount = 2048;

class FakeMmio {
 public:
  FakeMmio() : region_(sizeof(uint32_t), kRegisterCount) {
    for (size_t c = 0; c < kRegisterCount; c++) {
      region_[c * sizeof(uint32_t)].SetReadCallback([this, c]() { return reg_values_[c]; });
      region_[c * sizeof(uint32_t)].SetWriteCallback([this, c](uint64_t value) {
        reg_values_[c] = value;
        // Automatically acknowledge CR bus access for PHY3.
        if (c * sizeof(uint32_t) == PHY3_R4_OFFSET) {
          if (value & (1 << 19 /* cap_addr */ | 1 << 18 /* cap_data */ | 1 << 1 /* read */ |
                       1 << 0 /* write */)) {
            reg_values_[PHY3_R5_OFFSET >> 2] |= (1 << 16 /* cr_ack */);
          } else {
            reg_values_[PHY3_R5_OFFSET >> 2] &= ~(1 << 16 /* cr_ack */);
          }
        }
        // Automatically set PHY ready for USB 2.0 PHYs (U2P_R1).
        if (c * sizeof(uint32_t) == U2P_R1_OFFSET ||
            c * sizeof(uint32_t) == (U2P_R1_OFFSET + U2P_REGISTER_OFFSET)) {
          reg_values_[c] |= (1 << 0 /* phy_rdy */);
        }
      });
    }
  }

  fdf::MmioBuffer mmio() { return region_.GetMmioBuffer(); }

  uint64_t reg_values_[kRegisterCount] = {0};

 private:
  ddk_fake::FakeMmioRegRegion region_;
};

class TestAmlUsbPhyDevice : public AmlUsbPhyDevice {
 public:
  TestAmlUsbPhyDevice() = default;

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(
        fdf_internal::DriverServer2<TestAmlUsbPhyDevice>::initialize,
        fdf_internal::DriverServer2<TestAmlUsbPhyDevice>::destroy);
  }

  bool dwc2_connected() { return device()->dwc2_connected(); }
  FakeMmio& usbctrl_mmio() { return mmio_[0]; }

 private:
  zx::result<fdf::MmioBuffer> MapMmio(fdf::PDev& pdev, uint32_t idx) override {
    if (idx >= kRegisterBanks) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::ok(mmio_[idx].mmio());
  }

  FakeMmio mmio_[kRegisterBanks];
};

class AmlUsbPhyTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(const zx::interrupt& interrupt) {
    zx::interrupt duplicate;
    ASSERT_OK(interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    std::map<uint32_t, zx::interrupt> irqs;
    irqs[0] = std::move(duplicate);
    pdev_.SetConfig({.irqs = std::move(irqs)});

    static constexpr fuchsia_hardware_usb_phy::AmlogicPhyType kPhyType =
        fuchsia_hardware_usb_phy::AmlogicPhyType::kG12A;
    static const std::vector<fuchsia_hardware_usb_phy::UsbPhyMode> kPhyModes = {
        {{.protocol = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20,
          .dr_mode = fuchsia_hardware_usb_phy::Mode::kHost,
          .is_otg_capable = false}},
        {{.protocol = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20,
          .dr_mode = fuchsia_hardware_usb_phy::Mode::kOtg,
          .is_otg_capable = true}},
        {{.protocol = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb30,
          .dr_mode = fuchsia_hardware_usb_phy::Mode::kHost,
          .is_otg_capable = false}},
    };
    static const fuchsia_hardware_usb_phy::Metadata kMetadata{{
        .usb_phy_modes = kPhyModes,
        .phy_type = kPhyType,
    }};

    ASSERT_OK(
        pdev_.AddFidlMetadata(fuchsia_hardware_usb_phy::Metadata::kSerializableName, kMetadata));

    registers_.ExpectWrite<uint32_t>(RESET1_LEVEL_OFFSET, aml_registers::USB_RESET1_LEVEL_MASK,
                                     aml_registers::USB_RESET1_LEVEL_MASK);
    registers_.ExpectWrite<uint32_t>(RESET1_REGISTER_OFFSET,
                                     aml_registers::USB_RESET1_REGISTER_UNKNOWN_1_MASK,
                                     aml_registers::USB_RESET1_REGISTER_UNKNOWN_1_MASK);
    registers_.ExpectWrite<uint32_t>(RESET1_LEVEL_OFFSET, aml_registers::USB_RESET1_LEVEL_MASK,
                                     ~aml_registers::USB_RESET1_LEVEL_MASK);
    registers_.ExpectWrite<uint32_t>(RESET1_LEVEL_OFFSET, aml_registers::USB_RESET1_LEVEL_MASK,
                                     aml_registers::USB_RESET1_LEVEL_MASK);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_registers::Service>(
          std::move(registers_.GetInstanceHandler()), "register-reset");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  mock_registers::MockRegisters& registers() { return registers_; }

 private:
  fdf_fake::FakePDev pdev_;
  mock_registers::MockRegisters registers_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};
};

class AmlUsbPhyTestConfig {
 public:
  using DriverType = TestAmlUsbPhyDevice;
  using EnvironmentType = AmlUsbPhyTestEnvironment;
};

class AmlUsbPhyTest : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt_));
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.Init(interrupt_); });
    ASSERT_OK(driver_test_.StartDriver());

    // Wait for Init() to publish initial children.
    // Init() includes an OTG stabilization delay of 1s plus Mode 0 (Host) publication.
    // In this test environment, xhci is added as a sibling of the driver node under the root.
    driver_test_.runtime().RunUntil([this]() {
      return driver_test_.RunInNodeContext<bool>(
          [](auto& node) { return node.children().size() == 1; });
    });
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.registers().VerifyAll(); });
    ASSERT_NO_FATAL_FAILURE();
  }

  // This method fires the irq and then waits for the side effects of SetMode to have taken place.
  void TriggerInterruptAndCheckMode(fuchsia_hardware_usb_phy::Mode mode) {
    // Switch to appropriate mode. This will be read by the irq thread.
    driver_test_.RunInDriverContext([&](auto& driver) {
      driver.usbctrl_mmio().reg_values_[USB_R5_OFFSET >> 2] =
          (mode == fuchsia_hardware_usb_phy::Mode::kPeripheral) << 6;
    });

    // Trigger interrupt and wait for it to be acknowledged.
    interrupt_.trigger(0, zx::clock::get_boot());
    ASSERT_OK(interrupt_.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite(), nullptr));

    driver_test_.RunInDriverContext([&](auto& driver) {
      // Check that mode is as expected.
      auto& phy = driver.device();
      EXPECT_EQ(phy->usbphy(fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20, 0)->phy_mode(),
                fuchsia_hardware_usb_phy::Mode::kHost);
      EXPECT_EQ(phy->usbphy(fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20, 1)->phy_mode(),
                mode);
      EXPECT_EQ(phy->usbphy(fuchsia_hardware_usb_phy::ProtocolVersion::kUsb30, 0)->phy_mode(),
                fuchsia_hardware_usb_phy::Mode::kHost);
    });
  }

  void CheckDevices(std::span<const std::string> devices) {
    driver_test_.RunInNodeContext([&](auto& node) {
      auto& children = node.children();

      // We expect the driver node itself to be present.
      ASSERT_TRUE(children.contains(std::string(AmlUsbPhyDevice::kDeviceName)));
      auto& aml_usb_phy_device_node_children =
          children.at(std::string(AmlUsbPhyDevice::kDeviceName)).children();

      for (const auto& device : devices) {
        EXPECT_TRUE(aml_usb_phy_device_node_children.contains(device));
      }
      // Verify no extra devices are present beyond the driver and the requested devices.
      ASSERT_EQ(aml_usb_phy_device_node_children.size(), devices.size());
    });
  }

 protected:
  fdf_testing::BackgroundDriverTest<AmlUsbPhyTestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<AmlUsbPhyTestConfig> driver_test_;
  zx::interrupt interrupt_;
};

TEST_F(AmlUsbPhyTest, SetMode) {
  CheckDevices(std::vector<std::string>{"xhci"});

  // Trigger interrupt configuring initial Host mode.
  TriggerInterruptAndCheckMode(fuchsia_hardware_usb_phy::Mode::kHost);
  // Nothing should've changed.
  CheckDevices(std::vector<std::string>{"xhci"});

  // Trigger interrupt, and switch to Peripheral mode.
  TriggerInterruptAndCheckMode(fuchsia_hardware_usb_phy::Mode::kPeripheral);
  // PHY 0 and 2 are still in Host mode (xhci), while PHY 1 is now in Peripheral mode (dwc2).
  CheckDevices(std::vector<std::string>{"xhci", "dwc2-phy"});

  // Trigger interrupt, and switch (back) to Host mode.
  TriggerInterruptAndCheckMode(fuchsia_hardware_usb_phy::Mode::kHost);
  // The dwc2 device should be removed, leaving only xhci.
  CheckDevices(std::vector<std::string>{"xhci"});
}

TEST_F(AmlUsbPhyTest, ConnectStatusChanged) {
  CheckDevices(std::vector<std::string>{"xhci"});

  zx::result result = driver_test().Connect<fuchsia_hardware_usb_phy::Service::Device>("xhci");
  ASSERT_OK(result);

  driver_test().runtime().PerformBlockingWork([usb_phy = std::move(result.value())]() {
    fdf::Arena arena('TEST');
    auto wire_result = fidl::WireCall(usb_phy).buffer(arena)->ConnectStatusChanged(true, {});
    EXPECT_OK(wire_result.status());
    ASSERT_TRUE(wire_result.value().is_ok());
  });

  driver_test().RunInDriverContext([](auto& driver) { EXPECT_TRUE(driver.dwc2_connected()); });
}

}  // namespace aml_usb_phy

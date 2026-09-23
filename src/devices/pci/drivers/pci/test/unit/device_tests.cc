// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <unistd.h>
#include <zircon/limits.h>
#include <zircon/syscalls/object.h>

#include <algorithm>
#include <cstring>
#include <memory>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/devices/pci/drivers/pci/capabilities.h"
#include "src/devices/pci/drivers/pci/capabilities/power_management.h"
#include "src/devices/pci/drivers/pci/config.h"
#include "src/devices/pci/drivers/pci/device.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_allocator.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_bus.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_config.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_pciroot.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_upstream_node.h"
#include "src/devices/pci/drivers/pci/test/fakes/test_device.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace pci {

// Creates a test device with a given device config using test defaults)

template <bool IsExtended, UpstreamNode::Type UpstreamType = UpstreamNode::Type::ROOT>
class PciDeviceTests : public ::testing::Test {
 public:
  static pci_bdf_t bdf() { return {1, 2, 3}; }

 protected:
  FakeBus& bus() { return bus_; }
  FakeUpstreamNode& upstream() { return upstream_; }

  Device& CreateTestDevice(zx_device_t* parent, const uint8_t* cfg_buf, size_t cfg_size) {
    // Copy the config dump into a device entry in the ecam.
    auto& ecam = bus_.pciroot().ecam();
    ecam.get_config_view(bdf()).WriteBuffer(0, cfg_buf, cfg_size);
    // Create the config object for the device.

    auto mmio_cfg = ecam.CreateMmioConfig(bdf());
    auto view = mmio_cfg->get_view();
    EXPECT_TRUE(view.is_ok());
    auto fake_cfg = std::make_unique<FakeMmioConfig>(bdf(), std::move(view.value()));
    // Create and initialize the fake device.
    EXPECT_OK(Device::Create(parent, std::move(fake_cfg), &upstream(), &bus(),
                             /*has_acpi=*/false, /*has_devicetree=*/false));
    return bus().get_device(bdf());
  }
  void ConfigureDownstreamDevices() { return upstream_.ConfigureDownstreamDevices(); }

  // TODO(https://fxbug.dev/42075363): Migrate test to use dispatcher integration.
  PciDeviceTests()
      : bus_(/*bus_start=*/0, /*bus_end=*/2, /*is_extended=*/IsExtended),
        upstream_(UpstreamType, 0) {}
  ~PciDeviceTests() override {
    upstream_.DisableDownstream();
    upstream_.UnplugDownstream();
  }

 private:
  FakeBus bus_;
  FakeUpstreamNode upstream_;
};

using PciDeviceTestsCam = PciDeviceTests<false>;
using PciDeviceTestsExtendedCam = PciDeviceTests<true>;
using PciDeviceTestExtendedCamBridge = PciDeviceTests<true, UpstreamNode::Type::BRIDGE>;

// Counts how many composite node specs the bus driver has published. Tests
// reset this before exercising a code path and inspect it afterwards. It has
// external linkage so bus_tests.cc can share it (both are linked into the same
// pci-unit binary alongside the device_add_composite_spec mock below).
int g_pci_composite_spec_add_count = 0;

extern "C" {
// MockDevice does not cover adding composite devices within a driver, but
// BanjoDevice:Create only needs to think it succeeded.
__EXPORT zx_status_t device_add_composite_spec(zx_device_t* dev, const char* name,
                                               const composite_node_spec_t* spec) {
  ++g_pci_composite_spec_add_count;
  return ZX_OK;
}
}

// All tests below

TEST_F(PciDeviceTestsExtendedCam, CreationTest) {
  auto& ecam = bus().pciroot().ecam();
  // This test creates a device, goes through its init sequence, links it into
  // the toplogy, and then has it linger. It will be cleaned up by TearDown()
  // releasing all objects of upstream(). If creation succeeds here and no
  // asserts happen following the test it means the fakes are built properly
  // enough and the basic interface is fulfilled.
  ecam.get_config_view(bdf()).WriteBuffer(0, kFakeQuadroDeviceConfig.data(),
                                          kFakeQuadroDeviceConfig.max_size());
  auto mmio_cfg = MmioConfig::Create(bdf(), ecam.mmio(), ecam.bus_start(),
                                     ecam.bus_start() + ecam.bus_cnt(), ecam.is_extended());
  ASSERT_OK(mmio_cfg.status_value());
  auto view = mmio_cfg->get_view();
  EXPECT_TRUE(view.is_ok());
  auto fake_cfg = std::make_unique<FakeMmioConfig>(bdf(), std::move(view.value()));
  ASSERT_OK(Device::Create(MockDevice::FakeRootParent().get(), std::move(fake_cfg), &upstream(),
                           &bus(), /*has_acpi=*/false, /*has_devicetree=*/false));

  upstream().fake_mmio_regions().FailNextAllocation(true);
  upstream().fake_pf_mmio_regions().FailNextAllocation(true);
  ConfigureDownstreamDevices();

  // Verify the created device's BDF.
  auto& dev = bus().get_device(bdf());
  ASSERT_EQ(bdf().bus_id, dev.bus_id());
  ASSERT_EQ(bdf().device_id, dev.dev_id());
  ASSERT_EQ(bdf().function_id, dev.func_id());
  ASSERT_EQ(0u, dev.segment_group());
}

// Verify BARs are allocated in size-descending order so that a large BAR
// cannot be blocked by a small BAR fragmenting the available address window.
TEST_F(PciDeviceTestsExtendedCam, BarAllocationSizeDescendingOrder) {
  [[maybe_unused]] auto& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                       kFakeQuadroDeviceConfig.max_size());
  ASSERT_EQ(false, HasNonfatalFailure());
  ConfigureDownstreamDevices();

  const auto& allocation_log = upstream().fake_mmio_regions().allocation_log();

  // BAR 0 is pre-assigned so it is allocated first, and the remaining MMIO BARs of
  // |kFakeQuadroDeviceConfig| follow in size-descending order (BAR 1, 3, 2).
  const std::vector<AllocationLogEntry> expected = {
      {.size = kTestDeviceBars[0].size, .succeeded = true},
      {.size = kTestDeviceBars[1].size, .succeeded = true},
      {.size = kTestDeviceBars[3].size, .succeeded = true},
      {.size = kTestDeviceBars[2].size, .succeeded = true},
  };
  EXPECT_EQ(allocation_log, expected);
}

// Test a normal capability chain
TEST_F(PciDeviceTestsCam, StdCapabilityTest) {
  // Copy the config dump into a device entry in the ecam.
  auto& ecam = bus().pciroot().ecam();
  ecam.get_config_view(bdf()).WriteBuffer(0, kFakeVirtioInputDeviceConfig.data(),
                                          kFakeVirtioInputDeviceConfig.max_size());
  auto cfg = ecam.CreateMmioConfig(bdf());
  ASSERT_OK(Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg), &upstream(), &bus(),
                           /*has_acpi=*/false, /*has_devicetree=*/false));
  auto& dev = bus().get_device(bdf());

  // Ensure our faked Keyboard exists.
  ASSERT_EQ(0x1af4u, dev.vendor_id());
  ASSERT_EQ(0x1052u, dev.device_id());

  // Since this is a dump of an emulated device we know it has a single MSI-X
  // capability followed by five Vendor capabilities.
  const auto& list = dev.capabilities().list;
  ASSERT_EQ(list.size(), 6u);
  EXPECT_EQ(static_cast<Capability::Id>(list[0]->id()), Capability::Id::kMsiX);
  EXPECT_EQ(static_cast<Capability::Id>(list[1]->id()), Capability::Id::kVendor);
  EXPECT_EQ(static_cast<Capability::Id>(list[2]->id()), Capability::Id::kVendor);
  EXPECT_EQ(static_cast<Capability::Id>(list[3]->id()), Capability::Id::kVendor);
  EXPECT_EQ(static_cast<Capability::Id>(list[4]->id()), Capability::Id::kVendor);
  EXPECT_EQ(static_cast<Capability::Id>(list[5]->id()), Capability::Id::kVendor);
}

// A device that is described by the devicetree publishes its fragment but not a
// composite node spec: the devicetree (pci-child-visitor) owns the composite.
TEST_F(PciDeviceTestsCam, DevicetreeDeviceSkipsCompositeSpec) {
  auto& ecam = bus().pciroot().ecam();
  ecam.get_config_view(bdf()).WriteBuffer(0, kFakeVirtioInputDeviceConfig.data(),
                                          kFakeVirtioInputDeviceConfig.max_size());
  auto cfg = ecam.CreateMmioConfig(bdf());

  g_pci_composite_spec_add_count = 0;
  ASSERT_OK(Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg), &upstream(), &bus(),
                           /*has_acpi=*/false, /*has_devicetree=*/true));
  EXPECT_EQ(g_pci_composite_spec_add_count, 0);
}

// A device that is not described by the devicetree gets its composite from the
// bus driver, as before.
TEST_F(PciDeviceTestsCam, NonDevicetreeDeviceAddsCompositeSpec) {
  auto& ecam = bus().pciroot().ecam();
  ecam.get_config_view(bdf()).WriteBuffer(0, kFakeVirtioInputDeviceConfig.data(),
                                          kFakeVirtioInputDeviceConfig.max_size());
  auto cfg = ecam.CreateMmioConfig(bdf());

  g_pci_composite_spec_add_count = 0;
  ASSERT_OK(Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg), &upstream(), &bus(),
                           /*has_acpi=*/false, /*has_devicetree=*/false));
  EXPECT_EQ(g_pci_composite_spec_add_count, 1);
}

// Test an extended capability chain
TEST_F(PciDeviceTestsExtendedCam, ExtendedCapabilityTest) {
  auto& dev = CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                               kFakeQuadroDeviceConfig.max_size());
  ASSERT_EQ(false, HasNonfatalFailure());

  // Since this is a dump of an emulated device we that it should have:
  //
  //      Capabilities: [100] Virtual Channel
  //      Capabilities: [250] Latency Tolerance Reporting
  //      Capabilities: [258] L1 PM Substates
  //      Capabilities: [128] Power Budgeting
  //      Capabilities: [600] Vendor Specific Information
  const auto& ext_list = dev.capabilities().ext_list;
  ASSERT_EQ(ext_list.size(), 5u);
  EXPECT_EQ(static_cast<ExtCapability::Id>(ext_list[0]->id()),
            ExtCapability::Id::kVirtualChannelNoMFVC);
  EXPECT_EQ(static_cast<ExtCapability::Id>(ext_list[1]->id()),
            ExtCapability::Id::kLatencyToleranceReporting);
  EXPECT_EQ(static_cast<ExtCapability::Id>(ext_list[2]->id()), ExtCapability::Id::kL1PMSubstates);
  EXPECT_EQ(static_cast<ExtCapability::Id>(ext_list[3]->id()), ExtCapability::Id::kPowerBudgeting);
  EXPECT_EQ(static_cast<ExtCapability::Id>(ext_list[4]->id()), ExtCapability::Id::kVendor);
}

// This test checks for proper handling of capability pointers that are
// invalid by pointing to inside the config header.
TEST_F(PciDeviceTestsCam, InvalidPtrCapabilityTest) {
  FakeBus bus(0, 2, true);
  FakeEcam& ecam = bus.pciroot().ecam();
  auto raw_cfg = bus.pciroot().ecam().get_config_view(bdf());
  auto* fake_dev = bus.pciroot().ecam().get_device(bdf());

  // Two valid locations, followed by a third capability pointing at BAR 1.
  const uint8_t kCap1 = 0x80;
  const uint8_t kCap2 = 0x90;
  const uint8_t kInvalidCap = 0x10;

  // Point to 0x80 as the first capability.
  fake_dev->set_vendor_id(0x8086)
      .set_device_id(0x1234)
      .set_capabilities_list(1)
      .set_capabilities_ptr(kCap1);
  raw_cfg.Write(static_cast<uint8_t>(Capability::Id::kPciPowerManagement), kCap1);
  raw_cfg.Write(kCap2, kCap1 + 1);
  raw_cfg.Write(static_cast<uint8_t>(Capability::Id::kMsiX), kCap2);
  raw_cfg.Write(kInvalidCap, kCap2 + 1);

  auto cfg = MmioConfig::Create(bdf(), ecam.mmio(), ecam.bus_start(), 2, ecam.is_extended());
  ASSERT_OK(cfg.status_value());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg.value()), &upstream(),
                           &bus, /*has_acpi=*/false, /*has_devicetree=*/false));

  // Ensure no device was added.
  EXPECT_TRUE(bus.devices().empty());
}

// This test checks for proper handling (ZX_ERR_BAD_STATE) upon
// funding a pointer cycle while parsing capabilities.
TEST_F(PciDeviceTestsCam, PtrCycleCapabilityTest) {
  // Boilerplate to.get_device a corresponding to the bdf().
  auto raw_cfg = bus().pciroot().ecam().get_config_view(bdf());
  auto* fake_dev = bus().pciroot().ecam().get_device(bdf());

  // Two valid locations, followed by a third capability pointing at BAR 1.
  const uint8_t kCap1 = 0x80;
  const uint8_t kCap2 = 0x90;
  const uint8_t kCap3 = 0xA0;

  // Create a Cycle of Cap1 -> Cap2 -> Cap3 -> Cap1
  fake_dev->set_vendor_id(0x8086)
      .set_device_id(0x1234)
      .set_capabilities_list(1)
      .set_capabilities_ptr(kCap1);
  auto cap_id = static_cast<uint8_t>(Capability::Id::kVendor);
  raw_cfg.Write(cap_id, kCap1);
  raw_cfg.Write(kCap2, kCap1 + 1);
  raw_cfg.Write(cap_id, kCap2);
  raw_cfg.Write(kCap3, kCap2 + 1);
  raw_cfg.Write(cap_id, kCap3);
  raw_cfg.Write(kCap1, kCap3 + 1);

  auto cfg = bus().pciroot().ecam().CreateMmioConfig(bdf());
  EXPECT_EQ(ZX_ERR_BAD_STATE,
            Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg), &upstream(), &bus(),
                           /*has_acpi=*/false, /*has_devicetree=*/false));

  // Ensure no device was added.
  EXPECT_TRUE(bus().devices().empty());
}

// Test that we properly bail out if we see multiple of a capability
// type that only one should exist of in a system.
TEST_F(PciDeviceTestsCam, DuplicateFixedCapabilityTest) {
  // Boilerplate to.get_device a corresponding to the bdf().
  auto raw_cfg = bus().pciroot().ecam().get_config_view(bdf());
  auto* fake_dev = bus().pciroot().ecam().get_device(bdf());

  // Two valid locations, followed by a third capability pointing at BAR 1.
  const uint8_t kCap1 = 0x80;
  const uint8_t kCap2 = 0x90;
  const uint8_t kCap3 = 0xA0;

  // Create a device with three capabilities, two of which are kPciExpress
  fake_dev->set_vendor_id(0x8086)
      .set_device_id(0x1234)
      .set_capabilities_list(1)
      .set_capabilities_ptr(kCap1);
  auto pcie_id = static_cast<uint8_t>(Capability::Id::kPciExpress);
  auto null_id = static_cast<uint8_t>(Capability::Id::kNull);
  raw_cfg.Write8(pcie_id, kCap1);
  raw_cfg.Write8(kCap2, kCap1 + 1);
  raw_cfg.Write8(null_id, kCap2);
  raw_cfg.Write8(kCap3, kCap2 + 1);
  raw_cfg.Write8(pcie_id, kCap3);
  raw_cfg.Write8(0, kCap3 + 1);

  auto cfg = bus().pciroot().ecam().CreateMmioConfig(bdf());
  EXPECT_EQ(ZX_ERR_BAD_STATE,
            Device::Create(MockDevice::FakeRootParent().get(), std::move(cfg), &upstream(), &bus(),
                           /*has_acpi=*/false, /*has_devicetree=*/false));

  // Ensure no device was added.
  EXPECT_TRUE(bus().devices().empty());
}

// Ensure we parse MSI capabilities properly in the Quadro device.
// lspci output: Capabilities: [68] MSI: Enable+ Count=1/4 Maskable- 64bit+
TEST_F(PciDeviceTestsExtendedCam, MsiCapabilityTest) {
  auto& dev = CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                               kFakeQuadroDeviceConfig.max_size());
  ASSERT_EQ(false, HasNonfatalFailure());
  ASSERT_NE(nullptr, dev.capabilities().msi);

  auto& msi = *dev.capabilities().msi;
  EXPECT_EQ(0x68u, msi.base());
  EXPECT_EQ(static_cast<uint8_t>(Capability::Id::kMsi), msi.id());
  EXPECT_EQ(true, msi.is_64bit());
  EXPECT_EQ(4u, msi.vectors_avail());
  EXPECT_EQ(false, msi.supports_pvm());

  // MSI should be disabled by Device initialization.
  const MsiControlReg ctrl = {.value = dev.config()->Read(msi.ctrl())};
  EXPECT_EQ(0, ctrl.enable());
}

// Ensure we parse MSIX capabilities properly in the Virtio-input device.
TEST_F(PciDeviceTestsExtendedCam, MsixCapabilityTest) {
  auto& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeVirtioInputDeviceConfig.data(),
                       kFakeVirtioInputDeviceConfig.max_size());
  ASSERT_EQ(false, HasNonfatalFailure());
  ASSERT_NE(nullptr, dev.capabilities().msix);

  auto& msix = *dev.capabilities().msix;
  EXPECT_EQ(0x98u, msix.base());
  EXPECT_EQ(static_cast<uint8_t>(Capability::Id::kMsiX), msix.id());
  EXPECT_EQ(1u, msix.table_bar());
  EXPECT_EQ(0u, msix.table_offset());
  EXPECT_EQ(2u, msix.table_size());
  EXPECT_EQ(1u, msix.pba_bar());
  EXPECT_EQ(0x800u, msix.pba_offset());

  // MSI-X should be disabled by Device initialization.
  const MsixControlReg ctrl = {.value = dev.config()->Read(msix.ctrl())};
  EXPECT_EQ(0u, ctrl.enable());
}

TEST_F(PciDeviceTestsExtendedCam, MapInterruptMsi) {
  pci::Device& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                       kFakeQuadroDeviceConfig.max_size());
  const auto mode = fuchsia_hardware_pci::InterruptMode::kMsi;
  ASSERT_OK(dev.SetIrqMode(mode, 1));

  auto result = dev.MapInterrupt(0);
  ASSERT_TRUE(result.is_ok());
  EXPECT_TRUE(result.value().is_valid());
  result.value().reset();
}

TEST_F(PciDeviceTestsExtendedCam, MapInterruptMsiX) {
  pci::Device& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                       kFakeQuadroDeviceConfig.max_size());
  ConfigureDownstreamDevices();
  const auto mode = fuchsia_hardware_pci::InterruptMode::kMsiX;
  ASSERT_OK(dev.SetIrqMode(mode, 1));

  auto result = dev.MapInterrupt(0);
  ASSERT_TRUE(result.is_ok());
  EXPECT_TRUE(result.value().is_valid());
  result.value().reset();
}

// Verify that power state transitions wait the necessary amount of time, and that they end up in
// the correct state.
TEST_F(PciDeviceTestsExtendedCam, PowerStateTransitions) {
  pci::Device& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                       kFakeQuadroDeviceConfig.max_size());
  pci::Config& cfg = *dev.config();

  auto power = PowerManagementCapability(*dev.config(), kFakeQuadroPowerManagementCapabilityOffset);
  auto test_recovery_delay = [&cfg, &power](
                                 PowerManagementCapability::PowerState start_state,
                                 PowerManagementCapability::PowerState end_state) -> bool {
    // Manually update our starting state
    PmcsrReg pmcsr{.value = cfg.Read(power.pmcsr())};
    pmcsr.set_power_state(start_state);
    cfg.Write(power.pmcsr(), pmcsr.value);
    // Time the transition
    const zx::time start_time = zx::clock::get_monotonic();
    power.SetPowerState(cfg, end_state);
    const zx::time end_time = zx::clock::get_monotonic();
    const zx::duration min_delay =
        PowerManagementCapability::kStateRecoveryTime[start_state][end_state];
    return (end_time - start_time > min_delay);
  };

  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D0,
                                  PowerManagementCapability::PowerState::D1));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D1);

  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D0,
                                  PowerManagementCapability::PowerState::D2));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D2);

  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D0,
                                  PowerManagementCapability::PowerState::D3));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D3);

  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D3,
                                  PowerManagementCapability::PowerState::D0));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D0);

  // D0 to D0 should be essentially a no-op return.
  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D0,
                                  PowerManagementCapability::PowerState::D0));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D0);

  // D1 to D2 should actually run D1 > D0 > D2 and hit both code paths.
  ASSERT_TRUE(test_recovery_delay(PowerManagementCapability::PowerState::D2,
                                  PowerManagementCapability::PowerState::D1));
  ASSERT_EQ(dev.GetPowerState().value(), PowerManagementCapability::PowerState::D1);
}

// Verify that a prefetchable BAR behind a bridge falls back to the non-prefetchable
// MMIO window when PF-MMIO allocation fails.
TEST_F(PciDeviceTestExtendedCamBridge, PrefetchableBridgeBarMmioFallback) {
  [[maybe_unused]] auto& dev =
      CreateTestDevice(MockDevice::FakeRootParent().get(), kFakeQuadroDeviceConfig.data(),
                       kFakeQuadroDeviceConfig.max_size());

  // BAR 0 of |kFakeQuadroDeviceConfig| is non-prefetchable and allocates from the regular MMIO
  // window. BAR 1 of |kFakeQuadroDeviceConfig| is the first prefetchable BAR, so it will be the
  // first to attempt allocation from the PF-MMIO window.

  // Fail the next prefetchable allocation (BAR 1) in order to trigger the fallback to the
  // non-prefetchable MMIO window of the bridge.
  auto& pf_alloc = upstream().fake_pf_mmio_regions();
  auto& mmio_alloc = upstream().fake_mmio_regions();
  pf_alloc.FailNextAllocation(/*assigned_only=*/false);
  ConfigureDownstreamDevices();

  // BAR 1 fails the prefetchable allocation.
  const auto& pf_log = pf_alloc.allocation_log();
  EXPECT_TRUE(std::ranges::any_of(pf_log, [](const AllocationLogEntry& e) {
    return e.size == kTestDeviceBars[1].size && !e.succeeded;
  }));
  // BAR 1 successfully falls back to the non-prefetchable allocation.
  const auto& mmio_log = mmio_alloc.allocation_log();
  EXPECT_TRUE(std::ranges::any_of(mmio_log, [](const AllocationLogEntry& e) {
    return e.size == kTestDeviceBars[1].size && e.succeeded;
  }));
}

}  // namespace pci

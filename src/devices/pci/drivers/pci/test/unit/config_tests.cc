// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/zx/result.h>
#include <zircon/limits.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/devices/pci/drivers/pci/config.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_pciroot.h"
#include "src/lib/testing/predicates/status.h"

namespace pci {

static constexpr pci_bdf_t kDefaultBdf1 = {0, 1, 2};
static constexpr pci_bdf_t kDefaultBdf2 = {1, 2, 3};

template <bool IsExtended>
class PciConfigTests : public ::testing::Test {
 public:
  FakePciroot& pciroot() { return *pciroot_; }
  ddk::PcirootProtocolClient& pciroot_client() { return *client_; }

 protected:
  void SetUp() final {
    pciroot_ = std::make_unique<FakePciroot>(/*bus_start=*/0, /*bus_cnt=*/2, IsExtended);
    client_ = std::make_unique<ddk::PcirootProtocolClient>(pciroot_->proto());
  }

  void TearDown() final {
    client_.reset();
    pciroot_.reset();
  }

 private:
  std::unique_ptr<FakePciroot> pciroot_;
  std::unique_ptr<ddk::PcirootProtocolClient> client_;
};

using PciConfigBaseTests = PciConfigTests<false>;
using PciConfigExtendedTests = PciConfigTests<true>;

namespace {

void DeviceConfigIntegrationImpl(FakeEcam& ecam) {
  FakePciType0Config* dev1 = ecam.get_device(kDefaultBdf1);
  dev1->set_vendor_id(0x8086)
      .set_device_id(0x1234)
      .set_header_type(0x01)
      .set_revision_id(12)
      .set_expansion_rom_address(0xFF0000EE);
  // Test 8, 16, and 32 bit reads.
  auto cfg1 = ecam.CreateMmioConfig(kDefaultBdf1);
  EXPECT_EQ(dev1->revision_id(), cfg1->Read(Config::kRevisionId));
  EXPECT_EQ(dev1->vendor_id(), cfg1->Read(Config::kVendorId));
  EXPECT_EQ(dev1->device_id(), cfg1->Read(Config::kDeviceId));
  EXPECT_EQ(dev1->header_type(), cfg1->Read(Config::kHeaderType));
  EXPECT_EQ(dev1->expansion_rom_address(), cfg1->Read(Config::kExpansionRomAddress));
  // Now try the same thing for a different, unconfigured device and ensure they aren't
  // overlapping somehow.
  auto cfg2 = ecam.CreateMmioConfig(kDefaultBdf2);
  EXPECT_EQ(0x0u, cfg2->Read(Config::kRevisionId));
  EXPECT_EQ(0xFFFFu, cfg2->Read(Config::kVendorId));
  EXPECT_EQ(0xFFFFu, cfg2->Read(Config::kDeviceId));
  EXPECT_EQ(0x0u, cfg2->Read(Config::kHeaderType));
  EXPECT_EQ(0x0u, cfg2->Read(Config::kExpansionRomAddress));

  FakePciType0Config* dev2 = ecam.get_device(kDefaultBdf2);
  dev2->set_vendor_id(0x8680)
      .set_device_id(0x4321)
      .set_header_type(0x02)
      .set_revision_id(3)
      .set_expansion_rom_address(0xEE0000FF);

  EXPECT_EQ(dev2->revision_id(), cfg2->Read(Config::kRevisionId));
  EXPECT_EQ(dev2->vendor_id(), cfg2->Read(Config::kVendorId));
  EXPECT_EQ(dev2->device_id(), cfg2->Read(Config::kDeviceId));
  EXPECT_EQ(dev2->header_type(), cfg2->Read(Config::kHeaderType));
  EXPECT_EQ(dev2->expansion_rom_address(), cfg2->Read(Config::kExpansionRomAddress));
}

void ConfirmDevConfigReadResetImpl(pci::Config* cfg, FakePciType0Config* dev) {
  ASSERT_EQ(0xFFFFu, dev->vendor_id());
  ASSERT_EQ(0xFFFFu, dev->device_id());
  ASSERT_EQ(0x0u, dev->command());
  ASSERT_EQ(0x0u, dev->status());
  ASSERT_EQ(0x0u, dev->revision_id());
  ASSERT_EQ(0x0u, dev->program_interface());
  ASSERT_EQ(0x0u, dev->sub_class());
  ASSERT_EQ(0x0u, dev->base_class());
  ASSERT_EQ(0x0u, dev->cache_line_size());
  ASSERT_EQ(0x0u, dev->latency_timer());
  ASSERT_EQ(0x0u, dev->header_type());
  ASSERT_EQ(0x0u, dev->bist());
  ASSERT_EQ(0x0u, dev->cardbus_cis_ptr());
  ASSERT_EQ(0x0u, dev->subsystem_vendor_id());
  ASSERT_EQ(0x0u, dev->subsystem_id());
  ASSERT_EQ(0x0u, dev->expansion_rom_address());
  ASSERT_EQ(0x0u, dev->capabilities_ptr());
  ASSERT_EQ(0x0u, dev->interrupt_line());
  ASSERT_EQ(0x0u, dev->interrupt_pin());
  ASSERT_EQ(0x0u, dev->min_grant());
  ASSERT_EQ(0x0u, dev->max_latency());

  // Ensure the config header reads match the reset values above, this time
  // through the config interface.
  EXPECT_EQ(0xFFFFu, cfg->Read(Config::kVendorId));
  EXPECT_EQ(0xFFFFu, cfg->Read(Config::kDeviceId));
  EXPECT_EQ(0x0u, cfg->Read(Config::kCommand));
  EXPECT_EQ(0x0u, cfg->Read(Config::kStatus));
  EXPECT_EQ(0x0u, cfg->Read(Config::kRevisionId));
  EXPECT_EQ(0x0u, cfg->Read(Config::kProgramInterface));
  EXPECT_EQ(0x0u, cfg->Read(Config::kSubClass));
  EXPECT_EQ(0x0u, cfg->Read(Config::kBaseClass));
  EXPECT_EQ(0x0u, cfg->Read(Config::kCacheLineSize));
  EXPECT_EQ(0x0u, cfg->Read(Config::kLatencyTimer));
  EXPECT_EQ(0x0u, cfg->Read(Config::kHeaderType));
  EXPECT_EQ(0x0u, cfg->Read(Config::kBist));
  EXPECT_EQ(0x0u, cfg->Read(Config::kCardbusCisPtr));
  EXPECT_EQ(0x0u, cfg->Read(Config::kSubsystemVendorId));
  EXPECT_EQ(0x0u, cfg->Read(Config::kSubsystemId));
  EXPECT_EQ(0x0u, cfg->Read(Config::kExpansionRomAddress));
  EXPECT_EQ(0x0u, cfg->Read(Config::kCapabilitiesPtr));
  EXPECT_EQ(0x0u, cfg->Read(Config::kInterruptLine));
  EXPECT_EQ(0x0u, cfg->Read(Config::kInterruptPin));
  EXPECT_EQ(0x0u, cfg->Read(Config::kMinGrant));
  EXPECT_EQ(0x0u, cfg->Read(Config::kMaxLatency));
}

// This tests verifies the read/write interface for config works with both the actual config type,
// and the test based PciType0Config type.
void ConfigReadWriteIntegrationImpl(pci::Config* cfg, FakePciType0Config* dev) {
  // Write test data to the config header registers.
  uint32_t value = 0x00;
  cfg->Write(Config::kVendorId, ++value);
  EXPECT_EQ(value, dev->vendor_id());
  EXPECT_EQ(value, cfg->Read(Config::kVendorId));

  cfg->Write(Config::kDeviceId, ++value);
  EXPECT_EQ(value, dev->device_id());
  EXPECT_EQ(value, cfg->Read(Config::kDeviceId));

  cfg->Write(Config::kCommand, ++value);
  EXPECT_EQ(value, dev->command());
  EXPECT_EQ(value, cfg->Read(Config::kCommand));

  cfg->Write(Config::kStatus, ++value);
  EXPECT_EQ(value, dev->status());
  EXPECT_EQ(value, cfg->Read(Config::kStatus));

  cfg->Write(Config::kRevisionId, ++value);
  EXPECT_EQ(value, dev->revision_id());
  EXPECT_EQ(value, cfg->Read(Config::kRevisionId));

  cfg->Write(Config::kProgramInterface, ++value);
  EXPECT_EQ(value, dev->program_interface());
  EXPECT_EQ(value, cfg->Read(Config::kProgramInterface));

  cfg->Write(Config::kSubClass, ++value);
  EXPECT_EQ(value, dev->sub_class());
  EXPECT_EQ(value, cfg->Read(Config::kSubClass));

  cfg->Write(Config::kBaseClass, ++value);
  EXPECT_EQ(value, dev->base_class());
  EXPECT_EQ(value, cfg->Read(Config::kBaseClass));

  cfg->Write(Config::kCacheLineSize, ++value);
  EXPECT_EQ(value, dev->cache_line_size());
  EXPECT_EQ(value, cfg->Read(Config::kCacheLineSize));

  cfg->Write(Config::kLatencyTimer, ++value);
  EXPECT_EQ(value, dev->latency_timer());
  EXPECT_EQ(value, cfg->Read(Config::kLatencyTimer));

  cfg->Write(Config::kHeaderType, ++value);
  EXPECT_EQ(value, dev->header_type());
  EXPECT_EQ(value, cfg->Read(Config::kHeaderType));

  cfg->Write(Config::kBist, ++value);
  EXPECT_EQ(value, dev->bist());
  EXPECT_EQ(value, cfg->Read(Config::kBist));

  cfg->Write(Config::kCardbusCisPtr, ++value);
  EXPECT_EQ(value, dev->cardbus_cis_ptr());
  EXPECT_EQ(value, cfg->Read(Config::kCardbusCisPtr));

  cfg->Write(Config::kSubsystemVendorId, ++value);
  EXPECT_EQ(value, dev->subsystem_vendor_id());
  EXPECT_EQ(value, cfg->Read(Config::kSubsystemVendorId));

  cfg->Write(Config::kSubsystemId, ++value);
  EXPECT_EQ(value, dev->subsystem_id());
  EXPECT_EQ(value, cfg->Read(Config::kSubsystemId));

  cfg->Write(Config::kExpansionRomAddress, ++value);
  EXPECT_EQ(value, dev->expansion_rom_address());
  EXPECT_EQ(value, cfg->Read(Config::kExpansionRomAddress));

  cfg->Write(Config::kCapabilitiesPtr, ++value);
  EXPECT_EQ(value, dev->capabilities_ptr());
  EXPECT_EQ(value, cfg->Read(Config::kCapabilitiesPtr));

  cfg->Write(Config::kInterruptLine, ++value);
  EXPECT_EQ(value, dev->interrupt_line());
  EXPECT_EQ(value, cfg->Read(Config::kInterruptLine));

  cfg->Write(Config::kInterruptPin, ++value);
  EXPECT_EQ(value, dev->interrupt_pin());
  EXPECT_EQ(value, cfg->Read(Config::kInterruptPin));

  cfg->Write(Config::kMinGrant, ++value);
  EXPECT_EQ(value, dev->min_grant());
  EXPECT_EQ(value, cfg->Read(Config::kMinGrant));

  cfg->Write(Config::kMaxLatency, ++value);
  EXPECT_EQ(value, dev->max_latency());
  EXPECT_EQ(value, cfg->Read(Config::kMaxLatency));
}

void MmioConfigGetViewImpl(FakePciroot& pciroot) {
  auto& ecam = pciroot.ecam();
  zx::result<fdf::MmioView> view = ecam.CreateMmioConfig(kDefaultBdf1)->get_view();
  ASSERT_OK(view);
  ASSERT_EQ(view->get_size(), pciroot.ecam().config_size());
  ASSERT_EQ(view->get_offset(), ecam.GetConfigOffset(kDefaultBdf1));
  ASSERT_EQ(view->get_vmo()->get(), pciroot.ecam().mmio().get_vmo()->get());
}

}  // namespace

TEST_F(PciConfigBaseTests, DeviceConfigIntegration) {
  DeviceConfigIntegrationImpl(pciroot().ecam());
}

TEST_F(PciConfigExtendedTests, DeviceConfigIntegration) {
  DeviceConfigIntegrationImpl(pciroot().ecam());
}

TEST_F(PciConfigBaseTests, MmioConfigReadWrite) {
  auto cfg = pciroot().ecam().CreateMmioConfig(kDefaultBdf1);
  auto* dev = pciroot().ecam().get_device(kDefaultBdf1);
  ConfirmDevConfigReadResetImpl(cfg.get(), dev);
  ConfigReadWriteIntegrationImpl(cfg.get(), dev);
}

TEST_F(PciConfigExtendedTests, MmioConfigReadWrite) {
  auto cfg = pciroot().ecam().CreateMmioConfig(kDefaultBdf1);
  auto* dev = pciroot().ecam().get_device(kDefaultBdf1);
  ConfirmDevConfigReadResetImpl(cfg.get(), dev);
  ConfigReadWriteIntegrationImpl(cfg.get(), dev);
}

TEST_F(PciConfigBaseTests, ProxyConfigReadWrite) {
  auto cfg = ProxyConfig::Create(kDefaultBdf1, &pciroot_client());
  auto* dev = pciroot().ecam().get_device(kDefaultBdf1);
  ConfigReadWriteIntegrationImpl(cfg.value().get(), dev);
}

TEST_F(PciConfigExtendedTests, ProxyConfigReadWrite) {
  auto cfg = ProxyConfig::Create(kDefaultBdf1, &pciroot_client());
  auto* dev = pciroot().ecam().get_device(kDefaultBdf1);
  ConfigReadWriteIntegrationImpl(cfg.value().get(), dev);
}

TEST_F(PciConfigBaseTests, ProxyConfigGetView) {
  auto proxy_cfg = ProxyConfig::Create(kDefaultBdf1, &pciroot_client());
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, proxy_cfg->get_view().status_value());
}

TEST_F(PciConfigBaseTests, MmioConfigGetView) { MmioConfigGetViewImpl(pciroot()); }
TEST_F(PciConfigExtendedTests, MmioConfigGetView) { MmioConfigGetViewImpl(pciroot()); }

}  // namespace pci

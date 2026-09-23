// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/pci/constants.h>
#include <lib/zx/vmo.h>
#include <zircon/limits.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <memory>

#include <fbl/hard_int.h>
#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/devices/pci/drivers/pci/capabilities/msix.h"
#include "src/devices/pci/drivers/pci/config.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_allocator.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_config.h"
#include "src/lib/testing/predicates/status.h"

namespace pci {

namespace {
class PciCapabilityTests : public ::testing::Test {
 public:
  PciCapabilityTests() = default;
  fdf::MmioBuffer& mmio() { return *mmio_; }
  fdf::MmioView view() { return mmio_->View(0, mmio_->get_size()); }
  static pci_bdf_t bdf() { return {0, 0, 0}; }
  static Bar CreateBar(uint8_t bar_id, size_t size) {
    Bar bar = {
        .size = size,
        .bar_id = bar_id,
        .allocation =
            std::make_unique<FakeAllocation>(PCI_ADDRESS_SPACE_MEMORY, std::nullopt, size),
    };
    return bar;
  }

  static void ConfigureMsixCapability(Config& cfg, uint8_t tbar, uint8_t pbar, zx_paddr_t toffset,
                                      zx_paddr_t poffset, size_t vectors = 8) {
    MsixControlReg ctrl{};
    MsixTableReg table{};
    MsixPbaReg pba{};

    ctrl.set_table_size(vectors - 1);
    table.set_offset(toffset);
    table.set_bir(tbar);
    pba.set_offset(poffset);
    pba.set_bir(pbar);

    cfg.Write(PciReg16(MsixCapability::kMsixControlRegisterOffset), ctrl.value);
    cfg.Write(PciReg32(MsixCapability::kMsixTableRegisterOffset), table.reg_value());
    cfg.Write(PciReg32(MsixCapability::kMsixPbaRegisterOffset), pba.reg_value());
  }

 protected:
  void SetUp() final {
    zx::vmo vmo;
    mmio_ = fdf_testing::CreateMmioBuffer(pci::kBaseConfigSize, ZX_CACHE_POLICY_UNCACHED);
  }

  void TearDown() final { mmio_->reset(); }

 private:
  std::optional<fdf::MmioBuffer> mmio_;
};

TEST_F(PciCapabilityTests, FixtureTest) {
  MmioConfig cfg = FakeMmioConfig(bdf(), view());
  size_t vectors = 8;
  uint8_t tbar = 1;
  uint8_t pbar = 2;
  zx_paddr_t toffset = 0x4000;
  zx_paddr_t poffset = 0x8000;
  ConfigureMsixCapability(cfg, tbar, pbar, toffset, poffset, vectors);
  ASSERT_EQ(mmio().Read16(MsixCapability::kMsixControlRegisterOffset), vectors - 1u);
  ASSERT_EQ(mmio().Read32(MsixCapability::kMsixTableRegisterOffset), toffset | tbar);
  ASSERT_EQ(mmio().Read32(MsixCapability::kMsixPbaRegisterOffset), poffset | pbar);
}

TEST_F(PciCapabilityTests, InitTest) {
  MmioConfig cfg = FakeMmioConfig(bdf(), view());
  MsixCapability msix(cfg, 0);
  ConfigureMsixCapability(cfg, /*tbar=*/1, /*pbar=*/1, /*toffset=*/0x4000, /*poffset=*/0x8000);
  Bar bar = CreateBar(1, 0xC000);
  // Catch double initializations.
  ASSERT_OK(msix.Init(bar, bar));
  ASSERT_STATUS(msix.Init(bar, bar), ZX_ERR_BAD_STATE);
}

}  // namespace
}  // namespace pci

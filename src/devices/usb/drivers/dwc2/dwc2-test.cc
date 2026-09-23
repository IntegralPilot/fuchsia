// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc2/dwc2.h"

#include <fidl/fuchsia.driver.compat/cpp/test_base.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <mutex>

#include <gtest/gtest.h>

#include "src/devices/usb/drivers/dwc2/dwc2_config.h"
#include "src/devices/usb/drivers/dwc2/usb_dwc_regs.h"

namespace dwc2 {

namespace fpdev = fuchsia_hardware_platform_device;

class Environment : public fdf_testing::Environment {
 public:
  Environment() {
    fdf_fake::FakePDev::Config cfg{};
    cfg.mmios[0] = mmio_.GetMmioBuffer();
    cfg.use_fake_bti = true;
    cfg.use_fake_irq = true;

    // Pretend to be a v3.30a DWC2 core; the same version used in AML smart
    // display devices.
    //
    // All these tests care about is init/shutdown, not actually starting or
    // operating the controller.  All we should need to do is emulate:
    // 1) the SynopsisID register
    // 2) HWCFG2 (which reports whether or not dynamic FIFOs are supported)
    // 3) HWCFG4 (which reports whether or not IN endpoints use a shared FIFO or dedicated FIFOs)
    // 4) and the global reset register (GRSTCTL).
    mmio_[GSNPSID::Get().addr()].SetReadCallback([]() -> uint64_t { return 0x4f54330a; });
    mmio_[GHWCFG2::Get().addr()].SetReadCallback([]() -> uint64_t { return 0x2288d854; });
    mmio_[GHWCFG3::Get().addr()].SetReadCallback([]() -> uint64_t { return 0x40000000; });
    mmio_[GHWCFG4::Get().addr()].SetReadCallback([]() -> uint64_t { return 0xd6028030; });
    mmio_[GRSTCTL::Get().addr()].SetReadCallback([this]() { return fake_grstctl_.Read(); });
    mmio_[GRSTCTL::Get().addr()].SetWriteCallback([this](uint64_t v) { fake_grstctl_.Write(v); });

    pdev_.SetConfig(std::move(cfg));
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    static const fuchsia_hardware_usb_dwc2::Metadata kMetadata(
        {.dma_burst_len = fuchsia_hardware_usb_dwc2::DmaBurstLen::kSingle,
         .usb_turnaround_time = 10,
         .rx_fifo_size = 1024,
         .nptx_fifo_size = 1024,
         .tx_fifo_sizes =
             std::array<uint32_t, 15>{1024, 1024, 1024, 1024, 1024, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}});

    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    pdev_.AddFidlMetadata(fuchsia_hardware_usb_dwc2::Metadata::kSerializableName, kMetadata);
    EXPECT_EQ(to_driver_vfs.AddService<fpdev::Service>(pdev_.GetInstanceHandler(dispatcher), "pdev")
                  .status_value(),
              ZX_OK);

    return zx::ok();
  }

 private:
  // A class which manages a poor emulation of the global reset register (for
  // core silicon versions < 4.20a).  The only thing this emulation does is look
  // for writes to the Soft Reset bit, after which it will pretend to be
  // resetting for 3 read-cycles, and report that the AHB is busy for 6
  // read-cycles.
  class FakeGRSTCTL {
   public:
    FakeGRSTCTL() { Reset(); }

    void Reset() {
      val_ = GRSTCTL::Get().FromValue(0).set_ahbidle(1).reg_value();
      reset_in_progress_cycles_ = 0;
    }

    uint64_t Read() {
      GRSTCTL new_val = GRSTCTL::Get().FromValue(val_);

      if (reset_in_progress_cycles_ <= kSoftResetBitCycleThresh) {
        new_val.set_csftrst(0);
      }

      if (reset_in_progress_cycles_ <= kAHBIdleCycleThresh) {
        new_val.set_ahbidle(1);
      }

      if (reset_in_progress_cycles_) {
        --reset_in_progress_cycles_;
      }

      return val_ = new_val.reg_value();
    }

    void Write(uint64_t val64) {
      // We don't expect the DUT to be doing anything but reading the existing
      // value and attempting to set the csftrst bit.  If it tries to set any
      // other bit, fail the test so that someone can come back here and update
      // the test.
      ASSERT_EQ(uint64_t{0}, val64 & 0xFFFF'FFFF'0000'0000);
      const uint32_t val = static_cast<uint32_t>(val64);
      const uint32_t disallow_mask = ~GRSTCTL::Get()
                                          .FromValue(0)
                                          .set_csftrst(1)
                                          .set_ahbidle(1)
                                          .set_txfflsh(1)
                                          .set_rxfflsh(1)
                                          .set_txfnum(0x1F)
                                          .reg_value();
      EXPECT_EQ(0u, val & disallow_mask);

      // If the user is setting the Soft Reset bit, set the bit in our state as
      // well as clearing the AHBIdle state, and set the reset-cycle countdown
      // so that these bits will clear over time as the register is read.
      GRSTCTL set_val = GRSTCTL::Get().FromValue(val);
      if (set_val.csftrst()) {
        val_ = GRSTCTL::Get().FromValue(val_).set_csftrst(1).set_ahbidle(0).reg_value();
        reset_in_progress_cycles_ = kTotalResetCycles;
      }
    }

   private:
    uint32_t kTotalResetCycles = 6;
    uint32_t kSoftResetBitCycleThresh = 3;
    uint32_t kAHBIdleCycleThresh = 0;

    uint32_t val_;
    uint32_t reset_in_progress_cycles_;
  };

  fdf_fake::FakePDev pdev_;
  fake_mmio::FakeMmioRegRegion mmio_{sizeof(uint32_t), 4096};
  FakeGRSTCTL fake_grstctl_;
};

class Config {
 public:
  using DriverType = Dwc2;
  using EnvironmentType = Environment;
};

class Dwc2Test : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_TRUE(dut_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      dwc2_config::Config cfg;
                      cfg.enable_suspend() = false;
                      args.config(cfg.ToVmo());
                    })
                    .is_ok());
  }

  void TearDown() override { EXPECT_TRUE(dut_.StopDriver().is_ok()); }

 protected:
  Dwc2::Endpoint* GetEndpoint(Dwc2& driver, uint8_t ep_num) {
    if (ep_num < std::size(driver.endpoints_) && driver.endpoints_[ep_num].has_value()) {
      return &*driver.endpoints_[ep_num];
    }
    return nullptr;
  }

  void QueueNextRequest(Dwc2& driver, Dwc2::Endpoint* ep) __TA_REQUIRES(ep->lock) {
    driver.QueueNextRequest(ep);
  }

  fdf_testing::BackgroundDriverTest<Config> dut_;
};

TEST_F(Dwc2Test, DriverLifetimeTest) {}

TEST_F(Dwc2Test, GetHardwareInfo) {
  zx::result client_end = dut_.Connect<fuchsia_hardware_usb_dci::UsbDciService::Device>();
  ASSERT_TRUE(client_end.is_ok());
  fidl::SyncClient client(std::move(*client_end));

  auto result = client->GetHardwareInfo();
  ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();

  auto& info = result->info();
  // The number of endpoints (5 IN + 5 OUT) is derived from the fake GHWCFG2
  // and GHWCFG4 values set in the Environment.
  // 5 IN endpoints (capped by num_dev_ep = 6) + 5 OUT endpoints (num_dev_ep = 6, minus EP0)
  EXPECT_EQ(info.endpoints()->size(), 10u);

  // EP1 IN:
  EXPECT_EQ(info.endpoints()->at(0).ep_address(), 0x81);
  // The max_packet_size_limit for IN endpoints is 4096 because the emulated
  // GHWCFG4 (0xd6028030) indicates support for dedicated IN FIFOs, and the
  // driver combines 4x 1024-byte TX FIFOs (from metadata) for IN endpoints.
  ASSERT_TRUE(info.endpoints()->at(0).supported_types().has_value());
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->size(), 2u);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(0).max_packet_size_limit(), 4096u);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(0).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(1).max_packet_size_limit(), 4096u);
  EXPECT_EQ(info.endpoints()->at(0).supported_types()->at(1).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);

  // EP1 OUT (after 5 IN endpoints):
  EXPECT_EQ(info.endpoints()->at(5).ep_address(), 0x01);
  // The max_packet_size_limit for OUT endpoints is 1024, matching the
  // rx_fifo_size provided in the metadata.
  ASSERT_TRUE(info.endpoints()->at(5).supported_types().has_value());
  EXPECT_EQ(info.endpoints()->at(5).supported_types()->size(), 2u);
  EXPECT_EQ(info.endpoints()->at(5).supported_types()->at(0).max_packet_size_limit(), 1024u);
  EXPECT_EQ(info.endpoints()->at(5).supported_types()->at(0).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  EXPECT_EQ(info.endpoints()->at(5).supported_types()->at(1).max_packet_size_limit(), 1024u);
  EXPECT_EQ(info.endpoints()->at(5).supported_types()->at(1).endpoint_type(),
            fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
}

TEST_F(Dwc2Test, PendingZlp_SetOnInShortTransfer) {
  dut_.RunInDriverContext([this](Dwc2& driver) {
    uint8_t ep_num = DWC_ADDR_TO_INDEX(0x81);  // EP 1 IN
    auto* ep = GetEndpoint(driver, ep_num);
    ASSERT_NE(ep, nullptr);

    std::lock_guard<std::mutex> lock(ep->lock);
    ep->max_packet_size = 64;
    ep->enabled = true;
    EXPECT_FALSE(ep->pending_zlp);

    usb::FidlRequest fidl_req(usb::EndpointType::BULK);
    fidl_req->short_(true);
    fidl_req.add_data(std::vector<uint8_t>(64, 0), 64, 0);

    ep->queued_reqs.push(std::move(fidl_req));

    QueueNextRequest(driver, ep);
    EXPECT_TRUE(ep->pending_zlp);
  });
}

TEST_F(Dwc2Test, PendingZlp_NotSetWithoutShortFlag) {
  dut_.RunInDriverContext([this](Dwc2& driver) {
    uint8_t ep_num = DWC_ADDR_TO_INDEX(0x81);  // EP 1 IN
    auto* ep = GetEndpoint(driver, ep_num);
    ASSERT_NE(ep, nullptr);

    std::lock_guard<std::mutex> lock(ep->lock);
    ep->max_packet_size = 64;
    ep->enabled = true;
    EXPECT_FALSE(ep->pending_zlp);

    usb::FidlRequest fidl_req(usb::EndpointType::BULK);
    fidl_req->short_(false);
    fidl_req.add_data(std::vector<uint8_t>(64, 0), 64, 0);

    ep->queued_reqs.push(std::move(fidl_req));

    QueueNextRequest(driver, ep);
    EXPECT_FALSE(ep->pending_zlp);
  });
}

TEST_F(Dwc2Test, PendingZlp_NotSetIfLengthNotMultiple) {
  dut_.RunInDriverContext([this](Dwc2& driver) {
    uint8_t ep_num = DWC_ADDR_TO_INDEX(0x81);  // EP 1 IN
    auto* ep = GetEndpoint(driver, ep_num);
    ASSERT_NE(ep, nullptr);

    std::lock_guard<std::mutex> lock(ep->lock);
    ep->max_packet_size = 64;
    ep->enabled = true;
    EXPECT_FALSE(ep->pending_zlp);

    usb::FidlRequest fidl_req(usb::EndpointType::BULK);
    fidl_req->short_(true);
    fidl_req.add_data(std::vector<uint8_t>(50, 0), 50, 0);

    ep->queued_reqs.push(std::move(fidl_req));

    QueueNextRequest(driver, ep);
    EXPECT_FALSE(ep->pending_zlp);
  });
}

TEST_F(Dwc2Test, PendingZlp_ClearedOnCancelAll) {
  dut_.RunInDriverContext([this](Dwc2& driver) {
    uint8_t ep_num = DWC_ADDR_TO_INDEX(0x81);  // EP 1 IN
    auto* ep = GetEndpoint(driver, ep_num);
    ASSERT_NE(ep, nullptr);

    {
      std::lock_guard<std::mutex> lock(ep->lock);
      ep->max_packet_size = 64;
      ep->enabled = true;
      ASSERT_FALSE(ep->pending_zlp);

      usb::FidlRequest fidl_req(usb::EndpointType::BULK);
      fidl_req->short_(true);
      fidl_req.add_data(std::vector<uint8_t>(64, 0), 64, 0);

      ep->queued_reqs.push(std::move(fidl_req));

      QueueNextRequest(driver, ep);
      ASSERT_TRUE(ep->pending_zlp);
    }

    // CancelAll() acquires lock.
    ep->CancelAll();
    EXPECT_FALSE(ep->pending_zlp);
  });
}

}  // namespace dwc2

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <atomic>

#include <gtest/gtest.h>

#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3-test-fixture.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

class Dwc3CommandsTest : public TestFixture<true> {
 protected:
  std::atomic<uint32_t> depcmd_val_{0};
  std::atomic<uint32_t> depcfg0_val_{0};
  std::atomic<uint32_t> depcfg1_val_{0};
  std::atomic<uint32_t> depcmdpar0_val_{0};
  std::atomic<uint32_t> depcmdpar1_val_{0};
  std::atomic<bool> cmd_written_{false};
  std::atomic<uint32_t> command_param_{0xFFFFFFFFu};

  void MockDepCmd(uint8_t ep_num, std::atomic<bool>* out_written = nullptr, bool timeout = false,
                  std::atomic<uint32_t>* out_command_param = nullptr) {
    dut_.RunInEnvironmentTypeContext(
        [this, ep_num, out_written, timeout, out_command_param](Environment& env) {
          auto& depcmd = env.reg_region()[DEPCMD::Get(ep_num).addr()];
          depcmd.SetReadCallback([this]() -> uint32_t { return depcmd_val_.load(); });
          depcmd.SetWriteCallback(
              [this, out_written, timeout, out_command_param, ep_num](uint64_t val_raw) {
                uint32_t val = static_cast<uint32_t>(val_raw);
                if (out_written) {
                  out_written->store(true, std::memory_order_relaxed);
                }
                depcmd_val_.store(timeout ? val : (val & ~(1 << 10)));
                if (out_command_param) {
                  out_command_param->store(DEPCMD::Get(ep_num).FromValue(val).COMMANDPARAM(),
                                           std::memory_order_relaxed);
                }
              });
        });
  }
};

TEST_F(Dwc3CommandsTest, CmdEpSetConfig) {
  MockDepCmd(2);

  dut_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto& depcfg0 = env.reg_region()[DEPCFG_DEPCMDPAR0::Get(2).addr()];
    auto& depcfg1 = env.reg_region()[DEPCFG_DEPCMDPAR1::Get(2).addr()];

    depcfg0.SetWriteCallback([this](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      depcfg0_val_.store(val);
    });
    depcfg1.SetWriteCallback([this](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      depcfg1_val_.store(val);
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;
    ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;  // USB_ENDPOINT_BULK
    ep.max_packet_size = 512;
    ep.interval = 0;

    Dwc3TestHelper::CmdEpSetConfig(drv, ep, false);
  });

  // Verify register writes
  auto expected_par0 = DEPCFG_DEPCMDPAR0::Get(2)
                           .FromValue(0)
                           .set_FIFO_NUM(0)
                           .set_MAX_PACKET_SIZE(512)
                           .set_EP_TYPE(2)
                           .set_ACTION(DEPCFG_DEPCMDPAR0::ACTION_INITIALIZE)
                           .reg_value();
  auto expected_par1 = DEPCFG_DEPCMDPAR1::Get(2)
                           .FromValue(0)
                           .set_EP_NUMBER(2)
                           .set_INTERVAL(0)
                           .set_XFER_NOT_READY_EN(1)
                           .set_XFER_COMPLETE_EN(1)
                           .set_XFER_IN_PROGRESS_EN(1)
                           .set_INTR_NUM(0)
                           .reg_value();
  EXPECT_EQ(depcfg0_val_.load(), expected_par0);
  EXPECT_EQ(depcfg1_val_.load(), expected_par1);
}

TEST_F(Dwc3CommandsTest, CmdEpStartTransfer) {
  MockDepCmd(2);

  dut_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto& depcmdpar0 = env.reg_region()[DEPCMDPAR0::Get(2).addr()];
    auto& depcmdpar1 = env.reg_region()[DEPCMDPAR1::Get(2).addr()];

    depcmdpar0.SetWriteCallback([this](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      depcmdpar0_val_.store(val);
    });
    depcmdpar1.SetWriteCallback([this](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      depcmdpar1_val_.store(val);
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    // Use a specific address to verify splitting.
    uint64_t phys_addr = 0x1234567890ABCDEF;
    Dwc3TestHelper::CmdEpStartTransfer(drv, ep, phys_addr);
  });

  // Per the Synopsys DWC3 Databook specification for the StartTransfer command,
  // the 64-bit TRB address must be mapped as follows:
  // - DEPCMDPAR0 (offset 0x08) receives the High Address (bits 63:32)
  // - DEPCMDPAR1 (offset 0x04) receives the Low Address (bits 31:0)
  // This matches little-endian layout where the lower 32-bits sit at the lower offset.
  // 0x1234567890ABCDEF -> High Address: 0x12345678, Low Address: 0x90ABCDEF
  EXPECT_EQ(depcmdpar0_val_.load(), 0x12345678u);
  EXPECT_EQ(depcmdpar1_val_.load(), 0x90ABCDEFu);
}

TEST_F(Dwc3CommandsTest, CmdEpEndTransfer) {
  MockDepCmd(2, &cmd_written_);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;
    ep.rsrc_id = 1;  // Valid resource ID

    Dwc3TestHelper::SetPowerOn(drv, true);
    Dwc3TestHelper::CmdEpEndTransfer(drv, ep);
  });

  EXPECT_TRUE(cmd_written_.load());
}

// TODO(b/509735595): Re-enable once the production fixes are landed.
TEST_F(Dwc3CommandsTest, DISABLED_CmdEpEndTransferInvalidRsrc) {
  MockDepCmd(2, &cmd_written_);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;
    ep.rsrc_id = 0xFFFFFFFFu;  // kInvalidResourceId

    Dwc3TestHelper::SetPowerOn(drv, true);
    Dwc3TestHelper::CmdEpEndTransfer(drv, ep);
  });

  // Should NOT write to register if resource ID is invalid.
  EXPECT_FALSE(cmd_written_.load());
}

TEST_F(Dwc3CommandsTest, DISABLED_ForceCmdEpEndTransfer) {
  // TODO(b/509735595): Add tests for ForceCmdEpEndTransfer once the implementation lands.
}

TEST_F(Dwc3CommandsTest, CmdEpSetStall) {
  MockDepCmd(2, &cmd_written_);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    Dwc3TestHelper::CmdEpSetStall(drv, ep);
  });

  EXPECT_TRUE(cmd_written_.load());
}

TEST_F(Dwc3CommandsTest, CmdEpClearStall) {
  MockDepCmd(2, &cmd_written_);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    Dwc3TestHelper::CmdEpClearStall(drv, ep);
  });

  EXPECT_TRUE(cmd_written_.load());
}

TEST_F(Dwc3CommandsTest, CmdStartNewConfig) {
  MockDepCmd(2, &cmd_written_, false, &command_param_);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    Dwc3TestHelper::CmdStartNewConfig(drv, ep, 2);
  });

  EXPECT_TRUE(cmd_written_.load());
  EXPECT_EQ(command_param_.load(), 2u);
}

TEST_F(Dwc3CommandsTest, CmdEpTransferConfig) {
  MockDepCmd(2, &cmd_written_);

  dut_.RunInEnvironmentTypeContext([&](Environment& env) {
    auto& depcmdpar0 = env.reg_region()[DEPCMDPAR0::Get(2).addr()];

    depcmdpar0.SetWriteCallback([this](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      depcmdpar0_val_.store(val);
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    Dwc3TestHelper::CmdEpTransferConfig(drv, ep);
  });

  EXPECT_TRUE(cmd_written_.load());
  EXPECT_EQ(depcmdpar0_val_.load(), 1u);
}

TEST_F(Dwc3CommandsTest, WaitForCmdActTimeout) {
  MockDepCmd(2, &cmd_written_, true);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    auto& ep = uep->ep;

    // This should trigger the timeout and log a warning, but not hang.
    Dwc3TestHelper::CmdEpTransferConfig(drv, ep);
  });

  EXPECT_TRUE(cmd_written_.load());
}

}  // namespace dwc3

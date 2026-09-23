// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"

namespace ufs {
using RegisterTest = UfsTest;

TEST_F(RegisterTest, HostCapabilities) {
  // Read only register
  EXPECT_FALSE(CapabilityReg::Get().ReadFrom(&dut_->GetMmio()).crypto_support());
  EXPECT_FALSE(
      CapabilityReg::Get().ReadFrom(&dut_->GetMmio()).uic_dme_test_mode_command_supported());
  EXPECT_FALSE(
      CapabilityReg::Get().ReadFrom(&dut_->GetMmio()).out_of_order_data_delivery_supported());
  EXPECT_TRUE(CapabilityReg::Get().ReadFrom(&dut_->GetMmio())._64_bit_addressing_supported());
  EXPECT_FALSE(CapabilityReg::Get().ReadFrom(&dut_->GetMmio()).auto_hibernation_support());
  EXPECT_EQ(CapabilityReg::Get()
                    .ReadFrom(&dut_->GetMmio())
                    .number_of_utp_task_management_request_slots() +
                1,
            ufs_mock_device::UfsMockDevice::kNutmrs);
  EXPECT_EQ(
      CapabilityReg::Get().ReadFrom(&dut_->GetMmio()).number_of_utp_transfer_request_slots() + 1,
      ufs_mock_device::UfsMockDevice::kNutrs);
}

TEST_F(RegisterTest, Version) {
  // Read only register
  EXPECT_EQ(VersionReg::Get().ReadFrom(&dut_->GetMmio()).major_version_number(),
            ufs_mock_device::kMajorVersion);
  EXPECT_EQ(VersionReg::Get().ReadFrom(&dut_->GetMmio()).minor_version_number(),
            ufs_mock_device::kMinorVersion);
  EXPECT_EQ(VersionReg::Get().ReadFrom(&dut_->GetMmio()).version_suffix(),
            ufs_mock_device::kVersionSuffix);
}

TEST_F(RegisterTest, AutoHibernateIdleTimer) {
  // Test register address.
  EXPECT_EQ(AutoHibernateIdleTimerReg::Get().addr(), 0x18u);

  // Test scale and value encoding: 15000 usec.
  auto reg = AutoHibernateIdleTimerReg::Get().FromValue(0);
  reg.set_timer_scale(AutoHibernateIdleTimerReg::Scale::k100us);
  reg.set_timer_value(150);
  reg.WriteTo(&dut_->GetMmio());
  auto read_reg = AutoHibernateIdleTimerReg::Get().ReadFrom(&dut_->GetMmio());
  EXPECT_EQ(read_reg.timer_scale(), AutoHibernateIdleTimerReg::Scale::k100us);
  EXPECT_EQ(static_cast<uint32_t>(read_reg.timer_scale()), 2u);
  EXPECT_EQ(read_reg.timer_value(), 150u);

  // Test 1us scale (value 0 per spec).
  reg.set_timer_scale(AutoHibernateIdleTimerReg::Scale::k1us);
  reg.set_timer_value(500);
  reg.WriteTo(&dut_->GetMmio());
  read_reg = AutoHibernateIdleTimerReg::Get().ReadFrom(&dut_->GetMmio());
  EXPECT_EQ(read_reg.timer_scale(), AutoHibernateIdleTimerReg::Scale::k1us);
  EXPECT_EQ(static_cast<uint32_t>(read_reg.timer_scale()), 0u);
  EXPECT_EQ(read_reg.timer_value(), 500u);

  // Test boundary with maximum 10-bit timer value (1023 / 0x3FF) and 100ms scale.
  reg.set_timer_scale(AutoHibernateIdleTimerReg::Scale::k100ms);
  reg.set_timer_value(1023);
  reg.WriteTo(&dut_->GetMmio());
  read_reg = AutoHibernateIdleTimerReg::Get().ReadFrom(&dut_->GetMmio());
  EXPECT_EQ(read_reg.timer_scale(), AutoHibernateIdleTimerReg::Scale::k100ms);
  EXPECT_EQ(static_cast<uint32_t>(read_reg.timer_scale()), 5u);
  EXPECT_EQ(read_reg.timer_value(), 1023u);

  // Test FromTimeoutUs calculation across all scales.
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(0).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k1us);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(0).timer_value(), 0u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(1000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k1us);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(1000).timer_value(), 1000u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(5000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k10us);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(5000).timer_value(), 500u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(50000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k100us);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(50000).timer_value(), 500u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(500000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k1ms);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(500000).timer_value(), 500u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(5000000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k10ms);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(5000000).timer_value(), 500u);

  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(50000000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k100ms);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(50000000).timer_value(), 500u);

  // Exceeds maximum 102.3s timeout, should clamp to max.
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(200000000).timer_scale(),
            AutoHibernateIdleTimerReg::Scale::k100ms);
  EXPECT_EQ(AutoHibernateIdleTimerReg::FromTimeoutUs(200000000).timer_value(), 1023u);
}

TEST_F(RegisterTest, InterruptStatus) {
  // Clear IS register to zero.
  InterruptStatusReg::Get()
      .ReadFrom(&dut_->GetMmio())
      .set_reg_value(0xffffffff)
      .WriteTo(&dut_->GetMmio());
  EXPECT_EQ(InterruptStatusReg::Get().ReadFrom(&dut_->GetMmio()).reg_value(), 0U);

  // Send UIC command to set |uic_command_completion_status|
  DmeLinkStartUpUicCommand link_startup_command(*dut_);
  EXPECT_TRUE(link_startup_command.SendCommand().is_ok());

  // InterruptStatus is cleared by SendUicCommand().
  EXPECT_FALSE(
      InterruptStatusReg::Get().ReadFrom(&dut_->GetMmio()).uic_command_completion_status());

  // Send UPIU command to set |utp_transfer_request_completion_status|
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB*>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu unit_ready_upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  EXPECT_OK(dut_->GetTransferRequestProcessor().SendAdminScsiCmd(unit_ready_upiu, 0));

  // InterruptStatus is cleared by Isr().
  EXPECT_FALSE(InterruptStatusReg::Get()
                   .ReadFrom(&dut_->GetMmio())
                   .utp_transfer_request_completion_status());

  // Hook InterruptStatus handler to set interrupt status.
  mock_device_.GetRegisterMmioProcessor().SetHook(
      RegisterMap::kIS, [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t value) {
        InterruptStatusReg::Get().FromValue(value).WriteTo(mock_device.GetRegisters());
      });

  auto register_value = InterruptStatusReg::Get().FromValue(0);
  // Set error in InterruptStatus
  register_value.set_uic_error(true)
      .set_device_fatal_error_status(true)
      .set_host_controller_fatal_error_status(true)
      .set_system_bus_fatal_error_status(true)
      .set_crypto_engine_fatal_error_status(true);
  // Set unused command completion
  register_value.set_utp_task_management_request_completion_status(true);
  register_value.WriteTo(&dut_->GetMmio());

  // Restore the default handler.
  mock_device_.GetRegisterMmioProcessor().SetHook(
      RegisterMap::kIS, ufs_mock_device::RegisterMmioProcessor::DefaultISHandler);

  mock_device_.TriggerInterrupt();

  // Wait for the interrupt to complete.
  auto wait_for = [&]() -> bool {
    return InterruptStatusReg::Get().ReadFrom(&dut_->GetMmio()).reg_value() == 0;
  };
  fbl::String timeout_message = "Timeout waiting for ISR()";
  constexpr uint32_t kTimeoutUs = 1000000;
  ASSERT_OK(dut_->WaitWithTimeout(wait_for, zx::usec(kTimeoutUs), timeout_message));

  // Verify that the ISR has processed all interruptStatus
  EXPECT_EQ(InterruptStatusReg::Get().ReadFrom(&dut_->GetMmio()).reg_value(), 0U);
}

TEST_F(RegisterTest, InterruptEnable) {
  EXPECT_TRUE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).crypto_engine_fatal_error_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).system_bus_fatal_error_enable());
  EXPECT_TRUE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).host_controller_fatal_error_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).utp_error_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).device_fatal_error_enable());
  EXPECT_FALSE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_command_completion_enable());
  EXPECT_TRUE(InterruptEnableReg::Get()
                  .ReadFrom(&dut_->GetMmio())
                  .utp_transfer_request_completion_enable());
  EXPECT_FALSE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_link_startup_status_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_link_lost_status_enable());
  EXPECT_FALSE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_hibernate_enter_status_enable());
  EXPECT_FALSE(
      InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_hibernate_exit_status_enable());
  EXPECT_FALSE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_power_mode_status_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_test_mode_status_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_error_enable());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_dme_endpointreset());
  EXPECT_TRUE(InterruptEnableReg::Get()
                  .ReadFrom(&dut_->GetMmio())
                  .utp_task_management_request_completion_enable());

  // Set |uic_dme_endpointreset| to test interrupt enable
  InterruptEnableReg::Get()
      .ReadFrom(&dut_->GetMmio())
      .set_uic_dme_endpointreset(1)
      .WriteTo(&dut_->GetMmio());
  EXPECT_TRUE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_dme_endpointreset());
  // Clear |uic_dme_endpointreset| to test interrupt enable
  InterruptEnableReg::Get()
      .ReadFrom(&dut_->GetMmio())
      .set_uic_dme_endpointreset(0)
      .WriteTo(&dut_->GetMmio());
  EXPECT_FALSE(InterruptEnableReg::Get().ReadFrom(&dut_->GetMmio()).uic_dme_endpointreset());
}

TEST_F(RegisterTest, HostControllerStatus) {
  // Read only register
  EXPECT_EQ(HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).target_lun_of_utp_error(),
            0U);
  EXPECT_EQ(HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).task_tag_of_utp_error(), 0U);
  EXPECT_EQ(HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).utp_error_code(), 0U);
  EXPECT_EQ(HostControllerStatusReg::Get()
                .ReadFrom(&dut_->GetMmio())
                .uic_power_mode_change_request_status(),
            HostControllerStatusReg::PowerModeStatus::kPowerLocal);
  EXPECT_TRUE(HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).uic_command_ready());
  EXPECT_TRUE(HostControllerStatusReg::Get()
                  .ReadFrom(&dut_->GetMmio())
                  .utp_task_management_request_list_ready());
  EXPECT_TRUE(
      HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).utp_transfer_request_list_ready());
  EXPECT_TRUE(HostControllerStatusReg::Get().ReadFrom(&dut_->GetMmio()).device_present());
}

TEST_F(RegisterTest, HostControllerEnable) {
  EXPECT_FALSE(HostControllerEnableReg::Get().ReadFrom(&dut_->GetMmio()).crypto_general_enable());
  EXPECT_TRUE(HostControllerEnableReg::Get().ReadFrom(&dut_->GetMmio()).host_controller_enable());

  EXPECT_OK(DisableController());
  EXPECT_FALSE(HostControllerEnableReg::Get().ReadFrom(&dut_->GetMmio()).host_controller_enable());

  EXPECT_OK(EnableController());
  EXPECT_TRUE(HostControllerEnableReg::Get().ReadFrom(&dut_->GetMmio()).host_controller_enable());
}

TEST_F(RegisterTest, UtpTransferRequestListBaseAddress) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a transfer request list is
  // implemented
}

TEST_F(RegisterTest, UtpTransferRequestListDoorbell) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a transfer request list is
  // implemented
}

TEST_F(RegisterTest, UtpTransferRequestListRunStop) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a transfer request list is
  // implemented
}

TEST_F(RegisterTest, UtpTransferRequestListClear) {
  // Set doorbell directly in mock registers to avoid triggering request descriptor processing
  UtrListDoorBellReg::Get().FromValue((1u << 0) | (1u << 1)).WriteTo(mock_device_.GetRegisters());
  ASSERT_EQ(UtrListDoorBellReg::Get().ReadFrom(&dut_->GetMmio()).door_bell(), 0x3u);

  // Clear slot 0 using W0C mask (bit 0 is 0, other bits are 1) via MMIO write
  UtrListClearReg::Get().FromValue(~(1u << 0)).WriteTo(&dut_->GetMmio());
  ASSERT_EQ(UtrListDoorBellReg::Get().ReadFrom(&dut_->GetMmio()).door_bell(), (1u << 1));

  // Clear slot 1
  UtrListClearReg::Get().FromValue(~(1u << 1)).WriteTo(&dut_->GetMmio());
  ASSERT_EQ(UtrListDoorBellReg::Get().ReadFrom(&dut_->GetMmio()).door_bell(), 0x0u);
}

TEST_F(RegisterTest, UtpTaskManagementRequestListBaseAddress) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a task management list is implemented
}

TEST_F(RegisterTest, UtpTaskManagementRequestListDoorbell) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a task management list is implemented
}

TEST_F(RegisterTest, UTPTaskManagementRequestListRunStop) {
  // TODO(https://fxbug.dev/42075643): Writing unit test after a task management list is implemented
}

}  // namespace ufs

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/device_manager.h"
#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/uic/uic_commands.h"
#include "unit-lib.h"

namespace ufs {
using UicTest = UfsTest;

TEST_F(UicTest, DmeGet) {
  DmeGetUicCommand dme_get_max_rx_hs_gear_command(*dut_, PA_MaxRxHSGear, 0);
  auto value = dme_get_max_rx_hs_gear_command.SendCommand();
  ASSERT_OK(value);
  EXPECT_EQ(value.value(), ufs_mock_device::kMaxGear);

  DmeGetUicCommand dme_get_connected_tx_lanes_command(*dut_, PA_ConnectedTxDataLanes, 0);
  value = dme_get_connected_tx_lanes_command.SendCommand();
  ASSERT_OK(value);
  EXPECT_EQ(value.value(), ufs_mock_device::kConnectedDataLanes);

  DmeGetUicCommand dme_get_connected_rx_lanes_command(*dut_, PA_ConnectedRxDataLanes, 0);
  value = dme_get_connected_rx_lanes_command.SendCommand();
  ASSERT_OK(value);
  EXPECT_EQ(value.value(), ufs_mock_device::kConnectedDataLanes);

  DmeGetUicCommand dme_get_avail_tx_lanes_command(*dut_, PA_AvailTxDataLanes, 0);
  value = dme_get_avail_tx_lanes_command.SendCommand();
  ASSERT_OK(value);
  EXPECT_EQ(value.value(), ufs_mock_device::kConnectedDataLanes);

  DmeGetUicCommand dme_get_avail_rx_lanes_command(*dut_, PA_AvailRxDataLanes, 0);
  value = dme_get_avail_rx_lanes_command.SendCommand();
  ASSERT_OK(value);
  EXPECT_EQ(value.value(), ufs_mock_device::kConnectedDataLanes);
}

TEST_F(UicTest, DmeSet) {
  DmeSetUicCommand dme_set_command(*dut_, 0, 0, AttrSetType::kVolatile, 0);
  ASSERT_OK(dme_set_command.SendCommand());

  // TODO(https://fxbug.dev/42075643): Add more tests.
}

TEST_F(UicTest, DmeLinkStartUp) {
  DmeLinkStartUpUicCommand dme_link_startup_command(*dut_);
  EXPECT_OK(dme_link_startup_command.SendCommand().status_value());

  EXPECT_TRUE(
      HostControllerStatusReg::Get().ReadFrom(mock_device_.GetRegisters()).device_present());
  EXPECT_TRUE(HostControllerStatusReg::Get()
                  .ReadFrom(mock_device_.GetRegisters())
                  .utp_transfer_request_list_ready());
  EXPECT_TRUE(HostControllerStatusReg::Get()
                  .ReadFrom(mock_device_.GetRegisters())
                  .utp_task_management_request_list_ready());
}

TEST_F(UicTest, SendLinkStartUpRetrySuccess) {
  size_t attempts = 0;
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeLinkStartUp, [&attempts](ufs_mock_device::UfsMockDevice& mock_device,
                                                     uint32_t arg1, uint32_t arg2, uint32_t arg3) {
        ++attempts;
        if (attempts < 2) {
          UicCommandArgument2Reg::Get()
              .ReadFrom(mock_device.GetRegisters())
              .set_result_code(UicCommandArgument2Reg::GenericErrorCode::kFailure)
              .WriteTo(mock_device.GetRegisters());
        } else {
          ufs_mock_device::UicCmdProcessor::DefaultDmeLinkStartUpHandler(mock_device, arg1, arg2,
                                                                         arg3);
        }
      });

  ASSERT_OK(dut_->GetDeviceManager().SendLinkStartUp().status_value());
  ASSERT_EQ(attempts, 2u);
}

TEST_F(UicTest, SendLinkStartUpExhaustedRetries) {
  size_t attempts = 0;
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeLinkStartUp,
      [&attempts](ufs_mock_device::UfsMockDevice& mock_device, uint32_t, uint32_t, uint32_t) {
        ++attempts;
        UicCommandArgument2Reg::Get()
            .ReadFrom(mock_device.GetRegisters())
            .set_result_code(UicCommandArgument2Reg::GenericErrorCode::kFailure)
            .WriteTo(mock_device.GetRegisters());
      });

  ASSERT_EQ(dut_->GetDeviceManager().SendLinkStartUp().status_value(), ZX_ERR_INTERNAL);
  ASSERT_EQ(attempts, 3u);
}

TEST_F(UicTest, DmeHibernate) {
  DmeHibernateEnterCommand dme_hibernate_enter_command(*dut_);
  EXPECT_OK(dme_hibernate_enter_command.SendCommand().status_value());

  DmeHibernateExitCommand dme_hibernate_exit_command(*dut_);
  EXPECT_OK(dme_hibernate_exit_command.SendCommand().status_value());

  // TODO(https://fxbug.dev/42075643): Add more tests.
}

TEST_F(UicTest, SendUicCommandException) {
  // uic_command_completion_state is not cleared.
  {
    // Hook InterruptStatus handler to set interrupt status.
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kIS, [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t value) {
          InterruptStatusReg::Get().FromValue(value).WriteTo(mock_device.GetRegisters());
        });

    InterruptStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_uic_command_completion_status(true)
        .WriteTo(mock_device_.GetRegisters());

    DmeGetUicCommand dme_get_command(*dut_, PA_MaxRxHSGear, 0);
    auto value = dme_get_command.SendCommand();
    EXPECT_EQ(value.status_value(), ZX_ERR_BAD_STATE);

    InterruptStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_uic_command_completion_status(false)
        .WriteTo(mock_device_.GetRegisters());

    // Restore the default handler.
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kIS, ufs_mock_device::RegisterMmioProcessor::DefaultISHandler);
  }

  // uic_command_ready timed out.
  {
    HostControllerStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_uic_command_ready(0)
        .WriteTo(mock_device_.GetRegisters());

    DmeGetUicCommand dme_get_command(*dut_, PA_MaxRxHSGear, 0);
    dme_get_command.SetTimeoutUsec(1000);
    auto value = dme_get_command.SendCommand();
    EXPECT_EQ(value.status_value(), ZX_ERR_TIMED_OUT);

    HostControllerStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_uic_command_ready(1)
        .WriteTo(mock_device_.GetRegisters());
  }

  // uic_command_completion_status timed out.
  {
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kUICCMD, [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t value) {});

    DmeGetUicCommand dme_get_command(*dut_, PA_MaxRxHSGear, 0);
    dme_get_command.SetTimeoutUsec(1000);
    auto value = dme_get_command.SendCommand();
    EXPECT_EQ(value.status_value(), ZX_ERR_TIMED_OUT);

    // Restore the default handler.
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kUICCMD, ufs_mock_device::RegisterMmioProcessor::DefaultUICCMDHandler);
  }

  // GenericErrorCode is kFailure.
  {
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kUICCMDARG2, [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t value) {
          UicCommandArgument2Reg::Get()
              .ReadFrom(mock_device.GetRegisters())
              .set_result_code(UicCommandArgument2Reg::GenericErrorCode::kFailure)
              .WriteTo(mock_device.GetRegisters());
        });

    DmeGetUicCommand dme_get_command(*dut_, PA_MaxRxHSGear, 0);
    auto value = dme_get_command.SendCommand();
    EXPECT_EQ(value.status_value(), ZX_ERR_INTERNAL);

    // Restore the default handler
    mock_device_.GetRegisterMmioProcessor().SetHook(
        RegisterMap::kUICCMDARG2, [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t value) {
          mock_device.GetRegisters()->Write<uint32_t>(value, RegisterMap::kUICCMDARG2);
        });
  }

  // Hibernate command timed out.
  {
    mock_device_.GetUicCmdProcessor().SetHook(
        UicCommandOpcode::kDmeHibernateEnter,
        [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2,
           uint32_t ucmdarg3) {});

    DmeHibernateEnterCommand dme_hibernate_command(*dut_);
    dme_hibernate_command.SetTimeoutUsec(1000);
    auto result = dme_hibernate_command.SendCommand();
    EXPECT_EQ(result.status_value(), ZX_ERR_TIMED_OUT);

    // Restore the default handler.
    mock_device_.GetUicCmdProcessor().SetHook(
        UicCommandOpcode::kDmeHibernateEnter,
        ufs_mock_device::UicCmdProcessor::DefaultDmeHibernateEnterHandler);
  }

  // Hibernate command returns bad state
  {
    mock_device_.GetUicCmdProcessor().SetHook(
        UicCommandOpcode::kDmeHibernateEnter,
        [](ufs_mock_device::UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2,
           uint32_t ucmdarg3) {
          InterruptStatusReg::Get()
              .ReadFrom(mock_device.GetRegisters())
              .set_uic_hibernate_enter_status(true)
              .WriteTo(mock_device.GetRegisters());

          HostControllerStatusReg::Get()
              .ReadFrom(mock_device.GetRegisters())
              .set_uic_power_mode_change_request_status(
                  HostControllerStatusReg::PowerModeStatus::kPowerFatalError)
              .WriteTo(mock_device.GetRegisters());
        });

    DmeHibernateEnterCommand dme_hibernate_command(*dut_);
    auto result = dme_hibernate_command.SendCommand();
    EXPECT_EQ(result.status_value(), ZX_ERR_BAD_STATE);

    // Restore the default handler.
    mock_device_.GetUicCmdProcessor().SetHook(
        UicCommandOpcode::kDmeHibernateEnter,
        ufs_mock_device::UicCmdProcessor::DefaultDmeHibernateEnterHandler);
  }
}

}  // namespace ufs

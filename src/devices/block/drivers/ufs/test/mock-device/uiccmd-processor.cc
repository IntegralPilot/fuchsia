// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/driver/logging/cpp/logger.h>

#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {

void UicCmdProcessor::HandleUicCmd(UicCommandOpcode value) {
  uint32_t ucmdarg1 =
      UicCommandArgument1Reg::Get().ReadFrom(mock_device_.GetRegisters()).reg_value();
  uint32_t ucmdarg2 =
      UicCommandArgument2Reg::Get().ReadFrom(mock_device_.GetRegisters()).reg_value();
  uint32_t ucmdarg3 =
      UicCommandArgument3Reg::Get().ReadFrom(mock_device_.GetRegisters()).reg_value();
  if (auto it = handlers_.find(value); it != handlers_.end()) {
    (it->second)(mock_device_, ucmdarg1, ucmdarg2, ucmdarg3);
  } else {
    // TODO(https://fxbug.dev/42075643): Revisit it when UICCMD error handling logic is implemented
    // in the driver.
    fdf::error("UFS MOCK: uiccmd value: 0x{:x} is not supported", static_cast<unsigned int>(value));
  }

  InterruptStatusReg::Get()
      .ReadFrom(mock_device_.GetRegisters())
      .set_uic_command_completion_status(true)
      .WriteTo(mock_device_.GetRegisters());
  if (InterruptEnableReg::Get()
          .ReadFrom(mock_device_.GetRegisters())
          .uic_command_completion_enable()) {
    mock_device_.TriggerInterrupt();
  }
}

void UicCmdProcessor::DefaultDmeGetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                           uint32_t ucmdarg2, uint32_t ucmdarg3) {
  uint32_t mib_value = 0;
  auto mib_attribute = UicCommandArgument1Reg::Get().FromValue(ucmdarg1).mib_attribute();
  switch (mib_attribute) {
    case PA_MaxRxHSGear:
      mib_value = kMaxGear;
      break;
    case PA_ConnectedTxDataLanes:
    case PA_ConnectedRxDataLanes:
    case PA_AvailTxDataLanes:
    case PA_AvailRxDataLanes:
    case PA_ActiveTxDataLanes:
    case PA_ActiveRxDataLanes:
      mib_value = kConnectedDataLanes;
      break;
    case PA_TxGear:
    case PA_RxGear:
      mib_value = kGear;
      break;
    case PA_TxTermination:
    case PA_RxTermination:
      mib_value = kTermination;
      break;
    case PA_HSSeries:
      mib_value = kHSSeries;
      break;
    case PA_PWRModeUserData0:
    case PA_PWRModeUserData1:
    case PA_PWRModeUserData2:
    case PA_PWRModeUserData3:
    case PA_PWRModeUserData4:
    case PA_PWRModeUserData5:
    case DME_LocalFC0ProtectionTimeOutVal:
    case DME_LocalTC0ReplayTimeOutVal:
    case DME_LocalAFC0ReqTimeOutVal:
      mib_value = kPWRModeUserData;
      break;
    case PA_TxHsAdaptType:
      mib_value = kTxHsAdaptType;
      break;
    case PA_PWRMode:
      mib_value = kPWRMode;
      break;
    case PA_RemoteVerInfo:
    case PA_LocalVerInfo:
      mib_value = kUniproVersion;
      break;
    case PA_TActivate:
      mib_value = kTActivate;
      break;
    case PA_Granularity:
      mib_value = kGranularity;
      break;
    default:
      fdf::error("UFS MOCK: Get attribute 0x{:x} is not supported", mib_attribute);
      break;
  }
  UicCommandArgument3Reg::Get().FromValue(mib_value).WriteTo(mock_device.GetRegisters());
}

void UicCmdProcessor::DefaultDmeSetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                           uint32_t ucmdarg2, uint32_t ucmdarg3) {
  auto mib_attribute = UicCommandArgument1Reg::Get().FromValue(ucmdarg1).mib_attribute();
  switch (mib_attribute) {
    case PA_MaxRxHSGear:
    case PA_ConnectedTxDataLanes:
    case PA_ConnectedRxDataLanes:
    case PA_AvailTxDataLanes:
    case PA_AvailRxDataLanes:
    case PA_ActiveTxDataLanes:
    case PA_ActiveRxDataLanes:
    case PA_TxGear:
    case PA_RxGear:
    case PA_TxTermination:
    case PA_RxTermination:
    case PA_HSSeries:
    case PA_PWRModeUserData0:
    case PA_PWRModeUserData1:
    case PA_PWRModeUserData2:
    case PA_PWRModeUserData3:
    case PA_PWRModeUserData4:
    case PA_PWRModeUserData5:
    case DME_LocalFC0ProtectionTimeOutVal:
    case DME_LocalTC0ReplayTimeOutVal:
    case DME_LocalAFC0ReqTimeOutVal:
    case PA_TxHsAdaptType:
    case PA_TActivate:
    case PA_Granularity:
      break;
    case PA_PWRMode:
      HostControllerStatusReg::Get()
          .ReadFrom(mock_device.GetRegisters())
          .set_uic_power_mode_change_request_status(
              HostControllerStatusReg::PowerModeStatus::kPowerLocal)
          .WriteTo(mock_device.GetRegisters());
      // Send Power mode interrupt
      InterruptStatusReg::Get()
          .ReadFrom(mock_device.GetRegisters())
          .set_uic_power_mode_status(true)
          .WriteTo(mock_device.GetRegisters());
      if (InterruptEnableReg::Get()
              .ReadFrom(mock_device.GetRegisters())
              .uic_power_mode_status_enable()) {
        mock_device.TriggerInterrupt();
      }
      break;
    default:
      fdf::error("UFS MOCK: Set attribute 0x{:x} is not supported", mib_attribute);
      break;
  }
}

void UicCmdProcessor::DefaultDmePeerGetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                               uint32_t ucmdarg2, uint32_t ucmdarg3) {
  uint32_t mib_value = 0;
  auto mib_attribute = UicCommandArgument1Reg::Get().FromValue(ucmdarg1).mib_attribute();
  switch (mib_attribute) {
    case PA_TActivate:
      mib_value = kTActivate;
      break;
    case PA_Granularity:
      mib_value = kGranularity;
      break;
    default:
      fdf::error("UFS MOCK: Peer Get attribute 0x{:x} is not supported", mib_attribute);
      break;
  }
  UicCommandArgument3Reg::Get().FromValue(mib_value).WriteTo(mock_device.GetRegisters());
}

void UicCmdProcessor::DefaultDmePeerSetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                               uint32_t ucmdarg2, uint32_t ucmdarg3) {
  auto mib_attribute = UicCommandArgument1Reg::Get().FromValue(ucmdarg1).mib_attribute();
  switch (mib_attribute) {
    case PA_TActivate:
    case PA_Granularity:
      break;
    default:
      fdf::error("UFS MOCK: Peer Set attribute 0x{:x} is not supported", mib_attribute);
      break;
  }
}

void UicCmdProcessor::DefaultDmeLinkStartUpHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                                   uint32_t ucmdarg2, uint32_t ucmdarg3) {
  HostControllerStatusReg::Get()
      .ReadFrom(mock_device.GetRegisters())
      .set_device_present(true)
      .set_utp_transfer_request_list_ready(true)
      .set_utp_task_management_request_list_ready(true)
      .WriteTo(mock_device.GetRegisters());
}

void UicCmdProcessor::DefaultDmeHibernateEnterHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                                      uint32_t ucmdarg2, uint32_t ucmdarg3) {
  InterruptStatusReg::Get()
      .ReadFrom(mock_device.GetRegisters())
      .set_uic_hibernate_enter_status(true)
      .set_uic_link_startup_status(false)
      .WriteTo(mock_device.GetRegisters());

  HostControllerStatusReg::Get()
      .ReadFrom(mock_device.GetRegisters())
      .set_uic_power_mode_change_request_status(HostControllerStatusReg::PowerModeStatus::kPowerOk)
      .WriteTo(mock_device.GetRegisters());
}

void UicCmdProcessor::DefaultDmeHibernateExitHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                                     uint32_t ucmdarg2, uint32_t ucmdarg3) {
  InterruptStatusReg::Get()
      .ReadFrom(mock_device.GetRegisters())
      .set_uic_hibernate_exit_status(true)
      .set_uic_link_startup_status(true)
      .WriteTo(mock_device.GetRegisters());

  HostControllerStatusReg::Get()
      .ReadFrom(mock_device.GetRegisters())
      .set_uic_power_mode_change_request_status(HostControllerStatusReg::PowerModeStatus::kPowerOk)
      .WriteTo(mock_device.GetRegisters());
}

}  // namespace ufs_mock_device
}  // namespace ufs

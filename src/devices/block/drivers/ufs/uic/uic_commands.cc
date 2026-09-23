// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "uic_commands.h"

#include <lib/driver/logging/cpp/logger.h>

#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {

zx::result<std::optional<uint32_t>> UicCommand::SendCommand() {
  if (auto result = UicPreProcess(); result.is_error()) {
    return result.take_error();
  }

  if (auto result = SendUicCommand(); result.is_error()) {
    return result.take_error();
  }

  if (auto result = UicPostProcess(); result.is_error()) {
    return result.take_error();
  }

  return zx::ok(ReturnValue());
}

zx::result<> UicCommand::SendUicCommand() {
  const fdf::MmioBuffer &mmio = GetController().GetMmio();

  // Clear 'UIC command completion status' if set
  if (InterruptStatusReg::Get().ReadFrom(&mmio).uic_command_completion_status()) {
    fdf::error("The previously set uic_command_completion_state was not cleared. \n");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  const auto [argument_1, argument_2, argument_3] = Arguments();

  UicCommandArgument1Reg::Get().FromValue(argument_1).WriteTo(&mmio);
  UicCommandArgument2Reg::Get().FromValue(argument_2).WriteTo(&mmio);
  UicCommandArgument3Reg::Get().FromValue(argument_3).WriteTo(&mmio);

  // Wait for 'UIC command ready'
  auto wait_for_command_ready = [&]() -> bool {
    return HostControllerStatusReg::Get().ReadFrom(&mmio).uic_command_ready();
  };
  fbl::String timeout_message = "Timeout waiting for 'UIC command ready'";
  if (zx_status_t status = controller_.WaitWithTimeout(wait_for_command_ready,
                                                       zx::usec(timeout_usec_), timeout_message);
      status != ZX_OK) {
    return zx::error(status);
  }

  UicCommandReg::Get().FromValue(static_cast<uint8_t>(GetOpcode())).WriteTo(&mmio);

  // TODO(https://fxbug.dev/42075643): Currently, the UIC commands are only used during the
  // initialization process, so we implemented them as polling. However, if DME_HIBERNATE and
  // DME_POWERMODE are added in the future, it should be changed to an interrupt method.

  // Wait for 'UIC command completion status'
  auto wait_for_completion = [&]() -> bool {
    return InterruptStatusReg::Get().ReadFrom(&mmio).uic_command_completion_status();
  };
  timeout_message = "Timeout waiting for 'UIC command completion status'";
  if (zx_status_t status = controller_.WaitWithTimeout(wait_for_completion, zx::usec(timeout_usec_),
                                                       timeout_message);
      status != ZX_OK) {
    return zx::error(status);
  }
  // Clear 'UIC command completion status'
  InterruptStatusReg::Get().FromValue(0).set_uic_command_completion_status(true).WriteTo(&mmio);

  return zx::ok();
}

zx::result<> UicCommand::UicPostProcess() {
  if (uint32_t result_code =
          UicCommandArgument2Reg::Get().ReadFrom(&GetController().GetMmio()).result_code();
      result_code != UicCommandArgument2Reg::GenericErrorCode::kSuccess) {
    auto opcode = UicCommandReg::Get().ReadFrom(&GetController().GetMmio()).command_opcode();
    auto mib_attribute =
        UicCommandArgument1Reg::Get().ReadFrom(&GetController().GetMmio()).mib_attribute();
    fdf::error(
        "Failed to send UIC command, opcode=0x{:x}, mib_attribute=0x{:x}, result_code = {}\n",
        static_cast<uint32_t>(opcode), mib_attribute, result_code);
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

std::tuple<uint32_t, uint32_t, uint32_t> DmeGetUicCommand::Arguments() const {
  return std::make_tuple(UicCommandArgument1Reg::Get()
                             .FromValue(0)
                             .set_mib_attribute(GetMibAttribute())
                             .set_gen_selector_index(GetGenSelectorIndex())
                             .reg_value(),
                         0, 0);
}

std::optional<uint32_t> DmeGetUicCommand::ReturnValue() {
  return UicCommandArgument3Reg::Get().ReadFrom(&GetController().GetMmio()).value();
}

std::tuple<uint32_t, uint32_t, uint32_t> DmeSetUicCommand::Arguments() const {
  return std::make_tuple(
      UicCommandArgument1Reg::Get()
          .FromValue(0)
          .set_mib_attribute(GetMibAttribute())
          .set_gen_selector_index(GetGenSelectorIndex())
          .reg_value(),
      UicCommandArgument2Reg::Get().FromValue(0).set_attr_set_type(attr_set_type_).reg_value(),
      value_);
}

std::tuple<uint32_t, uint32_t, uint32_t> DmePeerGetUicCommand::Arguments() const {
  return std::make_tuple(UicCommandArgument1Reg::Get()
                             .FromValue(0)
                             .set_mib_attribute(GetMibAttribute())
                             .set_gen_selector_index(GetGenSelectorIndex())
                             .reg_value(),
                         0, 0);
}

std::optional<uint32_t> DmePeerGetUicCommand::ReturnValue() {
  return UicCommandArgument3Reg::Get().ReadFrom(&GetController().GetMmio()).value();
}

std::tuple<uint32_t, uint32_t, uint32_t> DmePeerSetUicCommand::Arguments() const {
  return std::make_tuple(
      UicCommandArgument1Reg::Get()
          .FromValue(0)
          .set_mib_attribute(GetMibAttribute())
          .set_gen_selector_index(GetGenSelectorIndex())
          .reg_value(),
      UicCommandArgument2Reg::Get().FromValue(0).set_attr_set_type(attr_set_type_).reg_value(),
      value_);
}

zx::result<> DmeLinkStartUpUicCommand::UicPostProcess() {
  if (auto result = UicCommand::UicPostProcess(); result.is_error()) {
    return result.take_error();
  }

  return GetController().Notify(NotifyEvent::kPostLinkStartup, 0);
}

zx::result<> DmeHibernateCommand::UicPostProcess() {
  if (auto result = UicCommand::UicPostProcess(); result.is_error()) {
    return result.take_error();
  }

  const fdf::MmioBuffer &mmio = GetController().GetMmio();
  uint32_t flag = GetFlag();
  uint32_t timeout_us = GetTimeoutUsec();

  auto wait_for = [&]() -> bool {
    return InterruptStatusReg::Get().ReadFrom(&mmio).reg_value() & flag;
  };
  fbl::String timeout_message = "Timeout waiting for hibernation transition";
  if (zx_status_t status =
          GetController().WaitWithTimeout(wait_for, zx::usec(timeout_us), timeout_message);
      status != ZX_OK) {
    return zx::error(status);
  }

  HostControllerStatusReg::PowerModeStatus power_mode_state =
      HostControllerStatusReg::Get().ReadFrom(&mmio).uic_power_mode_change_request_status();
  if (power_mode_state != HostControllerStatusReg::PowerModeStatus::kPowerOk &&
      power_mode_state != HostControllerStatusReg::PowerModeStatus::kPowerLocal &&
      power_mode_state != HostControllerStatusReg::PowerModeStatus::kPowerRemote) {
    fdf::error("Failed to change power mode, UPMCRS = 0x{:x}\n",
               static_cast<uint32_t>(power_mode_state));
    return zx::error(ZX_ERR_BAD_STATE);
  }

  InterruptStatusReg::Get().FromValue(flag).WriteTo(&mmio);

  return zx::ok();
}

uint32_t DmeHibernateEnterCommand::GetFlag() {
  return InterruptStatusReg::Get().FromValue(0).set_uic_hibernate_enter_status(true).reg_value();
}

uint32_t DmeHibernateExitCommand::GetFlag() {
  return InterruptStatusReg::Get().FromValue(0).set_uic_hibernate_exit_status(true).reg_value();
}

}  // namespace ufs

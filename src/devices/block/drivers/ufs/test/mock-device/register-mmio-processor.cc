// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/stdcompat/span.h>
#include <zircon/syscalls.h>

#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {

void RegisterMmioProcessor::Write32(const void* ctx, const mmio_buffer_t& mmio, uint32_t value,
                                    zx_off_t offset) {
  const_cast<UfsMockDevice*>(static_cast<const UfsMockDevice*>(ctx))
      ->GetRegisterMmioProcessor()
      .MmioWrite(value, static_cast<uint32_t>(offset));
}

uint32_t RegisterMmioProcessor::Read32(const void* ctx, const mmio_buffer_t& mmio,
                                       zx_off_t offset) {
  return const_cast<UfsMockDevice*>(static_cast<const UfsMockDevice*>(ctx))
      ->GetRegisterMmioProcessor()
      .MmioRead(static_cast<uint32_t>(offset));
}

void RegisterMmioProcessor::MmioWrite(uint32_t value, zx_off_t offset) {
  if (auto it = handlers_.find(static_cast<RegisterMap>(offset)); it != handlers_.end()) {
    (it->second)(mock_device_, value);
  } else {
    mock_device_.GetRegisters()->Write<uint32_t>(value, offset);
  }
}

uint32_t RegisterMmioProcessor::MmioRead(zx_off_t offset) {
  return mock_device_.GetRegisters()->Read<uint32_t>(offset);
}

void RegisterMmioProcessor::NoOpHandler(UfsMockDevice& mock_device, uint32_t value) {
  // This is a read only register handler. Nothing to do.
}

void RegisterMmioProcessor::DefaultISHandler(UfsMockDevice& mock_device, uint32_t value) {
  uint32_t is = InterruptStatusReg::Get().ReadFrom(mock_device.GetRegisters()).reg_value();
  is &= (~value);
  InterruptStatusReg::Get().FromValue(is).WriteTo(mock_device.GetRegisters());
}

void RegisterMmioProcessor::DefaultHCEHandler(UfsMockDevice& mock_device, uint32_t value) {
  auto prev_value = HostControllerEnableReg::Get().ReadFrom(mock_device.GetRegisters());
  auto new_value = HostControllerEnableReg::Get().FromValue(value);
  if (new_value.host_controller_enable() && !prev_value.host_controller_enable()) {
    prev_value.set_host_controller_enable(true).WriteTo(mock_device.GetRegisters());
    HostControllerStatusReg::Get()
        .ReadFrom(mock_device.GetRegisters())
        .set_uic_command_ready(true)
        .WriteTo(mock_device.GetRegisters());
  } else if ((!new_value.host_controller_enable() && prev_value.host_controller_enable())) {
    prev_value.set_host_controller_enable(false).WriteTo(mock_device.GetRegisters());
    HostControllerStatusReg::Get().FromValue(0).WriteTo(mock_device.GetRegisters());
  }
}

void RegisterMmioProcessor::DefaultUTRLDBRHandler(UfsMockDevice& mock_device, uint32_t value) {
  ZX_ASSERT_MSG(UtrListRunStopReg::Get().ReadFrom(mock_device.GetRegisters()).value() == true,
                "UtrListRunStopReg value is not true");

  // To clear the door bell manually.
  if (value == 0) {
    UtrListDoorBellReg::Get().FromValue(value).WriteTo(mock_device.GetRegisters());
    return;
  }

  zx_paddr_t transfer_request_list_base_paddr =
      (static_cast<zx_paddr_t>(
           UtrListBaseAddressUpperReg::Get().ReadFrom(mock_device.GetRegisters()).address_upper())
       << 32) |
      UtrListBaseAddressReg::Get().ReadFrom(mock_device.GetRegisters()).reg_value();
  zx::result<zx_vaddr_t> transfer_request_list_base_addr =
      mock_device.MapDmaPaddr(transfer_request_list_base_paddr);
  ZX_ASSERT_MSG(transfer_request_list_base_addr.is_ok(), "Failed to map address.");

  cpp20::span<TransferRequestDescriptor> transfer_request_descriptors(
      reinterpret_cast<TransferRequestDescriptor*>(transfer_request_list_base_addr.value()),
      UfsMockDevice::kNutrs);

  std::bitset<kMaxRequestListSize> pending_slots = value;
  std::bitset<kMaxRequestListSize> completed_slots = 0;
  zx_status_t status = ZX_OK;
  for (uint32_t slot = 0; slot < pending_slots.size(); ++slot) {
    if (!pending_slots.test(slot)) {
      continue;
    }

    status = mock_device.GetTransferRequestProcessor().HandleTransferRequest(
        transfer_request_descriptors[slot]);
    // If ZX_ERR_TIMED_OUT is returned, we emulate the timeout by not setting the completion
    // notification.
    if (status == ZX_ERR_TIMED_OUT) {
      // If timeout has occurred, doorbell should be kept at 1.
      uint32_t prev_doorbell =
          UtrListDoorBellReg::Get().ReadFrom(mock_device.GetRegisters()).door_bell();
      prev_doorbell |= 1 << slot;
      UtrListDoorBellReg::Get().FromValue(prev_doorbell).WriteTo(mock_device.GetRegisters());
    } else {
      pending_slots.reset(slot);
      completed_slots.set(slot);
    }
  }
  mock_device.GetTransferRequestProcessor().SetPendingSlots(pending_slots);

  uint32_t current_doorbell =
      UtrListDoorBellReg::Get().ReadFrom(mock_device.GetRegisters()).door_bell();
  current_doorbell &= ~static_cast<uint32_t>(completed_slots.to_ulong());
  UtrListDoorBellReg::Get().FromValue(current_doorbell).WriteTo(mock_device.GetRegisters());

  uint32_t prev_completion =
      UtrListCompletionNotificationReg::Get().ReadFrom(mock_device.GetRegisters()).notification();
  UtrListCompletionNotificationReg::Get()
      .FromValue(0)
      .set_notification(prev_completion | static_cast<uint32_t>(completed_slots.to_ulong()))
      .WriteTo(mock_device.GetRegisters());
}

void RegisterMmioProcessor::DefaultUTRLCLRHandler(UfsMockDevice& mock_device, uint32_t value) {
  uint32_t current_doorbell =
      UtrListDoorBellReg::Get().ReadFrom(mock_device.GetRegisters()).door_bell();
  current_doorbell &= value;
  UtrListDoorBellReg::Get().FromValue(current_doorbell).WriteTo(mock_device.GetRegisters());
}

void RegisterMmioProcessor::DefaultUTRLRSRHandler(UfsMockDevice& mock_device, uint32_t value) {
  ZX_ASSERT_MSG(value <= 1, "Invalid argument, UTRLRSR register can only be set to 1 or 0. ");

  if (HostControllerStatusReg::Get()
          .ReadFrom(mock_device.GetRegisters())
          .utp_transfer_request_list_ready()) {
    UtrListRunStopReg::Get()
        .ReadFrom(mock_device.GetRegisters())
        .set_value(value)
        .WriteTo(mock_device.GetRegisters());
  }
}

void RegisterMmioProcessor::DefaultUTRLCNRHandler(UfsMockDevice& mock_device, uint32_t value) {
  uint32_t notification =
      UtrListCompletionNotificationReg::Get().ReadFrom(mock_device.GetRegisters()).notification();
  notification &= (~value);
  UtrListCompletionNotificationReg::Get()
      .FromValue(notification)
      .WriteTo(mock_device.GetRegisters());
}

void RegisterMmioProcessor::DefaultUTMRLDBRHandler(UfsMockDevice& mock_device, uint32_t value) {
  ZX_ASSERT_MSG(UtmrListRunStopReg::Get().ReadFrom(mock_device.GetRegisters()).value() == true,
                "UtmrListRunStopReg value is not true");

  zx_paddr_t transfer_request_list_base_paddr =
      (static_cast<zx_paddr_t>(
           UtmrListBaseAddressUpperReg::Get().ReadFrom(mock_device.GetRegisters()).address_upper())
       << 32) |
      UtmrListBaseAddressReg::Get().ReadFrom(mock_device.GetRegisters()).reg_value();
  zx::result<zx_vaddr_t> transfer_request_list_base_addr =
      mock_device.MapDmaPaddr(transfer_request_list_base_paddr);
  ZX_ASSERT_MSG(transfer_request_list_base_addr.is_ok(), "Failed to map address.");

  cpp20::span<TaskManagementRequestDescriptor> task_management_request_descriptors(
      reinterpret_cast<TaskManagementRequestDescriptor*>(transfer_request_list_base_addr.value()),
      UfsMockDevice::kNutrs);

  std::bitset<kMaxRequestListSize> slots = value;
  for (uint32_t slot = 0; slot < slots.size(); ++slot) {
    if (!slots.test(slot)) {
      continue;
    }

    mock_device.GetTaskManagementRequestProcessor().HandleTaskManagementRequest(
        task_management_request_descriptors[slot]);
  }
}

void RegisterMmioProcessor::DefaultUTMRLRSRHandler(UfsMockDevice& mock_device, uint32_t value) {
  ZX_ASSERT_MSG(value <= 1, "Invalid argument, UTMRLRSR register can only be set to 1 or 0. ");

  if (HostControllerStatusReg::Get()
          .ReadFrom(mock_device.GetRegisters())
          .utp_task_management_request_list_ready()) {
    UtmrListRunStopReg::Get()
        .ReadFrom(mock_device.GetRegisters())
        .set_value(value)
        .WriteTo(mock_device.GetRegisters());
  }
}

void RegisterMmioProcessor::DefaultUICCMDHandler(UfsMockDevice& mock_device, uint32_t value) {
  mock_device.GetUicCmdProcessor().HandleUicCmd(static_cast<UicCommandOpcode>(value));
}

}  // namespace ufs_mock_device
}  // namespace ufs

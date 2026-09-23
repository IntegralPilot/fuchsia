// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/driver/logging/cpp/logger.h>

#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {

zx_status_t TransferRequestProcessor::HandleTransferRequest(TransferRequestDescriptor &descriptor) {
  zx_paddr_t command_desc_base_paddr =
      (static_cast<zx_paddr_t>(descriptor.utp_command_descriptor_base_address_upper()) << 32) |
      descriptor.utp_command_descriptor_base_address();

  zx::result<zx_vaddr_t> command_desc_base_addr = mock_device_.MapDmaPaddr(command_desc_base_paddr);
  ZX_ASSERT_MSG(command_desc_base_addr.is_ok(), "Failed to map address.");

  CommandDescriptorData command_descriptor_data;
  command_descriptor_data.command_upiu_base_addr = command_desc_base_addr.value();
  command_descriptor_data.response_upiu_base_addr =
      command_desc_base_addr.value() + descriptor.response_upiu_offset() * sizeof(uint32_t);
  command_descriptor_data.response_upiu_length =
      descriptor.response_upiu_length() * sizeof(uint32_t);
  command_descriptor_data.prdt_base_addr =
      command_desc_base_addr.value() + descriptor.prdt_offset() * sizeof(uint32_t);
  // prdt_length() is the number of PRDT entries (not dword offset count).
  command_descriptor_data.prdt_entry_count = descriptor.prdt_length();

  UpiuHeader *command_upiu_header =
      reinterpret_cast<UpiuHeader *>(command_descriptor_data.command_upiu_base_addr);

  UpiuHeader *response_upiu_header =
      reinterpret_cast<UpiuHeader *>(command_descriptor_data.response_upiu_base_addr);
  CustomMemCpy(response_upiu_header, command_upiu_header, sizeof(UpiuHeader));
  response_upiu_header->set_trans_code(command_upiu_header->trans_code() | (1 << 5));

  if (mock_device_.GetExceptionEventAlert()) {
    response_upiu_header->set_event_alert(true);
    mock_device_.SetExceptionEventAlert(false);
  }

  UpiuTransactionCodes opcode =
      static_cast<UpiuTransactionCodes>(command_upiu_header->trans_code());
  auto it = handlers_.find(opcode);
  if (it == handlers_.end()) {
    fdf::error("UFS MOCK: transfer request opcode: 0x{:x} is not supported",
               static_cast<uint32_t>(opcode));
    descriptor.set_overall_command_status(OverallCommandStatus::kInvalid);
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto trigger_intr = fit::defer([&]() {
    InterruptStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_utp_transfer_request_completion_status(true)
        .WriteTo(mock_device_.GetRegisters());
    if (descriptor.interrupt() && InterruptEnableReg::Get()
                                      .ReadFrom(mock_device_.GetRegisters())
                                      .utp_transfer_request_completion_enable()) {
      mock_device_.TriggerInterrupt();
    }
  });

  OverallCommandStatus ocs = OverallCommandStatus::kSuccess;
  zx_status_t status = (it->second)(mock_device_, command_descriptor_data);
  if (status != ZX_OK) {
    if (status == ZX_ERR_TIMED_OUT) {
      ocs = OverallCommandStatus::kAborted;
      // Timeout does not trigger a completion interrupt.
      trigger_intr.cancel();
    } else if (opcode == static_cast<uint8_t>(UpiuTransactionCodes::kCommand)) {
      // For SCSI commands, SCSI status and sense data are returned in the response UPIU.
      // The descriptor overall command status (OCS) remains kSuccess.
      ocs = OverallCommandStatus::kSuccess;
    } else {
      fdf::warn("UFS MOCK: transfer request opcode: 0x{:x} returned an error",
                static_cast<uint32_t>(opcode));
      ocs = OverallCommandStatus::kInvalid;
    }
  }
  descriptor.set_overall_command_status(ocs);
  return status;
}

zx_status_t TransferRequestProcessor::DefaultNopOutHandler(
    UfsMockDevice &mock_device, CommandDescriptorData command_descriptor_data) {
  NopInUpiuData *nop_in_upiu =
      reinterpret_cast<NopInUpiuData *>(command_descriptor_data.response_upiu_base_addr);
  nop_in_upiu->header.data_segment_length = 0;
  nop_in_upiu->header.flags = 0;
  nop_in_upiu->header.response = UpiuHeaderResponseCode::kTargetSuccess;
  return ZX_OK;
}

zx_status_t TransferRequestProcessor::DefaultQueryHandler(
    UfsMockDevice &mock_device, CommandDescriptorData command_descriptor_data) {
  QueryRequestUpiuData *request_upiu =
      reinterpret_cast<QueryRequestUpiuData *>(command_descriptor_data.command_upiu_base_addr);
  QueryResponseUpiuData *response_upiu =
      reinterpret_cast<QueryResponseUpiuData *>(command_descriptor_data.response_upiu_base_addr);

  response_upiu->opcode = request_upiu->opcode;
  response_upiu->idn = request_upiu->idn;
  response_upiu->index = request_upiu->index;
  response_upiu->selector = request_upiu->selector;
  response_upiu->header.response = static_cast<uint8_t>(QueryResponseCode::kSuccess);

  zx_status_t status =
      mock_device.GetQueryRequestProcessor().HandleQueryRequest(*request_upiu, *response_upiu);
  if (status != ZX_OK) {
    response_upiu->header.response = static_cast<uint8_t>(QueryResponseCode::kGeneralFailure);
  }
  response_upiu->header.data_segment_length = response_upiu->length;
  return status;
}

zx_status_t TransferRequestProcessor::DefaultCommandHandler(
    UfsMockDevice &mock_device, CommandDescriptorData command_descriptor_data) {
  CommandUpiuData *command_upiu =
      reinterpret_cast<CommandUpiuData *>(command_descriptor_data.command_upiu_base_addr);
  ResponseUpiuData *response_upiu =
      reinterpret_cast<ResponseUpiuData *>(command_descriptor_data.response_upiu_base_addr);
  cpp20::span<PhysicalRegionDescriptionTableEntry> prdt_upius(
      reinterpret_cast<PhysicalRegionDescriptionTableEntry *>(
          command_descriptor_data.prdt_base_addr),
      command_descriptor_data.prdt_entry_count);

  return mock_device.GetScsiCommandProcessor().HandleScsiCommand(*command_upiu, *response_upiu,
                                                                 prdt_upius);
}

}  // namespace ufs_mock_device
}  // namespace ufs

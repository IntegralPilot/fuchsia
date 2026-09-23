// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/driver/logging/cpp/logger.h>

#include <algorithm>
#include <cstdio>

#include "src/devices/block/lib/common/include/common.h"
#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {
namespace {
zx::result<uint8_t *> PrdtMapAndGetVirtualAddress(
    UfsMockDevice &mock_device, const PhysicalRegionDescriptionTableEntry &prdt_upiu) {
  zx_paddr_t data_buffer_paddr =
      (static_cast<zx_paddr_t>(prdt_upiu.data_base_address_upper()) << 32) |
      prdt_upiu.data_base_address();

  uint8_t *data_buffer;
  if (auto result = mock_device.MapDmaPaddr(data_buffer_paddr); result.is_error()) {
    fdf::error("UFS MOCK: scsi command could not get virtual address");
    return result.take_error();
  } else {
    data_buffer = reinterpret_cast<uint8_t *>(*result);
  }
  return zx::ok(data_buffer);
}

}  // namespace

zx_status_t CopyBufferToPhysicalRegion(UfsMockDevice &mock_device,
                                       cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius,
                                       const std::vector<uint8_t> &buffer) {
  uint64_t cur_pos = 0;
  for (auto &prdt_upiu : prdt_upius) {
    if (cur_pos >= buffer.size()) {
      break;
    }

    auto data_buffer = PrdtMapAndGetVirtualAddress(mock_device, prdt_upiu);
    if (data_buffer.is_error()) {
      fdf::error("UFS MOCK: Faild to map the data buffer.");
      return data_buffer.status_value();
    }

    uint64_t data_buffer_size = prdt_upiu.data_byte_count() + 1;
    uint64_t transfer_count = std::min(data_buffer_size, buffer.size() - cur_pos);
    CustomMemCpy(data_buffer.value(), buffer.data() + cur_pos, transfer_count);

    cur_pos += transfer_count;
  }

  if (cur_pos != buffer.size()) {
    fdf::error("UFS MOCK: scsi command buffer size is too small");
    return ZX_ERR_NO_SPACE;
  }

  return ZX_OK;
}

zx_status_t CopyPhysicalRegionToBuffer(
    UfsMockDevice &mock_device, std::vector<uint8_t> &buffer,
    const cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint64_t cur_pos = 0;
  for (auto &prdt_upiu : prdt_upius) {
    if (cur_pos >= buffer.size()) {
      break;
    }

    auto data_buffer = PrdtMapAndGetVirtualAddress(mock_device, prdt_upiu);
    if (data_buffer.is_error()) {
      return data_buffer.status_value();
    }

    uint64_t data_buffer_size = prdt_upiu.data_byte_count() + 1;
    uint64_t transfer_count = std::min(data_buffer_size, buffer.size() - cur_pos);
    CustomMemCpy(buffer.data() + cur_pos, data_buffer.value(), transfer_count);

    cur_pos += transfer_count;
  }

  if (cur_pos != buffer.size()) {
    fdf::error("UFS MOCK: scsi command buffer size is too small");
    return ZX_ERR_NO_SPACE;
  }

  return ZX_OK;
}

void ScsiCommandProcessor::BuildSenseData(ResponseUpiuData &response_upiu,
                                          const scsi::SenseKey sense_key) {
  response_upiu.header.data_segment_length = htobe16(sizeof(scsi::FixedFormatSenseDataHeader));
  response_upiu.sense_data_len = htobe16(sizeof(scsi::FixedFormatSenseDataHeader));
  auto *sense_data = reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response_upiu.sense_data);
  sense_data->set_response_code(scsi::SenseDataResponseCodes::kFixedCurrentInformation);
  sense_data->set_valid(0);
  sense_data->set_sense_key(sense_key);
  sense_data->additional_sense_length = 10;
  if (sense_key != scsi::SenseKey::NO_SENSE) {
    response_upiu.header.status = static_cast<uint8_t>(scsi::StatusCode::CHECK_CONDITION);
  }
}

bool ScsiCommandProcessor::IsProcessableCommand(uint8_t lun, scsi::Opcode opcode) const {
  switch (static_cast<WellKnownLuns>(lun)) {
    case WellKnownLuns::kReportLuns:
      return opcode == scsi::Opcode::INQUIRY || opcode == scsi::Opcode::REQUEST_SENSE ||
             opcode == scsi::Opcode::TEST_UNIT_READY || opcode == scsi::Opcode::REPORT_LUNS;
    case WellKnownLuns::kUfsDevice:
      return opcode == scsi::Opcode::INQUIRY || opcode == scsi::Opcode::REQUEST_SENSE ||
             opcode == scsi::Opcode::TEST_UNIT_READY || opcode == scsi::Opcode::START_STOP_UNIT ||
             opcode == scsi::Opcode::FORMAT_UNIT;
    case WellKnownLuns::kBoot:
      return opcode == scsi::Opcode::INQUIRY || opcode == scsi::Opcode::REQUEST_SENSE ||
             opcode == scsi::Opcode::TEST_UNIT_READY || opcode == scsi::Opcode::READ_10 ||
             opcode == scsi::Opcode::READ_16;
    case WellKnownLuns::kRpmb:
      return opcode == scsi::Opcode::INQUIRY || opcode == scsi::Opcode::REQUEST_SENSE ||
             opcode == scsi::Opcode::TEST_UNIT_READY ||
             opcode == scsi::Opcode::SECURITY_PROTOCOL_IN ||
             opcode == scsi::Opcode::SECURITY_PROTOCOL_OUT;
    default:
      // Returns true if the LUN is a scsi disk.
      return !(lun & kUfsWellKnownlunId);
  };
}

zx_status_t ScsiCommandProcessor::HandleScsiCommand(
    CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  scsi::Opcode opcode = static_cast<scsi::Opcode>(command_upiu.cdb[0]);
  std::vector<uint8_t> data;

  uint8_t lun = command_upiu.header.lun;
  if (!IsProcessableCommand(lun, opcode)) {
    fdf::error("UFS MOCK: Lun({}) does not support scsi command opcode: 0x{:x} is not supported",
               lun, static_cast<uint8_t>(opcode));
    return ZX_ERR_INVALID_ARGS;
  }

  if (auto it = handlers_.find(opcode); it != handlers_.end()) {
    if (auto result = (it->second)(mock_device_, command_upiu, response_upiu, prdt_upius);
        result.is_error()) {
      scsi::SenseKey sense_key = scsi::SenseKey::ILLEGAL_REQUEST;
      auto *response_data =
          reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response_upiu.sense_data);
      if (response_data->sense_key() != scsi::SenseKey::NO_SENSE) {
        sense_key = static_cast<scsi::SenseKey>(response_data->sense_key());
      }
      BuildSenseData(response_upiu, sense_key);
      return result.status_value();
    } else {
      data = std::move(*result);
    }
  } else {
    fdf::error("UFS MOCK: scsi command opcode: 0x{:x} is not supported",
               static_cast<uint8_t>(opcode));
    BuildSenseData(response_upiu, scsi::SenseKey::ILLEGAL_REQUEST);
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (response_upiu.header.status == static_cast<uint8_t>(scsi::StatusCode::GOOD)) {
    BuildSenseData(response_upiu, scsi::SenseKey::NO_SENSE);
  }

  zx_status_t status = ZX_OK;
  if (command_upiu.header_flags_r()) {
    status = CopyBufferToPhysicalRegion(mock_device_, prdt_upius, data);
  }
  return status;
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultRequestSenseHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  ScsiCommandUpiu request_sense_command(command_upiu);
  scsi::RequestSenseCDB *scsi_cdb = reinterpret_cast<scsi::RequestSenseCDB *>(
      request_sense_command.GetData<CommandUpiuData>()->cdb);

  if (scsi_cdb->desc != 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<uint8_t> data_buffer(scsi_cdb->allocation_length);
  auto *sense_data = reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(data_buffer.data());
  sense_data->set_response_code(scsi::SenseDataResponseCodes::kFixedCurrentInformation);
  sense_data->set_valid(0);
  sense_data->set_sense_key(scsi::SenseKey::NO_SENSE);
  sense_data->additional_sense_length = 10;

  ZX_ASSERT(command_upiu.header_flags_r());
  if (auto status = CopyBufferToPhysicalRegion(mock_device, prdt_upius, data_buffer);
      status != ZX_OK) {
    fdf::error("UFS MOCK: scsi command, Failed to CopyBufferToPhysicalRegion");
    return zx::error(status);
  }

  mock_device.SetUnitAttention(false);

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultRead10Handler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint8_t lun = command_upiu.header.lun;
  ScsiCommandUpiu read10_command(command_upiu);

  scsi::Read10CDB *scsi_cdb =
      reinterpret_cast<scsi::Read10CDB *>(read10_command.GetData<CommandUpiuData>()->cdb);
  size_t block_count = betoh16(scsi_cdb->transfer_length);
  off_t start_lba = be32toh(scsi_cdb->logical_block_address);

  if (betoh32(command_upiu.expected_data_transfer_length) != block_count * kMockBlockSize) {
    fdf::error(
        "UFS MOCK: scsi READ(10) command, expected_data_transfer_length({}) and "
        "transfer_length(%lu) are different.",
        betoh32(command_upiu.expected_data_transfer_length), block_count * kMockBlockSize);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<uint8_t> data_buffer(block_count * kMockBlockSize);

  if (auto status = mock_device.BufferRead(lun, data_buffer.data(), block_count, start_lba);
      status != ZX_OK) {
    return zx::error(status);
  }

  ZX_ASSERT(command_upiu.header_flags_r());
  if (auto status = CopyBufferToPhysicalRegion(mock_device, prdt_upius, data_buffer);
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultWrite10Handler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint8_t lun = command_upiu.header.lun;
  ScsiCommandUpiu write10_command(command_upiu);

  scsi::Write10CDB *scsi_cdb =
      reinterpret_cast<scsi::Write10CDB *>(write10_command.GetData<CommandUpiuData>()->cdb);
  size_t block_count = betoh16(scsi_cdb->transfer_length);
  off_t start_lba = be32toh(scsi_cdb->logical_block_address);

  if (betoh32(command_upiu.expected_data_transfer_length) != block_count * kMockBlockSize) {
    fdf::error(
        "UFS MOCK: scsi WRTIE(10) command, expected_data_transfer_length({}) and "
        "transfer_length(%lu) are different.",
        betoh32(command_upiu.expected_data_transfer_length), block_count * kMockBlockSize);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<uint8_t> data_buffer(block_count * kMockBlockSize);

  ZX_ASSERT(command_upiu.header_flags_w());
  if (auto status = CopyPhysicalRegionToBuffer(mock_device, data_buffer, prdt_upius);
      status != ZX_OK) {
    return zx::error(status);
  }

  if (auto status = mock_device.BufferWrite(lun, data_buffer.data(), block_count, start_lba);
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultReadCapacity10Handler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer(sizeof(scsi::ReadCapacity10ParameterData));
  auto *read_capacity_data =
      reinterpret_cast<scsi::ReadCapacity10ParameterData *>(data_buffer.data());

  // |returned_logical_block_address| is a 0-based value.
  read_capacity_data->returned_logical_block_address =
      htobe32((kMockTotalDeviceCapacity / kMockBlockSize) - 1);
  read_capacity_data->block_length_in_bytes = htobe32(kMockBlockSize);

  ZX_ASSERT(command_upiu.header_flags_r());
  if (auto status = CopyBufferToPhysicalRegion(mock_device, prdt_upius, data_buffer);
      status != ZX_OK) {
    fdf::error("UFS MOCK: scsi command, Failed to CopyBufferToPhysicalRegion");
    return zx::error(status);
  }

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultSynchronizeCache10Handler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer;

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultTestUnitReadyHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer;

  if (mock_device.GetUnitAttention()) {
    auto *response_data =
        reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response_upiu.sense_data);
    response_data->set_sense_key(scsi::SenseKey::UNIT_ATTENTION);
    mock_device.SetUnitAttention(false);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultInquiryHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  ScsiCommandUpiu Inquiry_command(command_upiu);
  scsi::InquiryCDB *scsi_cdb =
      reinterpret_cast<scsi::InquiryCDB *>(Inquiry_command.GetData<CommandUpiuData>()->cdb);

  std::vector<uint8_t> data_buffer;
  if (!scsi_cdb->reserved_and_evpd) {
    data_buffer.resize(sizeof(scsi::InquiryData));
    auto *inquiry_data = reinterpret_cast<scsi::InquiryData *>(data_buffer.data());

    std::strncpy(inquiry_data->t10_vendor_id, "Fuchsia", sizeof(inquiry_data->t10_vendor_id));
    std::strncpy(inquiry_data->product_id, "ufs-mock-device", sizeof(inquiry_data->product_id));
    std::strncpy(inquiry_data->product_revision, "1.00", sizeof(inquiry_data->product_revision));
  } else {
    if (scsi_cdb->page_code == scsi::InquiryCDB::kPageListVpdPageCode) {
      data_buffer.resize(sizeof(scsi::VPDPageList));
      auto *vpd_page_list = reinterpret_cast<scsi::VPDPageList *>(data_buffer.data());
      vpd_page_list->peripheral_qualifier_device_type = 0;
      vpd_page_list->page_code = scsi::InquiryCDB::kPageListVpdPageCode;
      vpd_page_list->page_length = 2;
      vpd_page_list->pages[0] = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
      vpd_page_list->pages[1] = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
    } else if (scsi_cdb->page_code == scsi::InquiryCDB::kBlockLimitsVpdPageCode) {
      data_buffer.resize(sizeof(scsi::VPDBlockLimits));
      auto *block_limits = reinterpret_cast<scsi::VPDBlockLimits *>(data_buffer.data());
      block_limits->peripheral_qualifier_device_type = 0;
      block_limits->page_code = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
      block_limits->maximum_transfer_blocks = htobe32(UINT32_MAX);
      block_limits->maximum_unmap_lba_count = htobe32(UINT32_MAX);
    } else if (scsi_cdb->page_code == scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode) {
      data_buffer.resize(sizeof(scsi::VPDLogicalBlockProvisioning));
      auto *provisioning =
          reinterpret_cast<scsi::VPDLogicalBlockProvisioning *>(data_buffer.data());
      provisioning->peripheral_qualifier_device_type = 0;
      provisioning->page_code = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
      provisioning->set_lbpu(true);
      provisioning->set_provisioning_type(0x02);  // The logical unit is thin provisioned
    } else {
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
  }
  ZX_ASSERT(command_upiu.header_flags_r());

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultModeSense10Handler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer;

  auto *mode_sense_10_cdb = reinterpret_cast<scsi::ModeSense10CDB *>(command_upiu.cdb);

  if (mode_sense_10_cdb->page_code() == scsi::PageCode::kAllPageCode) {
    data_buffer.resize(sizeof(scsi::Mode10ParameterHeader));
    auto *mode_page = reinterpret_cast<scsi::Mode10ParameterHeader *>(data_buffer.data());
    mode_page->set_dpo_fua_available(true);
    mode_page->set_write_protected(false);
  } else if (mode_sense_10_cdb->page_code() == scsi::PageCode::kCachingPageCode) {
    data_buffer.resize(sizeof(scsi::Mode10ParameterHeader) + sizeof(scsi::CachingModePage));
    auto *caching_mode_page = reinterpret_cast<scsi::CachingModePage *>(
        data_buffer.data() + sizeof(scsi::Mode10ParameterHeader));
    caching_mode_page->set_page_code(static_cast<uint8_t>(scsi::PageCode::kCachingPageCode));
    caching_mode_page->set_write_cache_enabled(true);
  } else {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_ASSERT(command_upiu.header_flags_r());

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultUnmapHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint8_t lun = command_upiu.header.lun;
  ScsiCommandUpiu unmap_command(command_upiu);

  constexpr uint32_t parameter_list_length =
      sizeof(scsi::UnmapParameterListHeader) + sizeof(scsi::UnmapBlockDescriptor);

  scsi::UnmapCDB *scsi_cdb =
      reinterpret_cast<scsi::UnmapCDB *>(unmap_command.GetData<CommandUpiuData>()->cdb);
  ZX_ASSERT(betoh16(scsi_cdb->parameter_list_length) == parameter_list_length);

  std::vector<uint8_t> data_buffer(parameter_list_length);
  ZX_ASSERT(command_upiu.header_flags_w());
  if (auto status = CopyPhysicalRegionToBuffer(mock_device, data_buffer, prdt_upius);
      status != ZX_OK) {
    return zx::error(status);
  }
  auto *header = reinterpret_cast<scsi::UnmapParameterListHeader *>(data_buffer.data());

  size_t block_count = betoh32(header->block_descriptors[0].blocks);
  off_t start_lba =
      safemath::checked_cast<off_t>(betoh64(header->block_descriptors[0].logical_block_address));

  auto zero_buf = std::make_unique<uint8_t[]>(block_count * kMockBlockSize);
  std::memset(zero_buf.get(), 0, block_count * kMockBlockSize);
  if (auto status = mock_device.BufferWrite(lun, zero_buf.get(), block_count, start_lba);
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultReportLunsHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer;

  auto *report_luns_cdb = reinterpret_cast<scsi::ReportLunsCDB *>(command_upiu.cdb);

  if (report_luns_cdb->select_report == 0) {
    data_buffer.resize(sizeof(scsi::ReportLunsParameterDataHeader));
    auto *report_luns_parameter =
        reinterpret_cast<scsi::ReportLunsParameterDataHeader *>(data_buffer.data());
    report_luns_parameter->lun_list_length = htobe32(mock_device.GetDeviceDesc().bNumberLU * 8);
  } else {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_ASSERT(command_upiu.header_flags_r());
  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultStartStopUnitHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  std::vector<uint8_t> data_buffer;

  // Nothing to do in mock device.

  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultReadBufferHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint8_t lun = command_upiu.header.lun;
  ScsiCommandUpiu readbuffer_command(command_upiu);

  scsi::ReadBufferCDB *scsi_cdb =
      reinterpret_cast<scsi::ReadBufferCDB *>(readbuffer_command.GetData<CommandUpiuData>()->cdb);
  uint32_t buffer_offset = block::ReadFromBigEndian24(scsi_cdb->buffer_offset);
  uint32_t length = block::ReadFromBigEndian24(scsi_cdb->allocation_length);

  std::vector<uint8_t> data_buffer(length);
  if (zx_status_t status =
          mock_device.ReadFromDeviceBuffer(lun, data_buffer.data(), buffer_offset, length);
      status != ZX_OK) {
    return zx::error(status);
  }

  ZX_DEBUG_ASSERT(command_upiu.header_flags_r());
  if (zx_status_t status = CopyBufferToPhysicalRegion(mock_device, prdt_upius, data_buffer);
      status != ZX_OK) {
    fdf::error("UFS MOCK: scsi command, Failed to CopyBufferToPhysicalRegion");
    return zx::error(status);
  }
  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultWriteBufferHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  uint8_t lun = command_upiu.header.lun;
  ScsiCommandUpiu writebuffer_command(command_upiu);

  scsi::WriteBufferCDB *scsi_cdb =
      reinterpret_cast<scsi::WriteBufferCDB *>(writebuffer_command.GetData<CommandUpiuData>()->cdb);
  uint32_t buffer_offset = block::ReadFromBigEndian24(scsi_cdb->buffer_offset);
  uint32_t length = block::ReadFromBigEndian24(scsi_cdb->parameter_list_length);

  std::vector<uint8_t> data_buffer(length);

  ZX_DEBUG_ASSERT(command_upiu.header_flags_w());
  if (zx_status_t status = CopyPhysicalRegionToBuffer(mock_device, data_buffer, prdt_upius);
      status != ZX_OK) {
    return zx::error(status);
  }

  if (zx_status_t status =
          mock_device.WriteToDeviceBuffer(lun, data_buffer.data(), buffer_offset, length);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultSecurityProtocolInHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  ScsiCommandUpiu security_in_command(command_upiu);
  auto *scsi_cdb = reinterpret_cast<scsi::SecurityProtocolInCDB *>(
      security_in_command.GetData<CommandUpiuData>()->cdb);
  uint32_t allocation_length_be;
  std::memcpy(&allocation_length_be, &scsi_cdb->allocation_length, sizeof(uint32_t));
  uint32_t allocation_length = be32toh(allocation_length_be);
  if (scsi_cdb->inc_512()) {
    allocation_length *= 512;
  }

  // Populate mock read frames with placeholder data (e.g. 0xAB)
  std::vector<uint8_t> data_buffer(allocation_length, 0xAB);

  ZX_DEBUG_ASSERT(command_upiu.header_flags_r());
  if (zx_status_t status = CopyBufferToPhysicalRegion(mock_device, prdt_upius, data_buffer);
      status != ZX_OK) {
    fdf::error("UFS MOCK: DefaultSecurityProtocolInHandler failed to CopyBufferToPhysicalRegion");
    return zx::error(status);
  }
  return zx::ok(std::move(data_buffer));
}

zx::result<std::vector<uint8_t>> ScsiCommandProcessor::DefaultSecurityProtocolOutHandler(
    UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
    cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
  ScsiCommandUpiu security_out_command(command_upiu);
  auto *scsi_cdb = reinterpret_cast<scsi::SecurityProtocolOutCDB *>(
      security_out_command.GetData<CommandUpiuData>()->cdb);
  uint32_t transfer_length_be;
  std::memcpy(&transfer_length_be, &scsi_cdb->transfer_length, sizeof(uint32_t));
  uint32_t transfer_length = be32toh(transfer_length_be);
  if (scsi_cdb->inc_512()) {
    transfer_length *= 512;
  }

  std::vector<uint8_t> data_buffer(transfer_length);

  ZX_DEBUG_ASSERT(command_upiu.header_flags_w());
  if (zx_status_t status = CopyPhysicalRegionToBuffer(mock_device, data_buffer, prdt_upius);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(data_buffer));
}

}  // namespace ufs_mock_device
}  // namespace ufs

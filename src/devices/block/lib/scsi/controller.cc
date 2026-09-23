// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <endian.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <zircon/status.h>

#include <cstdint>
#include <tuple>

#include <safemath/safe_math.h>

#include "src/devices/block/lib/common/include/common.h"
namespace scsi {

zx_status_t Controller::TestUnitReady(uint8_t target, uint16_t lun) {
  scsi::TestUnitReadyCDB cdb = {};
  cdb.opcode = scsi::Opcode::TEST_UNIT_READY;
  zx_status_t status =
      ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false, {nullptr, 0});
  if (status != ZX_OK) {
    driver_logger().log(fdf::DEBUG, "TEST_UNIT_READY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx_status_t Controller::RequestSense(uint8_t target, uint16_t lun, iovec data) {
  RequestSenseCDB cdb = {};
  cdb.opcode = Opcode::REQUEST_SENSE;
  cdb.allocation_length = static_cast<uint8_t>(data.iov_len);
  zx_status_t status =
      ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false, data);
  if (status != ZX_OK) {
    driver_logger().log(fdf::DEBUG, "REQUEST_SENSE failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx::result<InquiryData> Controller::Inquiry(uint8_t target, uint16_t lun) {
  InquiryCDB cdb = {};
  cdb.opcode = Opcode::INQUIRY;
  InquiryData data = {};
  cdb.allocation_length = htobe16(sizeof(data));
  zx_status_t status = ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false,
                                          {&data, sizeof(data)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::DEBUG, "INQUIRY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok(data);
}

zx::result<VPDBlockLimits> Controller::InquiryBlockLimits(uint8_t target, uint16_t lun) {
  InquiryCDB cdb = {};
  cdb.opcode = Opcode::INQUIRY;
  // Query for all supported VPD pages.
  cdb.reserved_and_evpd = 0x1;
  cdb.page_code = 0x00;
  VPDPageList vpd_pagelist = {};
  cdb.allocation_length = htobe16(sizeof(vpd_pagelist));
  zx_status_t status = ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false,
                                          {&vpd_pagelist, sizeof(vpd_pagelist)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "INQUIRY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
    return zx::error(status);
  }

  uint8_t i;
  for (i = 0; i < vpd_pagelist.page_length; ++i) {
    if (vpd_pagelist.pages[i] == InquiryCDB::kBlockLimitsVpdPageCode) {
      break;
    }
  }
  if (i == vpd_pagelist.page_length) {
    driver_logger().log(fdf::INFO,
                        "The Block Limits VPD page is not supported for target {}, lun {}.", target,
                        lun);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // The Block Limits VPD page is supported, fetch it.
  cdb.page_code = InquiryCDB::kBlockLimitsVpdPageCode;
  VPDBlockLimits block_limits = {};
  cdb.allocation_length = htobe16(sizeof(block_limits));
  status = ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false,
                              {&block_limits, sizeof(block_limits)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "INQUIRY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(block_limits);
}

zx::result<bool> Controller::InquirySupportUnmapCommand(uint8_t target, uint16_t lun) {
  InquiryCDB cdb = {};
  cdb.opcode = Opcode::INQUIRY;
  // Query for all supported VPD pages.
  cdb.reserved_and_evpd = 0x1;
  cdb.page_code = 0x00;
  VPDPageList vpd_pagelist = {};
  cdb.allocation_length = htobe16(sizeof(vpd_pagelist));
  zx_status_t status = ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false,
                                          {&vpd_pagelist, sizeof(vpd_pagelist)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "INQUIRY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
    return zx::error(status);
  }

  uint8_t i;
  for (i = 0; i < vpd_pagelist.page_length; ++i) {
    if (vpd_pagelist.pages[i] == InquiryCDB::kLogicalBlockProvisioningVpdPageCode) {
      break;
    }
  }
  if (i == vpd_pagelist.page_length) {
    driver_logger().log(
        fdf::ERROR,
        "The Logical Block Provisioning VPD page is not supported for target {}, lun {}.", target,
        lun);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // The Block Limits VPD page is supported, fetch it.
  cdb.page_code = InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
  VPDLogicalBlockProvisioning provisioning = {};
  cdb.allocation_length = htobe16(sizeof(provisioning));
  status = ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false,
                              {&provisioning, sizeof(provisioning)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "INQUIRY failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(provisioning.lbpu());
}

zx::result<> Controller::ModeSense(uint8_t target, uint16_t lun, PageCode page_code, iovec data,
                                   bool use_mode_sense_6) {
  // Allocate as much as the largest mode sense CDB.
  uint8_t cdb[sizeof(ModeSense10CDB)] = {};
  size_t cdb_size = 0;

  if (use_mode_sense_6 && data.iov_len <= UINT8_MAX) {  // MODE SENSE 6
    if (data.iov_len < sizeof(Mode6ParameterHeader)) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    ModeSense6CDB* cdb_6 = reinterpret_cast<ModeSense6CDB*>(cdb);
    cdb_6->opcode = Opcode::MODE_SENSE_6;
    cdb_6->set_page_code(page_code);
    cdb_6->allocation_length = safemath::checked_cast<uint8_t>(data.iov_len);
    // Do not return any block descriptors.
    cdb_6->set_disable_block_descriptors(true);

    cdb_size = sizeof(ModeSense6CDB);
  } else {  // MODE SENSE 10
    if (data.iov_len < sizeof(Mode10ParameterHeader) || data.iov_len > UINT16_MAX) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    ModeSense10CDB* cdb_10 = reinterpret_cast<ModeSense10CDB*>(cdb);
    cdb_10->opcode = Opcode::MODE_SENSE_10;
    cdb_10->set_page_code(page_code);
    cdb_10->allocation_length = htobe16(safemath::checked_cast<uint16_t>(data.iov_len));
    // Do not return any block descriptors.
    cdb_10->set_disable_block_descriptors(true);

    cdb_size = sizeof(ModeSense10CDB);
  }

  zx_status_t status = ExecuteCommandSync(target, lun, {&cdb, cdb_size},
                                          /*is_write=*/false, data);
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "MODE_SENSE_{} failed for target {}, lun {}: {}", cdb_size,
                        target, lun, zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<std::tuple<bool, bool>> Controller::ModeSenseDpoFuaAndWriteProtectedEnabled(
    uint8_t target, uint16_t lun, bool use_mode_sense_6) {
  constexpr uint8_t header_size =
      std::max(sizeof(Mode6ParameterHeader), sizeof(Mode10ParameterHeader));
  uint8_t data[header_size];

  zx::result result =
      ModeSense(target, lun, PageCode::kAllPageCode, {data, sizeof(data)}, use_mode_sense_6);
  if (result.is_error()) {
    driver_logger().log(fdf::ERROR, "MODE_SENSE failed for target {}, lun {}: {}", target, lun,
                        result.status_string());
    return result.take_error();
  }

  bool dpo_fua_available, write_protected;
  if (use_mode_sense_6) {
    Mode6ParameterHeader* parameter_header = reinterpret_cast<Mode6ParameterHeader*>(data);
    dpo_fua_available = parameter_header->dpo_fua_available();
    write_protected = parameter_header->write_protected();
  } else {
    Mode10ParameterHeader* parameter_header = reinterpret_cast<Mode10ParameterHeader*>(data);
    dpo_fua_available = parameter_header->dpo_fua_available();
    write_protected = parameter_header->write_protected();
  }

  return zx::ok(std::make_tuple(dpo_fua_available, write_protected));
}

zx::result<bool> Controller::ModeSenseWriteCacheEnabled(uint8_t target, uint16_t lun,
                                                        bool use_mode_sense_6) {
  constexpr uint8_t header_size =
      std::max(sizeof(Mode6ParameterHeader), sizeof(Mode10ParameterHeader));
  uint8_t data[header_size + sizeof(CachingModePage)];

  zx::result result =
      ModeSense(target, lun, PageCode::kCachingPageCode, {data, sizeof(data)}, use_mode_sense_6);
  if (result.is_error()) {
    driver_logger().log(fdf::ERROR, "MODE_SENSE failed for target {}, lun {}: {}", target, lun,
                        result.status_string());
    return result.take_error();
  }

  uint32_t mode_page_offset =
      use_mode_sense_6 ? sizeof(Mode6ParameterHeader) : sizeof(Mode10ParameterHeader);
  CachingModePage* mode_page = reinterpret_cast<CachingModePage*>(data + mode_page_offset);
  if (mode_page->page_code() != static_cast<uint8_t>(PageCode::kCachingPageCode)) {
    driver_logger().log(fdf::ERROR, "failed for target {}, lun {} to retrieve caching mode page",
                        target, lun);
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(mode_page->write_cache_enabled());
}

zx_status_t Controller::ReadCapacity(uint8_t target, uint16_t lun, uint64_t* block_count,
                                     uint32_t* block_size_bytes) {
  ReadCapacity10CDB cdb10 = {};
  cdb10.opcode = Opcode::READ_CAPACITY_10;
  ReadCapacity10ParameterData data10 = {};
  zx_status_t status = ExecuteCommandSync(target, lun, {&cdb10, sizeof(cdb10)}, /*is_write=*/false,
                                          {&data10, sizeof(data10)});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "READ_CAPACITY_10 failed for target {}, lun {}: {}", target,
                        lun, zx_status_get_string(status));
    return status;
  }

  *block_count = betoh32(data10.returned_logical_block_address);
  *block_size_bytes = betoh32(data10.block_length_in_bytes);

  if (*block_count == UINT32_MAX) {
    ReadCapacity16CDB cdb16 = {};
    cdb16.opcode = Opcode::READ_CAPACITY_16;
    cdb16.service_action = 0x10;
    ReadCapacity16ParameterData data16 = {};
    cdb16.allocation_length = htobe32(sizeof(data16));
    status = ExecuteCommandSync(target, lun, {&cdb16, sizeof(cdb16)}, /*is_write=*/false,
                                {&data16, sizeof(data16)});
    if (status != ZX_OK) {
      driver_logger().log(fdf::ERROR, "READ_CAPACITY_16 failed for target {}, lun {}: {}", target,
                          lun, zx_status_get_string(status));
      return status;
    }

    *block_count = betoh64(data16.returned_logical_block_address);
    *block_size_bytes = betoh32(data16.block_length_in_bytes);
  }

  // +1 because data.returned_logical_block_address returns the address of the final block, and
  // blocks are zero indexed.
  *block_count = *block_count + 1;
  return ZX_OK;
}

zx::result<uint16_t> Controller::ReportLuns(uint8_t target) {
  ReportLunsCDB cdb = {};
  cdb.opcode = Opcode::REPORT_LUNS;
  ReportLunsParameterDataHeader data = {};
  cdb.allocation_length = htobe32(sizeof(data));
  zx_status_t status =
      ExecuteCommandSync(target, 0, {&cdb, sizeof(cdb)}, /*is_write=*/false, {&data, sizeof(data)});
  if (status != ZX_OK) {
    // Do not log the error, as it generates too many messages. Instead, log on success.
    return zx::error(status);
  } else {
    driver_logger().log(fdf::DEBUG, "REPORT_LUNS succeeded for target {}.", target);
  }

  // data.lun_list_length is the number of bytes of LUN structures.
  const uint32_t lun_count = betoh32(data.lun_list_length) / 8;
  if (lun_count > UINT16_MAX) {
    driver_logger().log(fdf::ERROR, "REPORT_LUNS returned unexpectedly large LUN count: {}",
                        lun_count);
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  return zx::ok(static_cast<uint16_t>(lun_count));
}

zx_status_t Controller::StartStopUnit(uint8_t target, uint16_t lun, bool immed,
                                      PowerCondition power_condition, uint8_t modifier,
                                      std::optional<bool> load_or_unload) {
  StartStopUnitCDB cdb = {};
  cdb.opcode = Opcode::START_STOP_UNIT;
  cdb.set_immed(immed);
  cdb.set_power_condition(power_condition);
  cdb.set_power_condition_modifier(modifier);
  cdb.set_no_flush(false);  // Currently, we only support flush.
  if (load_or_unload.has_value()) {
    if (power_condition != PowerCondition::kStartValid) {
      driver_logger().log(
          fdf::ERROR,
          "Power condition must be START_VALID to perform load/unload, power_condition=0x{:x}",
          static_cast<uint8_t>(power_condition));
      return ZX_ERR_INVALID_ARGS;
    }
    cdb.set_load_eject(true);
    cdb.set_start(load_or_unload.value());  // Load = true, unload = false
  }
  cdb.set_start(0);
  zx_status_t status =
      ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false, {nullptr, 0});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "START STOP UNIT failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx_status_t Controller::FormatUnit(uint8_t target, uint16_t lun) {
  FormatUnitCDB cdb = {};
  cdb.opcode = Opcode::FORMAT_UNIT;
  cdb.set_fmtpinfo(0);      // Currently, only supports type 0 protection.
  cdb.set_fmtdata(false);   // Currently, we do not send the parameter list.
  cdb.set_longlist(false);  // If the FMTDATA is set to zero, then the LONGLIST shall be ignored.
  cdb.set_cmplst(false);    // If the FMTDATA is set to zero, then the CMPLST shall be ignored.
  // If the FMTDATA is set to zero, then the DEFECT_LIST_FORMAT is not available.
  cdb.set_defect_list_format(0);

  zx_status_t status =
      ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false, {nullptr, 0});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "FORMAT UNIT failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx_status_t Controller::SendDiagnostic(uint8_t target, uint16_t lun, SelfTestCode code) {
  SendDiagnosticCDB cdb = {};
  cdb.opcode = Opcode::SEND_DIAGNOSTIC;
  cdb.set_self_test_code(code);

  // We only supports the default self-test feature.
  cdb.set_self_test(true);
  cdb.set_pf(false);
  cdb.parameter_list_length = 0;

  cdb.set_dev_off_l(false);
  cdb.set_unit_off_l(false);

  zx_status_t status =
      ExecuteCommandSync(target, lun, {&cdb, sizeof(cdb)}, /*is_write=*/false, {nullptr, 0});
  if (status != ZX_OK) {
    driver_logger().log(fdf::ERROR, "SEND DIAGNOSTIC failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx::result<uint32_t> Controller::ScanAndBindLogicalUnits(uint8_t target,
                                                         uint32_t max_transfer_bytes,
                                                         uint16_t max_lun, LuCallback lu_callback,
                                                         DeviceOptions device_options) {
  zx::result<uint16_t> lun_count = ReportLuns(target);
  if (lun_count.is_error()) {
    return lun_count.take_error();
  }

  // TODO(b/317838849): We should only attempt to bind to the luns obtained by ReportLuns().
  uint16_t luns_found = 0;
  for (uint16_t lun = 0; lun < max_lun; ++lun) {
    zx::result block_device = BindBlockDevice(target, lun, max_transfer_bytes, device_options);
    if (block_device.is_ok()) {
      scsi::BlockDevice* dev = block_device.value().get();
      {
        std::lock_guard<std::mutex> lock(lock_);
        block_devs_[target][lun] = std::move(block_device.value());
      }
      if (lu_callback) {
        zx::result result = lu_callback(lun, dev->block_size_bytes(), dev->block_count());
        if (result.is_error()) {
          driver_logger().log(fdf::ERROR, "SCSI: lu_callback for block device failed: {}",
                              result.status_string());
          return result.take_error();
        }
      }
      ++luns_found;
    }

    if (luns_found == lun_count.value()) {
      break;
    }
  }

  if (luns_found != lun_count.value()) {
    driver_logger().log(fdf::ERROR,
                        "SCSI: Lun count({}) and the number of luns found({}) are different.",
                        lun_count.value(), luns_found);
    return zx::error(ZX_ERR_BAD_STATE);
  }

  return zx::ok(lun_count.value());
}

zx::result<std::unique_ptr<BlockDevice>> Controller::BindBlockDevice(uint8_t target, uint16_t lun,
                                                                     uint32_t max_transfer_bytes,
                                                                     DeviceOptions device_options) {
  return BlockDevice::Bind(this, target, lun, max_transfer_bytes, device_options);
}

zx::result<PostProcess> Controller::CheckSenseData(const FixedFormatSenseDataHeader& sense_data) {
  // Currently, we only support fixed format sense data.
  if (sense_data.response_code() != SenseDataResponseCodes::kFixedCurrentInformation) {
    driver_logger().log(fdf::WARN,
                        "SCSI: It only supports FixedFormatSenseData, response_code=0x{:x}",
                        static_cast<uint8_t>(sense_data.response_code()));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (sense_data.filemark() || sense_data.eom() || sense_data.ili()) {
    driver_logger().log(fdf::WARN, "SCSI: Invalid flags, filemark={}, EOM={}, ILI={}",
                        sense_data.filemark(), sense_data.eom(), sense_data.ili());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  PostProcess post_process = PostProcess::kNone;
  switch (sense_data.sense_key()) {
    case SenseKey::NO_SENSE:
    case SenseKey::RECOVERED_ERROR:
      break;
    case SenseKey::ABORTED_COMMAND: {
      if (sense_data.additional_sense_code == 0x10) {  // DIF(Data Integrity Field)
        // If aborted due to a DIF error, there is no reason to retry.
        return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
      }
      // Check if the abort is due to a command timeout.
      // - ASC=0x2e, ASCQ=0x01: COMMAND TIMEOUT BEFORE PROCESSING
      // - ASC=0x2e, ASCQ=0x02: COMMAND TIMEOUT DURING PROCESSING
      // - ASC=0x2e, ASCQ=0x03: COMMAND TIMEOUT DURING PROCESSING DUE TO ERROR RECOVERY
      if (sense_data.additional_sense_code == 0x2e &&
          sense_data.additional_sense_code_qualifier >= 0x01 &&
          sense_data.additional_sense_code_qualifier <= 0x03) {
        return zx::error(ZX_ERR_TIMED_OUT);
      }
      post_process = PostProcess::kNeedsRetry;
      break;
    }
    case SenseKey::NOT_READY:
    case SenseKey::UNIT_ATTENTION:
      // Expecting a CHECK_CONDITION/UNIT_ATTENTION because of a bus reset. In this case, we
      // need to retry.
      // - ASC=0x28, ASCQ=0x00: NOT READY TO READY CHANGE, MEDIUM MAY HAVE CHANGED
      if (expect_check_condition_or_unit_attention_ &&
          (sense_data.additional_sense_code != 0x28 ||
           sense_data.additional_sense_code_qualifier != 0x00)) {
        expect_check_condition_or_unit_attention_ = false;
        post_process = PostProcess::kNeedsRetry;
        break;
      }

      // TODO(b/317838849): ASC=0x3f, ASCQ=0x0e: REPORTED LUN DATA HAS CHANGED
      // TODO(b/317838849): ASC=0x04, ASCQ=0x02: LOGICAL UNIT NOT READY, INITIALIZING COMMAND
      // REQUIRED

      // If the device is preparing, we should retry.
      // - ASC=0x04, ASCQ=0x01: LOGICAL UNIT IS IN PROCESS OF BECOMING READY
      if (sense_data.additional_sense_code == 0x04 &&
          sense_data.additional_sense_code_qualifier == 0x01) {
        post_process = PostProcess::kNeedsRetry;
        break;
      }
      return zx::error(ZX_ERR_BAD_STATE);
    default:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(post_process);
}

zx::result<PostProcess> Controller::CheckScsiStatus(
    StatusCode status_code,
    std::optional<std::reference_wrapper<FixedFormatSenseDataHeader>> sense_data) {
  PostProcess post_process = PostProcess::kNone;
  switch (status_code) {
    case StatusCode::GOOD:
    case StatusCode::TASK_ABORTED:
      break;
    case StatusCode::CHECK_CONDITION:
      if (!sense_data) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      return CheckSenseData(*sense_data);
    case StatusCode::TASK_SET_FULL:
    case StatusCode::BUSY:
      post_process = PostProcess::kNeedsRetry;
      break;
    case StatusCode::CONDITION_MET:
    case StatusCode::INTERMEDIATE:
    case StatusCode::INTERMEDIATE_CONDITION_MET:
    case StatusCode::ACA_ACTIVE:
    case StatusCode::RESERVATION_CONFILCT:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    default:
      return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(post_process);
}

zx::result<> Controller::ScsiComplete(
    StatusMessage status_message,
    std::optional<std::reference_wrapper<FixedFormatSenseDataHeader>> sense_data) {
  zx::result<PostProcess> post_process = zx::ok(PostProcess::kNone);

  switch (status_message.host_status_code) {
    case HostStatusCode::kOk: {
      post_process = CheckScsiStatus(status_message.scsi_status_code, sense_data);
      break;
    }
    case HostStatusCode::kTimeout:
      post_process = zx::ok(PostProcess::kNeedsErrorHandling);
      break;
    case HostStatusCode::kRequeue:
    case HostStatusCode::kError:
      post_process = zx::ok(PostProcess::kNeedsRetry);
      break;
    case HostStatusCode::kAbort:
      post_process = zx::error(ZX_ERR_IO_REFUSED);
      break;
    default:
      driver_logger().log(fdf::WARN, "SCSI: Unexpected host status value({})",
                          static_cast<uint8_t>(status_message.host_status_code));
      post_process = zx::error(ZX_ERR_BAD_STATE);
      break;
  }

  if (post_process.is_ok() && post_process == PostProcess::kNeedsErrorHandling) {
    // Always returns ZX_ERR_TIMED_OUT until we implement an error handler.
    post_process = zx::error(ZX_ERR_TIMED_OUT);
  }

  if (post_process.is_ok() && post_process == PostProcess::kNeedsRetry) {
    // Before retry is implemented, UNIT_ATTENTION is ignored by returning ZX_ERR_UNAVAILABLE.
    if (sense_data.has_value() && (sense_data->get().sense_key() == SenseKey::UNIT_ATTENTION)) {
      return zx::error(ZX_ERR_UNAVAILABLE);
    }

    // TODO(b/317838849): We need to implement the retry behavior.

    post_process = zx::error(ZX_ERR_BAD_STATE);
  }

  return zx::make_result(post_process.status_value());
}

zx_status_t Controller::ReadBuffer(uint8_t target, uint16_t lun, uint8_t mod, uint8_t buffer_id,
                                   uint32_t buffer_offset, iovec data) {
  ReadBufferCDB cdb = {};
  cdb.opcode = Opcode::READ_BUFFER;
  cdb.mod = mod;
  cdb.buffer_id = buffer_id;
  block::WriteToBigEndian24(buffer_offset, cdb.buffer_offset);
  if (data.iov_len > UINT32_MAX) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  block::WriteToBigEndian24(static_cast<uint32_t>(data.iov_len), cdb.allocation_length);

  zx_status_t status = ExecuteCommandSync(target, lun, {.iov_base = &cdb, .iov_len = sizeof(cdb)},
                                          /*is_write=*/false, data);
  if (status != ZX_OK) {
    driver_logger().log(fdf::DEBUG, "READ BUFFER failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

zx_status_t Controller::WriteBuffer(uint8_t target, uint16_t lun, uint8_t mod, uint8_t buffer_id,
                                    uint32_t buffer_offset, iovec data) {
  WriteBufferCDB cdb = {};
  cdb.opcode = Opcode::WRITE_BUFFER;
  cdb.mod = mod;
  cdb.buffer_id = buffer_id;
  block::WriteToBigEndian24(buffer_offset, cdb.buffer_offset);
  if (data.iov_len > UINT32_MAX) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  block::WriteToBigEndian24(static_cast<uint32_t>(data.iov_len), cdb.parameter_list_length);

  zx_status_t status = ExecuteCommandSync(target, lun, {.iov_base = &cdb, .iov_len = sizeof(cdb)},
                                          /*is_write=*/true, data);
  if (status != ZX_OK) {
    driver_logger().log(fdf::DEBUG, "WRITE BUFFER failed for target {}, lun {}: {}", target, lun,
                        zx_status_get_string(status));
  }
  return status;
}

}  // namespace scsi

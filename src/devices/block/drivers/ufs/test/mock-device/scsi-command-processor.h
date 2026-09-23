// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_SCSI_COMMAND_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_SCSI_COMMAND_PROCESSOR_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/stdcompat/span.h>

#include <functional>
#include <vector>

#include "handler.h"
#include "src/devices/block/drivers/ufs/ufs.h"
#include "src/devices/block/drivers/ufs/upiu/scsi_commands.h"

namespace ufs {
namespace ufs_mock_device {

class UfsMockDevice;

class ScsiCommandProcessor {
 public:
  using ScsiCommandHandler = std::function<zx::result<std::vector<uint8_t>>(
      UfsMockDevice &, CommandUpiuData &, ResponseUpiuData &,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &)>;

  ScsiCommandProcessor(const ScsiCommandProcessor &) = delete;
  ScsiCommandProcessor &operator=(const ScsiCommandProcessor &) = delete;
  ScsiCommandProcessor(const ScsiCommandProcessor &&) = delete;
  ScsiCommandProcessor &operator=(const ScsiCommandProcessor &&) = delete;
  ~ScsiCommandProcessor() = default;
  explicit ScsiCommandProcessor(UfsMockDevice &mock_device) : mock_device_(mock_device) {}
  zx_status_t HandleScsiCommand(CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
                                cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upiu);

  static zx::result<std::vector<uint8_t>> DefaultRequestSenseHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultRead10Handler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultWrite10Handler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultReadCapacity10Handler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultSynchronizeCache10Handler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultTestUnitReadyHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultInquiryHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultModeSense10Handler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultUnmapHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultReportLunsHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultStartStopUnitHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultReadBufferHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultWriteBufferHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultSecurityProtocolInHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  static zx::result<std::vector<uint8_t>> DefaultSecurityProtocolOutHandler(
      UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
      cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

  DEF_DEFAULT_HANDLER_BEGIN(scsi::Opcode, ScsiCommandHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::REQUEST_SENSE, DefaultRequestSenseHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::READ_10, DefaultRead10Handler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::WRITE_10, DefaultWrite10Handler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::READ_CAPACITY_10, DefaultReadCapacity10Handler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::SYNCHRONIZE_CACHE_10, DefaultSynchronizeCache10Handler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::TEST_UNIT_READY, DefaultTestUnitReadyHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::INQUIRY, DefaultInquiryHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::MODE_SENSE_10, DefaultModeSense10Handler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::UNMAP, DefaultUnmapHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::REPORT_LUNS, DefaultReportLunsHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::START_STOP_UNIT, DefaultStartStopUnitHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::READ_BUFFER, DefaultReadBufferHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::WRITE_BUFFER, DefaultWriteBufferHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::SECURITY_PROTOCOL_IN, DefaultSecurityProtocolInHandler)
  DEF_DEFAULT_HANDLER(scsi::Opcode::SECURITY_PROTOCOL_OUT, DefaultSecurityProtocolOutHandler)
  DEF_DEFAULT_HANDLER_END()

 private:
  void BuildSenseData(ResponseUpiuData &response_upiu, scsi::SenseKey sense_key);
  bool IsProcessableCommand(uint8_t lun, scsi::Opcode opcode) const;

  UfsMockDevice &mock_device_;
};

zx_status_t CopyPhysicalRegionToBuffer(
    UfsMockDevice &mock_device, std::vector<uint8_t> &buffer,
    const cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius);

zx_status_t CopyBufferToPhysicalRegion(UfsMockDevice &mock_device,
                                       cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius,
                                       const std::vector<uint8_t> &buffer);

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_SCSI_COMMAND_PROCESSOR_H_

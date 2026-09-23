// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/scsi/controller.h>

#include <cstdint>
#include <memory>

#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "src/devices/block/drivers/ufs/ufs.h"
#include "src/devices/block/drivers/ufs/upiu/scsi_commands.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"

namespace ufs {

using namespace ufs_mock_device;

class ScsiCommandTest : public UfsTest {
 public:
  void SetUp() override {
    UfsTest::SetUp();
    // Create a mapped and pinned vmo.
    ASSERT_OK(zx::vmo::create(kMockBlockSize, 0, &vmo_));
    zx::unowned_vmo unowned_vmo(vmo_);

    ASSERT_OK(MapVmo(unowned_vmo, mapper_, 0, block_count_ * block_size_));
  }

  void TearDown() override {
    mapper_.Unmap();
    vmo_.reset();
    UfsTest::TearDown();
  }

  void *GetVirtualAddress() const { return mapper_.start(); }
  zx::vmo &GetVmo() { return vmo_; }

  uint16_t GetBlockCount() const { return block_count_; }
  uint32_t GetBlockSize() const { return block_size_; }

 private:
  zx::vmo vmo_;
  fzl::VmoMapper mapper_;

  const uint16_t block_count_ = 1;
  const uint32_t block_size_ = kMockBlockSize;
};

TEST_F(ScsiCommandTest, Read10) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  // Write test data to the mock device
  char buf[kMockBlockSize] = {};
  constexpr char kTestString[] = "test";
  std::strncpy(buf, kTestString, sizeof(buf));
  ASSERT_OK(mock_device_.BufferWrite(kTestLun, buf, GetBlockCount(), block_offset));

  // Make READ 10 CDB
  uint8_t cdb_buffer[10] = {};
  auto cdb = reinterpret_cast<scsi::Read10CDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::READ_10;
  cdb->logical_block_address = htobe32(block_offset);
  cdb->transfer_length = htobe16(GetBlockCount());
  cdb->set_force_unit_access(false);

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost,
                       GetBlockCount() * GetBlockSize());
  ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::READ_10);
  sync_completion_t completion;
  zx_status_t completion_status = ZX_OK;
  zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_OK(slot);
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), GetVmo().borrow(),
                                                    0, GetBlockCount() * GetBlockSize(),
                                                    [&](zx_status_t status) {
                                                      completion_status = status;
                                                      sync_completion_signal(&completion);
                                                    });
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
  ASSERT_OK(completion_status);

  // Check the read data
  GetVmo().op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, 0, kMockBlockSize, nullptr, 0);
  if (memcmp(GetVirtualAddress(), buf, kMockBlockSize) != 0) {
    uint8_t *act = static_cast<uint8_t *>(GetVirtualAddress());
    uint8_t *exp = reinterpret_cast<uint8_t *>(buf);
    fdf::error(
        "Read10 mismatch! Actual: [{:02x} {:02x} {:02x} {:02x}], "
        "Expected: [{:02x} {:02x} {:02x} {:02x}]",
        act[0], act[1], act[2], act[3], exp[0], exp[1], exp[2], exp[3]);
  }
  ASSERT_EQ(memcmp(GetVirtualAddress(), buf, kMockBlockSize), 0);
}

TEST_F(ScsiCommandTest, Read10Exception) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  // Write test data to the mock device
  char buf[kMockBlockSize];
  std::memset(buf, 0xf0, sizeof(buf));

  ASSERT_OK(mock_device_.BufferWrite(kTestLun, buf, GetBlockCount(), block_offset));

  {
    // Make READ 10 CDB
    scsi::Read10CDB cdb = {};
    cdb.opcode = scsi::Opcode::READ_10;
    cdb.logical_block_address = htobe32(block_offset);
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kDeviceToHost, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::READ_10);

    // The command should be failed with not-created vmo.
    zx::vmo not_created_vmo;
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), not_created_vmo.borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_BAD_HANDLE);
  }

  {
    // Make READ 10 CDB
    scsi::Read10CDB cdb = {};
    cdb.opcode = scsi::Opcode::READ_10;
    cdb.logical_block_address = htobe32(block_offset);
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kDeviceToHost, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::READ_10);

    // The command should be failed with not-exist LUN.
    const uint8_t kTestFailureLun = 1;
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestFailureLun, slot.value(), GetVmo().borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_NOT_SUPPORTED);
  }

  {
    // Make READ 10 CDB with address exceeding device size.
    scsi::Read10CDB cdb = {};
    cdb.opcode = scsi::Opcode::READ_10;
    const uint32_t invalid_block_address = htobe32(kMockTotalDeviceCapacity / kMockBlockSize);
    cdb.logical_block_address = invalid_block_address;
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kDeviceToHost, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::READ_10);

    // The command should be failed with address exceeding device size.
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), GetVmo().borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(ScsiCommandTest, Write10) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  constexpr char kTestString[] = "test";
  std::strncpy(static_cast<char *>(GetVirtualAddress()), kTestString, kMockBlockSize);

  // Make WRITE 10 CDB
  uint8_t cdb_buffer[10] = {};
  auto cdb = reinterpret_cast<scsi::Write10CDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::WRITE_10;
  cdb->logical_block_address = htobe32(block_offset);
  cdb->transfer_length = htobe16(GetBlockCount());
  cdb->set_force_unit_access(false);

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kHostToDevice,
                       GetBlockCount() * GetBlockSize());
  ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::WRITE_10);
  sync_completion_t completion;
  zx_status_t completion_status = ZX_OK;
  zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_OK(slot);
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), GetVmo().borrow(),
                                                    0, GetBlockCount() * GetBlockSize(),
                                                    [&](zx_status_t status) {
                                                      completion_status = status;
                                                      sync_completion_signal(&completion);
                                                    });
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
  ASSERT_OK(completion_status);

  // Read test data form the mock device
  char buf[kMockBlockSize];
  ASSERT_OK(mock_device_.BufferRead(kTestLun, buf, GetBlockCount(), block_offset));

  // Check the written data
  if (memcmp(GetVirtualAddress(), buf, kMockBlockSize) != 0) {
    uint8_t *act = static_cast<uint8_t *>(GetVirtualAddress());
    uint8_t *exp = reinterpret_cast<uint8_t *>(buf);
    fdf::error(
        "Write10 mismatch! Actual: [{:02x} {:02x} {:02x} {:02x}], "
        "Expected: [{:02x} {:02x} {:02x} {:02x}]",
        act[0], act[1], act[2], act[3], exp[0], exp[1], exp[2], exp[3]);
  }
  ASSERT_EQ(memcmp(GetVirtualAddress(), buf, kMockBlockSize), 0);
}

TEST_F(ScsiCommandTest, Write10Exception) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  std::memset(static_cast<char *>(GetVirtualAddress()), 0xf0, kMockBlockSize);

  {
    // Make WRITE 10 CDB
    scsi::Write10CDB cdb = {};
    cdb.opcode = scsi::Opcode::WRITE_10;
    cdb.logical_block_address = htobe32(block_offset);
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kHostToDevice, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::WRITE_10);

    // The command should be failed with not-created vmo.
    zx::vmo not_created_vmo;
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), not_created_vmo.borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_BAD_HANDLE);
  }

  {
    // Make WRITE 10 CDB
    scsi::Write10CDB cdb = {};
    cdb.opcode = scsi::Opcode::WRITE_10;
    cdb.logical_block_address = htobe32(block_offset);
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kHostToDevice, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::WRITE_10);

    // The command should be failed with not-exist LUN.
    const uint8_t kTestFailureLun = 1;
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestFailureLun, slot.value(), GetVmo().borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_NOT_SUPPORTED);
  }

  {
    // Make WRITE 10 CDB with address exceeding device size.
    scsi::Write10CDB cdb = {};
    cdb.opcode = scsi::Opcode::WRITE_10;
    const uint32_t invalid_block_address = htobe32(kMockTotalDeviceCapacity / kMockBlockSize);
    cdb.logical_block_address = invalid_block_address;
    cdb.transfer_length = htobe16(GetBlockCount());
    cdb.set_force_unit_access(false);

    // TODO(https://fxbug.dev/42075643): remove "reinterpret_cast" after parameter type of the
    // constructor of |ScsiCommandUpiu| is modified.
    ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                         DataDirection::kHostToDevice, GetBlockCount() * GetBlockSize());
    ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::WRITE_10);

    // The command should be failed with address exceeding device size.
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    sync_completion_t completion;
    zx_status_t op_status = ZX_OK;
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), GetVmo().borrow(), 0, GetBlockCount() * GetBlockSize(),
        [&](zx_status_t status) {
          op_status = status;
          sync_completion_signal(&completion);
        });
    ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
    ASSERT_EQ(op_status, ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(ScsiCommandTest, TestUnitReady) {
  const uint8_t kTestLun = 0;

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::TEST_UNIT_READY);
  auto response = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_OK(response);

  auto *response_sense_data =
      reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response->GetSenseData());
  ASSERT_EQ(response_sense_data->response_code(),
            scsi::SenseDataResponseCodes::kFixedCurrentInformation);
  ASSERT_EQ(response_sense_data->valid(), 0);
  ASSERT_EQ(response_sense_data->sense_key(), scsi::SenseKey::NO_SENSE);

  // The TEST UNIT READY command does not have a data response.
  auto *data_sense_data = reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(GetVirtualAddress());
  scsi::FixedFormatSenseDataHeader empty_sense_data;
  std::memset(&empty_sense_data, 0, sizeof(scsi::FixedFormatSenseDataHeader));
  ASSERT_EQ(
      std::memcmp(data_sense_data, &empty_sense_data, sizeof(scsi::FixedFormatSenseDataHeader)), 0);
}

TEST_F(ScsiCommandTest, ReadCapacity10) {
  const uint8_t kTestLun = 0;

  // Make READ CAPACITY 10 CDB
  uint8_t cdb_buffer[10] = {};
  auto cdb = reinterpret_cast<scsi::ReadCapacity10CDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::READ_CAPACITY_10;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost,
                       sizeof(scsi::ReadCapacity10ParameterData));
  ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::READ_CAPACITY_10);
  ASSERT_OK(dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun,
                                                                 zx::unowned_vmo(GetVmo())));

  auto *read_capacity_data =
      reinterpret_cast<scsi::ReadCapacity10ParameterData *>(GetVirtualAddress());

  // |returned_logical_block_address| is a 0-based value.
  ASSERT_EQ(betoh32(read_capacity_data->returned_logical_block_address),
            (kMockTotalDeviceCapacity / kMockBlockSize) - 1);
  ASSERT_EQ(betoh32(read_capacity_data->block_length_in_bytes), kMockBlockSize);
}

TEST_F(ScsiCommandTest, RequestSense) {
  const uint8_t kTestLun = 0;

  // Make REQUEST SENSE CDB
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::RequestSenseCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::REQUEST_SENSE;
  cdb->allocation_length = static_cast<uint8_t>(sizeof(scsi::FixedFormatSenseDataHeader));

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost,
                       cdb->allocation_length);
  ASSERT_EQ(upiu.GetOpcode(), scsi::Opcode::REQUEST_SENSE);
  ASSERT_OK(dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun,
                                                                 zx::unowned_vmo(GetVmo())));

  auto *sense_data = reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(GetVirtualAddress());
  ASSERT_EQ(sense_data->response_code(), scsi::SenseDataResponseCodes::kFixedCurrentInformation);
  ASSERT_EQ(sense_data->valid(), 0);
  ASSERT_EQ(sense_data->sense_key(), scsi::SenseKey::NO_SENSE);
}

TEST_F(ScsiCommandTest, ShortSenseDataHeader) {
  const uint8_t kTestLun = 0;

  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::TEST_UNIT_READY,
      [](ufs_mock_device::UfsMockDevice &mock_device, CommandUpiuData &command_upiu,
         ResponseUpiuData &response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upiu)
          -> zx::result<std::vector<uint8_t>> {
        response_upiu.header.status = static_cast<uint8_t>(scsi::StatusCode::CHECK_CONDITION);
        response_upiu.header.data_segment_length = htobe16(2);
        response_upiu.sense_data_len = htobe16(2);
        return zx::ok(std::vector<uint8_t>{});
      });

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  auto response = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_EQ(response.status_value(), ZX_ERR_BAD_STATE);
}

TEST_F(ScsiCommandTest, LongSenseDataHeader) {
  const uint8_t kTestLun = 0;

  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::TEST_UNIT_READY,
      [](ufs_mock_device::UfsMockDevice &mock_device, CommandUpiuData &command_upiu,
         ResponseUpiuData &response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upiu)
          -> zx::result<std::vector<uint8_t>> {
        response_upiu.header.status = static_cast<uint8_t>(scsi::StatusCode::CHECK_CONDITION);
        response_upiu.header.data_segment_length = htobe16(24);
        response_upiu.sense_data_len = htobe16(24);
        return zx::ok(std::vector<uint8_t>{});
      });

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  auto response = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_EQ(response.status_value(), ZX_ERR_BAD_STATE);
}

TEST_F(ScsiCommandTest, IoCommandInvalidSenseDataLength) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  // Set mock device to return CHECK_CONDITION with short sense data (< 18 bytes) for READ_10.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::READ_10,
      [](ufs_mock_device::UfsMockDevice &mock_device, CommandUpiuData &command_upiu,
         ResponseUpiuData &response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upiu)
          -> zx::result<std::vector<uint8_t>> {
        response_upiu.header.status = static_cast<uint8_t>(scsi::StatusCode::CHECK_CONDITION);
        response_upiu.header.data_segment_length = htobe16(4);
        response_upiu.sense_data_len = htobe16(4);
        return zx::ok(std::vector<uint8_t>{});
      });

  scsi::Read10CDB cdb = {};
  cdb.opcode = scsi::Opcode::READ_10;
  cdb.logical_block_address = htobe32(block_offset);
  cdb.transfer_length = htobe16(GetBlockCount());

  ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                       DataDirection::kDeviceToHost, GetBlockCount() * GetBlockSize());

  sync_completion_t completion;
  zx_status_t op_status = ZX_OK;
  zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_OK(slot);
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), GetVmo().borrow(),
                                                    0, GetBlockCount() * GetBlockSize(),
                                                    [&](zx_status_t status) {
                                                      op_status = status;
                                                      sync_completion_signal(&completion);
                                                    });
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
  // Because sense_data_len != 18, sense_data is nullopt, causing ScsiComplete to return
  // ZX_ERR_BAD_STATE.
  ASSERT_EQ(op_status, ZX_ERR_BAD_STATE);
}

TEST_F(ScsiCommandTest, IoCommandInvalidAdditionalSenseLength) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  // Set mock device to return CHECK_CONDITION with 18 bytes of sense data, but invalid
  // additional_sense_length (e.g. 0 instead of 10).
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::READ_10,
      [](ufs_mock_device::UfsMockDevice &mock_device, CommandUpiuData &command_upiu,
         ResponseUpiuData &response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upiu)
          -> zx::result<std::vector<uint8_t>> {
        response_upiu.header.status = static_cast<uint8_t>(scsi::StatusCode::CHECK_CONDITION);
        response_upiu.header.data_segment_length =
            htobe16(sizeof(scsi::FixedFormatSenseDataHeader));
        response_upiu.sense_data_len = htobe16(sizeof(scsi::FixedFormatSenseDataHeader));
        auto *sense_data =
            reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response_upiu.sense_data);
        sense_data->set_response_code(scsi::SenseDataResponseCodes::kFixedCurrentInformation);
        sense_data->set_valid(0);
        sense_data->set_sense_key(scsi::SenseKey::ILLEGAL_REQUEST);
        sense_data->additional_sense_length = 0;  // Invalid! Must be 10 (0x0A) per UFS Table 10.19.
        return zx::ok(std::vector<uint8_t>{});
      });

  scsi::Read10CDB cdb = {};
  cdb.opcode = scsi::Opcode::READ_10;
  cdb.logical_block_address = htobe32(block_offset);
  cdb.transfer_length = htobe16(GetBlockCount());

  ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t *>(&cdb), sizeof(cdb),
                       DataDirection::kDeviceToHost, GetBlockCount() * GetBlockSize());

  sync_completion_t completion;
  zx_status_t op_status = ZX_OK;
  zx::result<uint8_t> slot2 = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_OK(slot2);
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(
      upiu, kTestLun, slot2.value(), GetVmo().borrow(), 0, GetBlockCount() * GetBlockSize(),
      [&](zx_status_t status) {
        op_status = status;
        sync_completion_signal(&completion);
      });
  ASSERT_OK(sync_completion_wait(&completion, ZX_TIME_INFINITE));
  // Because additional_sense_length != 10, sense_data is rejected, causing ScsiComplete to return
  // ZX_ERR_BAD_STATE.
  ASSERT_EQ(op_status, ZX_ERR_BAD_STATE);
}

TEST_F(ScsiCommandTest, SynchronizeCache10) {
  const uint8_t kTestLun = 0;
  uint32_t block_offset = 0;

  // Make SYNCHRONIZE CACHE 10 CDB
  uint8_t cdb_buffer[10] = {};
  auto cdb = reinterpret_cast<scsi::SynchronizeCache10CDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::SYNCHRONIZE_CACHE_10;
  cdb->logical_block_address = htobe32(block_offset);
  cdb->number_of_logical_blocks = htobe16(GetBlockCount());

  ScsiCommandUpiu cache_upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  ASSERT_EQ(cache_upiu.GetOpcode(), scsi::Opcode::SYNCHRONIZE_CACHE_10);
  ASSERT_OK(dut_->GetTransferRequestProcessor().SendAdminScsiCmd(cache_upiu, kTestLun));
}

TEST_F(ScsiCommandTest, WellKnownLuns) {
  EXPECT_TRUE(dut_->HasWellKnownLun(WellKnownLuns::kReportLuns));
  EXPECT_TRUE(dut_->HasWellKnownLun(WellKnownLuns::kUfsDevice));
  EXPECT_TRUE(dut_->HasWellKnownLun(WellKnownLuns::kBoot));
  EXPECT_TRUE(dut_->HasWellKnownLun(WellKnownLuns::kRpmb));

  // Make TEST UNIT READY CDB
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  // Check well known logical units.
  std::array<WellKnownLuns, static_cast<uint8_t>(WellKnownLuns::kCount)> well_known_luns = {
      WellKnownLuns::kReportLuns, WellKnownLuns::kUfsDevice, WellKnownLuns::kBoot,
      WellKnownLuns::kRpmb};

  for (auto lun : well_known_luns) {
    ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
    EXPECT_EQ(upiu.GetOpcode(), scsi::Opcode::TEST_UNIT_READY);
    EXPECT_OK(
        dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, static_cast<uint8_t>(lun)));
  }
}

}  // namespace ufs

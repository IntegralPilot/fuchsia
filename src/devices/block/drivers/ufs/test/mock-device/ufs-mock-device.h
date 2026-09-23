// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UFS_MOCK_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UFS_MOCK_DEVICE_H_

#include <endian.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <bitset>

#include <gtest/gtest.h>

#include "fake-dma-handler.h"
#include "query-request-processor.h"
#include "register-mmio-processor.h"
#include "scsi-command-processor.h"
#include "src/devices/block/drivers/ufs/ufs.h"
#include "task-management-request-processor.h"
#include "transfer-request-processor.h"
#include "uiccmd-processor.h"

namespace ufs {
namespace ufs_mock_device {

constexpr uint64_t kMockBlockSizeShift = 12;
constexpr uint64_t kMockBlockSize = (1 << kMockBlockSizeShift);
constexpr uint64_t kMockTotalDeviceCapacity = (1 << 24);  // 16MB

constexpr uint8_t kMockRpmbLogicalBlockSizeExponent = 9;  // 512 bytes
constexpr uint64_t kMockRpmbLogicalBlockCount = 1024;     // 512KB

constexpr uint32_t kMajorVersion = 3;
constexpr uint32_t kMinorVersion = 1;
constexpr uint32_t kVersionSuffix = 2;

constexpr uint32_t kMaxGear = 4;
constexpr uint32_t kConnectedDataLanes = 2;
constexpr uint32_t kGear = 4;
constexpr uint32_t kTermination = 1;
constexpr uint32_t kHSSeries = 2;
constexpr uint32_t kPWRModeUserData = 0xffff;
constexpr uint32_t kTxHsAdaptType = 3;
constexpr uint32_t kPWRMode = 0x11;

constexpr uint32_t kUniproVersion = 5;
constexpr uint32_t kTActivate = 2;
constexpr uint32_t kGranularity = 6;
constexpr uint32_t kMaxDeviceBufferSize = 64 * 1024;

class FakeRegisters final {
 public:
  FakeRegisters() {
    ZX_ASSERT(zx::vmo::create(RegisterMap::kRegisterSize, /*options=*/0, &registers_vmo_) == ZX_OK);
    ZX_ASSERT(registers_vmo_.set_cache_policy(ZX_CACHE_POLICY_UNCACHED) == ZX_OK);
    ZX_ASSERT(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                         /*vmar_offset=*/0, registers_vmo_,
                                         /*vmo_offset=*/0, /*len=*/RegisterMap::kRegisterSize,
                                         reinterpret_cast<zx_vaddr_t *>(&registers_)) == ZX_OK);
  }

  template <typename T>
  T Read(zx_off_t offs) const {
    ZX_ASSERT(offs + sizeof(T) <= RegisterMap::kRegisterSize);
    ZX_ASSERT(registers_ != nullptr);
    return *reinterpret_cast<const T *>(registers_ + offs);
  }

  template <typename T>
  void Write(T val, zx_off_t offs) {
    ZX_ASSERT(offs + sizeof(T) <= RegisterMap::kRegisterSize);
    ZX_ASSERT(registers_ != nullptr);
    *reinterpret_cast<T *>(registers_ + offs) = val;
  }

  zx::vmo GetRegistersVmo() {
    zx::vmo vmo;
    zx_status_t status = registers_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
    ZX_ASSERT(status == ZX_OK);
    return vmo;
  }

  void Reset() { CustomMemSet(registers_, 0, RegisterMap::kRegisterSize); }

 private:
  zx::vmo registers_vmo_;
  uint8_t *registers_ = nullptr;
};

class UfsDeviceBuffer {
 public:
  zx_status_t Write(const void *buf, uint32_t offset, uint32_t length);
  zx_status_t Read(void *buf, uint32_t offset, uint32_t length);
  void SetBufferSize(const size_t size);

 private:
  std::vector<uint8_t> data_buffer_;
};

// Simulates a logical unit and its contents.
class UfsLogicalUnit {
 public:
  UfsLogicalUnit();

  zx_status_t Enable(uint8_t lun, uint64_t block_count);

  zx_status_t BufferWrite(const void *buf, size_t block_count, off_t block_offset);
  zx_status_t BufferRead(void *buf, size_t block_count, off_t block_offset);
  UnitDescriptor &GetUnitDesc() { return unit_desc_; }
  bool IsEnable() const { return unit_desc_.bLUEnable != 0; }

  zx_status_t WriteToDeviceBuffer(const void *buf, uint32_t offset, uint32_t length);
  zx_status_t ReadFromDeviceBuffer(void *buf, uint32_t offset, uint32_t length);

 private:
  uint64_t block_count_;
  std::vector<uint8_t> buffer_;
  UnitDescriptor unit_desc_;
  UfsDeviceBuffer device_buffer_;
};

class UfsMockDevice {
 public:
  static constexpr uint32_t kNutrs = 32;
  static constexpr uint32_t kNutmrs = 8;

  UfsMockDevice()
      : register_mmio_processor_(*this),
        uiccmd_processor_(*this),
        transfer_request_processor_(*this),
        task_management_request_processor_(*this),
        query_request_processor_(*this),
        scsi_command_processor_(*this) {}
  UfsMockDevice(const UfsMockDevice &) = delete;
  UfsMockDevice &operator=(const UfsMockDevice &) = delete;
  UfsMockDevice(const UfsMockDevice &&) = delete;
  UfsMockDevice &operator=(const UfsMockDevice &&) = delete;
  ~UfsMockDevice() = default;

  void Init(std::unique_ptr<zx::interrupt> irq);

  fdf::MmioBuffer GetMmioBuffer(zx::vmo vmo) {
    return fdf_testing::CreateMmioBuffer(std::move(vmo), ZX_CACHE_POLICY_UNCACHED,
                                         &RegisterMmioProcessor::GetMmioOps(), this);
  }

  zx::vmo GetVmo() { return registers_.GetRegistersVmo(); }
  zx::interrupt GetIrq();
  zx::bti GetFakeBti() { return dma_handler_.DuplicateFakeBti(); }
  zx::result<zx_vaddr_t> MapDmaPaddr(zx_paddr_t paddr) { return dma_handler_.PhysToVirt(paddr); }

  zx_status_t AddLun(uint8_t lun);

  void TriggerInterrupt() { irq_->trigger(0, zx::clock::get_boot()); }

  zx_status_t BufferWrite(uint8_t lun, const void *buf, size_t block_count, off_t block_offset);
  zx_status_t BufferRead(uint8_t lun, void *buf, size_t block_count, off_t block_offset);
  zx_status_t WriteToDeviceBuffer(uint8_t lun, const void *buf, uint32_t offset, uint32_t length);
  zx_status_t ReadFromDeviceBuffer(uint8_t lun, void *buf, uint32_t offset, uint32_t length);

  FakeRegisters *GetRegisters() { return &registers_; }
  DeviceDescriptor &GetDeviceDesc() { return device_desc_; }
  GeometryDescriptor &GetGeometryDesc() { return geometry_desc_; }
  PowerParametersDescriptor &GetPowerDesc() { return power_desc_; }
  void SetAttribute(Attributes idn, uint32_t value) {
    attributes_[static_cast<size_t>(idn)] = value;
  }
  uint32_t GetAttribute(Attributes idn) const { return attributes_[static_cast<size_t>(idn)]; }
  void SetFlag(Flags idn, bool value) { flags_[static_cast<size_t>(idn)] = value; }
  bool GetFlag(Flags idn) const { return flags_[static_cast<size_t>(idn)]; }

  void SetUnitAttention(bool value) { unit_attention_ = value; }
  bool GetUnitAttention() const { return unit_attention_; }
  void SetExceptionEventAlert(bool value) { exception_event_alert_ = value; }
  bool GetExceptionEventAlert() const { return exception_event_alert_; }

  UfsLogicalUnit &GetLogicalUnit(uint8_t lun) { return logical_units_[lun]; }
  RegisterMmioProcessor &GetRegisterMmioProcessor() { return register_mmio_processor_; }
  UicCmdProcessor &GetUicCmdProcessor() { return uiccmd_processor_; }
  TransferRequestProcessor &GetTransferRequestProcessor() { return transfer_request_processor_; }
  TaskManagementRequestProcessor &GetTaskManagementRequestProcessor() {
    return task_management_request_processor_;
  }
  QueryRequestProcessor &GetQueryRequestProcessor() { return query_request_processor_; }
  ScsiCommandProcessor &GetScsiCommandProcessor() { return scsi_command_processor_; }
  FakeDmaHandler &GetDmaHandler() { return dma_handler_; }

 private:
  std::array<UfsLogicalUnit, kMaxLunCount> logical_units_;
  DeviceDescriptor device_desc_;
  GeometryDescriptor geometry_desc_;
  PowerParametersDescriptor power_desc_;
  std::array<uint32_t, static_cast<size_t>(Attributes::kAttributeCount)> attributes_;
  std::array<bool, static_cast<size_t>(Flags::kFlagCount)> flags_;

  std::unique_ptr<zx::interrupt> irq_;
  FakeDmaHandler dma_handler_;
  FakeRegisters registers_;
  RegisterMmioProcessor register_mmio_processor_;
  UicCmdProcessor uiccmd_processor_;
  TransferRequestProcessor transfer_request_processor_;
  TaskManagementRequestProcessor task_management_request_processor_;
  QueryRequestProcessor query_request_processor_;
  ScsiCommandProcessor scsi_command_processor_;

  bool unit_attention_ = false;
  bool exception_event_alert_ = false;
};

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UFS_MOCK_DEVICE_H_

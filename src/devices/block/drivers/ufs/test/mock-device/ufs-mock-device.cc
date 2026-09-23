// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ufs-mock-device.h"

#include <lib/stdcompat/span.h>

#include "src/devices/block/drivers/ufs/upiu/attributes.h"

namespace ufs {
namespace ufs_mock_device {
zx_status_t UfsDeviceBuffer::Write(const void *buf, uint32_t offset, uint32_t length) {
  if (offset >= data_buffer_.size() || length > data_buffer_.size() - offset) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::memcpy(data_buffer_.data() + offset, buf, length);
  return ZX_OK;
}

zx_status_t UfsDeviceBuffer::Read(void *buf, uint32_t offset, uint32_t length) {
  if (offset >= data_buffer_.size() || length > data_buffer_.size() - offset) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::memcpy(buf, data_buffer_.data() + offset, length);
  return ZX_OK;
}

void UfsDeviceBuffer::SetBufferSize(const size_t size) {
  ZX_DEBUG_ASSERT(size <= kMaxDeviceBufferSize);
  data_buffer_.resize(size);
}

UfsLogicalUnit::UfsLogicalUnit() {
  unit_desc_ = UnitDescriptor{
      .bLength = sizeof(UnitDescriptor),
      .bDescriptorIDN = static_cast<size_t>(DescriptorType::kUnit),
      .bLUEnable = 0x00,
      .bLogicalBlockSize = kMockBlockSizeShift,
      .dLUNumWriteBoosterBufferAllocUnits = 0,
  };
}

zx_status_t UfsLogicalUnit::Enable(uint8_t lun, uint64_t block_count) {
  if (unit_desc_.bLUEnable != 0) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  block_count_ = block_count;
  unit_desc_.bUnitIndex = lun;
  unit_desc_.qLogicalBlockCount = htobe64(block_count);
  unit_desc_.bLUEnable = 0x01;
  buffer_.resize(kMockBlockSize * block_count);
  device_buffer_.SetBufferSize(kMaxDeviceBufferSize);
  return ZX_OK;
}

zx_status_t UfsLogicalUnit::BufferWrite(const void *buf, size_t block_count, off_t block_offset) {
  if (unit_desc_.bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }

  uint64_t offset = block_offset * kMockBlockSize;
  uint64_t count = block_count * kMockBlockSize;
  if (offset + count > buffer_.size() || offset >= buffer_.size()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::memcpy(buffer_.data() + offset, buf, count);
  return ZX_OK;
}

zx_status_t UfsLogicalUnit::BufferRead(void *buf, size_t block_count, off_t block_offset) {
  if (unit_desc_.bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }

  uint64_t offset = block_offset * kMockBlockSize;
  uint64_t count = block_count * kMockBlockSize;
  if (offset + count > buffer_.size() || offset >= buffer_.size()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::memcpy(buf, buffer_.data() + offset, count);
  return ZX_OK;
}

zx_status_t UfsLogicalUnit::WriteToDeviceBuffer(const void *buf, uint32_t offset, uint32_t length) {
  if (unit_desc_.bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }
  return device_buffer_.Write(buf, offset, length);
}

zx_status_t UfsLogicalUnit::ReadFromDeviceBuffer(void *buf, uint32_t offset, uint32_t length) {
  if (unit_desc_.bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }
  return device_buffer_.Read(buf, offset, length);
}

void UfsMockDevice::Init(std::unique_ptr<zx::interrupt> irq) {
  irq_ = std::move(irq);
  registers_.Reset();
  for (auto &lun : logical_units_) {
    lun = UfsLogicalUnit();
  }

  register_mmio_processor_.Reset();
  uiccmd_processor_.Reset();
  transfer_request_processor_.Reset();
  task_management_request_processor_.Reset();
  query_request_processor_.Reset();
  scsi_command_processor_.Reset();
  dma_handler_.Reset();

  VersionReg::Get()
      .ReadFrom(&registers_)
      .set_major_version_number(kMajorVersion)
      .set_minor_version_number(kMinorVersion)
      .set_version_suffix(kVersionSuffix)
      .WriteTo(&registers_);
  CapabilityReg::Get()
      .ReadFrom(&registers_)
      .set__64_bit_addressing_supported(true)
      .set_number_of_utp_task_management_request_slots(kNutmrs - 1)
      .set_number_of_utp_transfer_request_slots(kNutrs - 1)
      .WriteTo(&registers_);
  HostControllerStatusReg::Get()
      .ReadFrom(&registers_)
      .set_utp_task_management_request_list_ready(true)
      .set_utp_transfer_request_list_ready(true)
      .set_device_present(true)
      .WriteTo(&registers_);
  HostControllerEnableReg::Get()
      .ReadFrom(&registers_)
      .set_host_controller_enable(true)
      .WriteTo(&registers_);

  std::memset(&device_desc_, 0, sizeof(device_desc_));
  device_desc_.bLength = sizeof(DeviceDescriptor);
  device_desc_.bDescriptorIDN = static_cast<size_t>(DescriptorType::kDevice);
  device_desc_.bDeviceSubClass = 0x01;
  device_desc_.bNumberWLU = 0x04;
  device_desc_.bInitPowerMode = 0x01;
  device_desc_.bHighPriorityLUN = 0x7F;
  device_desc_.wSpecVersion = htobe16(0x0310);
  device_desc_.bUD0BaseOffset = 0x16;
  device_desc_.bUDConfigPLength = 0x1A;

  ExtendedUfsFeaturesSupport extended_ufs_feature_support;
  extended_ufs_feature_support.set_writebooster_support(true);
  device_desc_.dExtendedUfsFeaturesSupport = htobe32(extended_ufs_feature_support.value);

  std::memset(&geometry_desc_, 0, sizeof(geometry_desc_));
  geometry_desc_.bLength = sizeof(GeometryDescriptor);
  geometry_desc_.bDescriptorIDN = static_cast<size_t>(DescriptorType::kGeometry);
  geometry_desc_.qTotalRawDeviceCapacity = htobe64(kMockTotalDeviceCapacity >> 9);  // 16MB
  geometry_desc_.bMinAddrBlockSize = 0x08;
  geometry_desc_.bMaxInBufferSize = 0x08;
  geometry_desc_.bMaxOutBufferSize = 0x08;
  static_assert(kMaxLunCount == 8 || kMaxLunCount == 32,
                "Max Number of Logical Unit should be 8 or 32.");
  if constexpr (kMaxLunCount == 8) {
    geometry_desc_.bMaxNumberLU = 0x00;
  } else if (kMaxLunCount == 32) {
    geometry_desc_.bMaxNumberLU = 0x01;
  }
  geometry_desc_.bAllocationUnitSize = 0x01;
  geometry_desc_.dSegmentSize = htobe32(0x2000);  // 4MiB segment size

  std::memset(&power_desc_, 0, sizeof(power_desc_));
  power_desc_.bLength = sizeof(PowerParametersDescriptor);
  power_desc_.bDescriptorIDN = static_cast<size_t>(DescriptorType::kPower);

  UnitDescriptor default_unit_descriptor = UnitDescriptor{
      .bLength = sizeof(UnitDescriptor),
      .bDescriptorIDN = static_cast<size_t>(DescriptorType::kUnit),
      .bLUEnable = 0x00,
      .bLogicalBlockSize = kMockBlockSizeShift,
      .dLUNumWriteBoosterBufferAllocUnits = 0,
  };
  for (auto &lu : logical_units_) {
    std::memcpy(&lu.GetUnitDesc(), &default_unit_descriptor, sizeof(default_unit_descriptor));
  }

  std::memset(&attributes_, 0, sizeof(attributes_));
  SetAttribute(Attributes::bBootLunEn, true);
  SetAttribute(Attributes::bCurrentPowerMode, static_cast<uint32_t>(UfsPowerMode::kActive));

  SetAttribute(Attributes::bActiveIccLevel, kHighestActiveIcclevel);

  // Enable WriteBooster
  device_desc_.bWriteBoosterBufferType =
      static_cast<uint8_t>(WriteBoosterBufferType::kSharedBuffer);
  device_desc_.dNumSharedWriteBoosterBufferAllocUnits = betoh32(1);
  device_desc_.bWriteBoosterBufferPreserveUserSpaceEn =
      static_cast<uint8_t>(UserSpaceConfigurationOption::kPreserveUserSpace);
  SetAttribute(Attributes::bWBBufferLifeTimeEst,
               0x01);  // 0% - 10% WriteBooster Buffer life time used
  SetAttribute(Attributes::bAvailableWBBufferSize, 10);  // 100%
  SetAttribute(Attributes::dCurrentWBBufferSize, 0x01);
  SetAttribute(Attributes::wExceptionEventStatus, 0);
  SetAttribute(Attributes::bBackgroundOpStatus, 0);

  std::memset(&flags_, 0, sizeof(flags_));
}

zx::interrupt UfsMockDevice::GetIrq() {
  zx::interrupt irq_duplicated;
  zx_status_t status = irq_->duplicate(ZX_RIGHT_SAME_RIGHTS, &irq_duplicated);
  ZX_ASSERT(status == ZX_OK);
  return irq_duplicated;
}

zx_status_t UfsMockDevice::AddLun(uint8_t lun) {
  if (lun >= logical_units_.size()) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (auto err = logical_units_[lun].Enable(lun, kMockTotalDeviceCapacity / kMockBlockSize);
      err != ZX_OK) {
    return err;
  }
  ++device_desc_.bNumberLU;
  return ZX_OK;
}

zx_status_t UfsMockDevice::BufferWrite(uint8_t lun, const void *buf, size_t block_count,
                                       off_t block_offset) {
  if (lun >= logical_units_.size()) {
    return ZX_ERR_INVALID_ARGS;
  }

  return logical_units_[lun].BufferWrite(buf, block_count, block_offset);
}

zx_status_t UfsMockDevice::BufferRead(uint8_t lun, void *buf, size_t block_count,
                                      off_t block_offset) {
  if (lun >= logical_units_.size()) {
    return ZX_ERR_INVALID_ARGS;
  }

  return logical_units_[lun].BufferRead(buf, block_count, block_offset);
}

zx_status_t UfsMockDevice::WriteToDeviceBuffer(uint8_t lun, const void *buf, uint32_t offset,
                                               uint32_t length) {
  if (lun >= logical_units_.size() || !logical_units_[lun].IsEnable()) {
    return ZX_ERR_INVALID_ARGS;
  }
  return logical_units_[lun].WriteToDeviceBuffer(buf, offset, length);
}

zx_status_t UfsMockDevice::ReadFromDeviceBuffer(uint8_t lun, void *buf, uint32_t offset,
                                                uint32_t length) {
  if (lun >= logical_units_.size() || !logical_units_[lun].IsEnable()) {
    return ZX_ERR_INVALID_ARGS;
  }
  return logical_units_[lun].ReadFromDeviceBuffer(buf, offset, length);
}

}  // namespace ufs_mock_device
}  // namespace ufs

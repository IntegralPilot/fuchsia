// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/driver/logging/cpp/logger.h>

#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {

zx_status_t QueryRequestProcessor::HandleQueryRequest(QueryRequestUpiuData &req_upiu,
                                                      QueryResponseUpiuData &rsp_upiu) {
  zx_status_t status;
  if (auto it = handlers_.find(static_cast<QueryOpcode>(req_upiu.opcode)); it != handlers_.end()) {
    status = (it->second)(mock_device_, req_upiu, rsp_upiu);
  } else {
    status = ZX_ERR_NOT_SUPPORTED;
    fdf::error("UFS MOCK: query request opcode: 0x{:x} is not supported",
               static_cast<uint32_t>(req_upiu.opcode));
  }
  return status;
}

zx_status_t QueryRequestProcessor::DefaultReadDescriptorHandler(UfsMockDevice &mock_device,
                                                                QueryRequestUpiuData &req_upiu,
                                                                QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardReadRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (req_upiu.idn == static_cast<uint8_t>(DescriptorType::kDevice)) {
    CustomMemCpy(rsp_upiu.command_data.data(), &mock_device.GetDeviceDesc(),
                 sizeof(DeviceDescriptor));
  } else if (req_upiu.idn == static_cast<uint8_t>(DescriptorType::kGeometry)) {
    CustomMemCpy(rsp_upiu.command_data.data(), &mock_device.GetGeometryDesc(),
                 sizeof(GeometryDescriptor));
  } else if (req_upiu.idn == static_cast<uint8_t>(DescriptorType::kUnit)) {
    uint8_t lun = req_upiu.index;
    if (lun == static_cast<uint8_t>(WellKnownLuns::kRpmb)) {
      RpmbUnitDescriptor rpmb_desc = {
          .bLength = sizeof(RpmbUnitDescriptor),
          .bDescriptorIDN = static_cast<uint8_t>(DescriptorType::kUnit),
          .bUnitIndex = lun,
          .bLUEnable = 1,
          .bLogicalBlockSize = kMockRpmbLogicalBlockSizeExponent,
          .qLogicalBlockCount = htobe64(kMockRpmbLogicalBlockCount),
      };
      CustomMemCpy(rsp_upiu.command_data.data(), &rpmb_desc, sizeof(RpmbUnitDescriptor));
    } else if (lun >= kMaxLunCount) {
      fdf::error("UFS MOCK: read descriptor index (LUN) 0x{:x} is out of bounds", lun);
      return ZX_ERR_OUT_OF_RANGE;
    } else {
      CustomMemCpy(rsp_upiu.command_data.data(), &mock_device.GetLogicalUnit(lun).GetUnitDesc(),
                   sizeof(UnitDescriptor));
    }
  } else if (req_upiu.idn == static_cast<uint8_t>(DescriptorType::kPower)) {
    CustomMemCpy(rsp_upiu.command_data.data(), &mock_device.GetPowerDesc(),
                 sizeof(PowerParametersDescriptor));
  } else {
    fdf::error("UFS MOCK: read descriptor idn: 0x{:x} is not supported", req_upiu.idn);
    return ZX_ERR_NOT_SUPPORTED;
  }
  rsp_upiu.length = htobe16(rsp_upiu.command_data[0]);

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultWriteDescriptorHandler(UfsMockDevice &mock_device,
                                                                 QueryRequestUpiuData &req_upiu,
                                                                 QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardWriteRequest) ||
      req_upiu.idn != static_cast<uint8_t>(DescriptorType::kConfiguration)) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto *config_desc = reinterpret_cast<ConfigurationDescriptor *>(req_upiu.command_data.data());
  // 0xE6 is a legacy descriptor length value from earlier UFS spec revisions.
  if ((config_desc->bLength != 0xE6 && config_desc->bLength != sizeof(ConfigurationDescriptor)) ||
      config_desc->bDescriptorIDN != static_cast<uint8_t>(DescriptorType::kConfiguration)) {
    return ZX_ERR_INVALID_ARGS;
  }

  mock_device.GetDeviceDesc().bHighPriorityLUN = config_desc->bHighPriorityLUN;

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultReadAttributeHandler(UfsMockDevice &mock_device,
                                                               QueryRequestUpiuData &req_upiu,
                                                               QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardReadRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  rsp_upiu.value = htobe32(mock_device.GetAttribute(static_cast<Attributes>(req_upiu.idn)));

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultWriteAttributeHandler(UfsMockDevice &mock_device,
                                                                QueryRequestUpiuData &req_upiu,
                                                                QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardWriteRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  rsp_upiu.value = req_upiu.value;
  mock_device.SetAttribute(static_cast<Attributes>(req_upiu.idn), betoh32(req_upiu.value));

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultReadFlagHandler(UfsMockDevice &mock_device,
                                                          QueryRequestUpiuData &req_upiu,
                                                          QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardReadRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  rsp_upiu.flag_value = mock_device.GetFlag(static_cast<Flags>(req_upiu.idn));

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultSetFlagHandler(UfsMockDevice &mock_device,
                                                         QueryRequestUpiuData &req_upiu,
                                                         QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardWriteRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  mock_device.SetFlag(static_cast<Flags>(req_upiu.idn), true);
  if (req_upiu.idn == static_cast<uint8_t>(Flags::fDeviceInit)) {
    mock_device.SetFlag(Flags::fDeviceInit, false);
    rsp_upiu.flag_value = 0;
  } else {
    rsp_upiu.flag_value = 1;
  }

  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultToggleFlagHandler(UfsMockDevice &mock_device,
                                                            QueryRequestUpiuData &req_upiu,
                                                            QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardWriteRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  mock_device.SetFlag(static_cast<Flags>(req_upiu.idn),
                      !mock_device.GetFlag(static_cast<Flags>(req_upiu.idn)));
  return ZX_OK;
}

zx_status_t QueryRequestProcessor::DefaultClearFlagHandler(UfsMockDevice &mock_device,
                                                           QueryRequestUpiuData &req_upiu,
                                                           QueryResponseUpiuData &rsp_upiu) {
  if (req_upiu.header.function != static_cast<uint8_t>(QueryFunction::kStandardWriteRequest)) {
    return ZX_ERR_INVALID_ARGS;
  }

  mock_device.SetFlag(static_cast<Flags>(req_upiu.idn), false);
  return ZX_OK;
}

}  // namespace ufs_mock_device
}  // namespace ufs

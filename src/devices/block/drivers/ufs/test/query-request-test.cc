// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>
#include <memory>

#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "src/devices/block/drivers/ufs/upiu/attributes.h"
#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"

namespace ufs {
using namespace ufs_mock_device;

using QueryRequestTest = UfsTest;

TEST_F(QueryRequestTest, DeviceDescriptor) {
  ReadDescriptorUpiu device_desc_upiu(DescriptorType::kDevice);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(device_desc_upiu);
  ASSERT_OK(response);
  DeviceDescriptor device_descriptor;
  CustomMemCpy(&device_descriptor,
               &response->GetResponse<DescriptorResponseUpiu>().GetDescriptor<DeviceDescriptor>(),
               sizeof(device_descriptor));

  ASSERT_EQ(device_descriptor.bLength, mock_device_.GetDeviceDesc().bLength);
  ASSERT_EQ(device_descriptor.bDescriptorIDN, mock_device_.GetDeviceDesc().bDescriptorIDN);
  ASSERT_EQ(device_descriptor.bDeviceSubClass, mock_device_.GetDeviceDesc().bDeviceSubClass);
  ASSERT_EQ(device_descriptor.bNumberWLU, mock_device_.GetDeviceDesc().bNumberWLU);
  ASSERT_EQ(device_descriptor.bInitPowerMode, mock_device_.GetDeviceDesc().bInitPowerMode);
  ASSERT_EQ(device_descriptor.bHighPriorityLUN, mock_device_.GetDeviceDesc().bHighPriorityLUN);
  ASSERT_EQ(device_descriptor.wSpecVersion, mock_device_.GetDeviceDesc().wSpecVersion);
  ASSERT_EQ(device_descriptor.bUD0BaseOffset, mock_device_.GetDeviceDesc().bUD0BaseOffset);
  ASSERT_EQ(device_descriptor.bUDConfigPLength, mock_device_.GetDeviceDesc().bUDConfigPLength);
}

TEST_F(QueryRequestTest, GeometryDescriptor) {
  ReadDescriptorUpiu geometry_desc_upiu(DescriptorType::kGeometry);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(geometry_desc_upiu);
  ASSERT_OK(response);
  GeometryDescriptor geometry_desc;
  CustomMemCpy(&geometry_desc,
               &response->GetResponse<DescriptorResponseUpiu>().GetDescriptor<GeometryDescriptor>(),
               sizeof(geometry_desc));

  ASSERT_EQ(geometry_desc.bLength, mock_device_.GetGeometryDesc().bLength);
  ASSERT_EQ(geometry_desc.bDescriptorIDN, mock_device_.GetGeometryDesc().bDescriptorIDN);
  ASSERT_EQ(geometry_desc.bMaxNumberLU, mock_device_.GetGeometryDesc().bMaxNumberLU);
}

TEST_F(QueryRequestTest, UnitDescriptor) {
  uint8_t lun = 0;
  ReadDescriptorUpiu unit_desc_upiu(DescriptorType::kUnit, lun);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(unit_desc_upiu);
  ASSERT_OK(response);
  UnitDescriptor unit_desc;
  CustomMemCpy(&unit_desc,
               &response->GetResponse<DescriptorResponseUpiu>().GetDescriptor<UnitDescriptor>(),
               sizeof(unit_desc));

  const auto& mock_desc = mock_device_.GetLogicalUnit(lun).GetUnitDesc();
  ASSERT_EQ(unit_desc.bLength, mock_desc.bLength);
  ASSERT_EQ(unit_desc.bDescriptorIDN, mock_desc.bDescriptorIDN);
  ASSERT_EQ(unit_desc.bLUEnable, mock_desc.bLUEnable);
  ASSERT_EQ(unit_desc.bLogicalBlockSize, mock_desc.bLogicalBlockSize);
}

TEST_F(QueryRequestTest, WriteDescriptor) {
  // 0x7F: Same priority for all LUNs
  // If all LUNs already have the same priority, change the priority of LUN0 higher
  // If specific LUN has higher priority, change to the same priority for all LUNs
  uint8_t high_priority_lun = mock_device_.GetDeviceDesc().bHighPriorityLUN == 0x7F ? 0 : 0x7F;

  ConfigurationDescriptor descriptor;
  std::memset(&descriptor, 0, sizeof(ConfigurationDescriptor));
  descriptor.bLength = 0xE6;
  descriptor.bDescriptorIDN = 0x01;
  descriptor.bConfDescContinue = 0x00;
  descriptor.bHighPriorityLUN = high_priority_lun;

  WriteDescriptorUpiu config_desc_upiu(DescriptorType::kConfiguration, &descriptor);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(config_desc_upiu);
  ASSERT_OK(response);

  ASSERT_EQ(high_priority_lun, mock_device_.GetDeviceDesc().bHighPriorityLUN);
}

TEST_F(QueryRequestTest, WriteAttribute) {
  mock_device_.SetAttribute(Attributes::bCurrentPowerMode, 0);

  uint8_t power_mode = static_cast<uint8_t>(UfsPowerMode::kActive);
  auto response = WriteAttribute(Attributes::bCurrentPowerMode, power_mode);
  ASSERT_OK(response);
  ASSERT_EQ(power_mode, mock_device_.GetAttribute(Attributes::bCurrentPowerMode));
}

TEST_F(QueryRequestTest, ReadAttribute) {
  uint8_t power_mode = static_cast<uint8_t>(UfsPowerMode::kActive);
  mock_device_.SetAttribute(Attributes::bCurrentPowerMode, power_mode);

  auto attribute = ReadAttribute(Attributes::bCurrentPowerMode);
  ASSERT_OK(attribute);
  ASSERT_EQ(attribute, power_mode);
}

TEST_F(QueryRequestTest, ReadAttributeExcpetion) {
  mock_device_.GetQueryRequestProcessor().SetHook(
      QueryOpcode::kReadAttribute,
      [](ufs_mock_device::UfsMockDevice& mock_device, QueryRequestUpiuData& req_upiu,
         QueryResponseUpiuData& rsp_upiu) {
        rsp_upiu.header.response = UpiuHeaderResponseCode::kTargetFailure;  // Error injection
        return ZX_OK;
      });
  auto attribute = ReadAttribute(Attributes::bCurrentPowerMode);
  ASSERT_EQ(attribute.status_value(), ZX_ERR_BAD_STATE);
  mock_device_.GetQueryRequestProcessor().Reset();
}

TEST_F(QueryRequestTest, ReadFlag) {
  bool device_init = false;
  mock_device_.SetFlag(Flags::fDeviceInit, device_init);

  ReadFlagUpiu read_flag_upiu(Flags::fDeviceInit);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(read_flag_upiu);
  ASSERT_OK(response);
  auto flag = response->GetResponse<FlagResponseUpiu>().GetFlag();

  ASSERT_EQ(flag, device_init);
}

TEST_F(QueryRequestTest, SetFlag) {
  mock_device_.SetFlag(Flags::fPermanentWPEn, false);

  SetFlagUpiu set_flag_upiu(Flags::fPermanentWPEn);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(set_flag_upiu);
  ASSERT_OK(response);

  ASSERT_EQ(true, mock_device_.GetFlag(Flags::fPermanentWPEn));
}

TEST_F(QueryRequestTest, ToggleFlag) {
  bool device_init = false;
  mock_device_.SetFlag(Flags::fDeviceInit, device_init);

  ToggleFlagUpiu toggle_flag_upiu(Flags::fDeviceInit);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(toggle_flag_upiu);
  ASSERT_OK(response);

  ASSERT_EQ(!device_init, mock_device_.GetFlag(Flags::fDeviceInit));
}

TEST_F(QueryRequestTest, ClearFlag) {
  bool device_init = true;
  mock_device_.SetFlag(Flags::fDeviceInit, device_init);

  ClearFlagUpiu clear_flag_upiu(Flags::fDeviceInit);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(clear_flag_upiu);
  ASSERT_OK(response);

  device_init = false;
  ASSERT_EQ(device_init, mock_device_.GetFlag(Flags::fDeviceInit));
}

TEST(QueryOpcodeToString, QueryOpcodeToString) {
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kNop), "Nop");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kReadDescriptor), "Read Descriptor");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kWriteDescriptor), "Write Descriptor");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kReadAttribute), "Read Attribute");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kWriteAttribute), "Write Attribute");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kReadFlag), "Read Flag");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kSetFlag), "Set Flag");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kClearFlag), "Clear Flag");
  ASSERT_EQ(QueryOpcodeToString(QueryOpcode::kToggleFlag), "Toggle Flag");
}

}  // namespace ufs

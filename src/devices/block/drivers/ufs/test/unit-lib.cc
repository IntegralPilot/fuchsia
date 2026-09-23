// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "unit-lib.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fake-bti/bti.h>

#include <memory>

namespace ufs {

ufs_mock_device::UfsMockDevice *TestUfs::mock_device_;

void UfsTest::InitMockDevice() {
  std::unique_ptr<zx::interrupt> irq = std::make_unique<zx::interrupt>();
  ZX_ASSERT(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, irq.get()) == ZX_OK);

  // Configure mock device.
  mock_device_.Init(std::move(irq));
  ZX_ASSERT(mock_device_.AddLun(0) == ZX_OK);
}

void UfsTest::StartDriver(bool supply_power_framework) {
  driver_test().RunInEnvironmentTypeContext([&](Environment &env) {
    // Device parameters for physical (parent) device
    env.pci_server().SetMockDevice(&mock_device_);
  });

  TestUfs::SetMockDevice(&mock_device_);

  zx::result result = driver_test().StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs &args) {
    ufs_config::Config fake_config;
    fake_config.enable_suspend() = supply_power_framework;
    fake_config.enable_auto_hibernate() = true;
    fake_config.auto_hibernate_timer_us() = 1000;
    args.config(fake_config.ToVmo());
  });
  ASSERT_OK(result);

  dut_ = driver_test().driver();
}

void UfsTest::SetUp() {
  InitMockDevice();
  StartDriver(/*supply_power_framework=*/false);
  dut_->GetTransferRequestProcessor().SetTimeout(kCommandTimeout);
  dut_->GetTaskManagementRequestProcessor().SetTimeout(kCommandTimeout);
}

void UfsTest::TearDown() {
  if (!driver_stopped_) {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }
  mock_device_.GetRegisterMmioProcessor().Reset();
  mock_device_.GetUicCmdProcessor().Reset();
  mock_device_.GetTransferRequestProcessor().Reset();
  mock_device_.GetTaskManagementRequestProcessor().Reset();
  mock_device_.GetQueryRequestProcessor().Reset();
  mock_device_.GetScsiCommandProcessor().Reset();
  mock_device_.GetDmaHandler().Reset();
  mock_device_.GetRegisters()->Reset();
}

zx_status_t UfsTest::DisableController() { return dut_->DisableHostController(); }

zx_status_t UfsTest::EnableController() { return dut_->EnableHostController(); }

zx::result<> UfsTest::TransferFillDescriptorAndSendRequest(uint8_t slot, DataDirection ddir,
                                                           uint16_t resp_offset, uint16_t resp_len,
                                                           uint16_t prdt_offset,
                                                           uint16_t prdt_entry_count) {
  std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().slot_lock_);
  return dut_->GetTransferRequestProcessor().FillDescriptorAndSendRequest(
      slot, ddir, resp_offset, resp_len, prdt_offset, prdt_entry_count);
}

zx::result<> UfsTest::TaskManagementFillDescriptorAndSendRequest(
    uint8_t slot, TaskManagementRequestUpiu &request) {
  std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().slot_lock_);
  return dut_->GetTaskManagementRequestProcessor().FillDescriptorAndSendRequest(slot, request);
}

zx::result<> UfsTest::MapVmo(zx::unowned_vmo &vmo, fzl::VmoMapper &mapper, uint64_t offset,
                             uint64_t length) {
  if (zx_status_t status = mapper.Map(*vmo, offset, length); status != ZX_OK) {
    fdf::error("Failed to map IO buffer: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

uint8_t UfsTest::GetSlotStateCount(SlotState slot_state) {
  std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
  uint8_t count = 0;
  for (uint8_t slot_num = 0; slot_num < dut_->GetTransferRequestProcessor().GetSlotCount();
       ++slot_num) {
    if (dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state ==
        slot_state) {
      ++count;
    }
  }
  return count;
}

zx::result<uint32_t> UfsTest::ReadAttribute(Attributes attribute, uint8_t index) {
  return dut_->GetDeviceManager().ReadAttribute(attribute, index);
}

zx::result<> UfsTest::WriteAttribute(Attributes attribute, uint32_t value, uint8_t index) {
  return dut_->GetDeviceManager().WriteAttribute(attribute, value, index);
}

zx::result<fdf::MmioBuffer> TestUfs::CreateMmioBuffer(zx_off_t offset, size_t size, zx::vmo vmo) {
  ZX_ASSERT(mock_device_ != nullptr);
  return zx::ok(mock_device_->GetMmioBuffer(std::move(vmo)));
}

}  // namespace ufs

FUCHSIA_DRIVER_EXPORT2(ufs::TestUfs);

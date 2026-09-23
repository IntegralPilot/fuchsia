// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/test_util/platform_device_helper.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-vsi-vip/src/msd_vsi_device.h"

// These tests are unit testing the functionality of MsdVsiDevice.
// All of these tests instantiate the device in test mode, that is without the device thread active.
class TestIrqQueue : public ::testing::Test {
 public:
  void SetUp() override {
    constexpr bool kEnableSuspend = true;
    device_ = MsdVsiDevice::Create(GetTestDeviceHandle(), kEnableSuspend);
    EXPECT_NE(device_, nullptr);
  }

 protected:
  std::unique_ptr<MsdVsiDevice> device_;  // Device should be destroyed last.
};

TEST_F(TestIrqQueue, EmptyQueue) {
  {
    // Verify an interrupt is queued correctly - empty queue
    registers::IrqAck irqack;
    irqack.set_reg_value(0x0001);
    auto request = std::make_unique<MsdVsiDevice::InterruptRequest>(std::move(irqack));
    device_->EnqueueDeviceRequest(std::move(request));

    ASSERT_EQ(1UL, device_->device_request_list_.size());
    ASSERT_EQ(MsdVsiDevice::InterruptRequest::kRequestType,
              device_->device_request_list_.front()->RequestType());
  }

  {
    // Verify second interrupt is queued correctly
    registers::IrqAck irqack;
    irqack.set_reg_value(0x0002);
    auto request = std::make_unique<MsdVsiDevice::InterruptRequest>(std::move(irqack));
    device_->EnqueueDeviceRequest(std::move(request));

    ASSERT_EQ(2UL, device_->device_request_list_.size());
    uint32_t count = 0;
    for (auto it = device_->device_request_list_.begin(); it != device_->device_request_list_.end();
         ++it) {
      auto event = it->get();
      ASSERT_EQ(MsdVsiDevice::InterruptRequest::kRequestType, event->RequestType());
      auto interrupt_event = static_cast<MsdVsiDevice::InterruptRequest*>(event);
      ASSERT_EQ(++count, interrupt_event->irq_status().reg_value());
    }
  }

  {
    // Verify non interrupt is queued correctly
    device_->EnqueueDeviceRequest(std::make_unique<MsdVsiDevice::DumpRequest>());

    ASSERT_EQ(3UL, device_->device_request_list_.size());
    ASSERT_EQ(MsdVsiDevice::DumpRequest::kRequestType,
              device_->device_request_list_.back()->RequestType());
  }
}

TEST_F(TestIrqQueue, Queue) {
  {
    // Verify non interrupt is queued correctly
    device_->EnqueueDeviceRequest(std::make_unique<MsdVsiDevice::DumpRequest>());

    ASSERT_EQ(1UL, device_->device_request_list_.size());
    ASSERT_EQ(MsdVsiDevice::DumpRequest::kRequestType,
              device_->device_request_list_.back()->RequestType());
  }

  {
    // Verify an interrupt is queued correctly
    registers::IrqAck irqack;
    irqack.set_reg_value(0x0001);
    auto request = std::make_unique<MsdVsiDevice::InterruptRequest>(std::move(irqack));
    device_->EnqueueDeviceRequest(std::move(request));

    ASSERT_EQ(2UL, device_->device_request_list_.size());
    ASSERT_EQ(MsdVsiDevice::InterruptRequest::kRequestType,
              device_->device_request_list_.front()->RequestType());
  }

  {
    // Verify second interrupt is queued correctly
    registers::IrqAck irqack;
    irqack.set_reg_value(0x0002);
    auto request = std::make_unique<MsdVsiDevice::InterruptRequest>(std::move(irqack));
    device_->EnqueueDeviceRequest(std::move(request));

    ASSERT_EQ(3UL, device_->device_request_list_.size());
    uint32_t count = 0;
    for (auto it = device_->device_request_list_.begin(); it != device_->device_request_list_.end();
         ++it) {
      auto event = it->get();
      if (count < 2) {
        ASSERT_EQ(MsdVsiDevice::InterruptRequest::kRequestType, event->RequestType());
        auto interrupt_event = static_cast<MsdVsiDevice::InterruptRequest*>(event);
        ASSERT_EQ(++count, interrupt_event->irq_status().reg_value());
      } else {
        ASSERT_EQ(MsdVsiDevice::DumpRequest::kRequestType, event->RequestType());
      }
    }
  }
}

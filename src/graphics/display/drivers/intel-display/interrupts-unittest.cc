// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/interrupts.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/sync/completion.h>

#include <gtest/gtest.h>

#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/lib/testing/predicates/status.h"

namespace intel_display {

namespace {

void NopPipeVsyncCallback(PipeId, zx::time_monotonic) {}
void NopHotplugCallback(DdiId, bool) {}
void NopIrqCallback(void*, uint32_t, uint64_t) {}

class InterruptTest : public testing::Test {
 public:
  static constexpr uint32_t kMmioRegCount = 0xd0000 / sizeof(uint32_t);

  InterruptTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    loop_.StartThread("pci-fidl-server-thread");
    pci_ = fake_pci_.SetUpFidlServer(loop_);
  }

 protected:
  // `logger_` must outlive `driver_runtime_` to allow for any
  // logging in driver de-initialization code.
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher interrupt_dispatcher_ =
      driver_runtime_.StartBackgroundDispatcher();

  async::Loop loop_;
  ddk::Pci pci_;
  pci::FakePciProtocol fake_pci_;

  fake_mmio::FakeMmioRegRegion mmio_space_{32, kMmioRegCount};
  fdf::MmioBuffer mmio_buffer_{mmio_space_.GetMmioBuffer()};
};

TEST_F(InterruptTest, InitErrorWithoutAvailablePciInterrupt) {
  async_patterns::TestDispatcherBound<Interrupts> interrupts(
      interrupt_dispatcher_->async_dispatcher(), std::in_place);
  zx_status_t init_status = interrupts.SyncCall([&](Interrupts* interrupts) {
    return interrupts->Init(NopPipeVsyncCallback, NopHotplugCallback, pci_, &mmio_buffer_,
                            kTestDeviceDid);
  });
  EXPECT_STATUS(ZX_ERR_INTERNAL, init_status);
}

TEST_F(InterruptTest, InitWithLegacyInterrupt) {
  pci::RunAsync(loop_, [&] { fake_pci_.AddLegacyInterrupt(); });

  async_patterns::TestDispatcherBound<Interrupts> interrupts(
      interrupt_dispatcher_->async_dispatcher(), std::in_place);
  zx_status_t init_status = interrupts.SyncCall([&](Interrupts* interrupts) {
    return interrupts->Init(NopPipeVsyncCallback, NopHotplugCallback, pci_, &mmio_buffer_,
                            kTestDeviceDid);
  });
  EXPECT_OK(init_status);
}

TEST_F(InterruptTest, InitWithMsiInterrupt) {
  pci::RunAsync(loop_, [&] { fake_pci_.AddMsiInterrupt(); });

  async_patterns::TestDispatcherBound<Interrupts> interrupts(
      interrupt_dispatcher_->async_dispatcher(), std::in_place);
  zx_status_t init_status = interrupts.SyncCall([&](Interrupts* interrupts) {
    return interrupts->Init(NopPipeVsyncCallback, NopHotplugCallback, pci_, &mmio_buffer_,
                            kTestDeviceDid);
  });
  EXPECT_OK(init_status);

  pci::RunAsync(loop_, [&] {
    EXPECT_EQ(1u, fake_pci_.GetIrqCount());
    EXPECT_EQ(fuchsia_hardware_pci::InterruptMode::kMsi, fake_pci_.GetIrqMode());
  });
}

TEST_F(InterruptTest, InitWithMsiAndLegacyInterrupts) {
  pci::RunAsync(loop_, [&] {
    fake_pci_.AddLegacyInterrupt();
    fake_pci_.AddMsiInterrupt();
  });

  async_patterns::TestDispatcherBound<Interrupts> interrupts(
      interrupt_dispatcher_->async_dispatcher(), std::in_place);
  zx_status_t init_status = interrupts.SyncCall([&](Interrupts* interrupts) {
    return interrupts->Init(NopPipeVsyncCallback, NopHotplugCallback, pci_, &mmio_buffer_,
                            kTestDeviceDid);
  });
  EXPECT_OK(init_status);

  pci::RunAsync(loop_, [&] {
    EXPECT_EQ(1u, fake_pci_.GetIrqCount());
    EXPECT_EQ(fuchsia_hardware_pci::InterruptMode::kMsi, fake_pci_.GetIrqMode());
  });
}

TEST_F(InterruptTest, SetInterruptCallback) {
  Interrupts interrupts;

  constexpr intel_gpu_core_interrupt_t callback = {.callback = NopIrqCallback, .ctx = nullptr};
  const uint32_t gpu_interrupt_mask = 0;
  EXPECT_OK(interrupts.SetGpuInterruptCallback(callback, gpu_interrupt_mask));

  // Setting a callback when one is already assigned should fail.
  EXPECT_STATUS(ZX_ERR_ALREADY_BOUND,
                interrupts.SetGpuInterruptCallback(callback, gpu_interrupt_mask));

  // Clearing the existing callback with a null callback should fail.
  constexpr intel_gpu_core_interrupt_t null_callback = {.callback = nullptr, .ctx = nullptr};
  EXPECT_OK(interrupts.SetGpuInterruptCallback(null_callback, gpu_interrupt_mask));

  // It should be possible to set a new callback after clearing the old one.
  EXPECT_OK(interrupts.SetGpuInterruptCallback(callback, gpu_interrupt_mask));
}

}  // namespace

}  // namespace intel_display

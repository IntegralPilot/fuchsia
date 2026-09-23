// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/hot-plug-detection.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <atomic>
#include <cstddef>
#include <memory>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_display {

namespace {

struct GpioResources {
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> client;
  zx::interrupt interrupt;
};

class HotPlugDetectionTest : public ::testing::Test {
 public:
  void SetUp() override {
    fake_gpio_loop_.StartThread("fake-gpio-loop");
    pin_gpio_.SetDefaultReadResponse(zx::ok(uint8_t{0u}));
  }

  void TearDown() override { fake_gpio_loop_.Shutdown(); }

  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> GetPinGpioClient() {
    auto [gpio_client, gpio_server] = fidl::Endpoints<fuchsia_hardware_gpio::Gpio>::Create();
    fidl::BindServer(fake_gpio_loop_.dispatcher(), std::move(gpio_server), &pin_gpio_);
    return std::move(gpio_client);
  }

  GpioResources GetPinGpioResources() {
    fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> pin_gpio_client = GetPinGpioClient();
    zx::interrupt pin_gpio_interrupt = runtime_.PerformBlockingWork([&]() -> zx::interrupt {
      fidl::WireResult result = fidl::WireCall(pin_gpio_client)->GetInterrupt({});
      ZX_ASSERT_MSG(result.ok(), "FIDL connection failed: %s", result.status_string());
      fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::GetInterrupt>& interrupt_value =
          result.value();
      ZX_ASSERT_MSG(interrupt_value.is_ok(), "GPIO GetInterrupt failed: %s",
                    zx_status_get_string(interrupt_value.error_value()));
      return std::move(interrupt_value.value()->interrupt);
    });

    return {
        .client = std::move(pin_gpio_client),
        .interrupt = std::move(pin_gpio_interrupt),
    };
  }

  std::unique_ptr<HotPlugDetection> CreateAndInitHotPlugDetection() {
    // The existing pin GPIO interrupt may be invalid or have been destroyed
    // and cannot be used; we need to create a new virtual interrupt for each
    // new HotPlugDetection created.
    ResetPinGpioInterrupt();

    GpioResources pin_gpio_resources = GetPinGpioResources();

    zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
        fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                            "hot-plug-detection-thread",
                                            /*shutdown_handler=*/[](fdf_dispatcher_t*) {},
                                            /*scheduler_role=*/{});
    ZX_ASSERT(create_dispatcher_result.status_value() == ZX_OK);

    fdf::SynchronizedDispatcher dispatcher = std::move(create_dispatcher_result).value();

    auto hpd = std::make_unique<HotPlugDetection>(
        std::move(pin_gpio_resources.client), std::move(pin_gpio_resources.interrupt),
        [this](HotPlugDetectionState state) { RecordHotPlugDetectionState(state); },
        std::move(dispatcher));

    // HotPlugDetection::Init() sets up the GPIO using synchronous FIDL calls.
    // The fake GPIO FIDL server can only be bound on the test thread's default
    // dispatcher, so Init() must be called on another thread.
    zx::result<> init_result = runtime_.PerformBlockingWork([&] { return hpd->Init(); });
    EXPECT_OK(init_result);

    return hpd;
  }

  void DestroyHotPlugDetection(std::unique_ptr<HotPlugDetection>& hpd) {
    // The driver framework shuts down all dispatchers before destroying the
    // HotPlugDetection instance.
    runtime_.ShutdownAllDispatchers(/*dut_initial_dispatcher=*/nullptr);

    hpd.reset();
  }

  void ResetPinGpioInterrupt() {
    zx_status_t status =
        zx::interrupt::create(zx::resource(), 0u, ZX_INTERRUPT_VIRTUAL, &pin_gpio_interrupt_);
    ASSERT_OK(status);

    zx::interrupt gpio_interrupt;
    status = pin_gpio_interrupt_.duplicate(ZX_RIGHT_SAME_RIGHTS, &gpio_interrupt);
    ASSERT_OK(status);

    pin_gpio_.SetInterrupt(zx::ok(std::move(gpio_interrupt)));
  }

  void RecordHotPlugDetectionState(HotPlugDetectionState state) {
    fbl::AutoLock lock(&mutex_);
    recorded_detection_states_.push_back(state);
  }

  std::vector<HotPlugDetectionState> GetHotPlugDetectionStates() const {
    fbl::AutoLock lock(&mutex_);
    return recorded_detection_states_;
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime runtime_;

  async::Loop fake_gpio_loop_{&kAsyncLoopConfigNeverAttachToThread};
  fake_gpio::FakeGpio pin_gpio_;
  zx::interrupt pin_gpio_interrupt_;

  mutable fbl::Mutex mutex_;
  std::vector<HotPlugDetectionState> recorded_detection_states_ __TA_GUARDED(&mutex_);
};

TEST_F(HotPlugDetectionTest, NoHotplugEvents) {
  std::unique_ptr<HotPlugDetection> hpd = CreateAndInitHotPlugDetection();
  DestroyHotPlugDetection(hpd);
}

TEST_F(HotPlugDetectionTest, DisplayPlug) {
  std::unique_ptr<HotPlugDetection> hpd = CreateAndInitHotPlugDetection();

  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.SetDefaultReadResponse(zx::ok(uint8_t{1u}));

  runtime_.RunUntil([&] { return GetHotPlugDetectionStates().size() >= 1; });
  EXPECT_THAT(GetHotPlugDetectionStates(), testing::ElementsAre(HotPlugDetectionState::kDetected));
  EXPECT_EQ(pin_gpio_.GetInterruptMode(), fuchsia_hardware_gpio::InterruptMode::kLevelLow);

  DestroyHotPlugDetection(hpd);
}

TEST_F(HotPlugDetectionTest, DisplayPlugUnplug) {
  std::unique_ptr<HotPlugDetection> hpd = CreateAndInitHotPlugDetection();

  // Simulate plugging the display.
  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.SetDefaultReadResponse(zx::ok(uint8_t{1u}));
  runtime_.RunUntil([&] { return GetHotPlugDetectionStates().size() >= 1; });
  EXPECT_THAT(GetHotPlugDetectionStates(), testing::ElementsAre(HotPlugDetectionState::kDetected));

  // Simulate unplugging the display.
  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.SetDefaultReadResponse(zx::ok(uint8_t{0u}));
  runtime_.RunUntil([&] { return GetHotPlugDetectionStates().size() >= 2; });
  EXPECT_THAT(
      GetHotPlugDetectionStates(),
      testing::ElementsAre(HotPlugDetectionState::kDetected, HotPlugDetectionState::kNotDetected));

  EXPECT_EQ(pin_gpio_.GetInterruptMode(), fuchsia_hardware_gpio::InterruptMode::kLevelHigh);

  DestroyHotPlugDetection(hpd);
}

TEST_F(HotPlugDetectionTest, SpuriousPlugInterrupt) {
  std::unique_ptr<HotPlugDetection> hpd = CreateAndInitHotPlugDetection();

  const size_t num_state_changes_before_hotplug_gpio_read = pin_gpio_.GetStateLog().size();

  std::atomic<bool> hotplug_gpio_read = false;
  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.PushReadCallback([&](fake_gpio::FakeGpio& gpio) {
    hotplug_gpio_read.store(true, std::memory_order_relaxed);
    return zx::ok(uint8_t{0});
  });
  runtime_.RunUntil([&] { return hotplug_gpio_read.load(std::memory_order_relaxed); });

  const size_t num_state_changes_after_hotplug_gpio_read = pin_gpio_.GetStateLog().size();

  // The GPIO state (polarity, input / output config) should not change if the
  // GPIO reading doesn't change.
  EXPECT_EQ(num_state_changes_after_hotplug_gpio_read, num_state_changes_before_hotplug_gpio_read);

  EXPECT_THAT(GetHotPlugDetectionStates(), testing::IsEmpty());

  DestroyHotPlugDetection(hpd);
}

TEST_F(HotPlugDetectionTest, SpuriousUnplugInterrupt) {
  std::unique_ptr<HotPlugDetection> hpd = CreateAndInitHotPlugDetection();

  std::atomic<bool> first_hotplug_gpio_read = false;
  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.PushReadCallback([&](fake_gpio::FakeGpio& gpio) {
    first_hotplug_gpio_read.store(true, std::memory_order_relaxed);
    return zx::ok(uint8_t{1});
  });
  runtime_.RunUntil([&] {
    return first_hotplug_gpio_read.load(std::memory_order_relaxed) &&
           GetHotPlugDetectionStates().size() >= 1;
  });

  EXPECT_THAT(GetHotPlugDetectionStates(), testing::ElementsAre(HotPlugDetectionState::kDetected));

  const size_t num_state_changes_before_second_hotplug_gpio_read = pin_gpio_.GetStateLog().size();

  std::atomic<bool> second_hotplug_gpio_read = false;
  pin_gpio_interrupt_.trigger(0u, zx::clock::get_boot());
  pin_gpio_.PushReadCallback([&](fake_gpio::FakeGpio& gpio) {
    second_hotplug_gpio_read.store(true, std::memory_order_relaxed);
    return zx::ok(uint8_t{1});
  });
  runtime_.RunUntil([&] { return second_hotplug_gpio_read.load(std::memory_order_relaxed); });

  const size_t num_state_changes_after_second_hotplug_gpio_read = pin_gpio_.GetStateLog().size();

  EXPECT_THAT(GetHotPlugDetectionStates(), testing::ElementsAre(HotPlugDetectionState::kDetected));

  // The GPIO state (polarity, input / output config) should not change if the
  // GPIO reading doesn't change.
  EXPECT_EQ(num_state_changes_after_second_hotplug_gpio_read,
            num_state_changes_before_second_hotplug_gpio_read);

  DestroyHotPlugDetection(hpd);
}

}  // namespace

}  // namespace amlogic_display

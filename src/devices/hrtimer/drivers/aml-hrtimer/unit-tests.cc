// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fpromise/result.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/reader.h>

#include <gtest/gtest.h>
#include <src/lib/testing/predicates/status.h>

#include "src/devices/hrtimer/drivers/aml-hrtimer/aml-hrtimer-regs.h"
#include "src/devices/hrtimer/drivers/aml-hrtimer/aml-hrtimer.h"
#include "src/devices/hrtimer/drivers/aml-hrtimer/aml_hrtimer_config.h"

namespace hrtimer {

// In-process fake SAG for wake leases to avoid Power Framework Testing Client dependencies.
class FakeSystemActivityGovernor
    : public fidl::testing::TestBase<fuchsia_power_system::ActivityGovernor> {
 public:
  FakeSystemActivityGovernor() = default;

  fidl::ProtocolHandler<fuchsia_power_system::ActivityGovernor> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

  void GetPowerElements(GetPowerElementsCompleter::Sync& completer) override {
    fuchsia_power_system::PowerElements elements;
    completer.Reply({{std::move(elements)}});
  }

  zx::eventpair AcquireWakeLease() {
    zx::eventpair wake_lease_local, wake_lease_remote;
    EXPECT_OK(zx::eventpair::create(0, &wake_lease_local, &wake_lease_remote));
    wake_leases_.emplace_back(std::move(wake_lease_local));
    lease_requested_ = true;
    return wake_lease_remote;
  }

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    completer.Reply(fit::ok(AcquireWakeLease()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << name << " is not implemented";
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_system::ActivityGovernor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  bool GetLeaseRequested() const { return lease_requested_; }

 private:
  bool lease_requested_ = false;
  std::vector<zx::eventpair> wake_leases_;
  fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor> bindings_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    InitResources(to_driver_vfs);
    if (::testing::Test::HasFatalFailure()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  }

  void SetEnableSag(bool enable) { enable_sag_ = enable; }

  void SetTimerCurrentTicks(size_t timer_id, uint32_t count) {
    ZX_ASSERT(timer_id < 4 || (timer_id >= 5 && timer_id < kNumberOfTimers));
    uint32_t offset = (timer_id < 4)
                          ? (IsaTimerA::Get().addr() + static_cast<uint32_t>(timer_id) * 4)
                          : (IsaTimerF::Get().addr() + static_cast<uint32_t>(timer_id - 5) * 4);
    mmio_.Write32((count & 0xffff) << 16, offset);
  }

  void TriggerIrq(size_t timer_index) {
    EXPECT_OK(
        fake_interrupts_[*kTimerToIrqsIndexes[timer_index]].trigger(0, zx::clock::get_boot()));
  }

  void TriggerAllIrqs() {
    for (uint32_t i = 0; i < AmlHrtimer::GetNumberOfIrqs(); ++i) {
      EXPECT_OK(fake_interrupts_[i].trigger(0, zx::clock::get_boot()));
    }
  }

  FakeSystemActivityGovernor& system_activity_governor() { return system_activity_governor_; }

 private:
  void InitResources(fdf::OutgoingDirectory& to_driver_vfs) {
    fdf_fake::FakePDev::Config config;
    auto& mmio_info = config.mmios[0].emplace<fdf::PDev::MmioInfo>();
    mmio_info.offset = 0;
    mmio_info.size = kMmioSize;
    ASSERT_OK(mmio_.get_vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &mmio_info.vmo));

    for (uint32_t i = 0; i < AmlHrtimer::GetNumberOfIrqs(); ++i) {
      ASSERT_OK(
          zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &fake_interrupts_[i]));
      ASSERT_OK(fake_interrupts_[i].duplicate(ZX_RIGHT_SAME_RIGHTS, &config.irqs[i]));
    }

    // Power elements / GetPowerConfiguration are not needed for these tests as aml-hrtimer
    // only acquires wake leases and does not manage power elements directly.
    pdev_.SetConfig(std::move(config));

    ASSERT_OK(to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher())));

    if (enable_sag_) {
      ASSERT_OK(
          to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_system::ActivityGovernor>(
              system_activity_governor_.CreateHandler()));
    }
  }
  static constexpr size_t kMmioSize = 0x10000;
  static constexpr size_t kNumberOfTimers = 9;
  static constexpr std::optional<size_t> kTimerToIrqsIndexes[kNumberOfTimers] = {
      0, 1, 2, 3, std::nullopt, 4, 5, 6, 7};

  bool enable_sag_ = true;
  fdf::MmioBuffer mmio_ = fdf_testing::CreateMmioBuffer(kMmioSize);
  zx::interrupt fake_interrupts_[AmlHrtimer::GetNumberOfIrqs()];
  fdf_fake::FakePDev pdev_;
  FakeSystemActivityGovernor system_activity_governor_;
};

class FixtureConfig final {
 public:
  using DriverType = AmlHrtimer;
  using EnvironmentType = TestEnvironment;
};

class DriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(
        driver_test_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& start_args) mutable {
          aml_hrtimer_config::Config fake_config;
          fake_config.enable_suspend() = true;
          start_args.config(fake_config.ToVmo());
        }));
    zx::result device_result =
        driver_test_.ConnectThroughDevfs<fuchsia_hardware_hrtimer::Device>("aml-hrtimer");
    ASSERT_OK(device_result);
    client_.Bind(std::move(device_result.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  void CheckLeaseRequested(size_t timer_id) {
    driver_test_.RunInEnvironmentTypeContext([](TestEnvironment& env) {
      ASSERT_FALSE(env.system_activity_governor().GetLeaseRequested());
    });
    zx::eventpair lease;
    std::thread thread([this, timer_id, &lease]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start = client_->StartAndWait(
          {timer_id, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
           std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
      ASSERT_TRUE(result_start->keep_alive().is_valid());
      lease = std::move(result_start->keep_alive());
    });

    // Wait until the driver has acquired the timer wait completer before triggering the IRQ.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test_.RunInDriverContext([timer_id, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(timer_id);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
    driver_test_.RunInEnvironmentTypeContext(
        [timer_id](TestEnvironment& env) { env.TriggerIrq(timer_id); });
    thread.join();
    driver_test_.RunInEnvironmentTypeContext([](TestEnvironment& env) {
      ASSERT_TRUE(env.system_activity_governor().GetLeaseRequested());
    });
  }

  void CheckInspect(const char* path, const char* type, uint64_t id, uint64_t data) {
    driver_test_.RunInDriverContext([&](AmlHrtimer& driver) {
      auto& inspector = driver.inspect();
      fpromise::single_threaded_executor executor;
      executor.schedule_task(inspect::ReadFromInspector(inspector).then(
          [&](fpromise::result<inspect::Hierarchy>& hierarchy) {
            ASSERT_TRUE(hierarchy.is_ok());
            const inspect::Hierarchy* events =
                hierarchy.value().GetByPath({"hrtimer-trace", "events"});
            ASSERT_TRUE(events);
            const auto* event = events->GetByPath({path});
            auto local_id = event->node().get_property<inspect::UintPropertyValue>("id")->value();
            auto local_type =
                event->node().get_property<inspect::StringPropertyValue>("type")->value();
            auto local_data =
                event->node().get_property<inspect::UintPropertyValue>("data")->value();
            ASSERT_EQ(local_type.compare(type), 0);
            ASSERT_EQ(local_id, id);
            ASSERT_EQ(local_data, data);
          }));
      executor.run();
    });
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::SyncClient<fuchsia_hardware_hrtimer::Device> client_;
};

TEST_F(DriverTest, Properties) {
  auto result = client_->GetProperties();
  ASSERT_FALSE(result.is_error());
  ASSERT_FALSE(result->properties().IsEmpty());
  ASSERT_TRUE(result->properties().timers_properties().has_value());
  ASSERT_EQ(result->properties().timers_properties()->size(), std::size_t{9});
  auto& timers = result->properties().timers_properties().value();

  // Resolutions and range for all timers, except timer id 4.
  for (auto& i : kTimersAll) {
    ASSERT_TRUE(timers[i].id());
    if (timers[i].id().value() == 4) {
      continue;
    }
    ASSERT_EQ(timers[i].id().value(), static_cast<uint64_t>(i));
    ASSERT_EQ(timers[i].supported_resolutions()->size(), 4ULL);
    auto& resolutions = timers[i].supported_resolutions().value();
    ASSERT_EQ(resolutions[0].duration().value(), 1'000);
    ASSERT_EQ(resolutions[1].duration().value(), 10'000);
    ASSERT_EQ(resolutions[2].duration().value(), 100'000);
    ASSERT_EQ(resolutions[3].duration().value(), 1'000'000);
    if (i >= 5 && i <= 8) {
      ASSERT_EQ(timers[i].max_ticks().value(), 0xffff'ffff'ffff'ffffULL);  // extended max ticks.
    } else {
      ASSERT_EQ(timers[i].max_ticks().value(), 0xffffULL);
    }
    ASSERT_FALSE(timers[i].supports_event().value());
  }

  /// Timer id 4 has no IRQ and higher max_range.
  ASSERT_EQ(timers[4].id().value(), static_cast<uint64_t>(4));
  ASSERT_EQ(timers[4].supported_resolutions()->size(), 3ULL);
  auto& resolutions = timers[4].supported_resolutions().value();
  ASSERT_EQ(resolutions[0].duration().value(), 1'000);
  ASSERT_EQ(resolutions[1].duration().value(), 10'000);
  ASSERT_EQ(resolutions[2].duration().value(), 100'000);
  ASSERT_EQ(timers[4].max_ticks().value(), 0xffff'ffff'ffff'ffffULL);
  ASSERT_FALSE(timers[4].supports_event().value());
}

TEST_F(DriverTest, StartTimerNoticks) {
  // All timers return kNotSupported for Start.
  for (auto& i : kTimersAll) {
    auto result0 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0});
    ASSERT_TRUE(result0.is_error());
    ASSERT_EQ(result0.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
    auto result1 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(10'000ULL), 0});
    ASSERT_TRUE(result1.is_error());
    ASSERT_EQ(result1.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
    auto result2 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(100'000ULL), 0});
    ASSERT_TRUE(result2.is_error());
    ASSERT_EQ(result2.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
  }
}

TEST_F(DriverTest, StartTimerMaxTicks) {
  // All timers return kNotSupported for Start.
  for (auto& i : kTimersAll) {
    auto result0 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0xffff});
    ASSERT_TRUE(result0.is_error());
    ASSERT_EQ(result0.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
    auto result1 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(10'000ULL), 0xffff});
    ASSERT_TRUE(result1.is_error());
    ASSERT_EQ(result1.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
    auto result2 =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(100'000ULL), 0xffff});
    ASSERT_TRUE(result2.is_error());
    ASSERT_EQ(result2.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
  }
}

TEST_F(DriverTest, StartStop) {
  // All timers return kNotSupported for Start.
  for (auto& i : kTimersAll) {
    auto result_start =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 1});
    ASSERT_TRUE(result_start.is_error());
    ASSERT_EQ(result_start.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
  }

  for (auto& i : kTimersAll) {
    auto result_stop = client_->Stop(i);
    ASSERT_FALSE(result_stop.is_error());
  }
}

TEST_F(DriverTest, EventTriggering) {
  zx::event events[kNumberOfTimers];
  for (auto& i : kTimersSupportWait) {
    ASSERT_OK(zx::event::create(0, &events[i]));
    zx::event duplicate_event;
    events[i].duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_event);
    auto result_event = client_->SetEvent({i, std::move(duplicate_event)});
    ASSERT_TRUE(result_event.is_error());
    ASSERT_EQ(result_event.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
    auto result_start =
        client_->Start({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0});
    ASSERT_TRUE(result_start.is_error());
    ASSERT_EQ(result_start.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);
  }
}

TEST_F(DriverTest, GetTicksTimers0123) {
  // Can start up to 16 bits.
  constexpr uint64_t kArbitraryTicksRequest = 0xffff;

  constexpr uint32_t kArbitraryCount16bits0 = 0x1234;
  constexpr uint32_t kArbitraryCount16bits1 = 0x5678;
  constexpr uint32_t kArbitraryCount16bits2 = 0x90ab;
  constexpr uint32_t kArbitraryCount16bits3 = 0xcdef;
  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.SetTimerCurrentTicks(0, kArbitraryCount16bits0);
    env.SetTimerCurrentTicks(1, kArbitraryCount16bits1);
    env.SetTimerCurrentTicks(2, kArbitraryCount16bits2);
    env.SetTimerCurrentTicks(3, kArbitraryCount16bits3);
  });

  std::vector<std::thread> threads;
  for (uint64_t i = 0; i < 4; ++i) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                 kArbitraryTicksRequest, std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
    });

    // Wait until the driver has acquired the timer wait completer before continuing.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }

  // Reads from the registers.
  {
    auto result = client_->GetTicksLeft(0);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryCount16bits0);
  }
  {
    auto result = client_->GetTicksLeft(1);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryCount16bits1);
  }
  {
    auto result = client_->GetTicksLeft(2);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryCount16bits2);
  }
  {
    auto result = client_->GetTicksLeft(3);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryCount16bits3);
  }

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });
  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, GetTicksTimer4) {
  // Can start up to 64 bits.
  constexpr uint64_t kArbitraryTicksRequest = 0x1234'5678'90ab'cdef;

  auto result_start = client_->Start(
      {4, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), kArbitraryTicksRequest});
  ASSERT_TRUE(result_start.is_error());
  ASSERT_EQ(result_start.error_value().domain_error(),
            fuchsia_hardware_hrtimer::DriverError::kNotSupported);

  auto result_stop = client_->Stop(4);
  ASSERT_FALSE(result_stop.is_error());
}

TEST_F(DriverTest, GetTicksTimers5678TicksStayAtRequested) {
  // Can start up to 64 bits because they support ticks extension.
  // Use a value that exceeds 16 bits but is small enough to finish quickly.
  constexpr uint64_t kArbitraryTicksRequest = 0x1'1234;

  // The count starts at max for the register since the request goes beyond the register max.
  constexpr uint64_t kMaxCount = 0xffff;
  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.SetTimerCurrentTicks(5, kMaxCount);
    env.SetTimerCurrentTicks(6, kMaxCount);
    env.SetTimerCurrentTicks(7, kMaxCount);
    env.SetTimerCurrentTicks(8, kMaxCount);
  });

  std::vector<std::thread> threads;
  for (uint64_t i = 5; i < 9; ++i) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                 kArbitraryTicksRequest, std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
    });

    // Wait until the driver has acquired the timer wait completer before continuing.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    // Ticks left stay at the ticks requested since the register reads 0xffff.
    auto result = client_->GetTicksLeft(i);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest);
  }

  // Trigger IRQs twice to finish the StartAndWait calls.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, GetTicksTimers5678TicksDownBy0xffff) {
  // Can start up to 64 bits because they support ticks extension.
  // Use a value that exceeds 16 bits but is small enough to finish quickly.
  constexpr uint64_t kArbitraryTicksRequest = 0x1'1234;

  // The count has decreased by 0xffff to 0.
  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.SetTimerCurrentTicks(5, 0);
    env.SetTimerCurrentTicks(6, 0);
    env.SetTimerCurrentTicks(7, 0);
    env.SetTimerCurrentTicks(8, 0);
  });

  std::vector<std::thread> threads;
  for (uint64_t i = 5; i < 9; ++i) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                 kArbitraryTicksRequest, std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
    });

    // Wait until the driver has acquired the timer wait completer before continuing.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    // Ticks have decreased by 0xffff since the register reads 0.
    auto result = client_->GetTicksLeft(i);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest - 0xffff);
  }

  // Trigger IRQs twice to finish the StartAndWait calls.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, GetTicksTimers5678ArbitraryCount) {
  // Can start up to 64 bits because they support ticks extension.
  // Use a value that exceeds 16 bits but is small enough to finish quickly.
  constexpr uint64_t kArbitraryTicksRequest = 0x1'1234;

  constexpr uint64_t kArbitraryCount5 = 0x1234;
  constexpr uint64_t kArbitraryCount6 = 0x5678;
  constexpr uint64_t kArbitraryCount7 = 0x90ab;
  constexpr uint64_t kArbitraryCount8 = 0xcdef;
  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.SetTimerCurrentTicks(5, kArbitraryCount5);
    env.SetTimerCurrentTicks(6, kArbitraryCount6);
    env.SetTimerCurrentTicks(7, kArbitraryCount7);
    env.SetTimerCurrentTicks(8, kArbitraryCount8);
  });

  std::vector<std::thread> threads;
  for (uint64_t i = 5; i < 9; ++i) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                 kArbitraryTicksRequest, std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
    });

    // Wait until the driver has acquired the timer wait completer before continuing.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }

  // Ticks have decreased by 0xffff - kArbitraryCount (register read).
  {
    auto result = client_->GetTicksLeft(5);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest - (0xffff - kArbitraryCount5));
  }
  {
    auto result = client_->GetTicksLeft(6);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest - (0xffff - kArbitraryCount6));
  }
  {
    auto result = client_->GetTicksLeft(7);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest - (0xffff - kArbitraryCount7));
  }
  {
    auto result = client_->GetTicksLeft(8);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryTicksRequest - (0xffff - kArbitraryCount8));
  }

  // Trigger IRQs twice to finish the StartAndWait calls.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, GetTicksTimers5678ArbitraryCountWithIrq) {
  // Can start up to 64 bits because they support ticks extension.
  constexpr uint64_t kTicksRequestEnoughFor2Irqs = 0x1'1235;

  constexpr uint64_t kArbitraryCount = 0x1234;
  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.SetTimerCurrentTicks(5, kArbitraryCount);
    env.SetTimerCurrentTicks(6, kArbitraryCount);
    env.SetTimerCurrentTicks(7, kArbitraryCount);
    env.SetTimerCurrentTicks(8, kArbitraryCount);
  });

  std::vector<std::thread> threads;
  for (uint64_t i = 5; i < 9; ++i) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                 kTicksRequestEnoughFor2Irqs, std::move(setup_event)});
      ASSERT_FALSE(result_start.is_error());
    });

    // Wait until the driver has acquired the timer wait completer before continuing.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    // Because the requested ticks is biggger than 0xffff, before any IRQ triggers we'll get
    // a decrease of 0xffff - kArbitraryCount (register read).
    auto result = client_->GetTicksLeft(i);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kTicksRequestEnoughFor2Irqs - (0xffff - kArbitraryCount));
  }

  // Trigger IRQs, indicates that the first 0xffff passed.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  for (uint64_t i = 5; i < 9; ++i) {
    // Wait until after the IRQ is handled and start ticks left fit in the hardware capabilities.
    bool start_ticks_left_fit = false;
    while (!start_ticks_left_fit) {
      driver_test().RunInDriverContext([i, &start_ticks_left_fit](AmlHrtimer& driver) {
        start_ticks_left_fit = driver.StartTicksLeftFitInHardware(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    // Now that we have received at least an IRQ for the first 0xffff passed, GetTicksLeft starts
    // returning the read from the register.
    auto result = client_->GetTicksLeft(i);
    ASSERT_FALSE(result.is_error());
    ASSERT_EQ(result->ticks(), kArbitraryCount);
  }

  // Trigger IRQs again to finish the StartAndWait calls.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });
  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, StartAndWaitTriggering) {
  std::vector<std::thread> threads;
  for (auto& i : kTimersSupportWait) {
    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      zx::event duplicate_event;
      setup_event.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_event);
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
                                 std::move(duplicate_event)});
      ASSERT_FALSE(result_start.is_error());
      ASSERT_TRUE(result_start->keep_alive().is_valid());

      // setup_event must have been signaled since the timer expired.
      zx_signals_t signals = {};
      ASSERT_OK(setup_event.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), &signals));
    });

    // Wait until the driver has acquired the timer wait completer before triggering the IRQ.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  // Join the threads such that we check for timers triggered.
  for (auto& thread : threads) {
    thread.join();
  }

  CheckInspect("0", "StartAndWait", 0, 0);
  CheckInspect("1", "StartHardware", 0, 0);
  CheckInspect("2", "StartAndWait", 1, 0);
  CheckInspect("3", "StartHardware", 1, 0);
  CheckInspect("4", "StartAndWait", 2, 0);
  CheckInspect("5", "StartHardware", 2, 0);
  CheckInspect("6", "StartAndWait", 3, 0);
  CheckInspect("7", "StartHardware", 3, 0);
  CheckInspect("8", "StartAndWait", 5, 0);
  CheckInspect("9", "StartHardware", 5, 0);
  CheckInspect("10", "StartAndWait", 6, 0);
  CheckInspect("11", "StartHardware", 6, 0);
  CheckInspect("12", "StartAndWait", 7, 0);
  CheckInspect("13", "StartHardware", 7, 0);
  CheckInspect("14", "StartAndWait", 8, 0);
  CheckInspect("15", "StartHardware", 8, 0);
  // Not checking TriggerIrqWait since we are not ordering IRQ triggers.
}

TEST_F(DriverTest, StartAndWait2Triggering) {
  std::vector<std::thread> threads;
  for (auto& i : kTimersSupportWait) {
    threads.emplace_back([this, i]() {
      zx::eventpair local_wake_lease, remote_wake_lease;
      ASSERT_OK(fuchsia_power_system::LeaseToken::create(0, &local_wake_lease, &remote_wake_lease));
      auto result_start =
          client_->StartAndWait2({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                  0, std::move(remote_wake_lease)});
      ASSERT_FALSE(result_start.is_error());
      ASSERT_TRUE(result_start->expiration_keep_alive().is_valid());
    });
    // Wait until the driver has acquired the timer wait completer before triggering the IRQ.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) { env.TriggerAllIrqs(); });

  // Join the threads such that we check for timers triggered.
  for (auto& thread : threads) {
    thread.join();
  }

  CheckInspect("0", "StartAndWait2", 0, 0);
  CheckInspect("1", "StartHardware", 0, 0);
  CheckInspect("2", "StartAndWait2", 1, 0);
  CheckInspect("3", "StartHardware", 1, 0);
  CheckInspect("4", "StartAndWait2", 2, 0);
  CheckInspect("5", "StartHardware", 2, 0);
  CheckInspect("6", "StartAndWait2", 3, 0);
  CheckInspect("7", "StartHardware", 3, 0);
  CheckInspect("8", "StartAndWait2", 5, 0);
  CheckInspect("9", "StartHardware", 5, 0);
  CheckInspect("10", "StartAndWait2", 6, 0);
  CheckInspect("11", "StartHardware", 6, 0);
  CheckInspect("12", "StartAndWait2", 7, 0);
  CheckInspect("13", "StartHardware", 7, 0);
  CheckInspect("14", "StartAndWait2", 8, 0);
  CheckInspect("15", "StartHardware", 8, 0);
  // Not checking TriggerIrqWait2 since we are not ordering IRQ triggers.
}

TEST_F(DriverTest, StartAndWaitStop) {
  for (auto& i : kTimersSupportWait) {
    std::thread thread([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      zx::event duplicate_event;
      setup_event.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_event);
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
                                 std::move(duplicate_event)});
      ASSERT_TRUE(result_start.is_error());
      ASSERT_EQ(result_start.error_value().domain_error(),
                fuchsia_hardware_hrtimer::DriverError::kCanceled);

      // setup_event must have been signaled since the timer expired.
      zx_signals_t signals = {};
      ASSERT_OK(setup_event.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), &signals));
    });

    // Wait until the driver has acquired a wait completer such that we can cancel the timer.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    auto result_start_stop = client_->Stop(i);
    ASSERT_FALSE(result_start_stop.is_error());
    thread.join();
  }
}

TEST_F(DriverTest, StartAndWait2Stop) {
  for (auto& i : kTimersSupportWait) {
    std::thread thread([this, i]() {
      zx::eventpair local_wake_lease, remote_wake_lease;
      ASSERT_OK(fuchsia_power_system::LeaseToken::create(0, &local_wake_lease, &remote_wake_lease));
      auto result_start =
          client_->StartAndWait2({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                                  0, std::move(remote_wake_lease)});
      ASSERT_TRUE(result_start.is_error());
      ASSERT_EQ(result_start.error_value().domain_error(),
                fuchsia_hardware_hrtimer::DriverError::kCanceled);
    });

    // Wait until the driver has acquired a wait completer such that we can cancel the timer.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    auto result_start_stop = client_->Stop(i);
    ASSERT_FALSE(result_start_stop.is_error());
    thread.join();
  }
}

class DriverTestNoAutoStop : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(
        driver_test_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& start_args) mutable {
          aml_hrtimer_config::Config fake_config;
          fake_config.enable_suspend() = true;
          start_args.config(fake_config.ToVmo());
        }));
    zx::result device_result =
        driver_test_.ConnectThroughDevfs<fuchsia_hardware_hrtimer::Device>("aml-hrtimer");
    ASSERT_OK(device_result);
    client_.Bind(std::move(device_result.value()));
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;

  fidl::SyncClient<fuchsia_hardware_hrtimer::Device> client_;
};

TEST_F(DriverTestNoAutoStop, CancelOnDriverStop) {
  std::vector<std::thread> threads;
  zx::event events[kNumberOfTimers];
  for (auto& i : kTimersSupportWait) {
    ASSERT_OK(zx::event::create(0, &events[i]));
    zx::event duplicate_event;
    events[i].duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_event);
    auto result_event = client_->SetEvent({i, std::move(duplicate_event)});
    ASSERT_TRUE(result_event.is_error());
    ASSERT_EQ(result_event.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kNotSupported);

    threads.emplace_back([this, i]() {
      zx::event setup_event;
      ASSERT_OK(zx::event::create(0, &setup_event));
      auto result_start =
          client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
                                 std::move(setup_event)});
      ASSERT_TRUE(result_start.is_error());
      // Check that we cancel on driver stop.
      ASSERT_EQ(result_start.error_value().domain_error(),
                fuchsia_hardware_hrtimer::DriverError::kCanceled);
    });

    // Wait until the driver has acquired a wait completer such that it can be canceled.
    bool has_wait_completer = false;
    while (!has_wait_completer) {
      driver_test().RunInDriverContext([i, &has_wait_completer](AmlHrtimer& driver) {
        has_wait_completer = driver.HasWaitCompleter(i);
      });
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }
  // Start timer 4 as well.
  auto result_start =
      client_->Start({4ULL, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL),
                      0xffff'ffff'ffff'ffffULL});
  ASSERT_TRUE(result_start.is_error());
  ASSERT_EQ(result_start.error_value().domain_error(),
            fuchsia_hardware_hrtimer::DriverError::kNotSupported);

  // Force driver stop.
  ASSERT_OK(driver_test().StopDriver());

  // Join the threads such that we check for timers canceled.
  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(DriverTest, LeaseRequested0) { CheckLeaseRequested(0); }
TEST_F(DriverTest, LeaseRequested1) { CheckLeaseRequested(1); }
TEST_F(DriverTest, LeaseRequested2) { CheckLeaseRequested(2); }
TEST_F(DriverTest, LeaseRequested3) { CheckLeaseRequested(3); }

TEST_F(DriverTest, LeaseNotRequested4) {
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_FALSE(env.system_activity_governor().GetLeaseRequested());
  });
  zx::event setup_event;
  ASSERT_OK(zx::event::create(0, &setup_event));
  auto result_start = client_->StartAndWait(
      {4, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0, std::move(setup_event)});
  ASSERT_TRUE(result_start.is_error());
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_FALSE(env.system_activity_governor().GetLeaseRequested());
  });
}

TEST_F(DriverTest, LeaseRequested5) { CheckLeaseRequested(5); }
TEST_F(DriverTest, LeaseRequested6) { CheckLeaseRequested(6); }
TEST_F(DriverTest, LeaseRequested7) { CheckLeaseRequested(7); }
TEST_F(DriverTest, LeaseRequested8) { CheckLeaseRequested(8); }

class DriverTestNoPower : public ::testing::Test {
 public:
  void SetUp() override {
    driver_test_.RunInEnvironmentTypeContext([](TestEnvironment& env) { env.SetEnableSag(false); });
    ASSERT_OK(
        driver_test_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& start_args) mutable {
          aml_hrtimer_config::Config fake_config;
          fake_config.enable_suspend() = false;
          start_args.config(fake_config.ToVmo());
        }));
    zx::result device_result =
        driver_test_.ConnectThroughDevfs<fuchsia_hardware_hrtimer::Device>("aml-hrtimer");
    ASSERT_OK(device_result);
    client_.Bind(std::move(device_result.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::SyncClient<fuchsia_hardware_hrtimer::Device> client_;
};

TEST_F(DriverTestNoPower, StartAndWaitTriggeringNoPower) {
  for (auto& i : kTimersSupportWait) {
    zx::event setup_event;
    ASSERT_OK(zx::event::create(0, &setup_event));
    auto result_start =
        client_->StartAndWait({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
                               std::move(setup_event)});
    ASSERT_TRUE(result_start.is_error());  // Must fail, no power configuration.
    ASSERT_EQ(result_start.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kBadState);

    // setup_event is not signaled since the timer was not setup.
  }
}

TEST_F(DriverTestNoPower, StartAndWait2TriggeringNoPower) {
  for (auto& i : kTimersSupportWait) {
    zx::eventpair local_wake_lease, remote_wake_lease;
    ASSERT_OK(fuchsia_power_system::LeaseToken::create(0, &local_wake_lease, &remote_wake_lease));
    auto result_start =
        client_->StartAndWait2({i, fuchsia_hardware_hrtimer::Resolution::WithDuration(1'000ULL), 0,
                                std::move(remote_wake_lease)});
    ASSERT_TRUE(result_start.is_error());  // Must fail, no power configuration.
    ASSERT_EQ(result_start.error_value().domain_error(),
              fuchsia_hardware_hrtimer::DriverError::kBadState);
  }
}

}  // namespace hrtimer

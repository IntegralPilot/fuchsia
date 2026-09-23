// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_device.h"

#include <atomic>
#include <cstring>
#include <memory>
#include <thread>

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/audio_device_manager.h"
#include "src/media/audio/audio_core/audio_driver.h"
#include "src/media/audio/audio_core/device_config.h"
#include "src/media/audio/audio_core/device_registry.h"
#include "src/media/audio/audio_core/testing/fake_audio_driver.h"
#include "src/media/audio/audio_core/testing/threading_model_fixture.h"
#include "src/media/audio/lib/clock/testing/clock_test.h"

namespace media::audio {
namespace {

class FakeAudioDevice : public AudioDevice {
 public:
  FakeAudioDevice(AudioDevice::Type type, const DeviceConfig& config,
                  ThreadingModel* threading_model, DeviceRegistry* registry,
                  LinkMatrix* link_matrix, std::shared_ptr<AudioCoreClockFactory> clock_factory)
      : AudioDevice(type, "", config, threading_model, registry, link_matrix, clock_factory,
                    std::make_unique<AudioDriver>(this)) {}

  // Needed because AudioDevice is an abstract class
  void ApplyGainLimits(fuchsia::media::AudioGainInfo* in_out_info,
                       fuchsia::media::AudioGainValidFlags set_flags) override {}
  void OnWakeup() FXL_EXCLUSIVE_LOCKS_REQUIRED(mix_domain().token()) override {
    driver()->GetDriverInfo();
  }
  void OnDriverInfoFetched() FXL_EXCLUSIVE_LOCKS_REQUIRED(mix_domain().token()) override {
    driver_info_fetched_ = true;
  }
  bool driver_info_fetched_ = false;
};

class AudioDeviceTest : public testing::ThreadingModelFixture {
 protected:
  static constexpr uint32_t kCustomClockDomain = 42;

  void SetUp() override {
    device_ = std::make_shared<FakeAudioDevice>(
        AudioObject::Type::Input, context().process_config().device_config(), &threading_model(),
        &context().device_manager(), &context().link_matrix(), context().clock_factory());

    zx::channel c1, c2;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &c1, &c2));
    remote_driver_ = std::make_unique<testing::FakeAudioDriver>(std::move(c1), dispatcher());
    remote_driver_->set_clock_domain(kCustomClockDomain);

    device_->driver()->Init(std::move(c2));
    remote_driver_->Start();
  }
  std::shared_ptr<FakeAudioDevice> device_;
  std::unique_ptr<testing::FakeAudioDriver> remote_driver_;
};

// After GetDriverInfo, the clock domain has been set and the ref clock is valid.
TEST_F(AudioDeviceTest, ReferenceClockIsAdvancing) {
  threading_model().FidlDomain().ScheduleTask(device_->Startup());

  RunLoopUntilIdle();
  EXPECT_TRUE(device_->driver_info_fetched_);
  clock::testing::VerifyAdvances(*device_->reference_clock(),
                                 context().clock_factory()->synthetic());
}

TEST_F(AudioDeviceTest, DefaultClockIsClockMono) {
  threading_model().FidlDomain().ScheduleTask(device_->Startup());
  RunLoopUntilIdle();
  EXPECT_TRUE(device_->driver_info_fetched_);

  clock::testing::VerifyIsSystemMonotonic(*device_->reference_clock());
}

class InitFailingAudioDevice : public FakeAudioDevice {
 public:
  using FakeAudioDevice::FakeAudioDevice;
  zx_status_t Init() override { return ZX_ERR_INTERNAL; }
};

// Verify that calling Shutdown on an AudioDevice after its initial Startup failed (calling Shutdown
// when mix_domain is null) does not dereference that null execution domain pointer.
TEST_F(AudioDeviceTest, ShutdownAfterStartupFailure) {
  auto failing_device = std::make_shared<InitFailingAudioDevice>(
      AudioObject::Type::Input, context().process_config().device_config(), &threading_model(),
      &context().device_manager(), &context().link_matrix(), context().clock_factory());

  threading_model().FidlDomain().ScheduleTask(failing_device->Startup().then(
      [](fpromise::result<void, zx_status_t>& res) { EXPECT_TRUE(res.is_error()); }));
  RunLoopUntilIdle();

  threading_model().FidlDomain().ScheduleTask(failing_device->Shutdown());
  // Give tasks on other threads a chance to run before tearing down the test case.
  RunLoopUntilIdle();
}

// Verify concurrent access to device configuration and routing profile without data-races.
TEST_F(AudioDeviceTest, ConcurrentConfigAndProfileAccess) {
  std::atomic<bool> stop = false;
  std::thread profile_thread([&]() {
    while (!stop.load()) {
      {
        auto prof = device_->profile();
        (void)prof;
      }
      {
        auto cfg = device_->config();
        (void)cfg;
      }
    }
  });

  // Without proper synchronization, set_config() while profile_thread calls profile() or config()
  // causes data races in DeviceConfig, leading to undefined behavior, TSan/ASan errors, crashes.
  for (int i = 0; i < 100; ++i) {
    device_->set_config(context().process_config().device_config());
  }
  stop.store(true);
  profile_thread.join();

  // Verify device configuration and routing profile state (intact, uncorrupted, consistent) after
  // heavily contended concurrent reads and mutations. Validate defaults that were set in the ctor.
  EXPECT_TRUE(
      device_->profile()->supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND)));
  EXPECT_FALSE(
      device_->profile()->supports_usage(StreamUsage::WithRenderUsage(RenderUsage::MEDIA)));
  EXPECT_TRUE(device_->config()->default_output_device_profile().eligible_for_loopback());
  EXPECT_EQ(device_->config()->default_input_device_profile().rate(),
            DeviceConfig::InputDeviceProfile::kDefaultRate);
}

// Verify heavily contended multi-threaded access across multiple readers and mutators.
TEST_F(AudioDeviceTest, MultiThreadedConcurrentConfigAndProfileAccess) {
  std::atomic<bool> stop = false;
  std::atomic<int> started = 0;

  auto reader = [&]() {
    started.fetch_add(1);
    while (!stop.load()) {
      {
        auto prof = device_->profile();
        (void)prof->supports_usage(StreamUsage::WithRenderUsage(RenderUsage::MEDIA));
      }
      {
        auto cfg = device_->config();
        (void)cfg->default_output_device_profile();
      }
    }
  };

  auto mutator = [&]() {
    started.fetch_add(1);
    while (!stop.load()) {
      device_->set_config(context().process_config().device_config());
    }
  };

  std::vector<std::thread> threads;
  for (int i = 0; i < 4; ++i) {
    threads.emplace_back(reader);
  }
  for (int i = 0; i < 2; ++i) {
    threads.emplace_back(mutator);
  }

  while (started.load() < 6) {
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }

  for (int i = 0; i < 100; ++i) {
    device_->set_config(context().process_config().device_config());
  }

  stop.store(true);
  for (auto& t : threads) {
    t.join();
  }

  // Verify device configuration and routing profile state (intact, uncorrupted, consistent) after
  // heavily contended concurrent reads and mutations. Validate defaults that were set in the ctor.
  EXPECT_TRUE(
      device_->profile()->supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND)));
  EXPECT_FALSE(
      device_->profile()->supports_usage(StreamUsage::WithRenderUsage(RenderUsage::MEDIA)));
  EXPECT_TRUE(device_->config()->default_output_device_profile().eligible_for_loopback());
  EXPECT_EQ(device_->config()->default_input_device_profile().rate(),
            DeviceConfig::InputDeviceProfile::kDefaultRate);
}

class NullMixDomainThreadingModel : public ThreadingModel {
 public:
  explicit NullMixDomainThreadingModel(ThreadingModel* base) : base_(base) {}
  ExecutionDomain& FidlDomain() override { return base_->FidlDomain(); }
  ExecutionDomain& IoDomain() override { return base_->IoDomain(); }
  OwnedDomainPtr AcquireMixDomain(const std::string& name) override { return nullptr; }
  void RunAndJoinAllThreads() override {}
  void Quit() override {}

 private:
  ThreadingModel* base_;
};

// Verify calling Shutdown on an AudioDevice without a mix domain safely returns without crash.
TEST_F(AudioDeviceTest, ShutdownWithoutMixDomain) {
  NullMixDomainThreadingModel null_model(&threading_model());
  auto device = std::make_shared<FakeAudioDevice>(
      AudioDevice::Type::Output, context().process_config().device_config(), &null_model,
      &context().device_manager(), &context().link_matrix(), context().clock_factory());
  device->Shutdown();
}

}  // namespace
}  // namespace media::audio

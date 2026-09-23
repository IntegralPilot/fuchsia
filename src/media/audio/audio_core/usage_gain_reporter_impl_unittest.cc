// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_gain_reporter_impl.h"

#include <lib/fidl/cpp/binding.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/audio_core/device_id.h"

namespace media::audio {
namespace {

using fuchsia::media::AudioRenderUsage2;

const std::string DEVICE_ID_STRING = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const audio_stream_unique_id_t DEVICE_ID_AUDIO_STREAM =
    DeviceUniqueIdFromString(DEVICE_ID_STRING).take_value();

const std::string BLUETOOTH_DEVICE_ID_STRING = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const audio_stream_unique_id_t BLUETOOTH_DEVICE_ID_AUDIO_STREAM =
    DeviceUniqueIdFromString(BLUETOOTH_DEVICE_ID_STRING).take_value();

class FakeGainListener : public fuchsia::media::UsageGainListener {
 public:
  fidl::InterfaceHandle<fuchsia::media::UsageGainListener> NewBinding() {
    return binding_.NewBinding();
  }

  void CloseBinding() { binding_.Close(0); }

  bool muted() const { return last_muted_; }

  float gain_dbfs() const { return last_gain_dbfs_; }

  size_t call_count() const { return call_count_; }

 private:
  // |fuchsia::media::UsageGainListener|
  void OnGainMuteChanged(bool muted, float gain_dbfs, OnGainMuteChangedCallback callback) final {
    last_muted_ = muted;
    last_gain_dbfs_ = gain_dbfs;
    call_count_++;
  }

  fidl::Binding<fuchsia::media::UsageGainListener> binding_{this};
  bool last_muted_ = false;
  float last_gain_dbfs_ = 0.0;
  size_t call_count_ = 0;
};

class TestDeviceLister : public DeviceLister {
 public:
  void AddDeviceInfo(const fuchsia::media::AudioDeviceInfo& device_info) {
    device_info_.push_back(device_info);
  }

  // |DeviceLister|
  std::vector<fuchsia::media::AudioDeviceInfo> GetDeviceInfos() final { return device_info_; }

 private:
  std::vector<fuchsia::media::AudioDeviceInfo> device_info_;
};

}  // namespace

class UsageGainReporterTest : public gtest::TestLoopFixture {
 protected:
  UsageGainReporterTest()
      : process_config_(
            ProcessConfigBuilder()
                .SetDefaultVolumeCurve(VolumeCurve::DefaultForMinGain(-60.0))
                .AddDeviceProfile({std::vector<audio_stream_unique_id_t>{DEVICE_ID_AUDIO_STREAM},
                                   DeviceConfig::OutputDeviceProfile(
                                       /* eligible_for_loopback=*/true, /*supported_usages=*/{})})
                .AddDeviceProfile(
                    {std::vector<audio_stream_unique_id_t>{BLUETOOTH_DEVICE_ID_AUDIO_STREAM},
                     DeviceConfig::OutputDeviceProfile(
                         /* eligible_for_loopback=*/true,
                         /*supported_usages=*/{}, VolumeCurve::DefaultForMinGain(-60.0),
                         /* independent_volume_control=*/true, PipelineConfig::Default(),
                         /*driver_gain_db=*/0.0, /*software_gain_db=*/0.0)})
                .Build()),
        usage_(ToFidlUsage2(RenderUsage::MEDIA)) {}

  enum class ListenerType : uint8_t { kOld, kNew };

  std::unique_ptr<FakeGainListener> Listen(const std::string& device_id, ListenerType lt) {
    auto device_lister = std::make_unique<TestDeviceLister>();
    device_lister->AddDeviceInfo({.unique_id = device_id});

    stream_volume_manager_ = std::make_unique<StreamVolumeManager>(dispatcher());
    under_test_ = std::make_unique<UsageGainReporterImpl>(
        *device_lister.get(), *stream_volume_manager_, process_config());

    auto fake_gain_listener = std::make_unique<FakeGainListener>();

    if (lt == ListenerType::kOld) {
      under_test()->RegisterListener(device_id, fidl::Clone(*ToFidlUsageTry(usage())),
                                     fake_gain_listener->NewBinding());
    } else {
      under_test()->RegisterListener2(device_id, fidl::Clone(usage()),
                                      fake_gain_listener->NewBinding());
    }

    return fake_gain_listener;
  }

  void TestUpdatesSingleListenerUsageGain(ListenerType listener_type) {
    auto fake_listener = Listen(DEVICE_ID_STRING, listener_type);
    const float expected_gain_dbfs = -10.0;
    stream_volume_manager()->SetUsageGain(fidl::Clone(usage()), expected_gain_dbfs);

    RunLoopUntilIdle();
    EXPECT_FLOAT_EQ(fake_listener->gain_dbfs(), expected_gain_dbfs);
    EXPECT_EQ(fake_listener->call_count(), 2u);
  }

  void TestUpdatesSingleListenerUsageGainAdjustment(ListenerType listener_type) {
    auto fake_listener = Listen(DEVICE_ID_STRING, listener_type);
    const float expected_gain_dbfs = -10.0;
    stream_volume_manager()->SetUsageGainAdjustment(fidl::Clone(usage()), expected_gain_dbfs);

    RunLoopUntilIdle();
    EXPECT_FLOAT_EQ(fake_listener->gain_dbfs(), expected_gain_dbfs);
    EXPECT_EQ(fake_listener->call_count(), 2u);
  }

  void TestUpdatesSingleListenerUsageGainCombination(ListenerType listener_type) {
    auto fake_listener = Listen(DEVICE_ID_STRING, listener_type);
    const float expected_gain_dbfs = -10.0;
    stream_volume_manager()->SetUsageGain(fidl::Clone(usage()), expected_gain_dbfs);
    stream_volume_manager()->SetUsageGainAdjustment(fidl::Clone(usage()), expected_gain_dbfs);

    RunLoopUntilIdle();
    EXPECT_FLOAT_EQ(fake_listener->gain_dbfs(), (2 * expected_gain_dbfs));
    EXPECT_EQ(fake_listener->call_count(), 3u);
  }

  void TestNoUpdateIndependentVolumeControlSingleListener(ListenerType listener_type) {
    auto fake_listener = Listen(BLUETOOTH_DEVICE_ID_STRING, listener_type);
    const float attempted_gain_dbfs = -10.0;
    stream_volume_manager()->SetUsageGain(fidl::Clone(usage()), attempted_gain_dbfs);

    RunLoopUntilIdle();
    EXPECT_FLOAT_EQ(fake_listener->gain_dbfs(), 0.0f);
    EXPECT_EQ(fake_listener->call_count(), 0u);
  }

  void TestHandlesClosedChannel(ListenerType listener_type) {
    auto fake_listener = Listen(DEVICE_ID_STRING, listener_type);
    RunLoopUntilIdle();
    EXPECT_EQ(fake_listener->call_count(), 1u);
    EXPECT_EQ(NumListeners(), 1ul);

    fake_listener->CloseBinding();
    RunLoopUntilIdle();
    EXPECT_EQ(NumListeners(), 0ul);

    // Destruct.
    fake_listener = nullptr;

    // Verify we removed the listener from StreamVolumeManager. If we did not, this will crash.
    stream_volume_manager()->SetUsageGain(ToFidlUsage2(RenderUsage::MEDIA), 0.42f);
  }

  size_t NumListeners() { return under_test()->listeners_.size(); }

  std::unique_ptr<StreamVolumeManager>& stream_volume_manager() { return stream_volume_manager_; }
  std::unique_ptr<UsageGainReporterImpl>& under_test() { return under_test_; }
  const ProcessConfig& process_config() { return process_config_; }
  const fuchsia::media::Usage2& usage() { return usage_; }

 private:
  std::unique_ptr<StreamVolumeManager> stream_volume_manager_;
  std::unique_ptr<UsageGainReporterImpl> under_test_;
  ProcessConfig process_config_;
  const fuchsia::media::Usage2 usage_;
};

TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGain) {
  TestUpdatesSingleListenerUsageGain(ListenerType::kOld);
}
TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGain2) {
  TestUpdatesSingleListenerUsageGain(ListenerType::kNew);
}

TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGainAdjustment) {
  TestUpdatesSingleListenerUsageGainAdjustment(ListenerType::kOld);
}
TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGainAdjustment2) {
  TestUpdatesSingleListenerUsageGainAdjustment(ListenerType::kNew);
}

TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGainCombination) {
  TestUpdatesSingleListenerUsageGainCombination(ListenerType::kOld);
}
TEST_F(UsageGainReporterTest, UpdatesSingleListenerUsageGainCombination2) {
  TestUpdatesSingleListenerUsageGainCombination(ListenerType::kNew);
}

TEST_F(UsageGainReporterTest, NoUpdateIndependentVolumeControlSingleListener) {
  TestNoUpdateIndependentVolumeControlSingleListener(ListenerType::kOld);
}
TEST_F(UsageGainReporterTest, NoUpdateIndependentVolumeControlSingleListener2) {
  TestNoUpdateIndependentVolumeControlSingleListener(ListenerType::kNew);
}

TEST_F(UsageGainReporterTest, HandlesClosedChannel) {
  TestHandlesClosedChannel(ListenerType::kOld);
}
TEST_F(UsageGainReporterTest, HandlesClosedChannel2) {
  TestHandlesClosedChannel(ListenerType::kNew);
}

}  // namespace media::audio

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/audio/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>

#include <memory>
#include <utility>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"
#include "src/media/audio/lib/test/constants.h"
#include "src/media/audio/lib/test/test_fixture.h"

namespace media::audio::test {

using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage2;

namespace {
class FakeGainListener : public fuchsia::media::UsageGainListener {
 public:
  explicit FakeGainListener(TestFixture* fixture) : fixture_(fixture), binding_(this) {}

  fidl::InterfaceHandle<fuchsia::media::UsageGainListener> NewBinding() {
    auto handle = binding_.NewBinding();
    fixture_->AddErrorHandler(binding_, "FakeGainListener");
    return handle;
  }

  void ExpectDisconnect() { fixture_->ExpectDisconnects({fixture_->ErrorHandlerFor(binding_)}); }

  using Handler = std::function<void(bool muted, float gain_db)>;

  void SetNextHandler(Handler h) { next_handler_ = std::move(h); }

  // |fuchsia::media::UsageGainListener|
  void OnGainMuteChanged(bool muted, float gain_db, OnGainMuteChangedCallback callback) final {
    if (next_handler_) {
      next_handler_(muted, gain_db);
      next_handler_ = nullptr;
    }
    callback();
  }

 private:
  TestFixture* fixture_;
  fidl::Binding<fuchsia::media::UsageGainListener> binding_;
  Handler next_handler_;
};

class UsageGainReporterTest : public HermeticAudioTest {
 protected:
  // TODO(https://fxbug.dev/42132524): SetMute(true) events are broken
  static constexpr bool kSetMuteWorking = false;

  void SetUp() override {
    HermeticAudioTest::SetUp();

    // We need to create an output device to listen on.
    // The specific choice of format doesn't matter here, any format will do.
    constexpr auto kSampleFormat = fuchsia::media::AudioSampleFormat::SIGNED_16;
    constexpr auto kSampleRate = 48000;
    auto format = Format::Create<kSampleFormat>(2, kSampleRate).value();
    CreateOutput(device_id_array_, format, kSampleRate /* 1s buffer */);
  }

 public:
  struct Controller {
    explicit Controller(TestFixture* fixture) : fake_listener(fixture) {}

    fuchsia::media::audio::VolumeControlPtr volume_control;
    fuchsia::media::UsageGainReporterPtr gain_reporter;
    FakeGainListener fake_listener;
  };

  std::unique_ptr<Controller> CreateControllerWithRenderUsage(AudioRenderUsage2 render_usage) {
    std::unique_ptr<media::audio::test::UsageGainReporterTest::Controller> c;
    auto usage1 = fuchsia::media::Usage::WithRenderUsage(*ToFidlRenderUsageTry(render_usage));
    auto usage = fuchsia::media::Usage2::WithRenderUsage(fidl::Clone(render_usage));
    c = std::make_unique<Controller>(this);
    audio_core_->BindUsageVolumeControl2(std::move(usage), c->volume_control.NewRequest());
    AddErrorHandler(c->volume_control, "VolumeControl");

    realm().Connect(c->gain_reporter.NewRequest());
    AddErrorHandler(c->gain_reporter, "GainReporter");
    c->gain_reporter->RegisterListener(device_id_string_, std::move(usage1),
                                       c->fake_listener.NewBinding());

    return c;
  }

  std::unique_ptr<Controller> CreateControllerWithRenderUsage2(AudioRenderUsage2 render_usage) {
    std::unique_ptr<media::audio::test::UsageGainReporterTest::Controller> c;
    auto usage = fuchsia::media::Usage2::WithRenderUsage(fidl::Clone(render_usage));
    c = std::make_unique<Controller>(this);
    audio_core_->BindUsageVolumeControl2(std::move(usage), c->volume_control.NewRequest());
    AddErrorHandler(c->volume_control, "VolumeControl");

    realm().Connect(c->gain_reporter.NewRequest());
    AddErrorHandler(c->gain_reporter, "GainReporter");
    auto usage2 = fuchsia::media::Usage2::WithRenderUsage(fidl::Clone(render_usage));
    c->gain_reporter->RegisterListener2(device_id_string_, std::move(usage2),
                                        c->fake_listener.NewBinding());

    return c;
  }

  std::unique_ptr<Controller> CreateControllerWithCaptureUsage(
      fuchsia::media::AudioCaptureUsage capture_usage) {
    std::unique_ptr<media::audio::test::UsageGainReporterTest::Controller> c;
    auto usage = fuchsia::media::Usage::WithCaptureUsage(fidl::Clone(capture_usage));
    c = std::make_unique<Controller>(this);

    realm().Connect(c->gain_reporter.NewRequest());
    AddErrorHandler(c->gain_reporter, "GainReporter");
    c->gain_reporter->RegisterListener(device_id_string_, std::move(usage),
                                       c->fake_listener.NewBinding());

    return c;
  }

  std::unique_ptr<Controller> CreateControllerWithCaptureUsage2(AudioCaptureUsage2 capture_usage) {
    std::unique_ptr<media::audio::test::UsageGainReporterTest::Controller> c;
    auto usage = fuchsia::media::Usage2::WithCaptureUsage(fidl::Clone(capture_usage));
    c = std::make_unique<Controller>(this);

    realm().Connect(c->gain_reporter.NewRequest());
    AddErrorHandler(c->gain_reporter, "GainReporter");
    c->gain_reporter->RegisterListener2(device_id_string_, std::move(usage),
                                        c->fake_listener.NewBinding());

    return c;
  }

  // The device ID is arbitrary.
  const std::string device_id_string_ = "ff000000000000000000000000000000";
  const audio_stream_unique_id_t device_id_array_ = {
      {
          0xff,
          0x00,
      },
  };
};

TEST_F(UsageGainReporterTest, SetVolumeAndMute) {
  auto c = CreateControllerWithRenderUsage(AudioRenderUsage2::MEDIA);

  // The initial callback happens immediately.
  c->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged InitialCall"));
  ExpectCallbacks();

  bool last_muted;
  float last_gain_db;

  auto set_callback = [this, &c, &last_muted, &last_gain_db](const std::string& stage) {
    last_muted = true;
    last_gain_db = kTooHighGainDb;
    c->fake_listener.SetNextHandler(
        AddCallback("OnGainMuteChanged after " + stage,
                    [&last_muted, &last_gain_db](bool muted, float gain_db) {
                      last_muted = muted;
                      last_gain_db = gain_db;
                    }));
  };

  set_callback("SetVolume(0)");
  c->volume_control->SetVolume(0);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, fuchsia::media::audio::MUTED_GAIN_DB);

  set_callback("SetVolume(1)");
  c->volume_control->SetVolume(1);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, 0);

  // TODO(https://fxbug.dev/42132524): SetMute(true) events are broken
  if constexpr (kSetMuteWorking) {
    set_callback("SetMute(true)");
    c->volume_control->SetMute(true);
    ExpectCallbacks();
    EXPECT_TRUE(last_muted);
    EXPECT_FLOAT_EQ(last_gain_db, fuchsia::media::audio::MUTED_GAIN_DB);

    // Unmute should restore the volume.
    set_callback("SetMute(false)");
    c->volume_control->SetMute(false);
    ExpectCallbacks();
    EXPECT_FALSE(last_muted);
    EXPECT_FLOAT_EQ(last_gain_db, 0);
  }
}

TEST_F(UsageGainReporterTest, SetVolumeAndMute2) {
  auto c = CreateControllerWithRenderUsage2(AudioRenderUsage2::MEDIA);

  // The initial callback happens immediately.
  c->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged InitialCall"));
  ExpectCallbacks();

  bool last_muted;
  float last_gain_db;

  auto set_callback = [this, &c, &last_muted, &last_gain_db](const std::string& stage) {
    last_muted = true;
    last_gain_db = kTooHighGainDb;
    c->fake_listener.SetNextHandler(
        AddCallback("OnGainMuteChanged after " + stage,
                    [&last_muted, &last_gain_db](bool muted, float gain_db) {
                      last_muted = muted;
                      last_gain_db = gain_db;
                    }));
  };

  set_callback("SetVolume(0)");
  c->volume_control->SetVolume(0);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, fuchsia::media::audio::MUTED_GAIN_DB);

  set_callback("SetVolume(1)");
  c->volume_control->SetVolume(1);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, 0);

  // TODO(https://fxbug.dev/42132524): SetMute(true) events are broken
  if constexpr (kSetMuteWorking) {
    set_callback("SetMute(true)");
    c->volume_control->SetMute(true);
    ExpectCallbacks();
    EXPECT_TRUE(last_muted);
    EXPECT_FLOAT_EQ(last_gain_db, fuchsia::media::audio::MUTED_GAIN_DB);

    // Unmute should restore the volume.
    set_callback("SetMute(false)");
    c->volume_control->SetMute(false);
    ExpectCallbacks();
    EXPECT_FALSE(last_muted);
    EXPECT_FLOAT_EQ(last_gain_db, 0);
  }
}

TEST_F(UsageGainReporterTest, RoutedCorrectly) {
  auto c1 = CreateControllerWithRenderUsage(AudioRenderUsage2::MEDIA);
  auto c2 = CreateControllerWithRenderUsage(AudioRenderUsage2::BACKGROUND);

  // The initial callbacks happen immediately.
  c1->fake_listener.SetNextHandler(AddCallbackUnordered("OnGainMuteChanged1 InitialCall"));
  c2->fake_listener.SetNextHandler(AddCallbackUnordered("OnGainMuteChanged2 InitialCall"));
  ExpectCallbacks();

  // Routing to c1.
  c1->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged1 RouteTo1"));
  c2->fake_listener.SetNextHandler(AddUnexpectedCallback("OnGainMuteChanged2 RouteTo1"));
  c1->volume_control->SetVolume(0);
  ExpectCallbacks();

  // Routing to c2.
  c1->fake_listener.SetNextHandler(AddUnexpectedCallback("OnGainMuteChanged1 RouteTo2"));
  c2->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged2 RouteTo2"));
  c2->volume_control->SetVolume(0);
  ExpectCallbacks();
}

TEST_F(UsageGainReporterTest, RoutedCorrectly2) {
  auto c1 = CreateControllerWithRenderUsage2(AudioRenderUsage2::MEDIA);
  auto c2 = CreateControllerWithRenderUsage2(AudioRenderUsage2::BACKGROUND);

  // The initial callbacks happen immediately.
  c1->fake_listener.SetNextHandler(AddCallbackUnordered("OnGainMuteChanged1 InitialCall"));
  c2->fake_listener.SetNextHandler(AddCallbackUnordered("OnGainMuteChanged2 InitialCall"));
  ExpectCallbacks();

  // Routing to c1.
  c1->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged1 RouteTo1"));
  c2->fake_listener.SetNextHandler(AddUnexpectedCallback("OnGainMuteChanged2 RouteTo1"));
  c1->volume_control->SetVolume(0);
  ExpectCallbacks();

  // Routing to c2.
  c1->fake_listener.SetNextHandler(AddUnexpectedCallback("OnGainMuteChanged1 RouteTo2"));
  c2->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged2 RouteTo2"));
  c2->volume_control->SetVolume(0);
  ExpectCallbacks();
}

TEST_F(UsageGainReporterTest, SetCaptureUsageGain) {
  auto c = CreateControllerWithCaptureUsage(fuchsia::media::AudioCaptureUsage::SYSTEM_AGENT);

  // The initial callback happens immediately.
  c->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged InitialCall"));
  ExpectCallbacks();

  bool last_muted;
  float last_gain_db, capture_usage_gain_db;
  auto set_callback = [this, &c, &last_muted, &last_gain_db](const std::string& last_action) {
    last_muted = true;
    last_gain_db = kTooHighGainDb;
    c->fake_listener.SetNextHandler(
        AddCallback("OnGainMuteChanged after " + last_action,
                    [&last_muted, &last_gain_db](bool muted, float gain_db) {
                      last_muted = muted;
                      last_gain_db = gain_db;
                    }));
  };

  capture_usage_gain_db = -60.0f;
  set_callback("SetCaptureUsageGain-1");
  audio_core_->SetCaptureUsageGain(fuchsia::media::AudioCaptureUsage::SYSTEM_AGENT,
                                   capture_usage_gain_db);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, capture_usage_gain_db);

  capture_usage_gain_db = -20.0f;
  set_callback("SetCaptureUsageGain-2");
  audio_core_->SetCaptureUsageGain(fuchsia::media::AudioCaptureUsage::SYSTEM_AGENT,
                                   capture_usage_gain_db);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, capture_usage_gain_db);
}

TEST_F(UsageGainReporterTest, SetCaptureUsageGain2) {
  auto c = CreateControllerWithCaptureUsage2(AudioCaptureUsage2::SYSTEM_AGENT);

  // The initial callback happens immediately.
  c->fake_listener.SetNextHandler(AddCallback("OnGainMuteChanged InitialCall"));
  ExpectCallbacks();

  bool last_muted;
  float last_gain_db, capture_usage_gain_db;
  auto set_callback = [this, &c, &last_muted, &last_gain_db](const std::string& last_action) {
    last_muted = true;
    last_gain_db = kTooHighGainDb;
    c->fake_listener.SetNextHandler(
        AddCallback("OnGainMuteChanged after " + last_action,
                    [&last_muted, &last_gain_db](bool muted, float gain_db) {
                      last_muted = muted;
                      last_gain_db = gain_db;
                    }));
  };

  capture_usage_gain_db = -60.0f;
  set_callback("SetCaptureUsageGain2-1");
  audio_core_->SetCaptureUsageGain2(AudioCaptureUsage2::SYSTEM_AGENT, capture_usage_gain_db);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, capture_usage_gain_db);

  capture_usage_gain_db = -20.0f;
  set_callback("SetCaptureUsageGain2-2");
  audio_core_->SetCaptureUsageGain2(AudioCaptureUsage2::SYSTEM_AGENT, capture_usage_gain_db);
  ExpectCallbacks();
  EXPECT_FALSE(last_muted);
  EXPECT_FLOAT_EQ(last_gain_db, capture_usage_gain_db);
}

// When RegisterListener2 is called with an unknown RenderUsage2 enum, the passed-in listener should
// disconnect without causing the preexisting UsageGainReporter connection to disconnect.
TEST_F(UsageGainReporterTest, RegisterListener2UnknownFlexibleEnumRenderUsage2) {
  FakeGainListener fake_listener(this);
  fuchsia::media::UsageGainReporterPtr gain_reporter;
  realm().Connect(gain_reporter.NewRequest());
  AddErrorHandler(gain_reporter, "GainReporter");

  auto usage = fuchsia::media::Usage2::WithRenderUsage(static_cast<AudioRenderUsage2>(68));
  gain_reporter->RegisterListener2(device_id_string_, std::move(usage), fake_listener.NewBinding());
  fake_listener.ExpectDisconnect();
  EXPECT_TRUE(gain_reporter.is_bound());
}

// When RegisterListener2 is called with an unknown CaptureUsage2 enum, the passed-in listener
// should disconnect without causing the preexisting UsageGainReporter connection to disconnect.
TEST_F(UsageGainReporterTest, RegisterListener2UnknownFlexibleEnumCaptureUsage2) {
  FakeGainListener fake_listener(this);
  fuchsia::media::UsageGainReporterPtr gain_reporter;
  realm().Connect(gain_reporter.NewRequest());
  AddErrorHandler(gain_reporter, "GainReporter");

  auto usage = fuchsia::media::Usage2::WithCaptureUsage(static_cast<AudioCaptureUsage2>(42));
  gain_reporter->RegisterListener2(device_id_string_, std::move(usage), fake_listener.NewBinding());
  fake_listener.ExpectDisconnect();
  EXPECT_TRUE(gain_reporter.is_bound());
}

}  // namespace

}  // namespace media::audio::test

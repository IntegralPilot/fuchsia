// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.media/cpp/common_types.h>
#include <fidl/fuchsia.media/cpp/markers.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <zircon/errors.h>

#include <iterator>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

#include "src/media/audio/consumer/consumer.h"
#include "src/media/audio/consumer/test/fake_audio_core.h"
#include "src/media/audio/consumer/test/fake_audio_renderer.h"
#include "src/media/audio/consumer/test/fake_gain_control.h"
#include "src/media/audio/consumer/test/get_koid.h"

namespace media::audio::tests {

class AudioConsumerTests : public gtest::RealLoopFixture {
 protected:
  static constexpr size_t kBufferCount = 4;
  static constexpr size_t kBufferSize = 4096;
  static constexpr fuchsia_media::AudioSampleFormat kSampleFormat =
      fuchsia_media::AudioSampleFormat::kFloat;
  static constexpr uint32_t kChannels = 2;
  static constexpr uint32_t kFramesPerSecond = 48000;
  static constexpr int64_t kPts = 1234;
  static constexpr uint32_t kPayloadBufferId = 2;
  static constexpr uint64_t kPayloadOffset = 234;
  static constexpr uint64_t kPayloadSize = 8192;
  static constexpr uint32_t kFlags = 7;
  static constexpr uint64_t kBufferConfig = 345;
  static constexpr uint64_t kStreamSegmentId = 456;
  static constexpr int64_t kReferenceTime = 12345678;
  static constexpr int64_t kMediaTime = 1000;
  static constexpr float kVolume = 20.0f;
  static constexpr float kGain = 1.2f;
  static constexpr bool kMute = true;
  static constexpr uint64_t kMinLeadTime = ZX_MSEC(30);
  static constexpr uint64_t kMaxLeadTime = ZX_MSEC(500);

  AudioConsumerTests()
      : consumer_event_handler_(
            [this](fidl::UnbindInfo unbind_info) { consumer_unbind_artifact_ = unbind_info; }),
        stream_sink_event_handler_(
            [this](fidl::UnbindInfo unbind_info) { stream_sink_unbind_artifact_ = unbind_info; }),
        volume_control_event_handler_([this](fidl::UnbindInfo unbind_info) {
          volume_control_unbind_artifact_ = unbind_info;
        }) {}

  FakeAudioCore& fake_audio_core() {
    EXPECT_TRUE(fake_audio_core_);
    return *fake_audio_core_;
  }

  FakeAudioRenderer& fake_audio_renderer() {
    EXPECT_TRUE(fake_audio_renderer_);
    return *fake_audio_renderer_;
  }

  FakeGainControl& fake_gain_control() {
    EXPECT_TRUE(fake_gain_control_);
    return *fake_gain_control_;
  }

  fidl::Client<fuchsia_media::AudioConsumer>& consumer_under_test() { return consumer_under_test_; }

  fidl::Client<fuchsia_media::StreamSink>& stream_sink_under_test() {
    return stream_sink_under_test_;
  }

  fidl::Client<fuchsia_media_audio::VolumeControl>& volume_control_under_test() {
    return volume_control_under_test_;
  }

  std::optional<fidl::UnbindInfo> consumer_under_test_unbound() {
    auto result = consumer_unbind_artifact_;
    consumer_unbind_artifact_.reset();
    return result;
  }

  std::optional<fidl::UnbindInfo> stream_sink_under_test_unbound() {
    auto result = stream_sink_unbind_artifact_;
    stream_sink_unbind_artifact_.reset();
    return result;
  }

  std::optional<fidl::UnbindInfo> volume_control_under_test_unbound() {
    auto result = volume_control_unbind_artifact_;
    volume_control_unbind_artifact_.reset();
    return result;
  }

  bool CreateConsumerUnderTest() {
    // Create a |FakeAudioCore| instance.
    auto audio_core_endpoints = fidl::Endpoints<fuchsia_media::AudioCore>::Create();

    fake_audio_core_ =
        std::make_unique<FakeAudioCore>(dispatcher(), std::move(audio_core_endpoints.server));

    // Create and bind to a |Consumer| instance.
    zx::result audio_consumer_endpoints = fidl::CreateEndpoints<fuchsia_media::AudioConsumer>();
    EXPECT_TRUE(audio_consumer_endpoints.is_ok());
    if (!audio_consumer_endpoints.is_ok()) {
      return false;
    }

    Consumer::CreateAndBind(dispatcher(), std::move(audio_core_endpoints.client),
                            std::move(audio_consumer_endpoints->server));
    RunLoopUntilIdle();

    consumer_under_test_ = fidl::Client(std::move(audio_consumer_endpoints->client), dispatcher(),
                                        &consumer_event_handler_);

    // Expect that |CreateAudioRenderer| has been called on |FakeAudioCore|.
    fake_audio_renderer_ = fake_audio_core().WasCreateAudioRendererCalled();
    EXPECT_TRUE(fake_audio_renderer_);
    if (!fake_audio_renderer_) {
      return false;
    }

    // Expect that usage was set on the renderer.
    EXPECT_TRUE(
        fake_audio_renderer().ExpectSetUsage2Called(fuchsia_media::AudioRenderUsage2::kMedia));

    return true;
  }

  bool CreateStreamSink(size_t buffer_count = kBufferCount, size_t buffer_size = kBufferSize,
                        fuchsia_media::AudioSampleFormat sample_format = kSampleFormat,
                        uint32_t channels = kChannels,
                        uint32_t frames_per_second = kFramesPerSecond) {
    std::vector<zx::vmo> buffers(buffer_count);
    std::vector<zx_koid_t> buffer_koids;
    for (auto& buffer : buffers) {
      EXPECT_EQ(ZX_OK, zx::vmo::create(buffer_size, 0, &buffer));
      buffer_koids.push_back(GetKoid(buffer));
    }

    fuchsia_media::AudioStreamType stream_type{{
        .sample_format = sample_format,
        .channels = channels,
        .frames_per_second = frames_per_second,
    }};

    auto stream_sink_endpoints = fidl::Endpoints<fuchsia_media::StreamSink>::Create();

    fit::result result = consumer_under_test()->CreateStreamSink({{
        .buffers = std::move(buffers),
        .stream_type = std::move(stream_type),
        .compression = nullptr,
        .stream_sink_request = std::move(stream_sink_endpoints.server),
    }});
    EXPECT_TRUE(result.is_ok());
    if (!result.is_ok()) {
      return false;
    }

    stream_sink_under_test_ = fidl::Client(std::move(stream_sink_endpoints.client), dispatcher(),
                                           &stream_sink_event_handler_);
    RunLoopUntilIdle();

    // Expect that the stream type was set on the renderer.
    EXPECT_TRUE(fake_audio_renderer().ExpectSetPcmStreamTypeCalled({{
        .sample_format = sample_format,
        .channels = channels,
        .frames_per_second = frames_per_second,
    }}));

    // Expect that the provided buffers were added to the renderer.
    for (uint32_t i = 0; i < buffer_count; ++i) {
      EXPECT_TRUE(fake_audio_renderer().ExpectAddPayloadBufferCalled(i, buffer_koids[i]));
    }

    EXPECT_TRUE(fake_audio_renderer().ExpectNoOtherCalled());

    return true;
  }

  void CreateVolumeControl() {
    auto endpoints = fidl::Endpoints<fuchsia_media_audio::VolumeControl>::Create();
    EXPECT_TRUE(consumer_under_test()
                    ->BindVolumeControl({{
                        .volume_control_request = std::move(endpoints.server),
                    }})
                    .is_ok());

    volume_control_under_test_ =
        fidl::Client(std::move(endpoints.client), dispatcher(), &volume_control_event_handler_);
    RunLoopUntilIdle();

    fake_gain_control_ = fake_audio_renderer().ExpectBindGainControlCalled();
    EXPECT_TRUE(fake_gain_control_);
  }

  void CleanUp() {
    consumer_under_test_ = fidl::Client<fuchsia_media::AudioConsumer>();
    stream_sink_under_test_ = fidl::Client<fuchsia_media::StreamSink>();
    volume_control_under_test_ = fidl::Client<fuchsia_media_audio::VolumeControl>();
    RunLoopUntilIdle();

    if (fake_audio_core_) {
      fake_audio_core_->Unbind();
    }

    if (fake_audio_renderer_) {
      fake_audio_renderer_->Unbind();
    }

    if (fake_gain_control_) {
      fake_gain_control_->Unbind();
    }

    RunLoopUntil([this]() {
      return (!fake_audio_core_ || fake_audio_core_->UnbindCompleted()) &&
             (!fake_audio_renderer_ || fake_audio_renderer_->UnbindCompleted()) &&
             (!fake_gain_control_ || fake_gain_control_->UnbindCompleted());
    });

    fake_audio_core_.reset();
    fake_audio_renderer_.reset();
    fake_gain_control_.reset();
  }

  template <typename Protocol>
  class AsyncEventHandler : public fidl::AsyncEventHandler<Protocol> {
   public:
    explicit AsyncEventHandler(fit::function<void(fidl::UnbindInfo)> fidl_error_callback)
        : fidl_error_callback_(std::move(fidl_error_callback)) {}

    void on_fidl_error(fidl::UnbindInfo error) override { fidl_error_callback_(error); }

   private:
    fit::function<void(fidl::UnbindInfo)> fidl_error_callback_;
  };

 private:
  std::unique_ptr<FakeAudioCore> fake_audio_core_;
  std::unique_ptr<FakeAudioRenderer> fake_audio_renderer_;
  std::unique_ptr<FakeGainControl> fake_gain_control_;

  fidl::Client<fuchsia_media::AudioConsumer> consumer_under_test_;
  AsyncEventHandler<fuchsia_media::AudioConsumer> consumer_event_handler_;
  std::optional<fidl::UnbindInfo> consumer_unbind_artifact_;

  fidl::Client<fuchsia_media::StreamSink> stream_sink_under_test_;
  AsyncEventHandler<fuchsia_media::StreamSink> stream_sink_event_handler_;
  std::optional<fidl::UnbindInfo> stream_sink_unbind_artifact_;

  fidl::Client<fuchsia_media_audio::VolumeControl> volume_control_under_test_;
  AsyncEventHandler<fuchsia_media_audio::VolumeControl> volume_control_event_handler_;
  std::optional<fidl::UnbindInfo> volume_control_unbind_artifact_;
};

// Tests that |AudioConsumer| and |StreamSink| creation work properly.
TEST_F(AudioConsumerTests, CreateStreamSink) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CleanUp();
}

// Tests the sequencing of overlapping stream sink lifetimes.
TEST_F(AudioConsumerTests, CreateStreamSinkTwice) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  std::vector<zx::vmo> buffers(kBufferCount);
  std::vector<zx_koid_t> buffer_koids;
  for (auto& buffer : buffers) {
    EXPECT_EQ(ZX_OK, zx::vmo::create(kBufferSize, 0, &buffer));
    buffer_koids.push_back(GetKoid(buffer));
  }

  fuchsia_media::AudioStreamType stream_type{{.sample_format = kSampleFormat,
                                              .channels = kChannels,
                                              .frames_per_second = kFramesPerSecond}};

  auto stream_sink_endpoints = fidl::Endpoints<fuchsia_media::StreamSink>::Create();
  fit::result result = consumer_under_test()->CreateStreamSink({{
      .buffers = std::move(buffers),
      .stream_type = std::move(stream_type),
      .compression = nullptr,
      .stream_sink_request = std::move(stream_sink_endpoints.server),
  }});
  ASSERT_TRUE(result.is_ok());

  auto second_stream_sink_under_test =
      fidl::Client(std::move(stream_sink_endpoints.client), dispatcher());
  RunLoopUntilIdle();

  // Expect that the renderer has not yet been updated, because that operation is pending the
  // destruction of the first stream sink.
  EXPECT_TRUE(fake_audio_renderer().ExpectNoOtherCalled());

  // Destroy the first stream sink.
  stream_sink_under_test() = fidl::Client<fuchsia_media::StreamSink>();
  RunLoopUntilIdle();

  // Expect that the audio renderer has been prepared for the new stream sink.
  EXPECT_TRUE(fake_audio_renderer().ExpectSetPcmStreamTypeCalled({{
      .sample_format = kSampleFormat,
      .channels = kChannels,
      .frames_per_second = kFramesPerSecond,
  }}));

  for (uint32_t i = 0; i < kBufferCount; ++i) {
    EXPECT_TRUE(fake_audio_renderer().ExpectAddPayloadBufferCalled(i, buffer_koids[i]));
  }

  EXPECT_TRUE(fake_audio_renderer().ExpectNoOtherCalled());

  CleanUp();
}

// Tests stream sink method |SendPacket|.
TEST_F(AudioConsumerTests, StreamSinkSendPacket) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  bool send_packet_completed = false;
  stream_sink_under_test()
      ->SendPacket({{.packet = {{
                         .pts = kPts,
                         .payload_buffer_id = kPayloadBufferId,
                         .payload_offset = kPayloadOffset,
                         .payload_size = kPayloadSize,
                         .flags = kFlags,
                         .buffer_config = kBufferConfig,
                         .stream_segment_id = kStreamSegmentId,
                     }}}})
      .Then([&send_packet_completed](auto& result) {
        EXPECT_TRUE(result.is_ok());
        send_packet_completed = true;
      });
  RunLoopUntil([&send_packet_completed]() { return send_packet_completed; });

  fake_audio_renderer().ExpectSendPacketCalled({{
      .pts = kPts,
      .payload_buffer_id = kPayloadBufferId,
      .payload_offset = kPayloadOffset,
      .payload_size = kPayloadSize,
      .flags = kFlags,
      .buffer_config = kBufferConfig,
      .stream_segment_id = kStreamSegmentId,
  }});

  CleanUp();
}

// Tests stream sink method |SendPacketNoReply|.
TEST_F(AudioConsumerTests, StreamSinkSendPacketNoReply) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  EXPECT_TRUE(stream_sink_under_test()
                  ->SendPacketNoReply({{.packet = {{
                                            .pts = kPts,
                                            .payload_buffer_id = kPayloadBufferId,
                                            .payload_offset = kPayloadOffset,
                                            .payload_size = kPayloadSize,
                                            .flags = kFlags,
                                            .buffer_config = kBufferConfig,
                                            .stream_segment_id = kStreamSegmentId,
                                        }}}})
                  .is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectSendPacketNoReplyCalled({{
      .pts = kPts,
      .payload_buffer_id = kPayloadBufferId,
      .payload_offset = kPayloadOffset,
      .payload_size = kPayloadSize,
      .flags = kFlags,
      .buffer_config = kBufferConfig,
      .stream_segment_id = kStreamSegmentId,
  }});

  CleanUp();
}

// Tests stream sink method |EndOfStream|.
TEST_F(AudioConsumerTests, StreamSinkEndOfStream) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  EXPECT_TRUE(stream_sink_under_test()->EndOfStream().is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectEndOfStreamCalled();

  CleanUp();
}

// Tests stream sink method |SendPacket|.
TEST_F(AudioConsumerTests, StreamSinkDiscardAllPackets) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  bool discard_all_packets_completed = false;
  stream_sink_under_test()->DiscardAllPackets().Then(
      [&discard_all_packets_completed](auto& result) {
        EXPECT_TRUE(result.is_ok());
        discard_all_packets_completed = true;
      });
  RunLoopUntil([&discard_all_packets_completed]() { return discard_all_packets_completed; });

  fake_audio_renderer().ExpectDiscardAllPacketsCalled();

  CleanUp();
}

// Tests stream sink method |DiscardAllPacketsNoReply|.
TEST_F(AudioConsumerTests, StreamSinkDiscardAllPacketsNoReply) {
  CreateConsumerUnderTest();
  CreateStreamSink();

  EXPECT_TRUE(stream_sink_under_test()->DiscardAllPacketsNoReply().is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectDiscardAllPacketsNoReplyCalled();

  CleanUp();
}

// Tests consumer method |Start|.
TEST_F(AudioConsumerTests, ConsumerStart) {
  CreateConsumerUnderTest();

  EXPECT_TRUE(consumer_under_test()
                  ->Start({{
                      .reference_time = kReferenceTime,
                      .media_time = kMediaTime,
                  }})
                  .is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectPlayNoReplyCalled(kReferenceTime, kMediaTime);

  CleanUp();
}

// Tests consumer method |Stop|.
TEST_F(AudioConsumerTests, ConsumerStop) {
  CreateConsumerUnderTest();

  EXPECT_TRUE(consumer_under_test()->Stop().is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectPauseNoReplyCalled();

  CleanUp();
}

// Tests consumer method |BindVolumeControl|.
TEST_F(AudioConsumerTests, BindVolumeControl) {
  CreateConsumerUnderTest();
  CreateVolumeControl();

  EXPECT_TRUE(volume_control_under_test()->SetVolume({{.volume = kVolume}}).is_ok());
  RunLoopUntilIdle();

  const fuchsia_media::Usage2 kUsage =
      fuchsia_media::Usage2::WithRenderUsage(fuchsia_media::AudioRenderUsage2::kMedia);

  fake_audio_core().WasGetDbFromVolumeCalled(kUsage, kVolume, kGain);
  RunLoopUntilIdle();

  EXPECT_TRUE(fake_gain_control().WasSetGainCalled(kGain));

  EXPECT_TRUE(volume_control_under_test()->SetMute({{.mute = kMute}}).is_ok());
  RunLoopUntilIdle();

  EXPECT_TRUE(fake_gain_control().WasSetMuteCalled(kMute));

  CleanUp();
}

// Tests consumer method |WatchStatus|.
TEST_F(AudioConsumerTests, ConsumerWatchStatus) {
  CreateConsumerUnderTest();

  bool watch_status_completed = false;
  consumer_under_test()->WatchStatus().Then([&watch_status_completed](auto& result) {
    EXPECT_TRUE(result.is_ok());
    EXPECT_FALSE(result.value().status().error().has_value());
    EXPECT_TRUE(result.value().status().presentation_timeline().has_value());
    if (result.value().status().presentation_timeline().has_value()) {
      EXPECT_EQ(0, result.value().status().presentation_timeline()->subject_time());
      EXPECT_EQ(0, result.value().status().presentation_timeline()->reference_time());
      EXPECT_EQ(0u, result.value().status().presentation_timeline()->subject_delta());
      EXPECT_EQ(1u, result.value().status().presentation_timeline()->reference_delta());
    }

    EXPECT_EQ(kMinLeadTime, result.value().status().min_lead_time());
    EXPECT_EQ(kMaxLeadTime, result.value().status().max_lead_time());

    watch_status_completed = true;
  });
  RunLoopUntilIdle();

  // Expect the first |WatchStatus| call to complete immediately.
  EXPECT_TRUE(watch_status_completed);

  watch_status_completed = false;
  consumer_under_test()->WatchStatus().Then([&watch_status_completed](auto& result) {
    EXPECT_TRUE(result.is_ok());
    EXPECT_FALSE(result.value().status().error().has_value());
    EXPECT_TRUE(result.value().status().presentation_timeline().has_value());
    if (result.value().status().presentation_timeline().has_value()) {
      EXPECT_EQ(kMediaTime, result.value().status().presentation_timeline()->subject_time());
      EXPECT_EQ(kReferenceTime, result.value().status().presentation_timeline()->reference_time());
      EXPECT_EQ(1u, result.value().status().presentation_timeline()->subject_delta());
      EXPECT_EQ(1u, result.value().status().presentation_timeline()->reference_delta());
    }

    EXPECT_EQ(kMinLeadTime, result.value().status().min_lead_time());
    EXPECT_EQ(kMaxLeadTime, result.value().status().max_lead_time());

    watch_status_completed = true;
  });
  RunLoopUntilIdle();

  // Expect the second |WatchStatus| call to hang until a change occurs.
  EXPECT_FALSE(watch_status_completed);

  EXPECT_TRUE(consumer_under_test()
                  ->Start({{
                      .reference_time = kReferenceTime,
                      .media_time = kMediaTime,
                  }})
                  .is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectPlayNoReplyCalled(kReferenceTime, kMediaTime);

  // Expect the second |WatchStatus| call to complete now that the consumer has started.
  EXPECT_TRUE(watch_status_completed);

  watch_status_completed = false;
  consumer_under_test()->WatchStatus().Then([&watch_status_completed](auto& result) {
    EXPECT_TRUE(result.is_ok());
    EXPECT_FALSE(result->status().error().has_value());
    EXPECT_TRUE(result.value().status().presentation_timeline().has_value());
    if (result.value().status().presentation_timeline().has_value()) {
      // Reference time should be roughly 'now', but setting a limit on how roughly will just
      // produce flakes. We know what to expect in terms of the correlation between reference
      // time and subject time.
      int64_t expected_media_time =
          kMediaTime + result.value().status().presentation_timeline()->reference_time() -
          kReferenceTime;
      EXPECT_EQ(expected_media_time,
                result.value().status().presentation_timeline()->subject_time());
      EXPECT_EQ(0u, result.value().status().presentation_timeline()->subject_delta());
      EXPECT_EQ(1u, result.value().status().presentation_timeline()->reference_delta());
    }

    EXPECT_EQ(kMinLeadTime, result.value().status().min_lead_time());
    EXPECT_EQ(kMaxLeadTime, result.value().status().max_lead_time());

    watch_status_completed = true;
  });
  RunLoopUntilIdle();

  // Expect the third |WatchStatus| call to hang until a change occurs.
  EXPECT_FALSE(watch_status_completed);

  EXPECT_TRUE(consumer_under_test()->Stop().is_ok());
  RunLoopUntilIdle();

  fake_audio_renderer().ExpectPauseNoReplyCalled();

  // Expect the third |WatchStatus| call to complete now that the consumer has stopped.
  EXPECT_TRUE(watch_status_completed);

  CleanUp();
}

// Tests consumer behavior when the consumer connection is closed.
TEST_F(AudioConsumerTests, CloseConsumer) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |AudioConsumer| connection.
  consumer_under_test() = fidl::Client<fuchsia_media::AudioConsumer>();
  RunLoopUntilIdle();

  // Expect everything to be unbound.
  EXPECT_TRUE(fake_audio_core().UnbindCompleted());
  EXPECT_TRUE(fake_audio_renderer().UnbindCompleted());
  EXPECT_TRUE(fake_gain_control().UnbindCompleted());

  // |consumer_under_test_unbound()| returns false, because we closed the connection.
  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_TRUE(stream_sink_under_test_unbound());
  EXPECT_TRUE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests consumer behavior when the stream sink connection is closed.
TEST_F(AudioConsumerTests, CloseStreamSink) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |StreamSink| connection.
  stream_sink_under_test() = fidl::Client<fuchsia_media::StreamSink>();
  RunLoopUntilIdle();

  // Expect nothing to be unbound, aside from the |StreamSink|.
  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  // |stream_sink_under_test_unbound()| returns false, because we closed the connection.
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests consumer behavior when the volume control connection is closed.
TEST_F(AudioConsumerTests, CloseVolumeControl) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |VolumeControl| connection.
  volume_control_under_test() = fidl::Client<fuchsia_media_audio::VolumeControl>();
  RunLoopUntilIdle();

  // Expect only the |GainControl| to be unbound, aside from the |VolumeControl|.
  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_TRUE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  // |volume_control_under_test_unbound()| returns false, because we closed the connection.
  EXPECT_FALSE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests behavior when the |AudioCore| connection unexpectedly closes.
TEST_F(AudioConsumerTests, AudioCoreUnbound) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |AudioCore| connection unexpectedly.
  fake_audio_core().Unbind();
  RunLoopUntilIdle();

  // Expect everything to be disconnected.
  EXPECT_TRUE(fake_audio_core().UnbindCompleted());
  EXPECT_TRUE(fake_audio_renderer().UnbindCompleted());
  EXPECT_TRUE(fake_gain_control().UnbindCompleted());

  EXPECT_TRUE(consumer_under_test_unbound());
  EXPECT_TRUE(stream_sink_under_test_unbound());
  EXPECT_TRUE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests behavior when the |AudioRenderer| connection unexpectedly closes.
TEST_F(AudioConsumerTests, AudioRendererUnbound) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |AudioRenderer| connection unexpectedly.
  fake_audio_renderer().Unbind();
  RunLoopUntilIdle();

  // Expect everything to be disconnected.
  EXPECT_TRUE(fake_audio_core().UnbindCompleted());
  EXPECT_TRUE(fake_audio_renderer().UnbindCompleted());
  EXPECT_TRUE(fake_gain_control().UnbindCompleted());

  EXPECT_TRUE(consumer_under_test_unbound());
  EXPECT_TRUE(stream_sink_under_test_unbound());
  EXPECT_TRUE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests behavior when the |GainControl| connection unexpectedly closes.
TEST_F(AudioConsumerTests, GainControlUnbound) {
  CreateConsumerUnderTest();
  CreateStreamSink();
  CreateVolumeControl();

  EXPECT_FALSE(fake_audio_core().UnbindCompleted());
  EXPECT_FALSE(fake_audio_renderer().UnbindCompleted());
  EXPECT_FALSE(fake_gain_control().UnbindCompleted());

  EXPECT_FALSE(consumer_under_test_unbound());
  EXPECT_FALSE(stream_sink_under_test_unbound());
  EXPECT_FALSE(volume_control_under_test_unbound());

  // Close the |GainControl| connection unexpectedly.
  fake_gain_control().Unbind();
  RunLoopUntilIdle();

  // Expect everything to be disconnected.
  EXPECT_TRUE(fake_audio_core().UnbindCompleted());
  EXPECT_TRUE(fake_audio_renderer().UnbindCompleted());
  EXPECT_TRUE(fake_gain_control().UnbindCompleted());

  EXPECT_TRUE(consumer_under_test_unbound());
  EXPECT_TRUE(stream_sink_under_test_unbound());
  EXPECT_TRUE(volume_control_under_test_unbound());

  CleanUp();
}

// Tests that compressed formats are explicitly rejected.
TEST_F(AudioConsumerTests, RejectCompressed) {
  CreateConsumerUnderTest();

  std::vector<zx::vmo> buffers(kBufferCount);
  std::vector<zx_koid_t> buffer_koids;
  for (auto& buffer : buffers) {
    EXPECT_EQ(ZX_OK, zx::vmo::create(kBufferSize, 0, &buffer));
    buffer_koids.push_back(GetKoid(buffer));
  }

  fuchsia_media::AudioStreamType stream_type{{
      .sample_format = kSampleFormat,
      .channels = kChannels,
      .frames_per_second = kFramesPerSecond,
  }};

  auto stream_sink_endpoints = fidl::Endpoints<fuchsia_media::StreamSink>::Create();

  auto compression = std::make_unique<fuchsia_media::Compression>();
  compression->type() = fuchsia_media::kAudioEncodingAac;

  fit::result result = consumer_under_test()->CreateStreamSink({{
      .buffers = std::move(buffers),
      .stream_type = std::move(stream_type),
      .compression = std::move(compression),
      .stream_sink_request = std::move(stream_sink_endpoints.server),
  }});
  EXPECT_TRUE(result.is_ok());

  zx_status_t unbind_status = ZX_OK;
  AsyncEventHandler<fuchsia_media::StreamSink> stream_sink_event_handler(
      [&unbind_status](fidl::UnbindInfo unbind_info) { unbind_status = unbind_info.status(); });

  fidl::Client<fuchsia_media::StreamSink> stream_sink = fidl::Client(
      std::move(stream_sink_endpoints.client), dispatcher(), &stream_sink_event_handler);

  RunLoopUntilIdle();

  // Expect that the stream sink has been closed with epitaph |ZX_ERR_INVALID_ARGS|.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, unbind_status);

  CleanUp();
}

}  // namespace media::audio::tests

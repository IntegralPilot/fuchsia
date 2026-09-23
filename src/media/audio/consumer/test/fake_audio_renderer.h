// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_RENDERER_H_
#define SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_RENDERER_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <fidl/fuchsia.media/cpp/natural_types.h>
#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>

#include <list>
#include <memory>
#include <queue>

#include <gtest/gtest.h>

#include "src/media/audio/consumer/test/fake_gain_control.h"
#include "src/media/audio/consumer/test/get_koid.h"

namespace media::audio::tests {

class FakeAudioRenderer : public fidl::Server<fuchsia_media::AudioRenderer> {
 public:
  FakeAudioRenderer(async_dispatcher_t* dispatcher,
                    fidl::ServerEnd<fuchsia_media::AudioRenderer> server_end)
      : dispatcher_(dispatcher) {
    binding_ref_ = fidl::BindServer(
        dispatcher, std::move(server_end), this,
        [this](fidl::Server<fuchsia_media::AudioRenderer>* impl, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_media::AudioRenderer> server_end) {
          unbind_completed_ = true;
        });
  }

  ~FakeAudioRenderer() override = default;

  // Disallow copy, assign and move.
  FakeAudioRenderer(const FakeAudioRenderer&) = delete;
  FakeAudioRenderer& operator=(const FakeAudioRenderer&) = delete;
  FakeAudioRenderer(FakeAudioRenderer&&) = delete;
  FakeAudioRenderer& operator=(FakeAudioRenderer&&) = delete;

  void Unbind() {
    if (binding_ref_) {
      binding_ref_->Unbind();
    }
  }

  bool UnbindCompleted() const { return unbind_completed_; }

  // fuchsia_media::AudioRenderer implementation.
  void AddPayloadBuffer(AddPayloadBufferRequest& request,
                        AddPayloadBufferCompleter::Sync& completer) override {
    add_payload_buffer_artifacts_.push(
        {.id = request.id(), .payload_buffer_koid = GetKoid(request.payload_buffer())});
  }

  void RemovePayloadBuffer(RemovePayloadBufferRequest& request,
                           RemovePayloadBufferCompleter::Sync& completer) override {
    remove_payload_buffer_artifacts_.push(request.id());
  }

  void SendPacket(SendPacketRequest& request, SendPacketCompleter::Sync& completer) override {
    EXPECT_FALSE(send_packet_artifact_.has_value());
    send_packet_artifact_ = std::move(request.packet());
    completer.Reply();
  }

  void SendPacketNoReply(SendPacketNoReplyRequest& request,
                         SendPacketNoReplyCompleter::Sync& completer) override {
    EXPECT_FALSE(send_packet_no_reply_artifact_.has_value());
    send_packet_no_reply_artifact_ = std::move(request.packet());
  }

  void EndOfStream(EndOfStreamCompleter::Sync& completer) override {
    EXPECT_FALSE(end_of_stream_called_);
    end_of_stream_called_ = true;
  }

  void DiscardAllPackets(DiscardAllPacketsCompleter::Sync& completer) override {
    EXPECT_FALSE(discard_all_packets_called_);
    discard_all_packets_called_ = true;
    completer.Reply();
  }

  void DiscardAllPacketsNoReply(DiscardAllPacketsNoReplyCompleter::Sync& completer) override {
    EXPECT_FALSE(discard_all_packets_no_reply_called_);
    discard_all_packets_no_reply_called_ = true;
  }

  void BindGainControl(BindGainControlRequest& request,
                       BindGainControlCompleter::Sync& completer) override {
    EXPECT_FALSE(bind_gain_control_artifact_);

    bind_gain_control_artifact_ =
        std::make_unique<FakeGainControl>(dispatcher_, std::move(request.gain_control_request()));
  }

  void SetPtsUnits(SetPtsUnitsRequest& request, SetPtsUnitsCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void SetPtsContinuityThreshold(SetPtsContinuityThresholdRequest& request,
                                 SetPtsContinuityThresholdCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void GetReferenceClock(GetReferenceClockCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void SetReferenceClock(SetReferenceClockRequest& request,
                         SetReferenceClockCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void SetUsage(SetUsageRequest& request, SetUsageCompleter::Sync& completer) override {
    EXPECT_FALSE(set_usage_artifact_.has_value()) << fidl::ToUnderlying(*set_usage_artifact_);
    set_usage_artifact_ = request.usage();
  }

  void SetUsage2(SetUsage2Request& request, SetUsage2Completer::Sync& completer) override {
    EXPECT_FALSE(set_usage2_artifact_.has_value()) << fidl::ToUnderlying(*set_usage2_artifact_);
    set_usage2_artifact_ = request.usage2();
  }

  void SetPcmStreamType(SetPcmStreamTypeRequest& request,
                        SetPcmStreamTypeCompleter::Sync& completer) override {
    EXPECT_FALSE(set_stream_type_artifact_.has_value());
    set_stream_type_artifact_ = std::move(request.type());
  }

  void EnableMinLeadTimeEvents(EnableMinLeadTimeEventsRequest& request,
                               EnableMinLeadTimeEventsCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void GetMinLeadTime(GetMinLeadTimeCompleter::Sync& completer) override { FX_NOTIMPLEMENTED(); }

  void Play(PlayRequest& request, PlayCompleter::Sync& completer) override { FX_NOTIMPLEMENTED(); }

  void PlayNoReply(PlayNoReplyRequest& request, PlayNoReplyCompleter::Sync& completer) override {
    EXPECT_FALSE(play_no_reply_artifact_.has_value());
    play_no_reply_artifact_ = {.reference_time = request.reference_time(),
                               .media_time = request.media_time()};
  }

  void Pause(PauseCompleter::Sync& completer) override { FX_NOTIMPLEMENTED(); }

  void PauseNoReply(PauseNoReplyCompleter::Sync& completer) override {
    EXPECT_FALSE(pause_no_reply_called_);
    pause_no_reply_called_ = true;
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_media::AudioRenderer> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) final {
    FX_LOGS(ERROR) << "FakeAudioRenderer: AudioRenderer::handle_unknown_method(ordinal "
                   << metadata.method_ordinal << ", "
                   << (metadata.unknown_method_type == fidl::UnknownMethodType::kOneWay
                           ? "OneWay)"
                           : "TwoWay)");
  }

  // Checks
  bool ExpectAddPayloadBufferCalled(uint32_t id, zx_koid_t payload_buffer_koid) {
    EXPECT_FALSE(add_payload_buffer_artifacts_.empty());
    if (add_payload_buffer_artifacts_.empty()) {
      return false;
    }

    EXPECT_EQ(id, add_payload_buffer_artifacts_.front().id);
    EXPECT_EQ(payload_buffer_koid, add_payload_buffer_artifacts_.front().payload_buffer_koid);
    bool result = id == add_payload_buffer_artifacts_.front().id &&
                  payload_buffer_koid == add_payload_buffer_artifacts_.front().payload_buffer_koid;
    add_payload_buffer_artifacts_.pop();
    return result;
  }

  bool ExpectAddPayloadBufferNotCalled() {
    EXPECT_TRUE(add_payload_buffer_artifacts_.empty());
    return add_payload_buffer_artifacts_.empty();
  }

  bool ExpectRemovePayloadBufferCalled(uint32_t id) {
    EXPECT_FALSE(remove_payload_buffer_artifacts_.empty());
    if (remove_payload_buffer_artifacts_.empty()) {
      return false;
    }

    EXPECT_EQ(id, remove_payload_buffer_artifacts_.front());
    bool result = id == remove_payload_buffer_artifacts_.front();
    remove_payload_buffer_artifacts_.pop();
    return result;
  }

  bool ExpectRemovePayloadBufferNotCalled() {
    EXPECT_TRUE(remove_payload_buffer_artifacts_.empty());
    return remove_payload_buffer_artifacts_.empty();
  }

  bool ExpectSendPacketCalled(const fuchsia_media::StreamPacket& expected_packet) {
    EXPECT_TRUE(send_packet_artifact_.has_value());
    if (!send_packet_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_packet, send_packet_artifact_.value());
    auto result = expected_packet == send_packet_artifact_.value();
    send_packet_artifact_.reset();
    return result;
  }

  bool ExpectSendPacketNoReplyCalled(const fuchsia_media::StreamPacket& expected_packet) {
    EXPECT_TRUE(send_packet_no_reply_artifact_.has_value());
    if (!send_packet_no_reply_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_packet, send_packet_no_reply_artifact_.value());
    auto result = expected_packet == send_packet_no_reply_artifact_.value();
    send_packet_no_reply_artifact_.reset();
    return result;
  }

  bool ExpectEndOfStreamCalled() {
    EXPECT_TRUE(end_of_stream_called_);
    auto result = end_of_stream_called_;
    end_of_stream_called_ = false;
    return result;
  }

  bool ExpectDiscardAllPacketsCalled() {
    EXPECT_TRUE(discard_all_packets_called_);
    auto result = discard_all_packets_called_;
    discard_all_packets_called_ = false;
    return result;
  }

  bool ExpectDiscardAllPacketsNoReplyCalled() {
    EXPECT_TRUE(discard_all_packets_no_reply_called_);
    auto result = discard_all_packets_no_reply_called_;
    discard_all_packets_no_reply_called_ = false;
    return result;
  }

  std::unique_ptr<FakeGainControl> ExpectBindGainControlCalled() {
    EXPECT_TRUE(bind_gain_control_artifact_);
    return std::move(bind_gain_control_artifact_);
  }

  bool ExpectSetUsage2Called(fuchsia_media::AudioRenderUsage2 expected_usage) {
    EXPECT_TRUE(!set_usage_artifact_.has_value());
    EXPECT_TRUE(set_usage2_artifact_.has_value());
    if (!set_usage2_artifact_.has_value() || set_usage_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_usage, set_usage2_artifact_.value());
    auto result = expected_usage == set_usage2_artifact_.value();
    set_usage2_artifact_.reset();
    return result;
  }

  bool ExpectSetPcmStreamTypeCalled(const fuchsia_media::AudioStreamType& expected_stream_type) {
    EXPECT_TRUE(set_stream_type_artifact_.has_value());
    if (!set_stream_type_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_stream_type, set_stream_type_artifact_.value());
    auto result = expected_stream_type == set_stream_type_artifact_.value();
    set_stream_type_artifact_.reset();
    return result;
  }

  bool ExpectPlayNoReplyCalled(int64_t expected_reference_time, int64_t expected_media_time) {
    EXPECT_TRUE(play_no_reply_artifact_.has_value());
    if (!play_no_reply_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_reference_time, play_no_reply_artifact_.value().reference_time);
    EXPECT_EQ(expected_media_time, play_no_reply_artifact_.value().media_time);
    auto result = expected_reference_time == play_no_reply_artifact_.value().reference_time &&
                  expected_media_time == play_no_reply_artifact_.value().media_time;
    play_no_reply_artifact_.reset();
    return result;
  }

  bool ExpectPauseNoReplyCalled() {
    EXPECT_TRUE(pause_no_reply_called_);
    auto result = pause_no_reply_called_;
    pause_no_reply_called_ = false;
    return result;
  }

  bool ExpectNoOtherCalled() {
    EXPECT_TRUE(add_payload_buffer_artifacts_.empty());
    EXPECT_TRUE(remove_payload_buffer_artifacts_.empty());
    EXPECT_FALSE(discard_all_packets_no_reply_called_);
    EXPECT_FALSE(set_usage_artifact_.has_value()) << fidl::ToUnderlying(*set_usage_artifact_);
    EXPECT_FALSE(set_usage2_artifact_.has_value()) << fidl::ToUnderlying(*set_usage2_artifact_);
    EXPECT_FALSE(set_stream_type_artifact_.has_value());
    EXPECT_FALSE(pause_no_reply_called_);

    return add_payload_buffer_artifacts_.empty() && remove_payload_buffer_artifacts_.empty() &&
           !discard_all_packets_no_reply_called_ && !set_usage_artifact_.has_value() &&
           !set_usage2_artifact_.has_value() && !set_stream_type_artifact_.has_value() &&
           !pause_no_reply_called_;
  }

 private:
  struct AddPayloadBufferArtifact {
    uint32_t id;
    zx_koid_t payload_buffer_koid;
  };
  struct PlayNoReplyArtifact {
    int64_t reference_time;
    int64_t media_time;
  };

  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBindingRef<fuchsia_media::AudioRenderer>> binding_ref_;
  bool unbind_completed_ = false;

  std::queue<AddPayloadBufferArtifact> add_payload_buffer_artifacts_;
  std::queue<uint32_t> remove_payload_buffer_artifacts_;
  std::optional<fuchsia_media::StreamPacket> send_packet_artifact_;
  std::optional<fuchsia_media::StreamPacket> send_packet_no_reply_artifact_;
  bool end_of_stream_called_ = false;
  bool discard_all_packets_called_ = false;
  bool discard_all_packets_no_reply_called_ = false;
  std::unique_ptr<FakeGainControl> bind_gain_control_artifact_;
  std::optional<fuchsia_media::AudioRenderUsage> set_usage_artifact_;
  std::optional<fuchsia_media::AudioRenderUsage2> set_usage2_artifact_;
  std::optional<fuchsia_media::AudioStreamType> set_stream_type_artifact_;
  std::optional<PlayNoReplyArtifact> play_no_reply_artifact_;
  bool pause_no_reply_called_ = false;
};

}  // namespace media::audio::tests

#endif  // SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_AUDIO_RENDERER_H_

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_RENDERER_TEST_SHARED_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_RENDERER_TEST_SHARED_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>

#include <cstring>

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"

namespace media::audio::test {

// AudioRenderer contains an internal state machine; setting both the buffer and the audio format
// play a central role.
// - Upon construction, a renderer is in the "Initialized" state.
// - To enter "Configured" state, it must receive and successfully execute both SetPcmStreamType and
// AddPayloadBuffer (if only one or the other is called, we remain Initialized).
// - Once Configured, it transitions to "Operating" state, when packets are enqueued (received from
// SendPacket, but not yet played and/or released).
// - Once no enqueued packets remain, it transitions back to Configured state. Packets may be
// cancelled (by DiscardAllPackets), or completed (successfully played); either way their completion
// (if provided) is invoked.

// Additional restrictions on the allowed sequence of API calls:
// SetReferenceClock may only be called once for a given AudioRenderer.
// SetUsage2 and SetReferenceClock may only be called before SetPcmStreamType.
// SetPcmStreamType, AddPayloadBuffer/RemovePayloadBuffer may only be called when not Operating.
// A renderer must be Configured/Operating before calling SendPacket, Play, Pause.

// Note: the distinction between Configured/Operating is entirely orthogonal to Play/Pause state,
// although Play does cause the timeline to progress, leading to packet completion.

//
// AudioRendererTest
//
// This base class is reused by child classes that provide grouping of specific test areas.
//
// As currently implemented, AudioRenderer's four "NoReply" methods (PlayNoReply, PauseNoReply,
// SendPacketNoReply, DiscardAllPacketsNoReply) each simply redirect to their counterpart with a
// 'nullptr' callback parameter. For this reason, we don't exhaustively test the NoReply variants,
// instead covering them with 1-2 representative test cases each (in addition to those places where
// they are used instead of the "reply" variants for test simplicity).
class AudioRendererTest : public HermeticAudioTest {
 protected:
  // A valid but arbitrary |AudioStreamType|, for tests that don't care about the audio content.
  static constexpr fuchsia::media::AudioStreamType kTestStreamType{
      .sample_format = fuchsia::media::AudioSampleFormat::FLOAT,
      .channels = 2,
      .frames_per_second = 48000,
  };

  // The following are valid/invalid when used with |kTestStreamType|.
  // In bytes: payload buffer 40960 (~ 106 ms); default packet 3840 (10 ms).
  static inline size_t DefaultPayloadBufferSize() { return zx_system_get_page_size() * 10ul; }
  static constexpr uint64_t kDefaultPacketSize =
      sizeof(float) * kTestStreamType.channels * kTestStreamType.frames_per_second / 100;

  // Convenience packet of 10 ms, starting at the beginning of payload buffer 0.
  static constexpr fuchsia::media::StreamPacket kTestPacket{
      .payload_buffer_id = 0,
      .payload_offset = 0,
      .payload_size = kDefaultPacketSize,
  };

  void SetUp() override {
    HermeticAudioTest::SetUp();

    audio_core_->CreateAudioRenderer(audio_renderer_.NewRequest());
    AddErrorHandler(audio_renderer_, "AudioRenderer");
  }

  void TearDown() override {
    audio_renderer_.Unbind();

    HermeticAudioTest::TearDown();
  }

  // This can be used as a simple round-trip to indicate that all FIDL messages have been read out
  // of the channel, and thus have been handled successfully (i.e. no disconnect was triggered).
  void ExpectConnected() {
    audio_renderer_->GetMinLeadTime(AddCallback("GetMinLeadTime"));

    ExpectCallbacks();
  }

  // Discard in-flight packets and await a renderer response. This checks that the completions for
  // all enqueued packets are received, and that the Discard completion is received only afterward.
  // Thus, this also verifies more generally that the renderer is still connected.
  void ExpectConnectedAndDiscardAllPackets() {
    audio_renderer_->DiscardAllPackets(AddCallback("DiscardAllPackets"));

    ExpectCallbacks();
  }

  // Create a VMO and pass it to |AudioRenderer::AddPayloadBuffer| with a given |id|. This is
  // purely a convenience method: it does not provide access to the underlying buffer VMO (the VMO
  // is not mapped). As with all VMOs, the memory region is automatically zeroed-out.
  void CreateAndAddPayloadBuffer(uint32_t id) {
    zx::vmo payload_vmo;
    constexpr uint32_t kVmoOptionsNone = 0;
    ASSERT_EQ(zx::vmo::create(DefaultPayloadBufferSize(), kVmoOptionsNone, &payload_vmo), ZX_OK);
    audio_renderer_->AddPayloadBuffer(id, std::move(payload_vmo));
  }

  fuchsia::media::AudioRendererPtr& audio_renderer() { return audio_renderer_; }

 private:
  fuchsia::media::AudioRendererPtr audio_renderer_;
};

// AudioRenderer implements the base classes StreamBufferSet and StreamSink.

// Thin wrapper around AudioRendererTest for test case grouping only. This group validates
// AudioRenderer's implementation of StreamBufferSet (AddPayloadBuffer, RemovePayloadBuffer)
class AudioRendererBufferTest : public AudioRendererTest {};

//
// StreamSink validation
//

// Thin wrapper around AudioRendererTest for test case grouping only. This group validates
// AudioRenderer's implementation of StreamSink (SendPacket, DiscardAllPackets, EndOfStream).
class AudioRendererPacketTest : public AudioRendererTest {
 protected:
  // SetPcmStreamType and AddPayloadBuffer are callable in either order, as long as both are called
  // before Play. Thus, in these tests you see a mixture.
  void SendPacketCancellation(bool reply) {
    CreateAndAddPayloadBuffer(0);
    audio_renderer()->SetPcmStreamType(kTestStreamType);

    // Send a packet (we don't care about the actual packet data here).
    if (reply) {
      audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
    } else {
      audio_renderer()->SendPacketNoReply(kTestPacket);
    }

    ExpectConnectedAndDiscardAllPackets();
  }
};

// Thin wrapper around AudioRendererTest for test case grouping only. This group tests
// AudioRenderer's implementation of SetReferenceClock and GetReferenceClock.
class AudioRendererClockTest : public AudioRendererTest {
 protected:
  // The clock received from GetRefClock is read-only, but the original can still be adjusted.
  static constexpr auto kClockRights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_READ;

  zx::clock GetAndValidateReferenceClock() {
    zx::clock clock;

    audio_renderer()->GetReferenceClock(
        AddCallback("GetReferenceClock",
                    [&clock](zx::clock received_clock) { clock = std::move(received_clock); }));

    ExpectCallbacks();

    return clock;
  }
};

// Thin wrapper around AudioRendererTest for grouping only. This tests EnableMinLeadTimeEvents,
// GetMinLeadTime and OnMinLeadTimeChanged, as well as SetPtsUnits and SetPtsContinuityThreshold.
class AudioRendererLeadTimeTest : public AudioRendererTest {};
class AudioRendererPtsTest : public AudioRendererTest {};

// Thin wrapper around AudioRendererTest for test case grouping only.
// This group validates AudioRenderer's implementation of SetUsage2 and SetPcmStreamType.
class AudioRendererFormatUsageTest : public AudioRendererTest {};

// Thin wrapper around AudioRendererTest for test case grouping only.
// This group validates AudioRenderer's implementation of Play and Pause.
class AudioRendererTransportTest : public AudioRendererTest {};

// Thin wrapper around AudioRendererTest for test grouping, to test BindGainControl.
class AudioRendererGainTest : public AudioRendererTest {
  // Most gain tests were moved to gain_control_test.cc. Keep this test fixture intact for now, in
  // anticipation of cases that check interactions between SetGain and Play/Pause gain-ramping.
 protected:
  void SetUp() override {
    AudioRendererTest::SetUp();

    audio_renderer()->BindGainControl(gain_control_.NewRequest());
    AddErrorHandler(gain_control_, "AudioRenderer::GainControl");

    audio_core_->CreateAudioRenderer(audio_renderer_2_.NewRequest());
    AddErrorHandler(audio_renderer_2_, "AudioRenderer2");

    audio_renderer_2_->BindGainControl(gain_control_2_.NewRequest());
    AddErrorHandler(gain_control_2_, "AudioRenderer::GainControl2");
  }

  void TearDown() override {
    gain_control_.Unbind();

    AudioRendererTest::TearDown();
  }

  fuchsia::media::audio::GainControlPtr& gain_control() { return gain_control_; }
  fuchsia::media::AudioRendererPtr& audio_renderer_2() { return audio_renderer_2_; }
  fuchsia::media::audio::GainControlPtr& gain_control_2() { return gain_control_2_; }

 private:
  fuchsia::media::audio::GainControlPtr gain_control_;
  fuchsia::media::AudioRendererPtr audio_renderer_2_;
  fuchsia::media::audio::GainControlPtr gain_control_2_;
};

}  // namespace media::audio::test

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_RENDERER_TEST_SHARED_H_

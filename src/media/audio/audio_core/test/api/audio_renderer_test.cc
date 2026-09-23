// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <zircon/syscalls.h>

#include <limits>
#include <utility>

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/test/api/audio_renderer_test_shared.h"
#include "src/media/audio/lib/clock/clone_mono.h"
#include "src/media/audio/lib/clock/testing/clock_test.h"

namespace media::audio::test {

using fuchsia::media::AudioRenderUsage;
using fuchsia::media::AudioRenderUsage2;

// Sanity test adding a payload buffer. Just verify we don't get a disconnect.
TEST_F(AudioRendererBufferTest, AddPayloadBuffer) {
  CreateAndAddPayloadBuffer(0);
  CreateAndAddPayloadBuffer(1);
  CreateAndAddPayloadBuffer(2);

  ExpectConnectedAndDiscardAllPackets();
}

// AddPayloadBuffer is callable after packets are completed/discarded, regardless of play/pause
TEST_F(AudioRendererBufferTest, AddPayloadBufferWhileNotOperating) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket1"));
  ExpectConnectedAndDiscardAllPackets();
  CreateAndAddPayloadBuffer(1);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  CreateAndAddPayloadBuffer(2);

  ExpectCallbacks();
  CreateAndAddPayloadBuffer(3);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket2"));
  ExpectCallbacks();
  CreateAndAddPayloadBuffer(4);

  audio_renderer()->Pause(AddCallback("Pause"));
  CreateAndAddPayloadBuffer(5);

  ExpectCallbacks();
  CreateAndAddPayloadBuffer(6);

  ExpectConnectedAndDiscardAllPackets();
}

// Test removing payload buffers.
TEST_F(AudioRendererBufferTest, RemovePayloadBuffer) {
  CreateAndAddPayloadBuffer(0);
  CreateAndAddPayloadBuffer(1);
  CreateAndAddPayloadBuffer(2);
  CreateAndAddPayloadBuffer(3);
  audio_renderer()->RemovePayloadBuffer(2);
  audio_renderer()->RemovePayloadBuffer(3);
  audio_renderer()->RemovePayloadBuffer(0);
  audio_renderer()->RemovePayloadBuffer(1);

  ExpectConnectedAndDiscardAllPackets();
}

// A payload buffer can be added with a previously used id after the removal of the former.
TEST_F(AudioRendererBufferTest, RemovePayloadBufferThenAddDuplicateId) {
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  auto play_callback = AddCallback("Play");
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket1"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, play_callback);
  ExpectCallbacks();

  // Remove payload buffer, and re-add with the same id.
  audio_renderer()->RemovePayloadBuffer(0);
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket2"));
  ExpectCallbacks();

  audio_renderer()->Pause(AddCallback("Pause"));
  ExpectConnectedAndDiscardAllPackets();
}

// RemovePayloadBuffer is callable at ANY time if no packets are active
TEST_F(AudioRendererBufferTest, RemovePayloadBufferWhileNotOperating) {
  CreateAndAddPayloadBuffer(0);
  CreateAndAddPayloadBuffer(1);
  CreateAndAddPayloadBuffer(2);
  CreateAndAddPayloadBuffer(3);
  CreateAndAddPayloadBuffer(4);
  CreateAndAddPayloadBuffer(5);
  CreateAndAddPayloadBuffer(6);
  CreateAndAddPayloadBuffer(7);
  audio_renderer()->RemovePayloadBuffer(1);  // Don't remove buffer 0 yet: we use it in SendPacket

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->RemovePayloadBuffer(2);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket1"));
  ExpectConnectedAndDiscardAllPackets();  // cancel the packet and wait until it returns
  audio_renderer()->RemovePayloadBuffer(3);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  audio_renderer()->RemovePayloadBuffer(4);

  ExpectCallbacks();  // wait until Play completes
  audio_renderer()->RemovePayloadBuffer(5);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket2"));
  ExpectCallbacks();  // wait until the packet completes normally
  audio_renderer()->RemovePayloadBuffer(6);

  audio_renderer()->Pause(AddCallback("Pause"));
  audio_renderer()->RemovePayloadBuffer(7);

  ExpectCallbacks();  // wait until Pause completes
  audio_renderer()->RemovePayloadBuffer(0);

  ExpectConnected();
}

//
// StreamSink validation
//

TEST_F(AudioRendererPacketTest, SendPacketCompletion) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP,
                         [](int64_t, int64_t) {});
  ExpectCallbacks();
}

TEST_F(AudioRendererPacketTest, SendPacketInvokesCallbacksInOrder) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  // Play will complete and then each packet successively, so create this callback first.
  auto play_callback = AddCallback("Play");

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket1"));
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket2"));
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket3"));
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket4"));

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP, play_callback);
  ExpectCallbacks();
}

TEST_F(AudioRendererPacketTest, SendPacketCancellation) { SendPacketCancellation(true); }
// This is the sole test case to expressly target SendPacketNoReply.
TEST_F(AudioRendererPacketTest, SendPacketNoReplyCancellation) { SendPacketCancellation(false); }

// DiscardAllPackets cancels any outstanding (uncompleted) packets. Whether they complete normally
// or are cancelled before playing out, ALL packet callbacks should be invoked. These should be
// received in the original SendPacket order, followed finally by the DiscardAllPackets callback.
TEST_F(AudioRendererPacketTest, DiscardAllPacketsReturnsAfterAllPackets) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  // Even if the first packet completes almost immediately, others will still be outstanding.
  auto packet = kTestPacket;
  packet.payload_size = DefaultPayloadBufferSize();

  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket1"));
  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket2"));
  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket3"));
  audio_renderer()->SendPacket(std::move(packet), AddCallback("SendPacket4"));

  // We don't actually care where Play callback occurs in this sequence so we don't AddCallback it.
  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP);

  audio_renderer()->DiscardAllPackets(AddCallback("DiscardAllPackets"));

  // Our sequence of AddCallback calls reflects the expected ordering of callback invocation.
  // ExpectCallbacks enforces this ordering, and no unexpected callbacks, and no disconnects.
  ExpectCallbacks();
}

// This is the sole test case to expressly target DiscardAllPacketsNoReply.
// Packets are cancelled; completion callbacks should be invoked in-order.
TEST_F(AudioRendererPacketTest, DiscardAllPacketsNoReply) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  auto packet = kTestPacket;
  packet.payload_size = DefaultPayloadBufferSize();
  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket1"));
  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket2"));
  audio_renderer()->SendPacket(fidl::Clone(packet), AddCallback("SendPacket3"));
  audio_renderer()->SendPacket(std::move(packet), AddCallback("SendPacket4"));

  audio_renderer()->DiscardAllPacketsNoReply();

  ExpectCallbacks();
}

// Ensure that calling Discard before Play/Pause doesn't prevent the timeline from progressing.
TEST_F(AudioRendererPacketTest, DiscardAllPacketsBeforePlayDoesntComputeTimeline) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->DiscardAllPackets(AddCallback("DiscardAllPackets"));

  int64_t play_ref_time = -1, play_media_time = -1;
  int64_t pause_ref_time = -1, pause_media_time = -1;

  audio_renderer()->Play(
      fuchsia::media::NO_TIMESTAMP, 0,
      AddCallback("Play", [&play_ref_time, &play_media_time](auto ref_time, auto media_time) {
        play_ref_time = ref_time;
        play_media_time = media_time;
      }));

  ExpectCallbacks();
  EXPECT_EQ(play_media_time, 0);

  // If we call Play(NO_TIMESTAMP) then Pause immediately, it is possible for pause_ref_time <
  // play_ref_time. Even for ref_time NO_TIMESTAMP, audio_core still applies a small padding to the
  // effective Play ref_time, to guarantee that we can start exactly when we said we would.
  //
  // If pause_ref_time IS less than play_ref_time, the equivalent pause_media_time would be
  // negative. This is not necessarily incorrect behavior but would certainly confuse a caller.
  // Let's avoid the problem by adding this slight delay:
  do {
    zx_nanosleep(play_ref_time);
  } while (zx_clock_get_monotonic() < play_ref_time);

  audio_renderer()->Pause(
      AddCallback("Pause", [&pause_ref_time, &pause_media_time](auto ref_time, auto media_time) {
        pause_ref_time = ref_time;
        pause_media_time = media_time;
      }));

  ExpectCallbacks();

  // Renderer calculates Pause's media_time from its timeline function, which should be running.
  EXPECT_GT(pause_ref_time, play_ref_time);
  EXPECT_GT(pause_media_time, play_media_time);
}

// EndOfStream can be called at any time, regardless of the renderer's state.
TEST_F(AudioRendererPacketTest, EndOfStreamIsAlwaysCallable) {
  audio_renderer()->EndOfStream();

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->EndOfStream();

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->EndOfStream();

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  audio_renderer()->EndOfStream();

  ExpectCallbacks();
  audio_renderer()->EndOfStream();

  // Send a packet and allow it to drain out.
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  audio_renderer()->EndOfStream();

  ExpectCallbacks();
  audio_renderer()->EndOfStream();

  audio_renderer()->Pause(AddCallback("Pause"));
  audio_renderer()->EndOfStream();

  ExpectCallbacks();
  audio_renderer()->EndOfStream();

  ExpectConnected();  // Demonstrate we haven't disconnected
}

// If client-submitted clock has ZX_RIGHT_WRITE, this should be removed by GetReferenceClock
TEST_F(AudioRendererClockTest, GetRefClockRemovesWriteRight) {
  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());

  zx::clock received_clock = GetAndValidateReferenceClock();
  clock::testing::VerifyReadOnlyRights(received_clock);
}

// Accept the default clock that is returned if we set no clock
TEST_F(AudioRendererClockTest, SetRefClockDefault) {
  zx::clock ref_clock = GetAndValidateReferenceClock();

  clock::testing::VerifyReadOnlyRights(ref_clock);
  clock::testing::VerifyIsSystemMonotonic(ref_clock);

  clock::testing::VerifyAdvances(ref_clock);
  clock::testing::VerifyCannotBeRateAdjusted(ref_clock);
}

// Set a null clock; this represents selecting the AudioCore-generated clock.
TEST_F(AudioRendererClockTest, SetRefClockFlexible) {
  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  zx::clock provided_clock = GetAndValidateReferenceClock();

  clock::testing::VerifyReadOnlyRights(provided_clock);
  clock::testing::VerifyIsSystemMonotonic(provided_clock);

  clock::testing::VerifyAdvances(provided_clock);
  clock::testing::VerifyCannotBeRateAdjusted(provided_clock);
}

// Set a recognizable custom reference clock and validate that it is what we receive from
// GetReferenceClock. The received clock should be read-only; the original is still adjustable.
TEST_F(AudioRendererClockTest, SetRefClockCustom) {
  zx::clock dupe_clock, retained_clock, orig_clock = clock::AdjustableCloneOfMonotonic();
  zx::clock::update_args args;
  args.reset().set_rate_adjust(-100);
  ASSERT_EQ(orig_clock.update(args), ZX_OK) << "clock.update with rate_adjust failed";

  ASSERT_EQ(orig_clock.duplicate(kClockRights, &dupe_clock), ZX_OK);
  ASSERT_EQ(orig_clock.duplicate(kClockRights, &retained_clock), ZX_OK);

  audio_renderer()->SetReferenceClock(std::move(dupe_clock));
  zx::clock received_clock = GetAndValidateReferenceClock();

  clock::testing::VerifyReadOnlyRights(received_clock);
  clock::testing::VerifyIsNotSystemMonotonic(received_clock);

  clock::testing::VerifyAdvances(received_clock);
  clock::testing::VerifyCannotBeRateAdjusted(received_clock);

  clock::testing::VerifyCanBeRateAdjusted(orig_clock);
  clock::testing::VerifyAdvances(orig_clock);
}

// Setting the reference clock at any time before SetPcmStreamType should pass
TEST_F(AudioRendererClockTest, SetRefClockAfterAddBuffer) {
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->SetReferenceClock(clock::CloneOfMonotonic());
  auto ref_clock = GetAndValidateReferenceClock();

  clock::testing::VerifyReadOnlyRights(ref_clock);
  clock::testing::VerifyIsSystemMonotonic(ref_clock);
  clock::testing::VerifyAdvances(ref_clock);
  clock::testing::VerifyCannotBeRateAdjusted(ref_clock);
}

// Validate MinLeadTime events, when enabled. After enabling MinLeadTime events, we expect an
// initial notification. Because we have not yet set the format, we expect MinLeadTime to be 0.
TEST_F(AudioRendererLeadTimeTest, EnableMinLeadTimeEventsBeforeFormat) {
  int64_t min_lead_time = -1;
  audio_renderer().events().OnMinLeadTimeChanged = AddCallback(
      "OnMinLeadTimeChanged",
      [&min_lead_time](int64_t min_lead_time_nsec) { min_lead_time = min_lead_time_nsec; });

  audio_renderer()->EnableMinLeadTimeEvents(true);

  ExpectCallbacks();
  EXPECT_EQ(min_lead_time, 0);
}

// After setting format, MinLeadTime changes to reflect the delay properties of the output device,
// once it has been initialized to a certain audio format.
//
// If there is no valid output device, lead time remains 0 even after SetPcmStreamType is called
// (and no additional OnMinLeadTimeChanged event is generated). We don't test that behavior here.
//
// In this case, post-SetPcmStreamType lead time > 0 (RendererShim includes an AudioOutput).
TEST_F(AudioRendererLeadTimeTest, EnableMinLeadTimeEventsAfterFormat) {
  audio_renderer().events().OnMinLeadTimeChanged = AddCallback("OnMinLeadTimeChanged1");
  audio_renderer()->EnableMinLeadTimeEvents(true);
  ExpectCallbacks();

  int64_t lead_time = 0;
  audio_renderer().events().OnMinLeadTimeChanged =
      AddCallback("OnMinLeadTimeChanged2",
                  [&lead_time](int64_t lead_time_nsec) { lead_time = lead_time_nsec; });
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  ExpectCallbacks();
  EXPECT_GT(lead_time, 0);
}

// Validate no MinLeadTime events when disabled (nor should we Disconnect).
TEST_F(AudioRendererLeadTimeTest, DisableMinLeadTimeEvents) {
  audio_renderer().events().OnMinLeadTimeChanged = AddUnexpectedCallback("OnMinLeadTimeChanged");

  audio_renderer()->EnableMinLeadTimeEvents(false);
  ExpectConnected();

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  ExpectConnected();
}

// Before SetPcmStreamType is called, MinLeadTime should equal zero.
TEST_F(AudioRendererLeadTimeTest, GetMinLeadTimeBeforeFormat) {
  int64_t min_lead_time = -1;
  audio_renderer()->GetMinLeadTime(AddCallback(
      "GetMinLeadTime",
      [&min_lead_time](int64_t min_lead_time_nsec) { min_lead_time = min_lead_time_nsec; }));

  ExpectCallbacks();
  EXPECT_EQ(min_lead_time, 0);
}

// EnableMinLeadTimeEvents can be called at any time, regardless of the renderer's state.
TEST_F(AudioRendererLeadTimeTest, EnableMinLeadTimeEventsCanAlwaysBeCalled) {
  audio_renderer()->EnableMinLeadTimeEvents(true);

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->EnableMinLeadTimeEvents(false);

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->EnableMinLeadTimeEvents(true);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  audio_renderer()->EnableMinLeadTimeEvents(false);

  ExpectCallbacks();
  audio_renderer()->EnableMinLeadTimeEvents(true);

  // Send a packet and allow it to drain out.
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  audio_renderer()->EnableMinLeadTimeEvents(false);

  ExpectCallbacks();
  audio_renderer()->EnableMinLeadTimeEvents(true);

  audio_renderer()->Pause(AddCallback("Pause"));
  audio_renderer()->EnableMinLeadTimeEvents(false);

  ExpectCallbacks();
  audio_renderer()->EnableMinLeadTimeEvents(true);

  ExpectConnected();  // Demonstrate we haven't disconnected
}

// Verify that GetMinLeadTime can be called at any time, regardless of the renderer's state.
TEST_F(AudioRendererLeadTimeTest, GetMinLeadTimeCanAlwaysBeCalled) {
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime1"));
  ExpectCallbacks();

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime2"));
  ExpectCallbacks();

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime3"));
  ExpectCallbacks();

  // We use PlayNoReply and PauseNoReply here because there is no required callback ordering between
  // Play/Pause completion and the GetMinLeadTime callback.
  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, 0);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime4"));
  ExpectCallbacks();

  // Send a packet and allow it to drain out.
  audio_renderer()->SendPacketNoReply(kTestPacket);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime5"));
  ExpectCallbacks();

  audio_renderer()->PauseNoReply();
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime6"));
  ExpectCallbacks();

  ExpectConnectedAndDiscardAllPackets();  // Demonstrate we haven't disconnected
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime7"));
  ExpectCallbacks();
}

// SetPtsUnits accepts uint numerator and denominator that must be within certain range
//
// Ensure that the max and min PTS-unit values are accepted.
TEST_F(AudioRendererPtsTest, SetPtsUnitsLimits) {
  audio_renderer()->SetPtsUnits(1, 60);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime after SetPtsUnit(max)"));
  ExpectCallbacks();

  audio_renderer()->SetPtsUnits(1e9, 1);
  audio_renderer()->GetMinLeadTime(AddCallback("GetMinLeadTime after SetPtsUnits(min)"));
  ExpectCallbacks();
}

// SetPtsUnits can be called at any time, except when active packets are outstanding
TEST_F(AudioRendererPtsTest, SetPtsUnitsWhileNotOperating) {
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second, 1);

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second, 2);

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second, 3);

  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, 0);
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second, 1);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  ExpectCallbacks();  // Allow the sent packet to drain out.
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second * 2, 1);

  audio_renderer()->PauseNoReply();
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second * 3, 1);

  ExpectConnected();  // Demonstrate we haven't disconnected
}

// SetPtsContinuityThreshold is callable at any time, except when active packets are outstanding.
// SetPtsContinuityThreshold parameter must be in the non-negative range.
// Subnormal values such as std::numeric_limits<float>::min()/2 are OK.
TEST_F(AudioRendererPtsTest, SetPtsContThresholdWhileNotOperating) {
  audio_renderer()->SetPtsContinuityThreshold(0.0f);

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SetPtsContinuityThreshold(0.01f);

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPtsContinuityThreshold(0.02f);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  audio_renderer()->SetPtsContinuityThreshold(0.03f);

  ExpectCallbacks();
  audio_renderer()->SetPtsContinuityThreshold(std::numeric_limits<float>::min());

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  ExpectCallbacks();  // Send a packet and allow it to drain out.
  audio_renderer()->SetPtsContinuityThreshold(60.0f);

  audio_renderer()->Pause(AddCallback("Pause"));
  audio_renderer()->SetPtsContinuityThreshold(std::numeric_limits<float>::min());

  ExpectCallbacks();
  audio_renderer()->SetPtsContinuityThreshold(0.0f);

  ExpectConnected();  // Demonstrate we haven't disconnected
}

// A renderer stream's usage can be changed any time before the format is set.
TEST_F(AudioRendererFormatUsageTest, SetUsageBeforeFormat) {
  audio_renderer()->SetUsage(AudioRenderUsage::COMMUNICATION);

  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  audio_renderer()->SetUsage(AudioRenderUsage::SYSTEM_AGENT);

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetUsage(AudioRenderUsage::INTERRUPTION);

  audio_renderer()->GetReferenceClock(AddCallback("GetReferenceClock"));
  audio_renderer()->SetUsage(AudioRenderUsage::BACKGROUND);
  ExpectCallbacks();

  audio_renderer()->SetUsage(AudioRenderUsage::MEDIA);
  ExpectConnected();  // Demonstrate we haven't disconnected
}

// A renderer stream's usage can be changed any time before the format is set.
TEST_F(AudioRendererFormatUsageTest, SetUsage2BeforeFormat) {
  audio_renderer()->SetUsage2(AudioRenderUsage2::COMMUNICATION);

  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  audio_renderer()->SetUsage2(AudioRenderUsage2::SYSTEM_AGENT);

  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetUsage2(AudioRenderUsage2::INTERRUPTION);

  audio_renderer()->GetReferenceClock(AddCallback("GetReferenceClock"));
  audio_renderer()->SetUsage2(AudioRenderUsage2::BACKGROUND);
  ExpectCallbacks();

  audio_renderer()->SetUsage2(AudioRenderUsage2::MEDIA);
  ExpectConnected();  // Demonstrate we haven't disconnected
}

// Before renderers are Operating, SetPcmStreamType should succeed. Test twice because of a previous
// bug, where the first call succeeded but the second (pre-Play) caused a disconnect.
TEST_F(AudioRendererFormatUsageTest, SetPcmStreamType) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SetPcmStreamType({
      .sample_format = fuchsia::media::AudioSampleFormat::UNSIGNED_8,
      .channels = 1,
      .frames_per_second = 44100,
  });

  ExpectConnected();  // Allow for a Disconnect; expect a valid GetMinLeadTime callback instead
}

// Setting PCM format within supportable ranges should succeed, if no active packets.
// Test both post-cancellation and post-completion scenarios. This is the only test case to
TEST_F(AudioRendererFormatUsageTest, SetPcmStreamTypeAfterOperating) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket to discard"));
  audio_renderer()->DiscardAllPacketsNoReply();
  ExpectCallbacks();  // Wait for the packet to cancel/return

  audio_renderer()->SetPcmStreamType({
      .sample_format = fuchsia::media::AudioSampleFormat::UNSIGNED_8,
      .channels = 1,
      .frames_per_second = 44100,
  });

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket to play"));
  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP);
  ExpectCallbacks();  // Wait for the packet to complete normally

  audio_renderer()->SetPcmStreamType({
      .sample_format = fuchsia::media::AudioSampleFormat::SIGNED_16,
      .channels = 2,
      .frames_per_second = 44100,
  });

  ExpectConnected();
}

TEST_F(AudioRendererTransportTest, Play) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  auto packet = kTestPacket;
  packet.pts = ZX_MSEC(100);

  // We expect to receive |Play| callback _before_ |SendPacket| callback, so we add it first.
  auto play_callback = AddCallback("Play");
  audio_renderer()->SendPacket(std::move(packet), AddCallback("SendPacket"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, play_callback);

  ExpectCallbacks();
}

// This is the sole test case to expressly target PlayNoReply, although it is used elsewhere.
// Just touch the API in a cursory way.
TEST_F(AudioRendererTransportTest, PlayNoReply) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP);

  ExpectCallbacks();
}

// Pause stops the renderer timeline, so packets subsequently submitted should not complete.
TEST_F(AudioRendererTransportTest, Pause) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  // Ensure that the transition to Play has completed fully.
  ExpectCallbacks();

  int64_t pause_pts = fuchsia::media::NO_TIMESTAMP;
  audio_renderer()->Pause(
      AddCallback("Pause", [&pause_pts](int64_t pause_ref_time, int64_t pause_media_time) {
        pause_pts = pause_media_time;
      }));

  // Ensure that the transition to Pause has completed fully.
  ExpectCallbacks();
  EXPECT_NE(pause_pts, fuchsia::media::NO_TIMESTAMP);

  // Submit a packet after the stated Pause time. If we are paused, this packet should not complete.
  auto packet = kTestPacket;
  packet.pts = pause_pts + 1;
  audio_renderer()->SendPacket(std::move(packet), AddUnexpectedCallback("SendPacket"));

  ExpectConnected();  // fail on disconnect or the SendPacket completion
}

// This is the sole test case to expressly target PauseNoReply, although it is used elsewhere.
// Just touch the API in a cursory way.
TEST_F(AudioRendererTransportTest, PauseNoReply) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  ExpectCallbacks();

  audio_renderer()->PauseNoReply();

  ExpectConnected();
}

// "Quick" and "Multiple" cases validate synchronization via a series of immediate Play/Pause calls
//
// Immediate Play then Pause. Verify we are paused by failing if the packet completes
TEST_F(AudioRendererTransportTest, QuickPlayPause) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play"));
  int64_t pause_pts = fuchsia::media::NO_TIMESTAMP;
  audio_renderer()->Pause(
      AddCallback("Pause", [&pause_pts](int64_t pause_ref_time, int64_t pause_media_time) {
        pause_pts = pause_media_time;
      }));

  // Ensure that the transition to Pause has completed fully
  ExpectCallbacks();
  EXPECT_NE(pause_pts, fuchsia::media::NO_TIMESTAMP);

  // Submit a packet after the stated Pause time. If we are paused, this packet should not complete.
  auto packet = kTestPacket;
  packet.pts = pause_pts + 1;
  audio_renderer()->SendPacket(std::move(packet), AddUnexpectedCallback("SendPacket"));

  ExpectConnected();
}

// Immediate Pause then Play. Verify we are playing by expecting the packet completion
TEST_F(AudioRendererTransportTest, QuickPausePlay) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play1"));
  ExpectCallbacks();  // Ensure we are playing before proceeding

  audio_renderer()->Pause(AddCallback("Pause"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 1, AddCallback("Play"));

  // Are we playing? This packet will eventually complete, if so.
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  ExpectCallbacks();
}

TEST_F(AudioRendererTransportTest, MultiplePlayPause) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play1"));
  audio_renderer()->Pause(AddCallback("Pause1"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 1, AddCallback("Play2"));
  audio_renderer()->Pause(AddCallback("Pause2"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 2, AddCallback("Play3"));
  audio_renderer()->Pause(AddCallback("Pause3"));
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 3, AddCallback("Play4"));
  audio_renderer()->Pause(AddCallback("Pause4"));

  audio_renderer()->SendPacket(kTestPacket, AddUnexpectedCallback("SendPacket"));

  ExpectConnected();
}

TEST_F(AudioRendererTransportTest, CommandsSerializedAfterPause) {
  CreateAndAddPayloadBuffer(1);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  static constexpr fuchsia::media::StreamPacket packet1{
      .payload_buffer_id = 1,
      .payload_offset = 0,
      .payload_size = kDefaultPacketSize,
  };

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, 0, AddCallback("Play1"));
  ExpectCallbacks();  // Ensure we are playing before proceeding

  audio_renderer()->Pause(AddCallback("Pause1"));
  audio_renderer()->SendPacket(packet1, AddCallback("SendPacket1"));
  audio_renderer()->DiscardAllPackets(AddCallback("DiscardAllPackets1"));

  // {Add,Remove}PayloadBuffer don't have callbacks, however they will crash
  // if not invoked in the correct order: Add will crash if the packet queue
  // is not empty (not called after the above discard) and Remove will crash
  // if not called after Add.
  static constexpr fuchsia::media::StreamPacket packet2{
      .payload_buffer_id = 2,
      .payload_offset = 0,
      .payload_size = kDefaultPacketSize,
  };
  CreateAndAddPayloadBuffer(2);
  audio_renderer()->SendPacket(packet2, AddCallback("SendPacket2"));
  // Queue must be empty before removing the payload buffer.
  audio_renderer()->DiscardAllPackets(AddCallback("DiscardAllPackets2"));
  audio_renderer()->RemovePayloadBuffer(2);

  ExpectCallbacks();

  // Do this after ExpectCallbacks to ensure the above callbacks have fired,
  // otherwise the ping sent by ExpectedConnect might return before some of
  // the async methods (such as SendPacket) have completed.
  ExpectConnected();
}

// Validate AudioRenderers can create GainControl interfaces, that renderers persist after their
// gain_control is unbound, but that gain_controls do NOT persist after their renderer is unbound.
TEST_F(AudioRendererGainTest, BindGainControl) {
  // Validate gain_control_2 does NOT persist after audio_renderer_2 is unbound...
  audio_renderer_2().Unbind();

  // ... but validate that audio_renderer DOES persist without gain_control
  gain_control().Unbind();

  ExpectDisconnect(gain_control_2());

  ExpectConnected();  // Let audio_renderer show it is still alive (or let disconnects emerge)
}

}  // namespace media::audio::test

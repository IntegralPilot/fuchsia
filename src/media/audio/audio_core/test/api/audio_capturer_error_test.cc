// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>
#include <lib/zx/clock.h>

#include "src/media/audio/audio_core/test/api/audio_capturer_test_shared.h"
#include "src/media/audio/lib/clock/clone_mono.h"

namespace media::audio::test {
namespace {

class AudioCapturerErrorTestOldAPI : public AudioCapturerTestOldAPI {};
class AudioCapturerErrorTest : public AudioCapturerTest {};

//
// Test cases
//
// AudioCapturer implements the base classes StreamBufferSet and StreamSource, in addition to its
// own FIDL methods.

// TODO(https://fxbug.dev/318431483): more extensively test StreamBufferSet
// - AddPayloadBuffer(uint32 id, handle<vmo> payload_buffer);
//     Also: null or bad handle
// - RemovePayloadBuffer(uint32 id);
//     unknown or already-removed id
// - also, apply same tests to AudioRenderer and AudioCapturer
//     (although their implementations within AudioCore differ somewhat).

// TODO(https://fxbug.dev/318432150): more extensively test StreamSource
// - ->OnPacketProduced(StreamPacket packet);
//     Always received for every packet - even malformed ones?
// - ->OnEndOfStream();
//     Also proper sequence vis-a-vis other completion and disconnect callbacks
// - DiscardAllPacketsNoReply() post-stop
// - all capture StreamPacket flags

// TODO(https://fxbug.dev/318433705): more extensively test capture data pipeline methods
// - SetPcmStreamType(AudioStreamType stream_type);
//     Also when already set, when packets submitted, when started, malformed StreamType
// - CaptureAt(uint32 id, uint32 offset, uint32 num_frames)
//             -> (StreamPacket captured_packet);
//     Also when in async capture, before format set, before packets submitted
//     negative testing: bad id, bad offset, 0/tiny/huge num_frames
// - StartAsyncCapture(uint32 frames_per_packet);
//     Also when already started, before format set, before packets submitted
//     negative testing: 0/tiny/huge frames_per_packet (bigger than packet)
// - StopAsyncCapture() before format set, before packets submitted
// - StopAsyncCaptureNoReply() same
// - GetStreamType() -> (StreamType stream_type);
//     negative testing: before format set

TEST_F(AudioCapturerErrorTestOldAPI, AddPayloadBufferBadIdShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer(1);

  ExpectDisconnect(audio_capturer());
}

TEST_F(AudioCapturerErrorTestOldAPI, DiscardAllWithNoVmoShouldDisconnect) {
  SetFormat();

  audio_capturer()->DiscardAllPackets(AddUnexpectedCallback("DiscardAllPackets"));
  ExpectDisconnect(audio_capturer());
}

// DiscardAllPackets should fail, if async capture is active
TEST_F(AudioCapturerErrorTestOldAPI, DiscardAllDuringAsyncCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->DiscardAllPackets(AddUnexpectedCallback("DiscardAllPackets"));
  ExpectDisconnect(audio_capturer());
}

// DiscardAllPackets should fail, if async capture is in the process of stopping
TEST_F(AudioCapturerErrorTestOldAPI, DISABLED_DiscardAllAsyncCaptureStoppingShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->StopAsyncCaptureNoReply();
  audio_capturer()->DiscardAllPackets(AddUnexpectedCallback("DiscardAllPackets"));
  ExpectDisconnect(audio_capturer());
}

TEST_F(AudioCapturerErrorTestOldAPI, DiscardAllNoReplyWithNoVmoShouldDisconnect) {
  SetFormat();

  audio_capturer()->DiscardAllPacketsNoReply();
  ExpectDisconnect(audio_capturer());
}

// DiscardAllPacketsNoReply should fail, if async capture is active
TEST_F(AudioCapturerErrorTestOldAPI, DiscardAllNoReplyDuringAsyncCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->DiscardAllPacketsNoReply();
  ExpectDisconnect(audio_capturer());
}

// DiscardAllPacketsNoReply should fail, if async capture is in the process of stopping
TEST_F(AudioCapturerErrorTestOldAPI,
       DISABLED_DiscardAllNoReplyAsyncCaptureStoppingShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->StopAsyncCaptureNoReply();
  audio_capturer()->DiscardAllPacketsNoReply();
  ExpectDisconnect(audio_capturer());
}

TEST_F(AudioCapturerErrorTestOldAPI, StopWhenStoppedShouldDisconnect) {
  audio_capturer()->StopAsyncCapture(AddUnexpectedCallback("StopAsyncCapture"));
  ExpectDisconnect(audio_capturer());
}

TEST_F(AudioCapturerErrorTestOldAPI, StopNoReplyWhenStoppedShouldDisconnect) {
  audio_capturer()->StopAsyncCaptureNoReply();
  ExpectDisconnect(audio_capturer());
}

// Setting a payload buffer should fail, if format has not yet been set (even if it was retrieved).
TEST_F(AudioCapturerErrorTestOldAPI, AddPayloadBufferBeforeSetFormatShouldDisconnect) {
  // Give time for Disconnect to occur, if it must.
  audio_capturer()->GetStreamType(AddCallback("GetStreamType"));
  ExpectCallbacks();

  // Calling this before SetPcmStreamType should fail
  SetUpPayloadBuffer();
  ExpectDisconnect(audio_capturer());
}

// inadequate ZX_RIGHTS -- no DUPLICATE should cause SetReferenceClock to fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockNoDuplicateShouldDisconnect) {
  zx::clock dupe_clock, orig_clock = clock::CloneOfMonotonic();
  ASSERT_EQ(orig_clock.duplicate(kClockRights & ~ZX_RIGHT_DUPLICATE, &dupe_clock), ZX_OK);

  audio_capturer()->SetReferenceClock(std::move(dupe_clock));
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// inadequate ZX_RIGHTS -- no READ should cause SetReferenceClock to fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockNoReadShouldDisconnect) {
  zx::clock dupe_clock, orig_clock = clock::CloneOfMonotonic();
  ASSERT_EQ(orig_clock.duplicate(kClockRights & ~ZX_RIGHT_READ, &dupe_clock), ZX_OK);

  audio_capturer()->SetReferenceClock(std::move(dupe_clock));
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockCustomThenFlexibleShouldDisconnect) {
  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());

  audio_capturer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockSecondCustomShouldDisconnect) {
  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockSecondFlexibleShouldDisconnect) {
  audio_capturer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));

  audio_capturer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockFlexibleThenCustomShouldDisconnect) {
  audio_capturer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, once payload buffer has been added.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockAfterBufferShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, any time after payload buffer has been added.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockDuringCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer()->CaptureAt(0, 0, 8000, [](fuchsia::media::StreamPacket) {
    // Don't fail if this completes before SetReferenceClock can run.
    GTEST_SKIP() << "CaptureAt completed before SetReferenceClock could cancel it";
  });

  audio_capturer()->SetReferenceClock(clock::CloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, even after all active capture packets have returned.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockAfterCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer()->CaptureAt(0, 0, 8000, AddCallback("CaptureAt"));
  ExpectCallbacks();

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, any time after capture has started (even if cancelled).
//
// TODO(https://fxbug.dev/42134885): deflake and re-enable.
TEST_F(AudioCapturerClockTestOldAPI, DISABLED_SetRefClockCaptureCancelledShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer()->CaptureAt(0, 0, 8000, [](fuchsia::media::StreamPacket) {});
  audio_capturer()->DiscardAllPackets(AddCallback("DiscardAllPackets"));
  ExpectCallbacks();

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, if at least one capture packet is active.
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockDuringAsyncCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->SetReferenceClock(clock::CloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Setting the reference clock should fail, any time after capture has started (even if stopped).
TEST_F(AudioCapturerClockTestOldAPI, SetRefClockAfterAsyncCaptureShouldDisconnect) {
  SetFormat();
  SetUpPayloadBuffer();

  audio_capturer().events().OnPacketProduced = AddCallback("OnPacketProduced");
  audio_capturer()->StartAsyncCapture(1600);
  ExpectCallbacks();

  audio_capturer()->StopAsyncCapture(AddCallback("StopAsyncCapture"));
  ExpectCallbacks();

  audio_capturer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  WaitBeforeClockRelatedDisconnect();
  ExpectDisconnect(audio_capturer());
}

// Calling SetUsage2 with an unknown AudioCaptureUsage2 should cause the AudioCapturer to disconnect
TEST_F(AudioCapturerErrorTestOldAPI, SetUsage2UnknownFlexibleEnumShouldDisconnect) {
  audio_capturer()->SetUsage2(static_cast<fuchsia::media::AudioCaptureUsage2>(42));
  ExpectDisconnect(audio_capturer());
}

}  // namespace
}  // namespace media::audio::test

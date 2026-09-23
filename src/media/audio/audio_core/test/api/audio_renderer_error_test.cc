// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/types.h>

#include <cmath>
#include <cstdint>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/test/api/audio_renderer_test_shared.h"
#include "src/media/audio/lib/clock/clone_mono.h"

namespace media::audio::test {
namespace {

using fuchsia::media::AudioRenderUsage2;

// TODO(https://fxbug.dev/326083019): unittest AudioCore methods more directly (SetRenderUsageGain,
// SetRenderUsageGain2, SetCaptureUsageGain2, ResetInteractions, LoadDefaults).

class AudioRendererBufferErrorTest : public AudioRendererBufferTest {};

// It is invalid to add a payload buffer with a duplicate id.
TEST_F(AudioRendererBufferErrorTest, AddPayloadBufferDuplicateId) {
  CreateAndAddPayloadBuffer(0);
  CreateAndAddPayloadBuffer(0);

  ExpectDisconnect(audio_renderer());
}

// It is invalid to add a payload buffer while there are queued packets.
// Attempt to add new payload buffer while the packet is in flight. This should fail.
TEST_F(AudioRendererBufferErrorTest, AddPayloadBufferWhileOperatingShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SendPacketNoReply(kTestPacket);

  CreateAndAddPayloadBuffer(1);

  ExpectDisconnect(audio_renderer());
}

// It is invalid to remove a payload buffer while there are queued packets.
TEST_F(AudioRendererBufferErrorTest, RemovePayloadBufferWhileOperatingShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  ExpectConnected();  // ensure that if/when we disconnect, it is not because of the above

  audio_renderer()->SendPacketNoReply(kTestPacket);

  audio_renderer()->RemovePayloadBuffer(0);

  ExpectDisconnect(audio_renderer());
}

// Test RemovePayloadBuffer with an invalid ID (no corresponding AddPayloadBuffer).
TEST_F(AudioRendererBufferErrorTest, RemovePayloadBufferInvalidBufferIdShouldDisconnect) {
  audio_renderer()->RemovePayloadBuffer(0);

  ExpectDisconnect(audio_renderer());
}

class AudioRendererPacketErrorTest : public AudioRendererPacketTest {};

TEST_F(AudioRendererPacketErrorTest, SendPacketTooManyShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  // The exact limit is a function of the size of some internal data structures. We verify this
  // limit is somewhere between 500 and 600 packets.
  for (int i = 0; i < 500; ++i) {
    audio_renderer()->SendPacket(kTestPacket, []() {});
  }
  ExpectConnectedAndDiscardAllPackets();

  for (int i = 0; i < 600; ++i) {
    audio_renderer()->SendPacket(kTestPacket, []() {});
  }
  ExpectDisconnect(audio_renderer());
}

// SendPacket cannot be called before the stream type has been configured (SetPcmStreamType).
TEST_F(AudioRendererPacketErrorTest, SendPacketWithoutFormatShouldDisconnect) {
  // Add a payload buffer but no stream type.
  CreateAndAddPayloadBuffer(0);

  // SendPacket should trigger a disconnect due to a lack of a configured stream type.
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  ExpectDisconnect(audio_renderer());
}

// SendPacket cannot be called before the payload buffer has been added.
TEST_F(AudioRendererPacketErrorTest, SendPacketWithoutBufferShouldDisconnect) {
  // Add a stream type but no payload buffer.
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  // SendPacket should trigger a disconnect due to a lack of a configured stream type.
  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  ExpectDisconnect(audio_renderer());
}

// SendPacket with an unknown |payload_buffer_id|
TEST_F(AudioRendererPacketErrorTest, SendPacketInvalidPayloadBufferIdShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  // We never added a payload buffer with this ID, so this should cause a disconnect
  auto packet = kTestPacket;
  packet.payload_buffer_id = 1234;
  audio_renderer()->SendPacket(packet, []() {});

  ExpectDisconnect(audio_renderer());
}

// SendPacket with a |payload_size| that is invalid
TEST_F(AudioRendererPacketErrorTest, SendPacketInvalidPayloadBufferSizeShouldDisconnect) {
  // kTestStreamType frames are 8 bytes (float32 x Stereo).
  // As an invalid packet size, we specify a value (9) that is NOT a perfect multiple of 8.
  constexpr uint64_t kInvalidPayloadSize = (sizeof(float) * kTestStreamType.channels) + 1;

  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  auto packet = kTestPacket;
  packet.payload_size = kInvalidPayloadSize;
  audio_renderer()->SendPacket(packet, []() {});

  ExpectDisconnect(audio_renderer());
}

// |payload_offset| starts beyond the end of the payload buffer.
TEST_F(AudioRendererPacketErrorTest, SendPacketBufferOutOfBoundsShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  auto packet = kTestPacket;
  packet.payload_offset = DefaultPayloadBufferSize();
  audio_renderer()->SendPacket(packet, []() {});

  ExpectDisconnect(audio_renderer());
}

// |payload_offset| + |payload_size| extends beyond the end of the payload buffer.
TEST_F(AudioRendererPacketErrorTest, SendPacketBufferOverrunShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  auto packet = kTestPacket;
  packet.payload_size = kDefaultPacketSize * 2;
  packet.payload_offset = DefaultPayloadBufferSize() - kDefaultPacketSize;
  audio_renderer()->SendPacket(packet, []() {});

  ExpectDisconnect(audio_renderer());
}

class AudioRendererPtsErrorTest : public AudioRendererPtsTest {};

// SetPtsUnits accepts uint numerator and denominator that must be within certain range
//
// Numerator cannot be zero
TEST_F(AudioRendererPtsErrorTest, SetPtsUnitsZeroNumeratorShouldDisconnect) {
  audio_renderer()->SetPtsUnits(0, 1);
  ExpectDisconnect(audio_renderer());
}

// There cannot be more than one PTS tick per nanosecond. We use ratio 1e9/1 + epsilon to test this
// limit. The smallest such epsilon we can encode in uint32 / uint32 is (4e9+1)/4, where epsilon =
// 1/4. The next smallest (5e9+1)/5 cannot be encoded because 5e9+1 exceeds MAX_UINT32.
TEST_F(AudioRendererPtsErrorTest, SetPtsUnitsTooHighShouldDisconnect) {
  // This value equates to 0.99999999975 nanoseconds.
  audio_renderer()->SetPtsUnits(4'000'000'001, 4);
  ExpectDisconnect(audio_renderer());
}

// Denominator cannot be zero
TEST_F(AudioRendererPtsErrorTest, SetPtsUnitsZeroDenominatorShouldDisconnect) {
  audio_renderer()->SetPtsUnits(1000, 0);
  ExpectDisconnect(audio_renderer());
}

// There must be at least one PTS tick per minute. We test this limit with ratio 1/60 - epsilon.
// To compute the smallest epsilon that can be encoded in uint32 / uint32, we find the largest X
// and Y such that X/Y = 1/60, then use a ratio of X/(Y+1).
//   floor(2^32/60) = 71582788, so we use the ratio 71582788 / (4294967280+1).
TEST_F(AudioRendererPtsErrorTest, SetPtsUnitsTooLowShouldDisconnect) {
  // This value equates to 60.000000013969839 seconds.
  audio_renderer()->SetPtsUnits(71582788, 4294967281);
  ExpectDisconnect(audio_renderer());
}

TEST_F(AudioRendererPtsErrorTest, SetPtsUnitsWhileOperatingShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SendPacketNoReply(kTestPacket);
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second, 1);

  ExpectDisconnect(audio_renderer());
}

// If active packets are outstanding, calling SetPtsContinuityThreshold will cause a disconnect
TEST_F(AudioRendererPtsErrorTest, SetPtsContThresholdWhileOperatingCausesDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SendPacketNoReply(kTestPacket);
  audio_renderer()->SetPtsContinuityThreshold(0.01f);

  ExpectDisconnect(audio_renderer());
}

// SetPtsContinuityThreshold parameter must be non-negative and finite.
TEST_F(AudioRendererPtsErrorTest, SetPtsContThresholdNegativeValueCausesDisconnect) {
  audio_renderer()->SetPtsContinuityThreshold(-0.01f);
  ExpectDisconnect(audio_renderer());
}

// SetPtsContinuityThreshold parameter must be non-negative and finite.
TEST_F(AudioRendererPtsErrorTest, SetPtsContThresholdNanCausesDisconnect) {
  audio_renderer()->SetPtsContinuityThreshold(NAN);
  ExpectDisconnect(audio_renderer());
}

// SetPtsContinuityThreshold parameter must be non-negative and finite.
TEST_F(AudioRendererPtsErrorTest, SetPtsContThresholdInfinityCausesDisconnect) {
  audio_renderer()->SetPtsContinuityThreshold(INFINITY);
  ExpectDisconnect(audio_renderer());
}

class AudioRendererClockErrorTest : public AudioRendererClockTest {};

// Inadequate ZX_RIGHTS (no DUPLICATE) should cause SetReferenceClock to fail.
TEST_F(AudioRendererClockErrorTest, SetRefClockWithoutDuplicateShouldDisconnect) {
  zx::clock dupe_clock, orig_clock = clock::CloneOfMonotonic();
  ASSERT_EQ(orig_clock.duplicate(kClockRights & ~ZX_RIGHT_DUPLICATE, &dupe_clock), ZX_OK);

  audio_renderer()->SetReferenceClock(std::move(dupe_clock));
  ExpectDisconnect(audio_renderer());
}

// inadequate ZX_RIGHTS (no READ) should cause SetReferenceClock to fail.
TEST_F(AudioRendererClockErrorTest, SetRefClockWithoutReadShouldDisconnect) {
  zx::clock dupe_clock, orig_clock = clock::CloneOfMonotonic();
  ASSERT_EQ(orig_clock.duplicate(kClockRights & ~ZX_RIGHT_READ, &dupe_clock), ZX_OK);

  audio_renderer()->SetReferenceClock(std::move(dupe_clock));
  ExpectDisconnect(audio_renderer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
// Set a custom clock, then try to select the audio_core supplied 'flexible' clock.
TEST_F(AudioRendererClockErrorTest, SetRefClockCustomThenFlexibleShouldDisconnect) {
  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());

  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  ExpectDisconnect(audio_renderer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
// Select the audio_core supplied 'flexible' clock, then try to set a custom clock.
TEST_F(AudioRendererClockErrorTest, SetRefClockFlexibleThenCustomShouldDisconnect) {
  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));

  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  ExpectDisconnect(audio_renderer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
// Set a custom clock, then try to set a different custom clock.
TEST_F(AudioRendererClockErrorTest, SetRefClockSecondCustomShouldDisconnect) {
  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());

  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  ExpectDisconnect(audio_renderer());
}

// Regardless of the type of clock, calling SetReferenceClock a second time should fail.
// Select the audio_core supplied 'flexible' clock, then make the same call a second time.
TEST_F(AudioRendererClockErrorTest, SetRefClockSecondFlexibleShouldDisconnect) {
  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));

  audio_renderer()->SetReferenceClock(zx::clock(ZX_HANDLE_INVALID));
  ExpectDisconnect(audio_renderer());
}

// Setting the reference clock at any time afterSetPcmStreamType should fail
TEST_F(AudioRendererClockErrorTest, SetRefClockAfterSetFormatShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SetReferenceClock(clock::CloneOfMonotonic());
  ExpectDisconnect(audio_renderer());
}

// Once the format is set, setting a ref clock should fail even if post-Pause with no packets.
TEST_F(AudioRendererClockErrorTest, SetRefClockAfterPacketShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SendPacketNoReply(kTestPacket);

  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP);
  audio_renderer()->Pause(AddCallback("Pause"));
  ExpectCallbacks();

  audio_renderer()->DiscardAllPackets(AddCallback("DiscardAllPackets"));
  ExpectCallbacks();

  audio_renderer()->SetReferenceClock(clock::AdjustableCloneOfMonotonic());
  ExpectDisconnect(audio_renderer());
}

class AudioRendererFormatUsageErrorTest : public AudioRendererFormatUsageTest {};

// Once the format has been set, SetUsage may no longer be called any time thereafter.
TEST_F(AudioRendererFormatUsageErrorTest, SetUsageAfterFormatShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SetUsage(*ToFidlRenderUsageTry(AudioRenderUsage2::COMMUNICATION));

  ExpectDisconnect(audio_renderer());
}

// ... this restriction is not lifted even after all packets have been returned.
TEST_F(AudioRendererFormatUsageErrorTest, SetUsageAfterOperatingShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, 0);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  ExpectCallbacks();  // Send a packet and allow it to drain out.

  audio_renderer()->Pause(AddCallback("Pause"));
  ExpectCallbacks();

  audio_renderer()->SetUsage(*ToFidlRenderUsageTry(AudioRenderUsage2::BACKGROUND));

  ExpectDisconnect(audio_renderer());
}

// Calling SetUsage2 with an unknown AudioRenderUsage2 should cause the AudioRenderer to disconnect
TEST_F(AudioRendererFormatUsageErrorTest, SetUsage2UnknownFlexibleEnumShouldDisconnect) {
  audio_renderer()->SetUsage2(static_cast<AudioRenderUsage2>(42));
  ExpectDisconnect(audio_renderer());
}

// Once the format has been set, SetUsage2 may no longer be called any time thereafter.
TEST_F(AudioRendererFormatUsageErrorTest, SetUsage2AfterFormatShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  audio_renderer()->SetUsage2(AudioRenderUsage2::COMMUNICATION);

  ExpectDisconnect(audio_renderer());
}

// ... this restriction is not lifted even after all packets have been returned.
TEST_F(AudioRendererFormatUsageErrorTest, SetUsage2AfterOperatingShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->PlayNoReply(fuchsia::media::NO_TIMESTAMP, 0);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));
  ExpectCallbacks();  // Send a packet and allow it to drain out.

  audio_renderer()->Pause(AddCallback("Pause"));
  ExpectCallbacks();

  audio_renderer()->SetUsage2(AudioRenderUsage2::BACKGROUND);

  ExpectDisconnect(audio_renderer());
}

TEST_F(AudioRendererFormatUsageErrorTest, SetPcmStreamTypeWhileOperatingShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SendPacketNoReply(kTestPacket);

  audio_renderer()->SetPcmStreamType({
      .sample_format = fuchsia::media::AudioSampleFormat::UNSIGNED_8,
      .channels = 1,
      .frames_per_second = 44100,
  });

  ExpectDisconnect(audio_renderer());
}

class AudioRendererTransportErrorTest : public AudioRendererTransportTest {};

// Without a format, Play should not succeed.
TEST_F(AudioRendererTransportErrorTest, PlayWithoutFormatShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP,
                         AddUnexpectedCallback("Play"));
  zx_nanosleep(zx_deadline_after(ZX_MSEC(100)));

  ExpectDisconnect(audio_renderer());
}

// Without a payload buffer, Play should not succeed.
TEST_F(AudioRendererTransportErrorTest, PlayWithoutBufferShouldDisconnect) {
  audio_renderer()->SetPcmStreamType({
      .sample_format = fuchsia::media::AudioSampleFormat::FLOAT,
      .channels = 1,
      .frames_per_second = 32000,
  });

  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP,
                         AddUnexpectedCallback("Play"));
  zx_nanosleep(zx_deadline_after(ZX_MSEC(100)));

  ExpectDisconnect(audio_renderer());
}

TEST_F(AudioRendererTransportErrorTest, PlayWithLargeReferenceTimeShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  constexpr int64_t kLargeTimestamp = std::numeric_limits<int64_t>::max() - 1;
  audio_renderer()->Play(kLargeTimestamp, fuchsia::media::NO_TIMESTAMP,
                         AddUnexpectedCallback("Play"));

  ExpectDisconnect(audio_renderer());
}

TEST_F(AudioRendererTransportErrorTest, PlayWithLargeMediaTimeShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);
  CreateAndAddPayloadBuffer(0);

  // Use 1 tick per 2 frames to overflow the translation from PTS -> frames.
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second / 2, 1);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  constexpr int64_t kLargeTimestamp = (std::numeric_limits<int64_t>::max() / 2) + 1;
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, kLargeTimestamp,
                         AddUnexpectedCallback("Play"));

  ExpectDisconnect(audio_renderer());
}

TEST_F(AudioRendererTransportErrorTest, PlayWithLargeNegativeMediaTimeShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  // Use 1 tick per 2 frames to overflow the translation from PTS -> frames.
  audio_renderer()->SetPtsUnits(kTestStreamType.frames_per_second / 2, 1);

  audio_renderer()->SendPacket(kTestPacket, AddCallback("SendPacket"));

  constexpr int64_t kLargeTimestamp = std::numeric_limits<int64_t>::min() + 1;
  audio_renderer()->Play(fuchsia::media::NO_TIMESTAMP, kLargeTimestamp,
                         AddUnexpectedCallback("Play"));

  ExpectDisconnect(audio_renderer());
}

// Without a format, Pause should not succeed.
TEST_F(AudioRendererTransportErrorTest, PauseWithoutFormatShouldDisconnect) {
  CreateAndAddPayloadBuffer(0);

  audio_renderer()->Pause(AddUnexpectedCallback("Pause"));
  zx_nanosleep(zx_deadline_after(ZX_MSEC(100)));

  ExpectDisconnect(audio_renderer());
}

// Without a payload buffer, Pause should not succeed.
TEST_F(AudioRendererTransportErrorTest, PauseWithoutBufferShouldDisconnect) {
  audio_renderer()->SetPcmStreamType(kTestStreamType);

  audio_renderer()->Pause(AddUnexpectedCallback("Pause"));
  zx_nanosleep(zx_deadline_after(ZX_MSEC(100)));

  ExpectDisconnect(audio_renderer());
}

}  // namespace
}  // namespace media::audio::test

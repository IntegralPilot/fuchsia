// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_CAPTURER_TEST_SHARED_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_CAPTURER_TEST_SHARED_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/media/audio/cpp/types.h>
#include <lib/zx/clock.h>

#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"

namespace media::audio::test {

//
// AudioCapturerTestOldAPI
//
// "OldAPI" because these tests haven't been updated to use the new HermeticAudioTest Create
// functions.
class AudioCapturerTestOldAPI : public HermeticAudioTest {
 protected:
  void SetUp() override {
    HermeticAudioTest::SetUp();

    audio_core_->CreateAudioCapturer(false, audio_capturer_.NewRequest());
    AddErrorHandler(audio_capturer_, "AudioCapturer");
  }

  void TearDown() override {
    gain_control_.Unbind();
    audio_capturer_.Unbind();

    HermeticAudioTest::TearDown();
  }

  void SetFormat(int32_t frames_per_second = 16000) {
    auto t = media::CreateAudioStreamType(fuchsia::media::AudioSampleFormat::SIGNED_16, 1,
                                          frames_per_second);
    format_ = Format::Create(t).take_value();
    audio_capturer_->SetPcmStreamType(t);
  }

  void SetUpPayloadBuffer(uint32_t payload_buffer_id = 0, int64_t num_frames = 16000,
                          zx::vmo* vmo_out = nullptr) {
    zx::vmo audio_capturer_vmo;

    auto status = zx::vmo::create(num_frames * sizeof(int16_t), 0, &audio_capturer_vmo);
    ASSERT_EQ(status, ZX_OK) << "Failed to create payload buffer";

    if (vmo_out) {
      status = audio_capturer_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, vmo_out);
      ASSERT_EQ(status, ZX_OK);
    }

    audio_capturer_->AddPayloadBuffer(payload_buffer_id, std::move(audio_capturer_vmo));
  }

  std::optional<Format>& format() { return format_; }
  fuchsia::media::AudioCapturerPtr& audio_capturer() { return audio_capturer_; }
  fuchsia::media::audio::GainControlPtr& gain_control() { return gain_control_; }

 private:
  std::optional<Format> format_;
  fuchsia::media::AudioCapturerPtr audio_capturer_;
  fuchsia::media::audio::GainControlPtr gain_control_;
};

class AudioCapturerClockTestOldAPI : public AudioCapturerTestOldAPI {
 protected:
  // The clock received from GetRefClock is read-only, but the original can still be adjusted.
  static constexpr auto kClockRights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_READ;

  zx::clock GetAndValidateReferenceClock() {
    zx::clock clock;

    audio_capturer()->GetReferenceClock(
        AddCallback("GetReferenceClock",
                    [&clock](zx::clock received_clock) { clock = std::move(received_clock); }));

    ExpectCallbacks();

    return clock;
  }

  // Wait for >1 mix job, because clock-related disconnects start in SetReferenceClock on the FIDL
  // thread, but must transfer to the mix thread to complete. Although our deadline-scheduler mix
  // period is 10ms, our mix thread will not become idle until it finishes the entire mix job.
  // Capture mix jobs can be as long as 50ms (see kMaxTimePerCapture in base_capturer.cc).
  void WaitBeforeClockRelatedDisconnect() {
    usleep(150 * 1000);  // 3 x 50ms.

    audio_capturer()->GetReferenceClock([](zx::clock) {});
  }
};

// A simple fixture that uses the new HermeticAudioTest Create methods instead of raw
// FIDL InterfacePtrs (cf. AudioCapturerTestOldAPI).
class AudioCapturerTest : public HermeticAudioTest {};

}  // namespace media::audio::test

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_AUDIO_CAPTURER_TEST_SHARED_H_

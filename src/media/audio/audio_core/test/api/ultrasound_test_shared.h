// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_ULTRASOUND_TEST_SHARED_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_ULTRASOUND_TEST_SHARED_H_

#include "src/media/audio/audio_core/testing/integration/hermetic_audio_test.h"
#include "src/media/audio/audio_core/testing/integration/renderer_shim.h"

namespace media::audio::test {

constexpr uint32_t kUltrasoundSampleRate = 96000;
constexpr uint32_t kUltrasoundChannels = 2;
constexpr uint32_t kBufferSize = kUltrasoundSampleRate;  // 1s buffers

constexpr fuchsia::media::AudioSampleFormat kSampleFormat =
    fuchsia::media::AudioSampleFormat::FLOAT;

static const auto kUltrasoundFormat =
    Format::Create<kSampleFormat>(kUltrasoundChannels, kUltrasoundSampleRate).value();

// This matches the configuration in ultrasound_audio_core_config.json
static const audio_stream_unique_id_t kUltrasoundOutputDeviceId = {{
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
    0xff,
}};
static const audio_stream_unique_id_t kUltrasoundInputDeviceId = {{
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
    0xee,
}};

class UltrasoundTest : public HermeticAudioTest {
 protected:
  static void SetUpTestSuite() {
    HermeticAudioTest::SetTestSuiteRealmOptions([] {
      return HermeticAudioRealm::Options{
          .audio_core_config_data = MakeAudioCoreConfig({
              .output_device_config = R"x(
                "device_id": "*",
                "supported_stream_types": [
                  "render:accessibility",
                  "render:background",
                  "render:communication",
                  "render:interruption",
                  "render:media",
                  "render:system_agent",
                  "render:ultrasound",
                  "capture:loopback"
                ],
                "pipeline":  {
                  "name": "linearize",
                  "streams": [
                    "render:ultrasound"
                  ],
                  "output_rate": 96000,
                  "output_channels": 2,
                  "inputs": [
                    {
                      "name": "mix",
                      "streams": [
                        "render:accessibility",
                        "render:background",
                        "render:communication",
                        "render:interruption",
                        "render:media",
                        "render:system_agent"
                      ],
                      "output_rate": 96000,
                      "loopback": true
                    }
                  ]
                }
              )x",
              .input_device_config = R"x(
                "device_id": "*",
                "supported_stream_types": [
                  "capture:background",
                  "capture:communication",
                  "capture:foreground",
                  "capture:system_agent",
                  "capture:ultrasound"
                ],
                "rate": 96000
              )x",
          }),
      };
    });
  }

  VirtualOutput<kSampleFormat>* CreateOutput() {
    return HermeticAudioTest::CreateOutput(kUltrasoundOutputDeviceId, kUltrasoundFormat,
                                           kBufferSize);
  }

  VirtualInput<kSampleFormat>* CreateInput() {
    return HermeticAudioTest::CreateInput(kUltrasoundInputDeviceId, kUltrasoundFormat, kBufferSize);
  }

  UltrasoundRendererShim<kSampleFormat>* CreateRenderer() {
    return HermeticAudioTest::CreateUltrasoundRenderer(kUltrasoundFormat, kBufferSize);
  }

  UltrasoundCapturerShim<kSampleFormat>* CreateCapturer() {
    return HermeticAudioTest::CreateUltrasoundCapturer(kUltrasoundFormat, kBufferSize);
  }
};

}  // namespace media::audio::test

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_TEST_API_ULTRASOUND_TEST_SHARED_H_

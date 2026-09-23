// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/process_config_loader.h"

#include <lib/zx/time.h>

#include <iostream>
#include <ostream>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"

namespace media::audio {
namespace {

constexpr char kTestAudioCoreConfigFilename[] = "/tmp/audio_core_config.json";

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithOnlyVolumeCurve) {
  static const std::string kConfigWithVolumeCurve =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithVolumeCurve.data(),
                               kConfigWithVolumeCurve.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();
  const auto config = result.take_value();
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(0.0), -160.0);
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(1.0), 0.0);
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithMutedVolumeCurve) {
  static const std::string kConfigWithVolumeCurve =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": "MUTED"
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithVolumeCurve.data(),
                               kConfigWithVolumeCurve.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();
  const auto config = result.take_value();
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(0.0), -160.0);
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(1.0), 0.0);
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithMixProfile) {
  static const std::string kConfigWithMixProfile =
      R"JSON({
    "mix_profile": {
        "capacity_usec": 1000,
        "deadline_usec": 2000,
        "period_usec": 3000
    },
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithMixProfile.data(),
                               kConfigWithMixProfile.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();
  const auto config = result.take_value();
  EXPECT_EQ(config.mix_profile_config().capacity.to_usecs(), 1000);
  EXPECT_EQ(config.mix_profile_config().deadline.to_usecs(), 2000);
  EXPECT_EQ(config.mix_profile_config().period.to_usecs(), 3000);
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(0.0), -160.0);
  EXPECT_FLOAT_EQ(config.default_volume_curve().VolumeToDb(1.0), 0.0);
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithRoutingPolicy) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "capture:loopback"
        ]
      },
      {
        "device_id": "*",
        "supported_stream_types": [
          "render:media",
          "render:system_agent"
        ],
        "independent_volume_control": true
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};
  const audio_stream_unique_id_t unknown_id = {.data = {0x32, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                        0x81, 0x42, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                        0x22, 0x3a}};

  const auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config();

  EXPECT_TRUE(config.output_device_profile(expected_id).supports_usage(RenderUsage::INTERRUPTION));
  EXPECT_TRUE(config.output_device_profile(expected_id).supports_usage(RenderUsage::MEDIA));
  EXPECT_FALSE(config.output_device_profile(expected_id).supports_usage(RenderUsage::SYSTEM_AGENT));

  EXPECT_FALSE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::INTERRUPTION));
  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::MEDIA));

  EXPECT_TRUE(config.output_device_profile(expected_id).eligible_for_loopback());
  EXPECT_FALSE(config.output_device_profile(unknown_id).eligible_for_loopback());

  EXPECT_FALSE(config.output_device_profile(expected_id).independent_volume_control());
  EXPECT_TRUE(config.output_device_profile(unknown_id).independent_volume_control());
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithRoutingMultipleDeviceIds) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": ["34384e7da9d52c8062a9765baeb6053a", "34384e7da9d52c8062a9765baeb6053b" ],
        "supported_stream_types": [
          "render:media"
        ]
      },
      {
        "device_id": "*",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t expected_id1 = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                          0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                          0x05, 0x3a}};
  const audio_stream_unique_id_t expected_id2 = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                          0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                          0x05, 0x3b}};

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config();
  for (const auto& device_id : {expected_id1, expected_id2}) {
    EXPECT_FALSE(config.output_device_profile(device_id).supports_usage(RenderUsage::BACKGROUND));
    EXPECT_FALSE(
        config.output_device_profile(device_id).supports_usage(RenderUsage::COMMUNICATION));
    EXPECT_FALSE(config.output_device_profile(device_id).supports_usage(RenderUsage::INTERRUPTION));
    EXPECT_TRUE(config.output_device_profile(device_id).supports_usage(RenderUsage::MEDIA));
    EXPECT_FALSE(config.output_device_profile(device_id).supports_usage(RenderUsage::SYSTEM_AGENT));

    EXPECT_FALSE(config.output_device_profile(device_id).eligible_for_loopback());
    EXPECT_FALSE(config.output_device_profile(device_id).independent_volume_control());
  }
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithRoutingPolicyNoDefault) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "render:ultrasound",
          "capture:loopback"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t unknown_id = {.data = {0x32, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                        0x81, 0x42, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                        0x22, 0x3a}};

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config();

  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::BACKGROUND));
  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::COMMUNICATION));
  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::INTERRUPTION));
  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::MEDIA));
  EXPECT_TRUE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::SYSTEM_AGENT));
  EXPECT_FALSE(config.output_device_profile(unknown_id).supports_usage(RenderUsage::ULTRASOUND));

  EXPECT_TRUE(config.output_device_profile(unknown_id).eligible_for_loopback());
}

TEST(ProcessConfigLoaderTest, RejectConfigWithUnknownStreamTypes) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "render:invalid"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_THAT(result.error(), ::testing::HasSubstr("Parse error: Schema validation error"));
  ASSERT_THAT(
      result.error(),
      ::testing::HasSubstr("\"instanceRef\": \"#/output_devices/0/supported_stream_types/6\""));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"schemaRef\": \"#/definitions/stream_type\""));
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithRoutingPolicyInsufficientCoverage) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error(),
            "Parse error: Failed to parse output device policies: No output to support usage "
            "RenderUsage::BACKGROUND");
}

TEST(ProcessConfigLoaderTest, AllowConfigWithoutUltrasound) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithOutputGains) {
  static const std::string kConfigWithDriverGain =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ],
        "driver_gain_db": -6.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithDriverGain.data(),
                               kConfigWithDriverGain.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};
  const audio_stream_unique_id_t unknown_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                        0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                        0x05, 0x3b}};
  auto& config = result.value().device_config();
  ASSERT_TRUE(config.output_device_profile(expected_id).driver_gain_db());
  EXPECT_FLOAT_EQ(*config.output_device_profile(expected_id).driver_gain_db(), -6.0f);
  EXPECT_FALSE(config.output_device_profile(unknown_id).driver_gain_db());
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithInputGains) {
  static const std::string kConfigWithDriverGain =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "input_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "capture:background"
        ],
        "rate": 96000,
        "driver_gain_db": -6.0,
        "software_gain_db": -8.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithDriverGain.data(),
                               kConfigWithDriverGain.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};
  const audio_stream_unique_id_t unknown_id = {.data = {0x32, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                        0x81, 0x42, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                        0x22, 0x3a}};
  auto& config = result.value().device_config();
  ASSERT_TRUE(config.input_device_profile(expected_id).driver_gain_db());
  EXPECT_FLOAT_EQ(*config.input_device_profile(expected_id).driver_gain_db(), -6.0f);
  EXPECT_FLOAT_EQ(config.input_device_profile(expected_id).software_gain_db(), -8.0f);
  EXPECT_FALSE(config.input_device_profile(unknown_id).driver_gain_db());
  EXPECT_FLOAT_EQ(config.input_device_profile(unknown_id).software_gain_db(), 0.0f);
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithInputDevices) {
  static const std::string kConfigWithInputDevices =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "input_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "capture:background"
        ],
        "rate": 96000
      },
      {
        "device_id": "*",
        "rate": 24000
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithInputDevices.data(),
                               kConfigWithInputDevices.size()));

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};
  const audio_stream_unique_id_t unknown_id = {.data = {0x32, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                        0x81, 0x42, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                        0x22, 0x3a}};

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config();

  EXPECT_EQ(config.input_device_profile(expected_id).rate(), 96000u);
  EXPECT_TRUE(config.input_device_profile(expected_id)
                  .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::BACKGROUND)));
  EXPECT_FALSE(config.input_device_profile(expected_id)
                   .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::COMMUNICATION)));
  EXPECT_FALSE(config.input_device_profile(expected_id)
                   .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND)));
  EXPECT_FALSE(config.input_device_profile(expected_id)
                   .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::SYSTEM_AGENT)));
  EXPECT_FALSE(config.input_device_profile(expected_id)
                   .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::ULTRASOUND)));
  EXPECT_EQ(config.input_device_profile(unknown_id).rate(), 24000u);
  EXPECT_TRUE(config.input_device_profile(unknown_id)
                  .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::BACKGROUND)));
  EXPECT_TRUE(config.input_device_profile(unknown_id)
                  .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::COMMUNICATION)));
  EXPECT_TRUE(config.input_device_profile(unknown_id)
                  .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::FOREGROUND)));
  EXPECT_TRUE(config.input_device_profile(unknown_id)
                  .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::SYSTEM_AGENT)));
  EXPECT_FALSE(config.input_device_profile(unknown_id)
                   .supports_usage(StreamUsage::WithCaptureUsage(CaptureUsage::ULTRASOUND)));
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithEffectsV1) {
  static const std::string kConfigWithEffects =
      R"JSON({
    "volume_curve": [
      { "level": 0.0, "db": -160.0 },
      { "level": 1.0, "db": 0.0 }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ],
        "pipeline": {
          "streams": [
            "render:accessibility",
            "render:background",
            "render:interruption",
            "render:media",
            "render:system_agent"
          ],
          "output_rate": 96000,
          "output_channels": 4,
          "effects": [
            {
              "lib": "libbar2.so",
              "effect": "linearize_effect",
              "name": "instance_name",
              "_comment": "just a comment",
              "config": {
                "a": 123,
                "b": 456
              }
            }
          ],
          "inputs": [
            {
              "streams": [],
              "loopback": true,
              "output_rate": 48000,
              "effects": [
                {
                  "lib": "libfoo2.so",
                  "effect": "effect3",
                  "name": "ef3",
                  "output_channels": 4
                }
              ],
              "inputs": [
                {
                  "streams": [
                    "render:media"
                  ],
                  "name": "media",
                  "effects": [
                    {
                      "lib": "libfoo.so",
                      "effect": "effect1",
                      "name": "ef1",
                      "config": {
                        "some_config": 0
                      }
                    },
                    {
                      "lib": "libbar.so",
                      "effect": "effect2",
                      "name": "ef2",
                      "config": {
                        "arg1": 55,
                        "arg2": 3.14
                      }
                    }
                  ]
                },
                {
                  "streams": [
                    "render:communication"
                  ],
                  "name": "communication",
                  "min_gain_db": -30,
                  "max_gain_db": -20,
                  "effects": [
                    {
                      "lib": "libbaz.so",
                      "effect": "baz",
                      "name": "baz",
                      "_comment": "Ignore me",
                      "config": {
                        "string_param": "some string value"
                      }
                    }
                  ]
                }
              ]
            }
          ]
        }
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithEffects.data(),
                               kConfigWithEffects.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const audio_stream_unique_id_t device_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                       0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                       0x05, 0x3a}};
  const auto& config = result.value();
  const auto& root =
      config.device_config().output_device_profile(device_id).pipeline_config().root();
  {  // 'linearize' mix_group
    const auto& mix_group = root;
    EXPECT_EQ("", mix_group.name);
    EXPECT_EQ(5u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::ACCESSIBILITY, mix_group.input_streams[0]);
    EXPECT_EQ(RenderUsage::BACKGROUND, mix_group.input_streams[1]);
    EXPECT_EQ(RenderUsage::INTERRUPTION, mix_group.input_streams[2]);
    EXPECT_EQ(RenderUsage::MEDIA, mix_group.input_streams[3]);
    EXPECT_EQ(RenderUsage::SYSTEM_AGENT, mix_group.input_streams[4]);
    ASSERT_EQ(1u, mix_group.effects_v1.size());
    {
      const auto& effect = mix_group.effects_v1[0];
      EXPECT_EQ("libbar2.so", effect.lib_name);
      EXPECT_EQ("linearize_effect", effect.effect_name);
      EXPECT_EQ("instance_name", effect.instance_name);
      EXPECT_EQ("{\"a\":123,\"b\":456}", effect.effect_config);
      EXPECT_FALSE(effect.output_channels);
    }
    ASSERT_EQ(1u, mix_group.inputs.size());
    ASSERT_FALSE(mix_group.loopback);
    ASSERT_EQ(96000, mix_group.output_rate);
    EXPECT_EQ(4, mix_group.output_channels);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  const auto& mix = root.inputs[0];
  {  // 'mix' mix_group
    const auto& mix_group = mix;
    EXPECT_EQ("", mix_group.name);
    EXPECT_EQ(0u, mix_group.input_streams.size());
    ASSERT_EQ(1u, mix_group.effects_v1.size());
    {
      const auto& effect = mix_group.effects_v1[0];
      EXPECT_EQ("libfoo2.so", effect.lib_name);
      EXPECT_EQ("effect3", effect.effect_name);
      EXPECT_EQ("ef3", effect.instance_name);
      EXPECT_EQ("", effect.effect_config);
      EXPECT_TRUE(effect.output_channels);
      EXPECT_EQ(4, *effect.output_channels);
    }
    ASSERT_EQ(2u, mix_group.inputs.size());
    ASSERT_TRUE(mix_group.loopback);
    ASSERT_EQ(48000, mix_group.output_rate);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  {  // output mix_group 1
    const auto& mix_group = mix.inputs[0];
    EXPECT_EQ("media", mix_group.name);
    EXPECT_EQ(1u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::MEDIA, mix_group.input_streams[0]);
    ASSERT_EQ(2u, mix_group.effects_v1.size());
    {
      const auto& effect = mix_group.effects_v1[0];
      EXPECT_EQ("libfoo.so", effect.lib_name);
      EXPECT_EQ("effect1", effect.effect_name);
      EXPECT_EQ("ef1", effect.instance_name);
      EXPECT_EQ("{\"some_config\":0}", effect.effect_config);
      EXPECT_FALSE(effect.output_channels);
    }
    {
      const auto& effect = mix_group.effects_v1[1];
      EXPECT_EQ("libbar.so", effect.lib_name);
      EXPECT_EQ("effect2", effect.effect_name);
      EXPECT_EQ("ef2", effect.instance_name);
      EXPECT_EQ("{\"arg1\":55,\"arg2\":3.14}", effect.effect_config);
      EXPECT_FALSE(effect.output_channels);
    }
    ASSERT_FALSE(mix_group.loopback);
    EXPECT_EQ(48000, mix_group.output_rate);
    EXPECT_EQ(2, mix_group.output_channels);
    ASSERT_EQ(PipelineConfig::kDefaultMixGroupRate, mix_group.output_rate);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  {  // output mix_group 2
    const auto& mix_group = mix.inputs[1];
    EXPECT_EQ("communication", mix_group.name);
    EXPECT_EQ(1u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::COMMUNICATION, mix_group.input_streams[0]);
    ASSERT_EQ(1u, mix_group.effects_v1.size());
    {
      const auto& effect = mix_group.effects_v1[0];
      EXPECT_EQ("libbaz.so", effect.lib_name);
      EXPECT_EQ("baz", effect.effect_name);
      EXPECT_EQ("baz", effect.instance_name);
      EXPECT_EQ("{\"string_param\":\"some string value\"}", effect.effect_config);
      EXPECT_FALSE(effect.output_channels);
    }
    ASSERT_FALSE(mix_group.loopback);
    EXPECT_EQ(48000, mix_group.output_rate);
    EXPECT_EQ(2, mix_group.output_channels);
    ASSERT_EQ(PipelineConfig::kDefaultMixGroupRate, mix_group.output_rate);
    ASSERT_TRUE(mix_group.min_gain_db.has_value());
    ASSERT_TRUE(mix_group.max_gain_db.has_value());
    EXPECT_EQ(-30.0f, *mix_group.min_gain_db);
    EXPECT_EQ(-20.0f, *mix_group.max_gain_db);
  }
}

TEST(ProcessConfigLoaderTest, LoadProcessConfigWithEffectsV2) {
  static const std::string kConfigWithEffects =
      R"JSON({
    "volume_curve": [
      { "level": 0.0, "db": -160.0 },
      { "level": 1.0, "db": 0.0 }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ],
        "pipeline": {
          "streams": [
          "render:accessibility",
          "render:background",
          "render:interruption",
          "render:media",
          "render:system_agent"
          ],
          "output_rate": 96000,
          "output_channels": 4,
          "effect_over_fidl": {
            "name": "effect1"
          },
          "inputs": [
            {
              "streams": [],
              "loopback": true,
              "output_rate": 48000,
              "effect_over_fidl": {
                "name": "effect2"
              },
              "inputs": [
                {
                  "streams": [
                    "render:media"
                  ],
                  "name": "media",
                  "effect_over_fidl": {
                    "name": "effect3"
                  }
                },
                {
                  "streams": [
                    "render:communication"
                  ],
                  "name": "communication",
                  "min_gain_db": -30,
                  "max_gain_db": -20,
                  "effect_over_fidl": {
                    "name": "effect4"
                  }
                }
              ]
            }
          ]
        }
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithEffects.data(),
                               kConfigWithEffects.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  const audio_stream_unique_id_t device_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                       0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                       0x05, 0x3a}};
  const auto& config = result.value();
  const auto& root =
      config.device_config().output_device_profile(device_id).pipeline_config().root();
  {  // 'effect1' mix_group
    const auto& mix_group = root;
    EXPECT_EQ("", mix_group.name);
    EXPECT_EQ(5u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::ACCESSIBILITY, mix_group.input_streams[0]);
    EXPECT_EQ(RenderUsage::BACKGROUND, mix_group.input_streams[1]);
    EXPECT_EQ(RenderUsage::INTERRUPTION, mix_group.input_streams[2]);
    EXPECT_EQ(RenderUsage::MEDIA, mix_group.input_streams[3]);
    EXPECT_EQ(RenderUsage::SYSTEM_AGENT, mix_group.input_streams[4]);
    ASSERT_TRUE(mix_group.effects_v2.has_value());
    {
      const auto& effect = mix_group.effects_v2.value();
      EXPECT_EQ("effect1", effect.instance_name);
    }
    ASSERT_EQ(1u, mix_group.inputs.size());
    ASSERT_FALSE(mix_group.loopback);
    ASSERT_EQ(96000, mix_group.output_rate);
    EXPECT_EQ(4, mix_group.output_channels);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  const auto& mix = root.inputs[0];
  {  // 'effect2' mix_group
    const auto& mix_group = mix;
    EXPECT_EQ("", mix_group.name);
    EXPECT_EQ(0u, mix_group.input_streams.size());
    ASSERT_TRUE(mix_group.effects_v2.has_value());
    {
      const auto& effect = mix_group.effects_v2.value();
      EXPECT_EQ("effect2", effect.instance_name);
    }
    ASSERT_EQ(2u, mix_group.inputs.size());
    ASSERT_TRUE(mix_group.loopback);
    ASSERT_EQ(48000, mix_group.output_rate);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  {  // 'effect3' mix group
    const auto& mix_group = mix.inputs[0];
    EXPECT_EQ("media", mix_group.name);
    EXPECT_EQ(1u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::MEDIA, mix_group.input_streams[0]);
    ASSERT_TRUE(mix_group.effects_v2.has_value());
    {
      const auto& effect = mix_group.effects_v2.value();
      EXPECT_EQ("effect3", effect.instance_name);
    }
    ASSERT_FALSE(mix_group.loopback);
    EXPECT_EQ(48000, mix_group.output_rate);
    EXPECT_EQ(2, mix_group.output_channels);
    ASSERT_EQ(PipelineConfig::kDefaultMixGroupRate, mix_group.output_rate);
    ASSERT_FALSE(mix_group.min_gain_db.has_value());
    ASSERT_FALSE(mix_group.max_gain_db.has_value());
  }

  {  // 'effect4' mix_group 2
    const auto& mix_group = mix.inputs[1];
    EXPECT_EQ("communication", mix_group.name);
    EXPECT_EQ(1u, mix_group.input_streams.size());
    EXPECT_EQ(RenderUsage::COMMUNICATION, mix_group.input_streams[0]);
    ASSERT_TRUE(mix_group.effects_v2.has_value());
    {
      const auto& effect = mix_group.effects_v2.value();
      EXPECT_EQ("effect4", effect.instance_name);
    }
    ASSERT_FALSE(mix_group.loopback);
    EXPECT_EQ(48000, mix_group.output_rate);
    EXPECT_EQ(2, mix_group.output_channels);
    ASSERT_EQ(PipelineConfig::kDefaultMixGroupRate, mix_group.output_rate);
    ASSERT_TRUE(mix_group.min_gain_db.has_value());
    ASSERT_TRUE(mix_group.max_gain_db.has_value());
    EXPECT_EQ(-30.0f, *mix_group.min_gain_db);
    EXPECT_EQ(-20.0f, *mix_group.max_gain_db);
  }
}

TEST(ProcessConfigLoaderTest, FileNotFound) {
  const auto result = ProcessConfigLoader::LoadProcessConfig("not-present-file");
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error(), "File does not exist");
}

TEST(ProcessConfigLoaderTest, RejectConfigWithoutVolumeCurve) {
  static const std::string kConfigWithoutVolumeCurve = R"JSON({  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithoutVolumeCurve.data(),
                               kConfigWithoutVolumeCurve.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_THAT(result.error(), ::testing::HasSubstr("Parse error: Schema validation error"));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"required\":"));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"missing\": ["));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"volume_curve\""));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"instanceRef\": \"#\""));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"schemaRef\": \"#\""));
}

TEST(ProcessConfigLoaderTest, RejectConfigWithUnknownKeys) {
  static const std::string kConfigWithExtraKeys =
      R"JSON({
    "extra_key": 3,
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithExtraKeys.data(),
                               kConfigWithExtraKeys.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_THAT(result.error(), ::testing::HasSubstr("Parse error: Schema validation error"));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"additionalProperties\":"));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"disallowed\": \"extra_key\""));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"instanceRef\": \"#\""));
  ASSERT_THAT(result.error(), ::testing::HasSubstr("\"schemaRef\": \"#\""));
}

TEST(ProcessConfigLoaderTest, RejectConfigWithMultipleLoopbackStages) {
  static const std::string kConfigWithVolumeCurve =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ],
        "pipeline": {
          "inputs": [
            {
              "streams": [
                "render:accessibility",
                "render:background",
                "render:interruption",
                "render:media",
                "render:system_agent"
              ],
              "loopback": true
            }, {
              "streams": [
                "render:communication"
              ],
              "loopback": true
            }
          ]
        }
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithVolumeCurve.data(),
                               kConfigWithVolumeCurve.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(
      result.error(),
      "Parse error: Failed to parse output device policies: More than 1 loopback stage specified");
}

TEST(ProcessConfigLoaderTest, RejectConfigWithoutLoopbackPointSpecified) {
  static const std::string kConfigWithVolumeCurve =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent",
          "capture:loopback"
        ],
        "pipeline": {
          "streams": [
            "render:accessibility",
            "render:background",
            "render:communication",
            "render:interruption",
            "render:media",
            "render:system_agent"
          ]
        }
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithVolumeCurve.data(),
                               kConfigWithVolumeCurve.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error(),
            "Parse error: Failed to parse output device policies: Device supports loopback but no "
            "loopback point specified");
}

TEST(ProcessConfigLoaderTest, RejectConfigsWithInvalidChannelCount) {
  const auto& CreateConfig = [](int mix_stage_chans, int effect_chans) {
    std::ostringstream oss;
    oss << R"JSON({
      "volume_curve": [
        {
            "level": 0.0,
            "db": -160.0
        },
        {
            "level": 1.0,
            "db": 0.0
        }
      ],
      "output_devices": [
        {
          "device_id": "*",
          "supported_stream_types": [
            "render:accessibility",
            "render:background",
            "render:communication",
            "render:interruption",
            "render:media",
            "render:system_agent"
          ],
          "pipeline": {
            "streams": [
              "render:accessibility",
              "render:background",
              "render:communication",
              "render:interruption",
              "render:media",
              "render:system_agent"
            ],
            "output_channels": )JSON"
        << mix_stage_chans << R"JSON(,
            "effects": [
              {
                "lib": "fake_effects.so",
                "effect": "effect1",
                "name": "ef1",
                "output_channels": )JSON"
        << effect_chans << R"JSON(,
                "config": { }
              }
            ]
          }
        }
      ]
    })JSON";
    return oss.str();
  };

  // Sanity test our CreateConfig can build a valid config.
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(1, 1)).is_ok());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(8, 8)).is_ok());

  // Now verify we reject channel counts outside the range of [1, 8].
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(0, 1)).is_error());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(1, 0)).is_error());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(-1, 2)).is_error());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(2, -1)).is_error());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(8, 9)).is_error());
  EXPECT_TRUE(ProcessConfigLoader::ParseProcessConfig(CreateConfig(9, 8)).is_error());
}

TEST(ProcessConfigLoaderTest, RejectConfigWithInvalidDefaultVolumeRenderUsages) {
  static const std::string kConfigWithInvalidRenderUsages = R"JSON({
      "volume_curve": [
        {
            "level": 0.0,
            "db": -160.0
        },
        {
            "level": 1.0,
            "db": 0.0
        }
      ],
    "default_render_usage_volumes": {
      "invalid": 0.0
    }
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithInvalidRenderUsages.data(),
                               kConfigWithInvalidRenderUsages.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_error());
}

TEST(ProcessConfigLoaderTest, LoadThermalConfigWithStates) {
  static const std::string kConfigWithThermalStates =
      R"JSON({
    "volume_curve": [
        {
            "level": 0.0,
            "db": -160.0
        },
        {
            "level": 1.0,
            "db": 0.0
        }
    ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": {
                "target name A": {
                  "value": "state 0 config A"
                },
                "target name B": {
                  "value": "state 0 config B"
                }
            }
        },
        {
            "state_number": 1,
            "effect_configs": {
                "target name A": {
                  "value": "state 1 config A"
                },
                "target name B": {
                  "value": "state 1 config B"
                }
            }
        },
        {
            "state_number": 2,
            "effect_configs": {
                "target name A": {
                  "value": "state 2 config A"
                },
                "target name B": {
                  "value": "state 2 config B"
                }
            }
        }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithThermalStates.data(),
                               kConfigWithThermalStates.size()));

  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto config = result.value();
  const auto& states = config.thermal_config().states();
  EXPECT_EQ(3u, states.size());

  EXPECT_EQ(0u, states[0].thermal_state_number());
  EXPECT_EQ(2u, states[0].effect_configs().size());

  EXPECT_EQ("target name A", states[0].effect_configs()[0].name());
  EXPECT_EQ("{\"value\":\"state 0 config A\"}", states[0].effect_configs()[0].config_string());
  EXPECT_EQ("target name B", states[0].effect_configs()[1].name());
  EXPECT_EQ("{\"value\":\"state 0 config B\"}", states[0].effect_configs()[1].config_string());

  EXPECT_EQ(2u, states[1].effect_configs().size());
  EXPECT_EQ("target name A", states[1].effect_configs()[0].name());
  EXPECT_EQ("{\"value\":\"state 1 config A\"}", states[1].effect_configs()[0].config_string());
  EXPECT_EQ("target name B", states[1].effect_configs()[1].name());
  EXPECT_EQ("{\"value\":\"state 1 config B\"}", states[1].effect_configs()[1].config_string());

  EXPECT_EQ(2u, states[2].effect_configs().size());
  EXPECT_EQ("target name A", states[2].effect_configs()[0].name());
  EXPECT_EQ("{\"value\":\"state 2 config A\"}", states[2].effect_configs()[0].config_string());
  EXPECT_EQ("target name B", states[2].effect_configs()[1].name());
  EXPECT_EQ("{\"value\":\"state 2 config B\"}", states[2].effect_configs()[1].config_string());
}

// Attempt bad thermal configs of various types. All should return an error result.
TEST(ProcessConfigLoaderTest, MalformedThermalConfigs) {
  std::vector<std::pair<std::string, std::string>> kMalformedThermalConfigs{
      {"thermal_states is not an array",
       R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": "not an array"
    })JSON"},
      /*
      // thermal_states is empty array
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": []
    })JSON",
      // thermal state entry is not an object
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        "not an object"
    ]
    })JSON",
      // thermal state entry has no state_number key
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "effect_configs": { "config_key": "config_value" }
        }
    ]
    })JSON",
      // state_number is not an integer
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 1.5,
            "effect_configs": { "key": "value" }
        }
    ]
    })JSON",
      // state_number is negative
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": -42,
            "effect_configs": { "key": "value" }
        }
    ]
    })JSON",
      // state_number 0 not found
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 1,
            "effect_configs": { "effect_key": "effect_value" },
        }
    ]
    })JSON",
      // no effect_configs key
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
        }
    ]
    })JSON",
      // effect_configs is not an object
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": "not an object"
        }
    ]
    })JSON",
      // effect_configs object has no keys
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": {}
        }
    ]
    })JSON",
      // effect_configs entry: key is not a string
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": {
                0: {}
            }
        }
    ]
    })JSON",
      // effect_config entry: val is not an object
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": {
                "config_key": "not an object"
            }
        }
    ]
    })JSON",
      // states with different number of effect_configs
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": { "effect1_key": "effect1_value1", "effect2_key": "effect2_value" },
        },
        {
            "state_number": 1,
            "effect_configs": { "effect1_key": "effect1_value2" },
        }
    ]
    })JSON",
      // states with different effect_config names
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": { "effect1_key": "effect1_value" },
        },
        {
            "state_number": 1,
            "effect_configs": { "effect2_key": "effect2_value" },
        }
    ]
    })JSON",
      // unexpected additional key in thermal_states
      R"JSON({
    "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
    "thermal_states": [
        {
            "state_number": 0,
            "effect_configs": { "effect_key": "config_value" },
            "not_effect_configs":  { "key": "value" }
        }
    ]
    })JSON",
    */
  };

  for (auto idx = 0u; idx < kMalformedThermalConfigs.size(); ++idx) {
    const auto [bad_case_name, bad_config] = kMalformedThermalConfigs[idx];
    ASSERT_TRUE(
        files::WriteFile(kTestAudioCoreConfigFilename, bad_config.data(), bad_config.size()))
        << "case " << idx << " (" << bad_case_name << ") could not write file" << '\n';

    auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
    EXPECT_TRUE(result.is_error()) << "'" << bad_case_name << "'";
  }
}

TEST(ProcessConfigLoaderTest, LoadOutputDevicePolicyWithDefaultPipeline) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": [
          "render:media",
          "capture:loopback"
        ]
      },
      {
        "device_id": "*",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};

  const auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config().output_device_profile(expected_id);
  EXPECT_TRUE(config.pipeline_config().root().loopback);
  EXPECT_TRUE(config.pipeline_config().root().effects_v1.empty());
  EXPECT_TRUE(config.pipeline_config().root().inputs.empty());
  EXPECT_EQ(PipelineConfig::kDefaultMixGroupRate, config.pipeline_config().root().output_rate);
  EXPECT_EQ(PipelineConfig::kDefaultMixGroupChannels,
            config.pipeline_config().root().output_channels);
  ASSERT_EQ(1u, config.pipeline_config().root().input_streams.size());
  EXPECT_EQ(RenderUsage::MEDIA, config.pipeline_config().root().input_streams[0]);
}

TEST(ProcessConfigLoaderTest, LoadOutputDevicePolicyWithNoSupportedStreamTypes) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": []
      },
      {
        "device_id": "*",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};

  const auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();

  auto& config = result.value().device_config().output_device_profile(expected_id);
  for (const auto& render_usage : kRenderUsages) {
    EXPECT_FALSE(config.supports_usage(render_usage));
  }
  EXPECT_FALSE(config.pipeline_config().root().loopback);
  EXPECT_TRUE(config.pipeline_config().root().input_streams.empty());
  EXPECT_TRUE(config.pipeline_config().root().effects_v1.empty());
  EXPECT_TRUE(config.pipeline_config().root().inputs.empty());
  EXPECT_EQ(PipelineConfig::kDefaultMixGroupRate, config.pipeline_config().root().output_rate);
  EXPECT_EQ(PipelineConfig::kDefaultMixGroupChannels,
            config.pipeline_config().root().output_channels);
}

TEST(ProcessConfigLoaderTest, LoadOutputDevicePolicyVolumeCurve) {
  static const std::string kConfigWithRoutingPolicy =
      R"JSON({
    "volume_curve": [
      {
          "level": 0.0,
          "db": -160.0
      },
      {
          "level": 0.5,
          "db": -5.0
      },
      {
          "level": 1.0,
          "db": 0.0
      }
    ],
    "output_devices": [
      {
        "device_id": "34384e7da9d52c8062a9765baeb6053a",
        "supported_stream_types": []
      },
      {
        "device_id": "*",
        "supported_stream_types": [
          "render:accessibility",
          "render:background",
          "render:communication",
          "render:interruption",
          "render:media",
          "render:system_agent"
        ]
      }
    ]
  })JSON";
  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kConfigWithRoutingPolicy.data(),
                               kConfigWithRoutingPolicy.size()));

  const audio_stream_unique_id_t expected_id = {.data = {0x34, 0x38, 0x4e, 0x7d, 0xa9, 0xd5, 0x2c,
                                                         0x80, 0x62, 0xa9, 0x76, 0x5b, 0xae, 0xb6,
                                                         0x05, 0x3a}};

  const auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  ASSERT_TRUE(result.is_ok()) << result.error();
  auto& result_curve =
      result.value().device_config().output_device_profile(expected_id).volume_curve();

  // Matching volume curve in kConfigWithRoutingPolicy above.
  auto expected_curve = VolumeCurve::FromMappings({VolumeCurve::VolumeMapping(0.0, -160.0),
                                                   VolumeCurve::VolumeMapping(0.5, -5.0),
                                                   VolumeCurve::VolumeMapping(1.0, 0.0)});
  ASSERT_TRUE(expected_curve.is_ok()) << expected_curve.error();
  auto curve = expected_curve.take_value();

  EXPECT_FLOAT_EQ(curve.VolumeToDb(0.25), result_curve.VolumeToDb(0.25));
  EXPECT_FLOAT_EQ(curve.VolumeToDb(0.5), result_curve.VolumeToDb(0.5));
  EXPECT_FLOAT_EQ(curve.VolumeToDb(0.75), result_curve.VolumeToDb(0.75));
}

// Output driver_gain_db, input driver_gain_db, input software_gain_db
TEST(ProcessConfigLoaderTest, RejectConfigsWithIndefiniteDeviceGainDb) {
  std::vector<std::pair<std::string, std::string>> kMalformedDeviceGainDbs{
      {"Output driver_gain_db INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "output_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [
              "render:accessibility",
              "render:background",
              "render:communication",
              "render:interruption",
              "render:media",
              "render:system_agent",
              "capture:loopback"
            ],
            "driver_gain_db": Infinity
          }
        ]
      })JSON"},
      {"Output driver_gain_db -INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "output_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [
              "render:accessibility",
              "render:background",
              "render:communication",
              "render:interruption",
              "render:media",
              "render:system_agent",
              "capture:loopback"
            ],
            "driver_gain_db": -Infinity
          }
        ]
      })JSON"},
      {"Output driver_gain_db NAN",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "output_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [
              "render:accessibility",
              "render:background",
              "render:communication",
              "render:interruption",
              "render:media",
              "render:system_agent",
              "capture:loopback"
            ],
            "driver_gain_db": NaN
          }
        ]
      })JSON"},
      {"Input driver_gain_db INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": Infinity,
            "software_gain_db": -8.0
          },
        ]
      })JSON"},
      {"Input driver_gain_db -INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": -Infinity,
            "software_gain_db": -8.0
          },
        ]
      })JSON"},
      {"Input driver_gain_db NAN",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": NaN,
            "software_gain_db": -8.0
          },
        ]
      })JSON"},
      {"Input software_gain_db INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": -6.0,
            "software_gain_db": Infinity,
          },
        ]
      })JSON"},
      {"Input software_gain_db -INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": -6.0,
            "software_gain_db": -Infinity,
          },
        ]
      })JSON"},
      {"Input software_gain_db NAN",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 96000
            "driver_gain_db": -6.0,
            "software_gain_db": NaN,
          },
        ]
      })JSON"},
  };

  for (auto idx = 0u; idx < kMalformedDeviceGainDbs.size(); ++idx) {
    const auto [bad_case_name, bad_config] = kMalformedDeviceGainDbs[idx];
    ASSERT_TRUE(
        files::WriteFile(kTestAudioCoreConfigFilename, bad_config.data(), bad_config.size()))
        << "case " << idx << " (" << bad_case_name << ") could not write file" << '\n';

    auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
    EXPECT_TRUE(result.is_error()) << "'" << bad_case_name << "'";
  }
}

// input_device_profile 'rate' <= 0
TEST(ProcessConfigLoaderTest, RejectConfigWithNonPositiveInputDeviceRate) {
  std::string kMalformedDeviceRate{
      R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
        "input_devices": [
          {
            "device_id": "34384e7da9d52c8062a9765baeb6053a",
            "supported_stream_types": [ "capture:background" ],
            "rate": 0
          },
        ]
      })JSON"};

  ASSERT_TRUE(files::WriteFile(kTestAudioCoreConfigFilename, kMalformedDeviceRate.data(),
                               kMalformedDeviceRate.size()))
      << "Could not write file" << '\n';
  auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
  EXPECT_TRUE(result.is_error()) << "Non-positive device rate";
}

// For these invalid values, we actually expect to intentionally CHECK.
TEST(ProcessConfigLoaderTest, RejectConfigsWithInvalidMixGroupGainValues) {
  std::vector<std::pair<std::string, std::string>> kInvalidMixGroupGains{
      {"max_gain_db > 24",
       R"JSON({
      "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
      "output_devices": [
        {
          "device_id": "34384e7da9d52c8062a9765baeb6053a",
          "supported_stream_types": [  "render:accessibility", "render:background",
              "render:communication",  "render:interruption",  "render:media",
              "render:system_agent" ],
          "pipeline": {
            "name": "default",
            "streams": [                 "render:accessibility", "render:background",
                "render:communication",  "render:interruption",  "render:media",
                "render:system_agent" ],
            "max_gain_db": 25.0
          }
        }
      ]
    })JSON"},
      {"max_gain_db for ultrasound",
       R"JSON({
      "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
      "output_devices": [
        {
          "device_id": "34384e7da9d52c8062a9765baeb6053a",
          "supported_stream_types": [  "render:accessibility", "render:background",
              "render:communication",  "render:interruption",  "render:media",
              "render:system_agent",   "render:ultrasound" ],
          "pipeline": {
            "name": "default",
            "streams": [                 "render:accessibility", "render:background",
                "render:communication",  "render:interruption",  "render:media",
                "render:system_agent",   "render:ultrasound" ],
            "max_gain_db": -6.0
          }
        }
      ]
    })JSON"},
      {"min_gain_db < -160",
       R"JSON({
      "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
      "output_devices": [
        {
          "device_id": "34384e7da9d52c8062a9765baeb6053a",
          "supported_stream_types": [  "render:accessibility", "render:background",
              "render:communication",  "render:interruption",  "render:media",
              "render:system_agent" ],
          "pipeline": {
            "name": "default",
            "streams": [                 "render:accessibility", "render:background",
                "render:communication",  "render:interruption",  "render:media",
                "render:system_agent" ],
            "min_gain_db": -161
          }
        }
      ]
    })JSON"},
      {"min_gain_db for ultrasound",
       R"JSON({
      "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ],
      "output_devices": [
        {
          "device_id": "34384e7da9d52c8062a9765baeb6053a",
          "supported_stream_types": [  "render:accessibility", "render:background",
              "render:communication",  "render:interruption",  "render:media",
              "render:system_agent",   "render:ultrasound" ],
          "pipeline": {
            "name": "default",
            "streams": [                 "render:accessibility", "render:background",
                "render:communication",  "render:interruption",  "render:media",
                "render:system_agent",   "render:ultrasound" ],
            "min_gain_db": -6
          }
        }
      ]
    })JSON"},
  };

  for (auto idx = 0u; idx < kInvalidMixGroupGains.size(); ++idx) {
    const auto [bad_case_name, bad_config] = kInvalidMixGroupGains[idx];
    ASSERT_TRUE(
        files::WriteFile(kTestAudioCoreConfigFilename, bad_config.data(), bad_config.size()))
        << "case " << idx << " (" << bad_case_name << ") could not write file" << '\n';
    ASSERT_DEATH(ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename), "")
        << bad_case_name;
  }
}

// Attempt bad volume curves of various types. All should return an error result.
TEST(ProcessConfigLoaderTest, RejectConfigsWithInvalidVolumeCurves) {
  std::vector<std::pair<std::string, std::string>> kInvalidVolumeCurves{
      {"level < 0",
       R"JSON({
        "volume_curve": [ { "level": -0.01, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"level not increasing",
       R"JSON({
        "volume_curve": [
          { "level": 0.0, "db": -160.0 },
          { "level": 0.5, "db": -80.0 },
          { "level": 0.5, "db": -75.0 },
          { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"level > 1",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.001, "db": 0.0 } ]
        })JSON"},
      {"level +INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": Infinity, "db": 0.0 } ]
        })JSON"},
      {"level -INF",
       R"JSON({
        "volume_curve": [ { "level": -Infinity, "db": -160.0 }, { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"level NAN",
       R"JSON({
        "volume_curve": [
          { "level": 0.0, "db": -160.0 },
          { "level": NAN, "db": -80.0 },
          { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"db < MUTED_GAIN_DB",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -161.0 }, { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"db not increasing",
       R"JSON({
        "volume_curve": [
          { "level": 0.0, "db": -160.0 },
          { "level": 0.5, "db": -80.0 },
          { "level": 0.6, "db": -80.0 },
          { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"db > 0",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 1.0 } ]
        })JSON"},
      {"db > MAX_GAIN_DB",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": 25.0 } ]
        })JSON"},
      {"db -INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -Infinity }, { "level": 1.0, "db": 0.0 } ]
        })JSON"},
      {"db INF",
       R"JSON({
        "volume_curve": [ { "level": 0.0, "db": -160.0 }, { "level": 1.0, "db": Infinity } ]
        })JSON"},
      {"db NAN",
       R"JSON({
        "volume_curve": [
          { "level": 0.0, "db": -160.0 },
          { "level": 0.5, "db": NAN },
          { "level": 1.0, "db": 0.0 } ]
        })JSON"},
  };

  for (auto idx = 0u; idx < kInvalidVolumeCurves.size(); ++idx) {
    const auto [bad_case_name, bad_config] = kInvalidVolumeCurves[idx];
    ASSERT_TRUE(
        files::WriteFile(kTestAudioCoreConfigFilename, bad_config.data(), bad_config.size()))
        << "case " << idx << " (" << bad_case_name << ") could not write file" << '\n';

    auto result = ProcessConfigLoader::LoadProcessConfig(kTestAudioCoreConfigFilename);
    EXPECT_TRUE(result.is_error()) << "'" << bad_case_name << "'";
  }
}

}  // namespace
}  // namespace media::audio

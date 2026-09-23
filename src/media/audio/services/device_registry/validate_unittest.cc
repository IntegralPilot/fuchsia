// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/validate.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/zx/clock.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>

#include <optional>
#include <vector>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/signal_processing_utils.h"
#include "src/media/audio/services/device_registry/signal_processing_utils_unittest.h"

namespace media_audio {
namespace {

namespace fha = fuchsia_hardware_audio;

// These cases unittest the Validate... functions with inputs that cause INFO logging (if any).

const std::vector<uint8_t> kChannels = {
    1,
    8,
    255,
};
const std::vector<std::pair<uint8_t, fha::SampleFormat>> kFormats = {
    {
        1,
        fha::SampleFormat::kPcmUnsigned,
    },
    {
        2,
        fha::SampleFormat::kPcmSigned,
    },
    {
        4,
        fha::SampleFormat::kPcmSigned,
    },
    {
        4,
        fha::SampleFormat::kPcmFloat,
    },
    {
        8,
        fha::SampleFormat::kPcmFloat,
    },
};
const std::vector<uint32_t> kFrameRates = {
    1000,
    44100,
    48000,
    19200,
};

constexpr uint64_t kVmoContentSize = 4096;
constexpr uint8_t kChannelCount = 1;
constexpr uint8_t kSampleSize = 2;
constexpr uint32_t kNumFrames =
    static_cast<uint32_t>(kVmoContentSize / kChannelCount / kSampleSize);
const fha::Format2 kRingBufferFormat = fha::Format2::WithPcmFormat(fha::PcmFormat{{
    .number_of_channels = kChannelCount,
    .sample_format = fha::SampleFormat::kPcmSigned,
    .bytes_per_sample = kSampleSize,
    .valid_bits_per_sample = 16,
    .frame_rate = 8000,
}});

// Unittest ValidateCodecProperties -- the missing, min and max possibilities
TEST(ValidateTest, ValidateCodecProperties) {
  EXPECT_TRUE(ValidateCodecProperties(fha::CodecProperties{{
      // is_input missing
      .manufacturer = " ",  // min value (empty is disallowed)
      .product =            // max value
      "Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name extends to 321X",
      // unique_id missing
      .plug_detect_capabilities = fha::PlugDetectCapabilities::kHardwired,
  }}));
  EXPECT_TRUE(ValidateCodecProperties(fha::CodecProperties{{
      .is_input = false,
      .manufacturer =  // max value
      "Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long, which extends to... 321X",
      // product missing
      .unique_id = {{}},  // min value
      .plug_detect_capabilities = fha::PlugDetectCapabilities::kCanAsyncNotify,
  }}));
  EXPECT_TRUE(ValidateCodecProperties(fha::CodecProperties{{
      .is_input = true,
      // manufacturer missing
      .product = " ",  // min value (empty is disallowed)
      .unique_id = {{
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
      }},
      .plug_detect_capabilities = fha::PlugDetectCapabilities::kCanAsyncNotify,
  }}));
}

// Unittest ValidateCompositeProperties -- the missing, min and max possibilities
TEST(ValidateTest, ValidateCompositeProperties) {
  EXPECT_TRUE(ValidateCompositeProperties(fha::CompositeProperties{{
      // manufacturer missing
      .product = " ",  // min value (empty is disallowed)
      .unique_id = {{
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
          0xFF,
      }},
      .clock_domain = fha::kClockDomainMonotonic,
      // power_elements missing
  }}));
  EXPECT_TRUE(ValidateCompositeProperties(fha::CompositeProperties{{
      .manufacturer = " ",  // min value (empty is disallowed)
      .product =            // max value
      "Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name is 256 characters long; Maximum allowed Product name extends to 321X",
      // unique_id missing
      .clock_domain = fha::kClockDomainExternal,
  }}));
  EXPECT_TRUE(ValidateCompositeProperties(fha::CompositeProperties{{
      .manufacturer =  // max value
      "Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long; Maximum allowed Manufacturer name is 256 characters long, which extends to... 321X",
      // product missing
      .unique_id = {{}},  // min value
      .clock_domain = fha::kClockDomainExternal,
  }}));
}

// TODO(https://fxbug.dev/42069012): Unittest ValidateDeviceInfo, incl. manuf & prod 256 chars long

fha::SupportedFormats2 CompliantFormatSet() {
  return fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
      .channel_sets = {{
          fha::ChannelSet{{
              .attributes = {{
                  fha::ChannelAttributes{{
                      .min_frequency = 20,
                      .max_frequency = 20000,
                  }},
              }},
          }},
      }},
      .sample_formats = {{
          fha::SampleFormat::kPcmSigned,
      }},
      .bytes_per_sample = {{
          2,
      }},
      .valid_bits_per_sample = {{
          16,
      }},
      .frame_rates = {{
          48000,
      }},
  }});
}

TEST(ValidateTest, ValidateRingBufferProperties) {
  EXPECT_TRUE(ValidateRingBufferProperties(fha::RingBufferProperties{{
      .needs_cache_flush_or_invalidate = false,
      .turn_on_delay = 125,
      .driver_transfer_bytes = 32,
  }}));

  // TODO(b/311694769): Resolve driver_transfer_bytes lower limit: specifically is 0 allowed?
  EXPECT_TRUE(ValidateRingBufferProperties(fha::RingBufferProperties{{
      .needs_cache_flush_or_invalidate = true,
      .turn_on_delay = 0,  // can be zero
      .driver_transfer_bytes = 1,
  }}));

  // TODO(b/311694769): Resolve driver_transfer_bytes upper limit: no limit? Soft guideline?
  EXPECT_TRUE(ValidateRingBufferProperties(fha::RingBufferProperties{{
      .needs_cache_flush_or_invalidate = true,
      // turn_on_delay (optional) is missing
      .driver_transfer_bytes = 128,
  }}));
}

TEST(ValidateTest, ValidatePlugState) {
  EXPECT_TRUE(ValidatePlugState(fha::PlugState{{
      .plugged = true,
      .plug_state_time = 0,
  }}));
  EXPECT_TRUE(ValidatePlugState(fha::PlugState{{
      .plugged = false,
      .plug_state_time = zx::clock::get_monotonic().get(),
  }}));
  EXPECT_TRUE(ValidatePlugState(fha::PlugState{{
                                    .plugged = true,
                                    .plug_state_time = zx::clock::get_monotonic().get(),
                                }},
                                fha::PlugDetectCapabilities::kHardwired));
  EXPECT_TRUE(ValidatePlugState(fha::PlugState{{
                                    .plugged = false,
                                    .plug_state_time = zx::clock::get_monotonic().get(),
                                }},
                                fha::PlugDetectCapabilities::kCanAsyncNotify));
}

TEST(ValidateTest, ValidateRingBufferFormatSets) {
  EXPECT_TRUE(ValidateRingBufferFormatSets({CompliantFormatSet()}));

  fha::SupportedFormats2 min_values_format_set =
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              {{
                  .attributes = {{
                      {{
                          .max_frequency = 50,
                      }},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmUnsigned,
          }},
          .bytes_per_sample = {{
              1,
          }},
          .valid_bits_per_sample = {{
              1,
          }},
          .frame_rates = {{
              1000,
          }},
      }});
  EXPECT_TRUE(ValidateRingBufferFormatSets({min_values_format_set}));

  fha::SupportedFormats2 max_values_format_set =
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              {{
                  .attributes = {{
                      {},
                      {{
                          .max_frequency = 96000,
                      }},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmFloat,
          }},
          .bytes_per_sample = {{
              8,
          }},
          .valid_bits_per_sample = {{
              64,
          }},
          .frame_rates = {{
              192000,
          }},
      }});
  EXPECT_TRUE(ValidateRingBufferFormatSets({max_values_format_set}));

  // Probe fully-populated values, in multiple format sets.
  fha::SupportedFormats2 signed_format_set =
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              {{
                  .attributes = {{
                      {},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 1000,
                      }},
                      {{
                          .max_frequency = 2500,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 0,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 16000,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 0,
                          .max_frequency = 96000,
                      }},
                      {{
                          .min_frequency = 16000,
                          .max_frequency = 96000,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .max_frequency = 2500,
                      }},
                      {},
                      {{
                          .min_frequency = 24000,
                      }},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmSigned,
          }},
          .bytes_per_sample = {{
              2,
              4,
          }},
          .valid_bits_per_sample = {{
              1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 26, 28, 30, 32,
          }},
          .frame_rates = {{
              1000,
              2000,
              4000,
              8000,
              11025,
              16000,
              22050,
              24000,
              44100,
              48000,
              88200,
              96000,
              192000,
          }},
      }});
  EXPECT_TRUE(ValidateRingBufferFormatSets({signed_format_set}));

  fha::SupportedFormats2 unsigned_format_set =
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              {{
                  .attributes = {{
                      {},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 1000,
                      }},
                      {{
                          .max_frequency = 2500,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 0,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 16000,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 0,
                          .max_frequency = 96000,
                      }},
                      {{
                          .min_frequency = 16000,
                          .max_frequency = 96000,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .max_frequency = 2500,
                      }},
                      {},
                      {{
                          .min_frequency = 24000,
                      }},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmUnsigned,
          }},
          .bytes_per_sample = {{
              1,
          }},
          .valid_bits_per_sample = {{
              1,
              2,
              3,
              4,
              5,
              6,
              7,
              8,
          }},
          .frame_rates = {{
              1000,
              2000,
              4000,
              8000,
              11025,
              16000,
              22050,
              24000,
              44100,
              48000,
              88200,
              96000,
              192000,
          }},
      }});
  EXPECT_TRUE(ValidateRingBufferFormatSets({unsigned_format_set}));

  fha::SupportedFormats2 float_format_set =
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              {{
                  .attributes = {{
                      {},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 1000,
                      }},
                      {{
                          .max_frequency = 2500,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 0,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 2500,
                          .max_frequency = 24000,
                      }},
                      {{
                          .min_frequency = 16000,
                          .max_frequency = 96000,
                      }},
                      {{
                          .min_frequency = 2400,
                          .max_frequency = 96000,
                      }},
                  }},
              }},
              {{
                  .attributes = {{
                      {{
                          .min_frequency = 1000,
                      }},
                      {},
                      {{
                          .max_frequency = 2500,
                      }},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmFloat,
          }},
          .bytes_per_sample = {{
              4,
              8,
          }},
          .valid_bits_per_sample = {{
              1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15, 16,
              18, 20, 22, 24, 26, 28, 30, 32, 36, 40, 44, 48, 52, 56, 60, 64,
          }},
          .frame_rates = {{
              1000,
              2000,
              4000,
              8000,
              11025,
              16000,
              22050,
              24000,
              44100,
              48000,
              88200,
              96000,
              192000,
          }},
      }});
  EXPECT_TRUE(ValidateRingBufferFormatSets({float_format_set}));

  std::vector supported_formats = {CompliantFormatSet()};
  supported_formats.push_back(signed_format_set);
  supported_formats.push_back(unsigned_format_set);
  supported_formats.push_back(float_format_set);
  supported_formats.push_back(min_values_format_set);
  supported_formats.push_back(max_values_format_set);
  EXPECT_TRUE(ValidateRingBufferFormatSets(supported_formats));
}

// TODO(https://fxbug.dev/42069012): Unittest TranslateRingBufferFormatSets

TEST(ValidateTest, ValidatePcmFormat) {
  for (auto chans : kChannels) {
    for (auto [bytes, sample_format] : kFormats) {
      for (auto rate : kFrameRates) {
        EXPECT_TRUE(ValidatePcmFormat(fha::PcmFormat{{
            .number_of_channels = chans,
            .sample_format = sample_format,
            .bytes_per_sample = bytes,
            .valid_bits_per_sample = 1,
            .frame_rate = rate,
        }}));
        EXPECT_TRUE(ValidatePcmFormat(fha::PcmFormat{{
            .number_of_channels = chans,
            .sample_format = sample_format,
            .bytes_per_sample = bytes,
            .valid_bits_per_sample = static_cast<uint8_t>((bytes * 8) - 4),
            .frame_rate = rate,
        }}));
        EXPECT_TRUE(ValidatePcmFormat(fha::PcmFormat{{
            .number_of_channels = chans,
            .sample_format = sample_format,
            .bytes_per_sample = bytes,
            .valid_bits_per_sample = static_cast<uint8_t>(bytes * 8),
            .frame_rate = rate,
        }}));
      }
    }
  }
}

TEST(ValidateTest, ValidateSampleFormatCompatibility) {
  for (auto [bytes, sample_format] : kFormats) {
    EXPECT_TRUE(ValidateSampleFormatCompatibility(bytes, sample_format));
  }
}

TEST(ValidateTest, ValidateRingBufferVmoOutgoing) {
  zx::vmo vmo, outgoing;
  auto status = zx::vmo::create(kVmoContentSize, 0, &vmo);
  ASSERT_EQ(status, ZX_OK) << "could not create VMO for test input";
  status = vmo.replace(kRequiredVmoRightsForReadWrite, &outgoing);
  ASSERT_EQ(status, ZX_OK) << "could not change VMO rights for test input";

  // For VMOs we will fill with outgoing audio to the driver, they must be writable.
  EXPECT_TRUE(ValidateRingBufferVmo(outgoing, kNumFrames, kRingBufferFormat,
                                    kRequiredVmoRightsForReadWrite));
}

TEST(ValidateTest, ValidateRingBufferVmoIncoming) {
  zx::vmo vmo, incoming;
  auto status = zx::vmo::create(kVmoContentSize, 0, &vmo);
  ASSERT_EQ(status, ZX_OK) << "could not create VMO for test input";
  status = vmo.replace(kRequiredVmoRightsForRead, &incoming);
  ASSERT_EQ(status, ZX_OK) << "could not change VMO rights for test input";

  // For VMOs of incoming audio from the driver, we only need to read the data (not write).
  EXPECT_TRUE(
      ValidateRingBufferVmo(incoming, kNumFrames, kRingBufferFormat, kRequiredVmoRightsForRead));
}

TEST(ValidateTest, ValidateDelayInfo) {
  EXPECT_TRUE(ValidateDelayInfo(fha::DelayInfo{{
      .internal_delay = 0,
  }}));
  EXPECT_TRUE(ValidateDelayInfo(fha::DelayInfo{{
      .internal_delay = 125,
      .external_delay = 0,
  }}));
  EXPECT_TRUE(ValidateDelayInfo(fha::DelayInfo{{
      .internal_delay = 0,
      .external_delay = 125,
  }}));
}

TEST(ValidateTest, ValidatePacketStreamProperties) {
  EXPECT_TRUE(ValidatePacketStreamProperties(fha::PacketStreamProperties{{
      .needs_cache_flush_or_invalidate = true,
      .supported_buffer_types = fha::BufferType::kClientOwned,
  }}));
  EXPECT_TRUE(ValidatePacketStreamProperties(fha::PacketStreamProperties{{
      .needs_cache_flush_or_invalidate = false,
      .supported_buffer_types = fha::BufferType::kDriverOwned | fha::BufferType::kInline,
  }}));
}

TEST(ValidateTest, ValidatePacketStreamFormatSets) {
  EXPECT_TRUE(ValidatePacketStreamFormatSets({{
      fha::SupportedFormats2::WithPcmSupportedFormats(fha::PcmSupportedFormats{{
          .channel_sets = {{
              fha::ChannelSet{{
                  .attributes = {{
                      fha::ChannelAttributes{},
                  }},
              }},
          }},
          .sample_formats = {{
              fha::SampleFormat::kPcmSigned,
          }},
          .bytes_per_sample = {{
              2,
          }},
          .valid_bits_per_sample = {{
              16,
          }},
          .frame_rates = {{
              48000,
          }},
      }}),
      fha::SupportedFormats2::WithSupportedEncodings(fha::SupportedEncodings{{
          .decoded_channel_sets = {{
              fha::ChannelSet{{
                  .attributes = {{
                      fha::ChannelAttributes{},
                  }},
              }},
          }},
          .decoded_frame_rates = {{
              44100,
          }},
          .encoding_types = {{
              fha::EncodingType::kAac,
          }},
      }}),
  }}));
}

TEST(ValidateTest, ValidateEncoding) {
  EXPECT_TRUE(ValidateEncoding(fha::Encoding{{
      .decoded_channel_count = 1,
      .decoded_frame_rate = 16000,
      .average_encoding_bitrate = 16000,
      .encoding_type = fha::EncodingType::kSbc,
  }}));

  EXPECT_TRUE(ValidateEncoding(fha::Encoding{{
      .decoded_channel_count = 1,
      .average_encoding_bitrate = 128000,
      .encoding_type = fha::EncodingType::kAac,
      // decoded_frame_rate is optional, testing that absence is okay.
  }}));
}

TEST(ValidateTest, ValidateDaiFormatSets) {
  EXPECT_TRUE(ValidateDaiFormatSets({{
      {{
          .number_of_channels =
              {
                  1,
              },
          .sample_formats =
              {
                  fha::DaiSampleFormat::kPcmSigned,
              },
          .frame_formats = {fha::DaiFrameFormat::WithFrameFormatStandard(
              fha::DaiFrameFormatStandard::kI2S)},
          .frame_rates =
              {
                  48000,
              },
          .bits_per_slot =
              {
                  32,
              },
          .bits_per_sample =
              {
                  16,
              },
      }},
  }}));
}

TEST(ValidateTest, ValidateDaiFormat) {
  // Normal values
  EXPECT_TRUE(ValidateDaiFormat({{
      .number_of_channels = 2,
      .channels_to_use_bitmask = 0x03,
      .sample_format = fha::DaiSampleFormat::kPcmSigned,
      .frame_format =
          fha::DaiFrameFormat::WithFrameFormatStandard(fha::DaiFrameFormatStandard::kI2S),
      .frame_rate = 48000,
      .bits_per_slot = 32,
      .bits_per_sample = 16,
  }}));
  // Minimal values
  EXPECT_TRUE(ValidateDaiFormat({{
      .number_of_channels = 1,
      .channels_to_use_bitmask = 0x01,
      .sample_format = fha::DaiSampleFormat::kPdm,
      .frame_format =
          fha::DaiFrameFormat::WithFrameFormatStandard(fha::DaiFrameFormatStandard::kNone),
      .frame_rate = 1000,
      .bits_per_slot = 8,
      .bits_per_sample = 1,
  }}));
  // Maximal values
  EXPECT_TRUE(ValidateDaiFormat({{
      .number_of_channels = 32,
      .channels_to_use_bitmask = 0xFFFFFFFF,
      .sample_format = fha::DaiSampleFormat::kPcmSigned,
      .frame_format = fha::DaiFrameFormat::WithFrameFormatCustom({{
          .left_justified = true,
          .sclk_on_raising = false,
          .frame_sync_sclks_offset = 0,
          .frame_sync_size = 1,
      }}),
      .frame_rate = 192000,
      .bits_per_slot = 32,
      .bits_per_sample = 32,
  }}));
  // Maximal values
  EXPECT_TRUE(ValidateDaiFormat({{
      .number_of_channels = 64,
      .channels_to_use_bitmask = 0xFFFFFFFFFFFFFFFF,
      .sample_format = fha::DaiSampleFormat::kPcmFloat,
      .frame_format = fha::DaiFrameFormat::WithFrameFormatCustom({{
          .left_justified = true,
          .sclk_on_raising = false,
          .frame_sync_sclks_offset = -128,
          .frame_sync_size = 255,
      }}),
      .frame_rate = 192000 * 64 * 8,
      .bits_per_slot = kMaxSupportedDaiFormatBitsPerSlot,
      .bits_per_sample = kMaxSupportedDaiFormatBitsPerSlot,
  }}));
}

TEST(ValidateTest, ValidateCodecFormatInfo) {
  EXPECT_TRUE(ValidateCodecFormatInfo(fha::CodecFormatInfo{}));
  // For all three fields, test missing, min and max values.
  EXPECT_TRUE(ValidateCodecFormatInfo(fha::CodecFormatInfo{{
      .external_delay = 0,
      .turn_off_delay = zx::time::infinite().get(),
  }}));
  EXPECT_TRUE(ValidateCodecFormatInfo(fha::CodecFormatInfo{{
      .external_delay = zx::time::infinite().get(),
      .turn_on_delay = 0,
  }}));
  EXPECT_TRUE(ValidateCodecFormatInfo(fha::CodecFormatInfo{{
      .turn_on_delay = zx::time::infinite().get(),
      .turn_off_delay = 0,
  }}));
}

// signalprocessing functions
TEST(ValidateTest, ValidateTopologies) {
  EXPECT_TRUE(ValidateTopologies(kTopologies, MapElements(kElements)));
  EXPECT_TRUE(ValidateTopologies(kTopologiesWithLoop, MapElements(kElementsWithLoopDai)));
}

TEST(ValidateTest, ValidateTopology) {
  EXPECT_TRUE(ValidateTopology(kTopologyDaiAgcDynRb, MapElements(kElements)));
  EXPECT_TRUE(ValidateTopology(kTopologyDaiRb, MapElements(kElements)));
  EXPECT_TRUE(ValidateTopology(kTopologyRbDai, MapElements(kElements)));
  EXPECT_TRUE(ValidateTopology(kTopologyPsDai, MapElements(kElements)));
  EXPECT_TRUE(ValidateTopology(kTopologyDaiAgcDynRbAndLoop, MapElements(kElementsWithLoopDai)));
}

TEST(ValidateTest, ValidateElements) { EXPECT_TRUE(ValidateElements(kElements)); }

TEST(ValidateTest, ValidateElement) {
  EXPECT_TRUE(ValidateElement(kAgcElement));
  EXPECT_TRUE(ValidateElement(kPacketStreamElement));
  EXPECT_TRUE(ValidateElement(kRingBufferElement));
}

TEST(ValidateTest, ValidateDaiInterconnectElement) {
  EXPECT_TRUE(ValidateDaiInterconnectElement(kDaiInterconnectElement));

  EXPECT_TRUE(ValidateElement(kDaiInterconnectElement));
}

TEST(ValidateTest, ValidateDynamicsElement) {
  EXPECT_TRUE(ValidateDynamicsElement(kDynamicsElement));

  EXPECT_TRUE(ValidateElement(kDynamicsElement));
}

TEST(ValidateTest, ValidateEqualizerElement) {
  EXPECT_TRUE(ValidateEqualizerElement(kEqualizerElement));

  EXPECT_TRUE(ValidateElement(kEqualizerElement));
}

TEST(ValidateTest, ValidateGainElement) {
  EXPECT_TRUE(ValidateGainElement(kGainElement));

  EXPECT_TRUE(ValidateElement(kGainElement));
}

TEST(ValidateTest, ValidateVendorSpecificElement) {
  EXPECT_TRUE(ValidateVendorSpecificElement(kVendorSpecificElement));

  EXPECT_TRUE(ValidateElement(kVendorSpecificElement));
}

// ValidateElementState
TEST(ValidateTest, ValidateElementState) {
  EXPECT_TRUE(ValidateElementState(kGenericElementState, kAgcElement));
}

TEST(ValidateTest, ValidateDaiInterconnectElementState) {
  EXPECT_TRUE(
      ValidateDaiInterconnectElementState(kDaiInterconnectElementState, kDaiInterconnectElement));

  EXPECT_TRUE(ValidateElementState(kDaiInterconnectElementState, kDaiInterconnectElement));
}

TEST(ValidateTest, ValidateDynamicsElementState) {
  EXPECT_TRUE(ValidateDynamicsElementState(kDynamicsElementState, kDynamicsElement));

  EXPECT_TRUE(ValidateElementState(kDynamicsElementState, kDynamicsElement));
}

TEST(ValidateTest, ValidateEqualizerElementState) {
  EXPECT_TRUE(ValidateEqualizerElementState(kEqualizerElementState, kEqualizerElement));

  EXPECT_TRUE(ValidateElementState(kEqualizerElementState, kEqualizerElement));
}

TEST(ValidateTest, ValidateGainElementState) {
  EXPECT_TRUE(ValidateGainElementState(kGainElementState, kGainElement));

  EXPECT_TRUE(ValidateElementState(kGainElementState, kGainElement));
}

TEST(ValidateTest, ValidateVendorSpecificElementState) {
  EXPECT_TRUE(
      ValidateVendorSpecificElementState(kVendorSpecificElementState, kVendorSpecificElement));

  EXPECT_TRUE(ValidateElementState(kVendorSpecificElementState, kVendorSpecificElement));
}

TEST(ValidateTest, ValidateSettableElementState) {
  EXPECT_TRUE(ValidateSettableElementState(kSettableGenericElementState, kAgcElement));
}

TEST(ValidateTest, ValidateSettableDynamicsElementState) {
  EXPECT_TRUE(
      ValidateSettableDynamicsElementState(kSettableDynamicsElementState, kDynamicsElement));

  EXPECT_TRUE(ValidateSettableElementState(kSettableDynamicsElementState, kDynamicsElement));
}

TEST(ValidateTest, ValidateSettableEqualizerElementState) {
  EXPECT_TRUE(
      ValidateSettableEqualizerElementState(kSettableEqualizerElementState, kEqualizerElement));

  EXPECT_TRUE(ValidateSettableElementState(kSettableEqualizerElementState, kEqualizerElement));
}

TEST(ValidateTest, ValidateSettableGainElementState) {
  EXPECT_TRUE(ValidateSettableGainElementState(kSettableGainElementState, kGainElement));

  EXPECT_TRUE(ValidateSettableElementState(kSettableGainElementState, kGainElement));
}

TEST(ValidateTest, ValidateSettableVendorSpecificElementState) {
  EXPECT_TRUE(ValidateSettableVendorSpecificElementState(kSettableVendorSpecificElementState,
                                                         kVendorSpecificElement));

  EXPECT_TRUE(
      ValidateSettableElementState(kSettableVendorSpecificElementState, kVendorSpecificElement));
}

}  // namespace
}  // namespace media_audio

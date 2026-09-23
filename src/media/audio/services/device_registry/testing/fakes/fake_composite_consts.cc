// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/zx/time.h>

#include <unordered_map>

#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

namespace media_audio {

namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

// static definitions: signalprocessing elements/topologies and format sets

// DaiFormats and format sets
//
const fha::DaiFrameFormat FakeComposite::kDefaultDaiFrameFormat =
    fha::DaiFrameFormat::WithFrameFormatStandard(fha::DaiFrameFormatStandard::kI2S);
const fha::DaiFrameFormat FakeComposite::kDefaultDaiFrameFormat2 =
    fha::DaiFrameFormat::WithFrameFormatStandard(fha::DaiFrameFormatStandard::kNone);

const std::vector<uint32_t> FakeComposite::kDefaultDaiNumberOfChannelsSet{
    kDefaultDaiNumberOfChannels,
    kDefaultDaiNumberOfChannels2,
};
const std::vector<uint32_t> FakeComposite::kDefaultDaiNumberOfChannelsSet2{
    kDefaultDaiNumberOfChannels2,
};
const std::vector<fha::DaiSampleFormat> FakeComposite::kDefaultDaiSampleFormatSet{
    kDefaultDaiSampleFormat,
};
const std::vector<fha::DaiSampleFormat> FakeComposite::kDefaultDaiSampleFormatSet2{
    kDefaultDaiSampleFormat,
    kDefaultDaiSampleFormat2,
};
const std::vector<fha::DaiFrameFormat> FakeComposite::kDefaultDaiFrameFormatSet{
    kDefaultDaiFrameFormat,
    kDefaultDaiFrameFormat2,
};
const std::vector<fha::DaiFrameFormat> FakeComposite::kDefaultDaiFrameFormatSet2{
    kDefaultDaiFrameFormat2,
};
const std::vector<uint32_t> FakeComposite::kDefaultDaiFrameRateSet{kDefaultDaiFrameRate};
const std::vector<uint32_t> FakeComposite::kDefaultDaiFrameRateSet2{
    kDefaultDaiFrameRate,
    kDefaultDaiFrameRate2,
};
const std::vector<uint8_t> FakeComposite::kDefaultDaiBitsPerSlotSet{
    kDefaultDaiBitsPerSlot,
    kDefaultDaiBitsPerSlot2,
};
const std::vector<uint8_t> FakeComposite::kDefaultDaiBitsPerSlotSet2{
    kDefaultDaiBitsPerSlot2,
};
const std::vector<uint8_t> FakeComposite::kDefaultDaiBitsPerSampleSet{kDefaultDaiBitsPerSample};
const std::vector<uint8_t> FakeComposite::kDefaultDaiBitsPerSampleSet2{
    kDefaultDaiBitsPerSample,
    kDefaultDaiBitsPerSample2,
};

const fha::DaiSupportedFormats FakeComposite::kDefaultDaiFormatSet{{
    .number_of_channels = kDefaultDaiNumberOfChannelsSet,
    .sample_formats = kDefaultDaiSampleFormatSet,
    .frame_formats = kDefaultDaiFrameFormatSet,
    .frame_rates = kDefaultDaiFrameRateSet,
    .bits_per_slot = kDefaultDaiBitsPerSlotSet,
    .bits_per_sample = kDefaultDaiBitsPerSampleSet,
}};
const fha::DaiSupportedFormats FakeComposite::kDefaultDaiFormatSet2{{
    .number_of_channels = kDefaultDaiNumberOfChannelsSet2,
    .sample_formats = kDefaultDaiSampleFormatSet2,
    .frame_formats = kDefaultDaiFrameFormatSet2,
    .frame_rates = kDefaultDaiFrameRateSet2,
    .bits_per_slot = kDefaultDaiBitsPerSlotSet2,
    .bits_per_sample = kDefaultDaiBitsPerSampleSet2,
}};

// DaiFormatSets that are returned by the driver.
const std::vector<fha::DaiSupportedFormats> FakeComposite::kDefaultDaiFormatSets{
    kDefaultDaiFormatSet,
};
const std::vector<fha::DaiSupportedFormats> FakeComposite::kDefaultDaiFormatSets2{
    kDefaultDaiFormatSet2,
};

// Map of Dai format sets, by element. Used within the driver.
const std::unordered_map<ElementId, std::vector<fha::DaiSupportedFormats>>
    FakeComposite::kDefaultDaiFormatsMap = {{
        {kSourceDaiElementId, kDefaultDaiFormatSets},
        {
            kDestDaiElementId,
            kDefaultDaiFormatSets2,
        },
    }};

// Specific DAI formats
const fha::DaiFormat FakeComposite::kDefaultDaiFormat{{
    .number_of_channels = kDefaultDaiNumberOfChannels,
    .channels_to_use_bitmask = (1u << kDefaultDaiNumberOfChannels) - 1u,
    .sample_format = kDefaultDaiSampleFormat,
    .frame_format = kDefaultDaiFrameFormat,
    .frame_rate = kDefaultDaiFrameRate,
    .bits_per_slot = kDefaultDaiBitsPerSlot,
    .bits_per_sample = kDefaultDaiBitsPerSample,
}};
const fha::DaiFormat FakeComposite::kDefaultDaiFormat2{{
    .number_of_channels = kDefaultDaiNumberOfChannels2,
    .channels_to_use_bitmask = (1u << kDefaultDaiNumberOfChannels2) - 1u,
    .sample_format = kDefaultDaiSampleFormat2,
    .frame_format = kDefaultDaiFrameFormat2,
    .frame_rate = kDefaultDaiFrameRate2,
    .bits_per_slot = kDefaultDaiBitsPerSlot2,
    .bits_per_sample = kDefaultDaiBitsPerSample2,
}};

// RingBufferFormats and format sets
//
const fha::ChannelAttributes FakeComposite::kDefaultChannelAttributes1{{
    .min_frequency = kDefaultChannelAttributes1MinFrequency,
    .max_frequency = kDefaultChannelAttributes1MaxFrequency,
}};
const fha::ChannelAttributes FakeComposite::kDefaultChannelAttributes2{{
    .min_frequency = kDefaultChannelAttributes2MinFrequency,
    // no .max_frequency is specified
}};
const fha::ChannelAttributes FakeComposite::kDefaultChannelAttributes3{{
    // no .min_frequency is specified
    .max_frequency = kDefaultChannelAttributes3MaxFrequency,
}};
const std::vector<fha::ChannelAttributes> FakeComposite::kDefaultChannelAttributesSet1{
    kDefaultChannelAttributes1,
};
const std::vector<fha::ChannelAttributes> FakeComposite::kDefaultChannelAttributesSet2{
    kDefaultChannelAttributes2,
};
const std::vector<fha::ChannelAttributes> FakeComposite::kDefaultChannelAttributesSet3{
    kDefaultChannelAttributes3,
};
const fha::ChannelSet FakeComposite::kDefaultChannelSet1{{
    .attributes = kDefaultChannelAttributesSet1,
}};
const fha::ChannelSet FakeComposite::kDefaultChannelSet2{{
    .attributes = kDefaultChannelAttributesSet2,
}};
const fha::ChannelSet FakeComposite::kDefaultChannelSet3{{
    .attributes = kDefaultChannelAttributesSet3,
}};
const std::vector<fha::ChannelSet> FakeComposite::kDefaultChannelSets1{
    kDefaultChannelSet1,
};
const std::vector<fha::ChannelSet> FakeComposite::kDefaultChannelSets2{
    kDefaultChannelSet2,
};
const std::vector<fha::ChannelSet> FakeComposite::kDefaultChannelSets3{
    kDefaultChannelSet3,
};

const std::vector<fha::SampleFormat> FakeComposite::kDefaultRbSampleFormats1{
    kDefaultRbSampleFormat1,
};
const std::vector<fha::SampleFormat> FakeComposite::kDefaultRbSampleFormats2{
    kDefaultRbSampleFormat2,
};
const std::vector<fha::SampleFormat> FakeComposite::kDefaultRbSampleFormats3{
    kDefaultRbSampleFormat3,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbBytesPerSampleSet1{
    kDefaultRbBytesPerSample1,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbBytesPerSampleSet2{
    kDefaultRbBytesPerSample2,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbBytesPerSampleSet3{
    kDefaultRbBytesPerSample3,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbValidBitsPerSampleSet1{
    kDefaultRbValidBitsPerSample1,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbValidBitsPerSampleSet2{
    kDefaultRbValidBitsPerSample2,
};
const std::vector<uint8_t> FakeComposite::kDefaultRbValidBitsPerSampleSet3{
    kDefaultRbValidBitsPerSample3,
};
const std::vector<uint32_t> FakeComposite::kDefaultRbFrameRates1{
    kDefaultRbFrameRate1,
};
const std::vector<uint32_t> FakeComposite::kDefaultRbFrameRates2{
    kDefaultRbFrameRate2,
};
const std::vector<uint32_t> FakeComposite::kDefaultRbFrameRates3{
    kDefaultRbFrameRate3,
};
const std::vector<uint32_t> FakeComposite::kDefaultPsFrameRates1{
    kDefaultPsFrameRate1,
};
const std::vector<uint32_t> FakeComposite::kDefaultPsFrameRates2{
    kDefaultPsFrameRate2,
};

const std::vector<fuchsia_hardware_audio::EncodingType> FakeComposite::kDefaultPsEncodingTypes1{
    kDefaultPsEncodingType1,
};
const std::vector<fuchsia_hardware_audio::EncodingType> FakeComposite::kDefaultPsEncodingTypes2{
    kDefaultPsEncodingType2,
};

const fha::PcmSupportedFormats FakeComposite::kDefaultPcmRingBufferFormatSet1{{
    .channel_sets = kDefaultChannelSets1,
    .sample_formats = kDefaultRbSampleFormats1,
    .bytes_per_sample = kDefaultRbBytesPerSampleSet1,
    .valid_bits_per_sample = kDefaultRbValidBitsPerSampleSet1,
    .frame_rates = kDefaultRbFrameRates1,
}};
const fha::PcmSupportedFormats FakeComposite::kDefaultPcmRingBufferFormatSet2{{
    .channel_sets = kDefaultChannelSets2,
    .sample_formats = kDefaultRbSampleFormats2,
    .bytes_per_sample = kDefaultRbBytesPerSampleSet2,
    .valid_bits_per_sample = kDefaultRbValidBitsPerSampleSet2,
    .frame_rates = kDefaultRbFrameRates2,
}};
const fha::PcmSupportedFormats FakeComposite::kDefaultPcmRingBufferFormatSet3{{
    .channel_sets = kDefaultChannelSets3,
    .sample_formats = kDefaultRbSampleFormats3,
    .bytes_per_sample = kDefaultRbBytesPerSampleSet3,
    .valid_bits_per_sample = kDefaultRbValidBitsPerSampleSet3,
    .frame_rates = kDefaultRbFrameRates3,
}};
const fha::SupportedEncodings FakeComposite::kDefaultEncodingSet1{{
    .decoded_channel_sets = kDefaultChannelSets1,
    .decoded_frame_rates = kDefaultPsFrameRates1,
    .encoding_types = kDefaultPsEncodingTypes1,
}};
const fha::SupportedEncodings FakeComposite::kDefaultEncodingSet2{{
    .decoded_channel_sets = kDefaultChannelSets2,
    .decoded_frame_rates = kDefaultPsFrameRates2,
    .encoding_types = kDefaultPsEncodingTypes2,
}};

const fha::SupportedFormats2 FakeComposite::kDefaultRbFormatSet1 =
    fha::SupportedFormats2::WithPcmSupportedFormats(kDefaultPcmRingBufferFormatSet1);
const fha::SupportedFormats2 FakeComposite::kDefaultRbFormatSet2 =
    fha::SupportedFormats2::WithPcmSupportedFormats(kDefaultPcmRingBufferFormatSet2);
const fha::SupportedFormats2 FakeComposite::kDefaultPsFormatSet1 =
    fha::SupportedFormats2::WithPcmSupportedFormats(kDefaultPcmRingBufferFormatSet1);
const fha::SupportedFormats2 FakeComposite::kDefaultPsFormatSet2 =
    fha::SupportedFormats2::WithSupportedEncodings(kDefaultEncodingSet2);
const fha::SupportedFormats2 FakeComposite::kDefaultPsFormatSet3 =
    fha::SupportedFormats2::WithPcmSupportedFormats(kDefaultPcmRingBufferFormatSet3);

const fuchsia_audio::Format FakeComposite::kDefaultPsFormat1{{
    .sample_type = fuchsia_audio::SampleType::kInt16,
    .channel_count = 1,
    .frames_per_second = 48000,
}};
const fuchsia_audio::Format FakeComposite::kDefaultPsFormat2{{
    .sample_type = fuchsia_audio::SampleType::kFloat32,
    .channel_count = 1,
    .frames_per_second = 44100,
}};
const fha::Encoding FakeComposite::kDefaultPsFormat3{{
    .decoded_channel_count = 1,
    .decoded_frame_rate = 44100,
    .average_encoding_bitrate = 128000,
    .encoding_type = fha::EncodingType::kAac,
}};

// RingBuffer and PacketStream format sets that are returned by the driver.
const std::vector<fha::SupportedFormats2> FakeComposite::kDefaultRbFormatSets1{
    kDefaultRbFormatSet1,
};
const std::vector<fha::SupportedFormats2> FakeComposite::kDefaultRbFormatSets2{
    kDefaultRbFormatSet2,
};
const std::vector<fha::SupportedFormats2> FakeComposite::kDefaultPsFormatSets1{
    kDefaultPsFormatSet1,
    kDefaultPsFormatSet3,
};
const std::vector<fha::SupportedFormats2> FakeComposite::kDefaultPsFormatSets2{
    kDefaultPsFormatSet2,
};
const std::vector<fha::SupportedFormats2> FakeComposite::kSourceDualSupportPsFormatSets{
    kDefaultPsFormatSet1,
    kDefaultPsFormatSet2,
};

// Map of RingBuffer format sets, by element. Used internally by the driver.
const std::unordered_map<ElementId, std::vector<fha::SupportedFormats2>>
    FakeComposite::kDefaultRbFormatsMap = {{
        {
            kDestRbElementId,
            kDefaultRbFormatSets1,
        },
        {
            kSourceRbElementId,
            kDefaultRbFormatSets2,
        },
    }};
const std::unordered_map<ElementId, std::vector<fha::SupportedFormats2>>
    FakeComposite::kDefaultPsFormatsMap = {{
        {
            kDestPsElementId,
            kDefaultPsFormatSets1,
        },
        {
            kSourcePsElementId,
            kDefaultPsFormatSets2,
        },
        {
            kSourceDualSupportPsElementId,
            kSourceDualSupportPsFormatSets,
        },
    }};

// signalprocessing elements and topologies
//
// Individual elements
const std::string FakeComposite::kSourceDaiElementDescription =
    "DaiInterconnect source element description";
const fhasp::Element FakeComposite::kSourceDaiElement{{
    .id = kSourceDaiElementId,
    .type = fhasp::ElementType::kDaiInterconnect,
    .type_specific = fhasp::TypeSpecificElement::WithDaiInterconnect({{
        .plug_detect_capabilities = fhasp::PlugDetectCapabilities::kCanAsyncNotify,
    }}),
    .description = kSourceDaiElementDescription,
    .can_stop = true,
    .can_bypass = false,
}};

const std::string FakeComposite::kSourceRbElementDescription =
    "RingBuffer source element description";
const fhasp::Element FakeComposite::kSourceRbElement{{
    .id = kSourceRbElementId,
    .type = fhasp::ElementType::kRingBuffer,
    .description = kSourceRbElementDescription,
    .can_stop = false,
    .can_bypass = false,
}};

const std::string FakeComposite::kSourcePsElementDescription =
    "PacketStream source element description";
const fhasp::Element FakeComposite::kSourcePsElement{{
    .id = kSourcePsElementId,
    .type = fhasp::ElementType::kPacketStream,
    .description = kSourcePsElementDescription,
    .can_stop = false,
    .can_bypass = false,
}};

const std::string FakeComposite::kDestDaiElementDescription =
    "DaiInterconnect destination element description";
const fhasp::Element FakeComposite::kDestDaiElement{{
    .id = kDestDaiElementId,
    .type = fhasp::ElementType::kDaiInterconnect,
    .type_specific = fhasp::TypeSpecificElement::WithDaiInterconnect({{
        .plug_detect_capabilities = fhasp::PlugDetectCapabilities::kCanAsyncNotify,
    }}),
    .description = kDestDaiElementDescription,
    .can_stop = true,
    .can_bypass = false,
}};

const std::string FakeComposite::kDestRbElementDescription =
    "RingBuffer destination element description";
const fhasp::Element FakeComposite::kDestRbElement{{
    .id = kDestRbElementId,
    .type = fhasp::ElementType::kRingBuffer,
    .description = kDestRbElementDescription,
    .can_stop = false,
    .can_bypass = false,
}};

const std::string FakeComposite::kDestPsElementDescription =
    "PacketStream destination element description";
const fhasp::Element FakeComposite::kDestPsElement{{
    .id = kDestPsElementId,
    .type = fhasp::ElementType::kPacketStream,
    .description = kDestPsElementDescription,
    .can_stop = false,
    .can_bypass = false,
}};

const std::string FakeComposite::kSourceDualSupportPsElementDescription =
    "PacketStream dual format-type source element description";
const fhasp::Element FakeComposite::kSourceDualSupportPsElement{{
    .id = kSourceDualSupportPsElementId,
    .type = fhasp::ElementType::kPacketStream,
    .description = kSourceDualSupportPsElementDescription,
    .can_stop = false,
    .can_bypass = false,
}};

const std::string FakeComposite::kVendorSpecificElementDescription =
    "Vendor specific element description";
const fhasp::Element FakeComposite::kVendorSpecificElement{{
    .id = kVendorSpecificElementId,
    .type = fhasp::ElementType::kVendorSpecific,
    .type_specific = fhasp::TypeSpecificElement::WithVendorSpecific(fhasp::VendorSpecific{}),
    .description = kVendorSpecificElementDescription,
    .can_stop = false,
    .can_bypass = true,
}};

const std::string FakeComposite::kDynamicsElementDescription = "Dynamics element description";
const fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls
    FakeComposite::kDynamicsSupportedControls =
        fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls::kKneeWidth |
        fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls::kAttack |
        fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls::kRelease |
        fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls::kOutputGain;
const fhasp::Element FakeComposite::kDynamicsElement = []() {
  std::vector<fhasp::DynamicsBand> bands;

  fhasp::DynamicsBand band1;
  band1.id(kDynamicsBandId1);
  bands.push_back(std::move(band1));

  fhasp::DynamicsBand band2;
  band2.id(kDynamicsBandId2);
  bands.push_back(std::move(band2));

  fhasp::Dynamics dynamics;
  dynamics.bands(std::move(bands));
  dynamics.supported_controls(kDynamicsSupportedControls);

  return fhasp::Element{{
      .id = kDynamicsElementId,
      .type = fhasp::ElementType::kDynamics,
      .type_specific = fhasp::TypeSpecificElement::WithDynamics(std::move(dynamics)),
      .description = kDynamicsElementDescription,
      .can_stop = true,
      .can_bypass = true,
  }};
}();

const std::string FakeComposite::kEqualizerElementDescription = "Equalizer element description";
const fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls
    FakeComposite::kEqualizerSupportedControls =
        fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls::kCanControlFrequency |
        fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls::kCanControlQ |
        fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls::kSupportsTypePeak |
        fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls::kSupportsTypeNotch;
const fhasp::Element FakeComposite::kEqualizerElement = []() {
  std::vector<fhasp::EqualizerBand> bands;

  fhasp::EqualizerBand band1;
  band1.id(kEqualizerBandId1);
  bands.push_back(std::move(band1));

  fhasp::EqualizerBand band2;
  band2.id(kEqualizerBandId2);
  bands.push_back(std::move(band2));

  fhasp::Equalizer equalizer;
  equalizer.bands(std::move(bands));
  equalizer.supported_controls(kEqualizerSupportedControls);
  equalizer.can_disable_bands(true);
  equalizer.min_frequency(kEqualizerMinFrequency);
  equalizer.max_frequency(kEqualizerMaxFrequency);
  equalizer.max_q(kEqualizerMaxQ);
  equalizer.min_gain_db(kEqualizerMinGainDb);
  equalizer.max_gain_db(kEqualizerMaxGainDb);

  return fhasp::Element{{
      .id = kEqualizerElementId,
      .type = fhasp::ElementType::kEqualizer,
      .type_specific = fhasp::TypeSpecificElement::WithEqualizer(std::move(equalizer)),
      .description = kEqualizerElementDescription,
      .can_stop = true,
      .can_bypass = true,
  }};
}();

const std::string FakeComposite::kGainElementDescription = "Gain element description";
const fhasp::GainType FakeComposite::kGainType = fhasp::GainType::kDecibels;
const fhasp::GainDomain FakeComposite::kGainDomain = fhasp::GainDomain::kAnalog;
const fhasp::Element FakeComposite::kGainElement{{
    .id = kGainElementId,
    .type = fhasp::ElementType::kGain,
    .type_specific = fhasp::TypeSpecificElement::WithGain(fhasp::Gain{{
        .type = kGainType,
        .domain = kGainDomain,
        .min_gain = kGainMin,
        .max_gain = kGainMax,
        .min_gain_step = kGainStep,
    }}),
    .description = kGainElementDescription,
    .can_stop = false,
    .can_bypass = true,
}};

const std::string FakeComposite::kMuteElementDescription = "Mute element description";
const fhasp::Element FakeComposite::kMuteElement{{
    .id = kMuteElementId,
    .type = fhasp::ElementType::kMute,
    .description = kMuteElementDescription,
    .can_stop = false,
    .can_bypass = true,
}};

// ElementStates - note that the two Dai elements have vendor_specific_data that can be queried.
const zx::duration FakeComposite::kSourceDaiElementProcessingDelay = zx::nsec(0);
const fhasp::ElementState FakeComposite::kSourceDaiElementInitState{{
    .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect({{
        .plug_state = fhasp::PlugState{{
            .plugged = true,
            .plug_state_time = 0,
        }},
        .external_delay = 0,
    }}),
    .vendor_specific_data =
        std::vector<uint8_t>{
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
        },
    .started = false,
    .bypassed = false,
    .processing_delay = kSourceDaiElementProcessingDelay.get(),
}};

const zx::duration FakeComposite::kSourceRbElementProcessingDelay = zx::nsec(42);
const fhasp::ElementState FakeComposite::kSourceRbElementInitState{{
    .started = true,
    .bypassed = false,
    .processing_delay = kSourceRbElementProcessingDelay.get(),
}};

const zx::duration FakeComposite::kSourcePsElementProcessingDelay = zx::nsec(68);
const fhasp::ElementState FakeComposite::kSourcePsElementInitState{{
    .started = true,
    .bypassed = false,
    .processing_delay = kSourcePsElementProcessingDelay.get(),
}};

const zx::duration FakeComposite::kDestDaiElementExternalDelay = zx::nsec(123);
const zx::duration FakeComposite::kDestDaiElementProcessingDelay = zx::nsec(123);
const fhasp::ElementState FakeComposite::kDestDaiElementInitState{{
    .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect({{
        .plug_state = fhasp::PlugState{{
            .plugged = true,
            .plug_state_time = 0,
        }},
        .external_delay = kDestDaiElementExternalDelay.get(),
    }}),
    .vendor_specific_data =
        std::vector<uint8_t>{
            8,
            7,
            6,
            5,
            4,
            3,
            2,
            1,
            0,
        },
    .started = false,
    .bypassed = false,
    .processing_delay = kDestDaiElementProcessingDelay.get(),
}};

const fhasp::ElementState FakeComposite::kDestRbElementInitState{{
    .started = true,
    .bypassed = false,
}};

const fhasp::ElementState FakeComposite::kDestPsElementInitState{{
    .started = true,
    .bypassed = false,
}};

const zx::duration FakeComposite::kSourceDualSupportPsElementProcessingDelay = zx::nsec(75);
const fhasp::ElementState FakeComposite::kSourceDualSupportPsElementInitState{{
    .started = true,
    .bypassed = false,
}};

const fhasp::ElementState FakeComposite::kVendorSpecificElementInitState{{
    .type_specific = fhasp::TypeSpecificElementState::WithVendorSpecific({}),
    .vendor_specific_data =
        []() {
          std::vector<uint8_t> data(kVendorSpecificDataLength);
          for (uint8_t i = 0; i < kVendorSpecificDataLength; ++i) {
            data[i] = i;
          }
          return data;
        }(),
    .started = true,
    .bypassed = true,
}};

const fhasp::ElementState FakeComposite::kDynamicsElementInitState = []() {
  std::vector<fhasp::DynamicsBandState> band_states;

  fhasp::DynamicsBandState bs1;
  bs1.id(kDynamicsBandId1);
  bs1.min_frequency(kDynamicsMinFrequency1);
  bs1.max_frequency(kDynamicsMaxFrequency1);
  bs1.threshold_db(kDynamicsThresholdDb1);
  bs1.threshold_type(kDynamicsThresholdType1);
  bs1.ratio(kDynamicsRatio1);
  band_states.push_back(std::move(bs1));

  fhasp::DynamicsBandState bs2;
  bs2.id(kDynamicsBandId2);
  bs2.min_frequency(kDynamicsMinFrequency2);
  bs2.max_frequency(kDynamicsMaxFrequency2);
  bs2.threshold_db(kDynamicsThresholdDb2);
  bs2.threshold_type(kDynamicsThresholdType2);
  bs2.ratio(kDynamicsRatio2);
  band_states.push_back(std::move(bs2));

  fhasp::DynamicsElementState des;
  des.band_states(std::move(band_states));

  return fhasp::ElementState{{
      .type_specific = fhasp::TypeSpecificElementState::WithDynamics(std::move(des)),
      .started = false,
      .bypassed = false,
  }};
}();

const zx::duration FakeComposite::kEqualizerTurnOnDelay = zx::nsec(234);
const zx::duration FakeComposite::kEqualizerTurnOffDelay = zx::nsec(345);
const zx::duration FakeComposite::kEqualizerProcessingDelay = zx::nsec(456);
const fhasp::ElementState FakeComposite::kEqualizerElementInitState = []() {
  std::vector<fhasp::EqualizerBandState> band_states;

  fhasp::EqualizerBandState bs1;
  bs1.id(kEqualizerBandId1);
  bs1.type(kEqualizerType1);
  bs1.frequency(kEqualizerFrequency1);
  bs1.q(kEqualizerQ1);
  bs1.gain_db(kEqualizerGainDb1);
  bs1.enabled(kEqualizerEnabled1);
  band_states.push_back(std::move(bs1));

  fhasp::EqualizerBandState bs2;
  bs2.id(kEqualizerBandId2);
  bs2.type(kEqualizerType2);
  bs2.frequency(kEqualizerFrequency2);
  bs2.q(kEqualizerQ2);
  bs2.enabled(kEqualizerEnabled2);
  band_states.push_back(std::move(bs2));

  fhasp::EqualizerElementState ees;
  ees.band_states(std::move(band_states));

  return fhasp::ElementState{{
      .type_specific = fhasp::TypeSpecificElementState::WithEqualizer(std::move(ees)),
      .started = false,
      .bypassed = true,
      .turn_on_delay = kEqualizerTurnOnDelay.get(),
      .turn_off_delay = kEqualizerTurnOffDelay.get(),
      .processing_delay = kEqualizerProcessingDelay.get(),
  }};
}();

const fhasp::ElementState FakeComposite::kGainElementInitState{{
    .type_specific = fhasp::TypeSpecificElementState::WithGain({{
        .gain = kGainInitValue,
    }}),
    .started = true,
    .bypassed = false,
}};

const fhasp::ElementState FakeComposite::kMuteElementInitState{{
    .started = true,
    .bypassed = true,
}};

// Element set
const std::vector<fhasp::Element> FakeComposite::kElements{{
    kSourceDaiElement,
    kSourceRbElement,
    kSourcePsElement,
    kSourceDualSupportPsElement,
    kDestDaiElement,
    kDestRbElement,
    kDestPsElement,
    kVendorSpecificElement,
    kDynamicsElement,
    kEqualizerElement,
    kGainElement,
    kMuteElement,
}};

// Topologies and element paths
//
// element paths
const fhasp::EdgePair FakeComposite::kTopologyInputEdgePair{{
    .processing_element_id_from = kSourceDaiElementId,
    .processing_element_id_to = kDestRbElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyPsCaptureEdgePair{{
    .processing_element_id_from = kSourceDaiElementId,
    .processing_element_id_to = kDestPsElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyOutputEdgePair{{
    .processing_element_id_from = kSourceRbElementId,
    .processing_element_id_to = kDestDaiElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyPsOutputEdgePair{{
    .processing_element_id_from = kSourcePsElementId,
    .processing_element_id_to = kDestDaiElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologySourceDualSupportPsOutputEdgePair{{
    .processing_element_id_from = kSourceDualSupportPsElementId,
    .processing_element_id_to = kDestDaiElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyRbToVendorSpecificEdgePair{{
    .processing_element_id_from = kSourceRbElementId,
    .processing_element_id_to = kVendorSpecificElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyVendorSpecificToDynamicsEdgePair{{
    .processing_element_id_from = kVendorSpecificElementId,
    .processing_element_id_to = kDynamicsElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyDynamicsToEqualizerEdgePair{{
    .processing_element_id_from = kDynamicsElementId,
    .processing_element_id_to = kEqualizerElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyEqualizerToGainEdgePair{{
    .processing_element_id_from = kEqualizerElementId,
    .processing_element_id_to = kGainElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyGainToMuteEdgePair{{
    .processing_element_id_from = kGainElementId,
    .processing_element_id_to = kMuteElementId,
}};
const fhasp::EdgePair FakeComposite::kTopologyMuteToDaiEdgePair{{
    .processing_element_id_from = kMuteElementId,
    .processing_element_id_to = kDestDaiElementId,
}};

// Individual topologies
const fhasp::Topology FakeComposite::kInputOnlyTopology{{
    .id = kInputOnlyTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyInputEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kPacketStreamCaptureTopology{{
    .id = kPacketStreamCaptureTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyPsCaptureEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kFullDuplexTopology{{
    .id = kFullDuplexTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyInputEdgePair,
                kTopologyOutputEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kOutputOnlyTopology{{
    .id = kOutputOnlyTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyOutputEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kPacketStreamOutputTopology{{
    .id = kPacketStreamOutputTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyPsOutputEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kSourceDualSupportPsOutputTopology{{
    .id = kSourceDualSupportPsOutputTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologySourceDualSupportPsOutputEdgePair,
            },
        },
}};
const fhasp::Topology FakeComposite::kOutputWithProcessingTopology{{
    .id = kOutputWithProcessingTopologyId,
    .processing_elements_edge_pairs =
        {
            {
                kTopologyRbToVendorSpecificEdgePair,
                kTopologyVendorSpecificToDynamicsEdgePair,
                kTopologyDynamicsToEqualizerEdgePair,
                kTopologyEqualizerToGainEdgePair,
                kTopologyGainToMuteEdgePair,
                kTopologyMuteToDaiEdgePair,
            },
        },
}};

// Topology set
const std::vector<fhasp::Topology> FakeComposite::kTopologies{{
    kInputOnlyTopology,
    kPacketStreamCaptureTopology,
    kFullDuplexTopology,
    kOutputOnlyTopology,
    kPacketStreamOutputTopology,
    kOutputWithProcessingTopology,
    kSourceDualSupportPsOutputTopology,
}};

}  // namespace media_audio

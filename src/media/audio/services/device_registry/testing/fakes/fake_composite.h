// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_H_

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/test_base.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/test_base.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/zx/channel.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <optional>
#include <string_view>

#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

class FakeCompositeRingBuffer;
class FakeCompositePacketStream;

// This driver implements the audio driver interface and is configurable to simulate audio hardware.
class FakeComposite final
    : public std::enable_shared_from_this<FakeComposite>,
      public fidl::testing::TestBase<fuchsia_hardware_audio::Composite>,
      public fidl::testing::TestBase<fuchsia_hardware_audio_signalprocessing::SignalProcessing> {
 public:
  static constexpr char kDefaultManufacturer[] = "fake_composite device manufacturer";
  static constexpr char kDefaultProduct[] = "fake_composite device product";
  static constexpr UniqueId kDefaultUniqueInstanceId{
      0xF1, 0xD3, 0xB5, 0x97, 0x79, 0x5B, 0x3D, 0x1F,
      0x0E, 0x2C, 0x4A, 0x68, 0x86, 0xA4, 0xC2, 0xE0,
  };
  static constexpr ClockDomain kDefaultClockDomain = fuchsia_hardware_audio::kClockDomainMonotonic;
  static constexpr char kDefaultClockDomainStr[] = "0 (CLOCK_DOMAIN_MONOTONIC)";

  // DaiFormats and format sets
  //
  static constexpr uint32_t kDefaultDaiNumberOfChannels = 1;
  static constexpr uint32_t kDefaultDaiNumberOfChannels2 = 2;
  static constexpr fuchsia_hardware_audio::DaiSampleFormat kDefaultDaiSampleFormat =
      fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned;
  static constexpr fuchsia_hardware_audio::DaiSampleFormat kDefaultDaiSampleFormat2 =
      fuchsia_hardware_audio::DaiSampleFormat::kPcmFloat;
  static constexpr uint32_t kDefaultDaiFrameRate = 48000;
  static constexpr uint32_t kDefaultDaiFrameRate2 = 96000;
  static constexpr uint8_t kDefaultDaiBitsPerSlot = 16;
  static constexpr uint8_t kDefaultDaiBitsPerSlot2 = 32;
  static constexpr uint8_t kDefaultDaiBitsPerSample = 16;
  static constexpr uint8_t kDefaultDaiBitsPerSample2 = 32;

  static const fuchsia_hardware_audio::DaiFrameFormat kDefaultDaiFrameFormat;
  static const fuchsia_hardware_audio::DaiFrameFormat kDefaultDaiFrameFormat2;
  static const std::vector<uint32_t> kDefaultDaiNumberOfChannelsSet;
  static const std::vector<uint32_t> kDefaultDaiNumberOfChannelsSet2;
  static const std::vector<fuchsia_hardware_audio::DaiSampleFormat> kDefaultDaiSampleFormatSet;
  static const std::vector<fuchsia_hardware_audio::DaiSampleFormat> kDefaultDaiSampleFormatSet2;
  static const std::vector<fuchsia_hardware_audio::DaiFrameFormat> kDefaultDaiFrameFormatSet;
  static const std::vector<fuchsia_hardware_audio::DaiFrameFormat> kDefaultDaiFrameFormatSet2;
  static const std::vector<uint32_t> kDefaultDaiFrameRateSet;
  static const std::vector<uint32_t> kDefaultDaiFrameRateSet2;
  static const std::vector<uint8_t> kDefaultDaiBitsPerSlotSet;
  static const std::vector<uint8_t> kDefaultDaiBitsPerSlotSet2;
  static const std::vector<uint8_t> kDefaultDaiBitsPerSampleSet;
  static const std::vector<uint8_t> kDefaultDaiBitsPerSampleSet2;
  static const fuchsia_hardware_audio::DaiSupportedFormats kDefaultDaiFormatSet;
  static const fuchsia_hardware_audio::DaiSupportedFormats kDefaultDaiFormatSet2;
  static const std::vector<fuchsia_hardware_audio::DaiSupportedFormats> kDefaultDaiFormatSets;
  static const std::vector<fuchsia_hardware_audio::DaiSupportedFormats> kDefaultDaiFormatSets2;
  static const std::unordered_map<ElementId,
                                  std::vector<fuchsia_hardware_audio::DaiSupportedFormats>>
      kDefaultDaiFormatsMap;

  static const fuchsia_hardware_audio::DaiFormat kDefaultDaiFormat;
  static const fuchsia_hardware_audio::DaiFormat kDefaultDaiFormat2;

  // RingBufferFormats and format sets
  //
  static constexpr size_t kDefaultRingBufferAllocationSize = 8000;

  static constexpr uint8_t kDefaultNumberOfChannels1 = 2;
  static constexpr uint8_t kDefaultNumberOfChannels2 = 1;

  static constexpr uint32_t kDefaultChannelAttributes1MinFrequency = 50;
  static constexpr uint32_t kDefaultChannelAttributes1MaxFrequency = 22000;
  // Used in a ChannelSet with no maximum frequency specified
  static constexpr uint32_t kDefaultChannelAttributes2MinFrequency = 2000;
  // Used in a ChannelSet with no minimum frequency specified
  static constexpr uint32_t kDefaultChannelAttributes3MaxFrequency = 22050;
  static const fuchsia_hardware_audio::ChannelAttributes kDefaultChannelAttributes1;
  static const fuchsia_hardware_audio::ChannelAttributes kDefaultChannelAttributes2;
  static const fuchsia_hardware_audio::ChannelAttributes kDefaultChannelAttributes3;
  static const std::vector<fuchsia_hardware_audio::ChannelAttributes> kDefaultChannelAttributesSet1;
  static const std::vector<fuchsia_hardware_audio::ChannelAttributes> kDefaultChannelAttributesSet2;
  static const std::vector<fuchsia_hardware_audio::ChannelAttributes> kDefaultChannelAttributesSet3;
  static const fuchsia_hardware_audio::ChannelSet kDefaultChannelSet1;
  static const fuchsia_hardware_audio::ChannelSet kDefaultChannelSet2;
  static const fuchsia_hardware_audio::ChannelSet kDefaultChannelSet3;
  static const std::vector<fuchsia_hardware_audio::ChannelSet> kDefaultChannelSets1;
  static const std::vector<fuchsia_hardware_audio::ChannelSet> kDefaultChannelSets2;
  static const std::vector<fuchsia_hardware_audio::ChannelSet> kDefaultChannelSets3;

  static constexpr fuchsia_hardware_audio::SampleFormat kDefaultRbSampleFormat1 =
      fuchsia_hardware_audio::SampleFormat::kPcmSigned;
  static constexpr fuchsia_hardware_audio::SampleFormat kDefaultRbSampleFormat2 =
      fuchsia_hardware_audio::SampleFormat::kPcmSigned;
  static constexpr fuchsia_hardware_audio::SampleFormat kDefaultRbSampleFormat3 =
      fuchsia_hardware_audio::SampleFormat::kPcmFloat;
  static const std::vector<fuchsia_hardware_audio::SampleFormat> kDefaultRbSampleFormats1;
  static const std::vector<fuchsia_hardware_audio::SampleFormat> kDefaultRbSampleFormats2;
  static const std::vector<fuchsia_hardware_audio::SampleFormat> kDefaultRbSampleFormats3;

  static constexpr uint8_t kDefaultRbBytesPerSample1 = 2;
  static constexpr uint8_t kDefaultRbBytesPerSample2 = 4;
  static constexpr uint8_t kDefaultRbBytesPerSample3 = 4;
  static const std::vector<uint8_t> kDefaultRbBytesPerSampleSet1;
  static const std::vector<uint8_t> kDefaultRbBytesPerSampleSet2;
  static const std::vector<uint8_t> kDefaultRbBytesPerSampleSet3;

  static constexpr uint8_t kDefaultRbValidBitsPerSample1 = 16;
  static constexpr uint8_t kDefaultRbValidBitsPerSample2 = 20;
  static constexpr uint8_t kDefaultRbValidBitsPerSample3 = 32;
  static const std::vector<uint8_t> kDefaultRbValidBitsPerSampleSet1;
  static const std::vector<uint8_t> kDefaultRbValidBitsPerSampleSet2;
  static const std::vector<uint8_t> kDefaultRbValidBitsPerSampleSet3;

  static constexpr uint32_t kDefaultRbFrameRate1 = 48000;
  static constexpr uint32_t kDefaultRbFrameRate2 = 44100;
  static constexpr uint32_t kDefaultRbFrameRate3 = 44100;

  static const std::vector<uint32_t> kDefaultRbFrameRates1;
  static const std::vector<uint32_t> kDefaultRbFrameRates2;
  static const std::vector<uint32_t> kDefaultRbFrameRates3;

  static const fuchsia_hardware_audio::PcmSupportedFormats kDefaultPcmRingBufferFormatSet1;
  static const fuchsia_hardware_audio::PcmSupportedFormats kDefaultPcmRingBufferFormatSet2;
  static const fuchsia_hardware_audio::PcmSupportedFormats kDefaultPcmRingBufferFormatSet3;

  static const fuchsia_hardware_audio::SupportedFormats2 kDefaultRbFormatSet1;
  static const fuchsia_hardware_audio::SupportedFormats2 kDefaultRbFormatSet2;

  static const std::vector<fuchsia_hardware_audio::SupportedFormats2> kDefaultRbFormatSets1;
  static const std::vector<fuchsia_hardware_audio::SupportedFormats2> kDefaultRbFormatSets2;

  // PacketStreamFormats and format sets
  //
  // Note that for PacketStream elements supporting PCM formats, the default
  // RingBuffer format parameters (SampleFormat, BytesPerSample, ValidBitsPerSample, etc.) are used.
  static constexpr uint32_t kDefaultPsFrameRate1 = 48000;
  static constexpr uint32_t kDefaultPsFrameRate2 = 44100;

  static const std::vector<uint32_t> kDefaultPsFrameRates1;
  static const std::vector<uint32_t> kDefaultPsFrameRates2;

  static constexpr uint32_t kDefaultPsEncodingBitRate1 = 160'000;
  static constexpr uint32_t kDefaultPsEncodingBitRate2 = 256'000;

  static constexpr fuchsia_hardware_audio::EncodingType kDefaultPsEncodingType1 =
      fuchsia_hardware_audio::EncodingType::kSbc;
  static constexpr fuchsia_hardware_audio::EncodingType kDefaultPsEncodingType2 =
      fuchsia_hardware_audio::EncodingType::kAac;

  static const std::vector<fuchsia_hardware_audio::EncodingType> kDefaultPsEncodingTypes1;
  static const std::vector<fuchsia_hardware_audio::EncodingType> kDefaultPsEncodingTypes2;

  static const fuchsia_hardware_audio::SupportedEncodings kDefaultEncodingSet1;
  static const fuchsia_hardware_audio::SupportedEncodings kDefaultEncodingSet2;

  static const fuchsia_hardware_audio::SupportedFormats2 kDefaultPsFormatSet1;
  static const fuchsia_hardware_audio::SupportedFormats2 kDefaultPsFormatSet2;
  static const fuchsia_hardware_audio::SupportedFormats2 kDefaultPsFormatSet3;

  static const std::vector<fuchsia_hardware_audio::SupportedFormats2> kDefaultPsFormatSets1;
  static const std::vector<fuchsia_hardware_audio::SupportedFormats2> kDefaultPsFormatSets2;
  static const std::vector<fuchsia_hardware_audio::SupportedFormats2>
      kSourceDualSupportPsFormatSets;

  static const fuchsia_audio::Format kDefaultPsFormat1;
  static const fuchsia_audio::Format kDefaultPsFormat2;
  static const fuchsia_hardware_audio::Encoding kDefaultPsFormat3;

  static const std::unordered_map<ElementId, std::vector<fuchsia_hardware_audio::SupportedFormats2>>
      kDefaultRbFormatsMap;
  static const std::unordered_map<ElementId, std::vector<fuchsia_hardware_audio::SupportedFormats2>>
      kDefaultPsFormatsMap;

  // signalprocessing elements and topologies
  //
  // For min/max checks based on ranges, keep the DAI and RB element ID ranges contiguous.
  static constexpr ElementId kSourceDaiElementId = 0;
  static constexpr ElementId kDestDaiElementId = 1;
  static constexpr ElementId kMinDaiElementId = kSourceDaiElementId;
  static constexpr ElementId kMaxDaiElementId = kDestDaiElementId;

  static constexpr ElementId kDestRbElementId = 2;
  static constexpr ElementId kSourceRbElementId = 3;
  static constexpr ElementId kMinRingBufferElementId = kDestRbElementId;
  static constexpr ElementId kMaxRingBufferElementId = kSourceRbElementId;

  static constexpr ElementId kDestPsElementId = 4;
  static constexpr ElementId kSourcePsElementId = 5;
  static constexpr ElementId kSourceDualSupportPsElementId = 6;
  static constexpr ElementId kMinPacketStreamElementId = kDestPsElementId;
  static constexpr ElementId kMaxPacketStreamElementId = kSourceDualSupportPsElementId;

  static constexpr ElementId kVendorSpecificElementId = 7;
  static constexpr ElementId kDynamicsElementId = 8;
  static constexpr ElementId kEqualizerElementId = 9;
  static constexpr ElementId kGainElementId = 10;
  static constexpr ElementId kMuteElementId = 11;
  static constexpr ElementId kMinElementId = kSourceDaiElementId;
  static constexpr ElementId kMaxElementId = kMuteElementId;

  static const std::string kSourceDaiElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kSourceDaiElement;
  static const zx::duration kSourceDaiElementProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kSourceDaiElementInitState;

  static const std::string kSourceRbElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kSourceRbElement;
  static const zx::duration kSourceRbElementProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kSourceRbElementInitState;

  static const std::string kSourcePsElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kSourcePsElement;
  static const zx::duration kSourcePsElementProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kSourcePsElementInitState;

  static const std::string kSourceDualSupportPsElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kSourceDualSupportPsElement;
  static const zx::duration kSourceDualSupportPsElementProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState
      kSourceDualSupportPsElementInitState;

  static const std::string kDestDaiElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kDestDaiElement;
  static const zx::duration kDestDaiElementExternalDelay;
  static const zx::duration kDestDaiElementProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kDestDaiElementInitState;

  static const std::string kDestRbElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kDestRbElement;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kDestRbElementInitState;

  static const std::string kDestPsElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kDestPsElement;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kDestPsElementInitState;

  // Dynamics element properties
  static const std::string kDynamicsElementDescription;
  static constexpr uint64_t kDynamicsBandId1 = 42;
  static constexpr uint64_t kDynamicsBandId2 = 68;
  static const fuchsia_hardware_audio_signalprocessing::DynamicsSupportedControls
      kDynamicsSupportedControls;
  static const fuchsia_hardware_audio_signalprocessing::Element kDynamicsElement;
  // Dynamics initial state
  static constexpr uint32_t kDynamicsMinFrequency1 = 0;
  static constexpr uint32_t kDynamicsMaxFrequency1 = 20000;
  static constexpr float kDynamicsThresholdDb1 = 0.0f;
  static constexpr float kDynamicsRatio1 = 1.0f;
  static constexpr uint32_t kDynamicsMinFrequency2 = 1000;
  static constexpr uint32_t kDynamicsMaxFrequency2 = 5000;
  static constexpr float kDynamicsThresholdDb2 = -10.0f;
  static constexpr float kDynamicsRatio2 = 2.0f;
  static constexpr fuchsia_hardware_audio_signalprocessing::ThresholdType kDynamicsThresholdType1 =
      fuchsia_hardware_audio_signalprocessing::ThresholdType::kAbove;
  static constexpr fuchsia_hardware_audio_signalprocessing::ThresholdType kDynamicsThresholdType2 =
      fuchsia_hardware_audio_signalprocessing::ThresholdType::kBelow;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kDynamicsElementInitState;

  // Equalizer element properties
  static const std::string kEqualizerElementDescription;
  static constexpr uint64_t kEqualizerBandId1 = 10;
  static constexpr uint64_t kEqualizerBandId2 = 20;
  static const fuchsia_hardware_audio_signalprocessing::EqualizerSupportedControls
      kEqualizerSupportedControls;
  static constexpr uint32_t kEqualizerMinFrequency = 20;
  static constexpr uint32_t kEqualizerMaxFrequency = 20000;
  static constexpr float kEqualizerMaxQ = 5.0f;
  static constexpr float kEqualizerMinGainDb = -20.0f;
  static constexpr float kEqualizerMaxGainDb = 20.0f;
  static const fuchsia_hardware_audio_signalprocessing::Element kEqualizerElement;
  // Equalizer initial state
  static constexpr uint32_t kEqualizerFrequency1 = 500;
  static constexpr float kEqualizerQ1 = 1.0f;
  static constexpr float kEqualizerGainDb1 = -6.0f;
  static constexpr bool kEqualizerEnabled1 = true;
  static constexpr fuchsia_hardware_audio_signalprocessing::EqualizerBandType kEqualizerType1 =
      fuchsia_hardware_audio_signalprocessing::EqualizerBandType::kLowShelf;
  static constexpr uint32_t kEqualizerFrequency2 = 1000;
  static constexpr float kEqualizerQ2 = 10.0f;
  static constexpr bool kEqualizerEnabled2 = true;
  static constexpr fuchsia_hardware_audio_signalprocessing::EqualizerBandType kEqualizerType2 =
      fuchsia_hardware_audio_signalprocessing::EqualizerBandType::kNotch;
  static const zx::duration kEqualizerTurnOnDelay;
  static const zx::duration kEqualizerTurnOffDelay;
  static const zx::duration kEqualizerProcessingDelay;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kEqualizerElementInitState;

  // Gain element properties
  static const std::string kGainElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::GainType kGainType;
  static const fuchsia_hardware_audio_signalprocessing::GainDomain kGainDomain;
  static constexpr float kGainMin = -84.0f;
  static constexpr float kGainMax = 12.0f;
  static constexpr float kGainStep = 0.25f;
  static const fuchsia_hardware_audio_signalprocessing::Element kGainElement;
  // Gain initial state
  static constexpr float kGainInitValue = 0.0f;
  static const fuchsia_hardware_audio_signalprocessing::ElementState kGainElementInitState;

  // Vendor-specific element properties
  static const std::string kVendorSpecificElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kVendorSpecificElement;
  // Vendor-specific initial state
  static constexpr size_t kVendorSpecificDataLength = 42;
  static const fuchsia_hardware_audio_signalprocessing::ElementState
      kVendorSpecificElementInitState;

  // Mute element properties
  static const std::string kMuteElementDescription;
  static const fuchsia_hardware_audio_signalprocessing::Element kMuteElement;
  // Mute initial state
  static const fuchsia_hardware_audio_signalprocessing::ElementState kMuteElementInitState;

  static const std::vector<fuchsia_hardware_audio_signalprocessing::Element> kElements;

  // For min/max checks based on ranges, keep this range contiguous.
  static constexpr TopologyId kStartTopologyId = 10;
  static constexpr TopologyId kInputOnlyTopologyId = kStartTopologyId;
  static constexpr TopologyId kPacketStreamCaptureTopologyId = kInputOnlyTopologyId + 1;
  static constexpr TopologyId kFullDuplexTopologyId = kPacketStreamCaptureTopologyId + 1;
  static constexpr TopologyId kOutputOnlyTopologyId = kFullDuplexTopologyId + 1;
  static constexpr TopologyId kPacketStreamOutputTopologyId = kOutputOnlyTopologyId + 1;
  static constexpr TopologyId kOutputWithProcessingTopologyId = kPacketStreamOutputTopologyId + 1;
  static constexpr TopologyId kSourceDualSupportPsOutputTopologyId =
      kOutputWithProcessingTopologyId + 1;
  static constexpr TopologyId kEndTopologyId = kSourceDualSupportPsOutputTopologyId + 1;
  static constexpr TopologyId kDefaultTopologyId = kFullDuplexTopologyId;
  static constexpr TopologyId kSubsequentTopologyId = kInputOnlyTopologyId;

  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyInputEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyPsCaptureEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyOutputEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyPsOutputEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair
      kTopologyRbToVendorSpecificEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair
      kTopologyVendorSpecificToDynamicsEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair
      kTopologyDynamicsToEqualizerEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyEqualizerToGainEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyGainToMuteEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair kTopologyMuteToDaiEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::EdgePair
      kTopologySourceDualSupportPsOutputEdgePair;
  static const fuchsia_hardware_audio_signalprocessing::Topology kInputOnlyTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kPacketStreamCaptureTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kFullDuplexTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kOutputOnlyTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kPacketStreamOutputTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kOutputWithProcessingTopology;
  static const fuchsia_hardware_audio_signalprocessing::Topology kSourceDualSupportPsOutputTopology;
  static const std::vector<fuchsia_hardware_audio_signalprocessing::Topology> kTopologies;

  FakeComposite(const FakeComposite&) = delete;
  FakeComposite(FakeComposite&&) = delete;
  FakeComposite& operator=(const FakeComposite&) = delete;
  FakeComposite& operator=(FakeComposite&&) = delete;

  FakeComposite(zx::channel server_end, zx::channel client_end, async_dispatcher_t* dispatcher);
  ~FakeComposite() override;

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADR_WARN_OBJECT() << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  // This returns a fidl::client_end<fuchsia_hardware_audio::Composite>. The driver will not start
  // serving requests until Enable is called, which is why we separate construction and Enable().
  fidl::ClientEnd<fuchsia_hardware_audio::Composite> Enable();
  void DropComposite();
  void DropChildren();
  void DropRingBuffers();
  void DropPacketStreams();
  void DropRingBuffer(ElementId element_id);
  void DropPacketStream(ElementId element_id);
  static void on_rb_unbind(FakeCompositeRingBuffer* fake_ring_buffer, fidl::UnbindInfo info,
                           fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer>);
  static void on_ps_unbind(FakeCompositePacketStream* fake_packet_stream, fidl::UnbindInfo info,
                           fidl::ServerEnd<fuchsia_hardware_audio::PacketStreamControl>);
  void RingBufferWasDropped(ElementId element_id);
  void PacketStreamWasDropped(ElementId element_id);

  void InjectPacketStreamBufferTypes(ElementId element_id,
                                     fuchsia_hardware_audio::BufferType buffer_types) {
    inject_packet_stream_buffer_types_[element_id] = buffer_types;
  }
  void InjectPacketStreamAllocateVmosError(ElementId element_id, zx_status_t error) {
    inject_packet_stream_allocate_vmos_error_[element_id] = error;
  }
  void InjectPacketStreamRegisterVmosError(ElementId element_id, zx_status_t error) {
    inject_packet_stream_register_vmos_error_[element_id] = error;
  }

  // These may be called before the RingBuffer object is created; info must be cached until then.
  void ReserveRingBufferSize(ElementId element_id, size_t size);
  void EnableActiveChannelsSupport(ElementId element_id);
  void DisableActiveChannelsSupport(ElementId element_id);
  void PresetTurnOnDelay(ElementId element_id, std::optional<zx::duration> turn_on_delay);
  void PresetInternalExternalDelays(ElementId element_id, zx::duration internal_delay,
                                    std::optional<zx::duration> external_delay);

  async_dispatcher_t* dispatcher() { return dispatcher_; }
  bool is_bound() const { return binding_.has_value(); }

  bool responsive() const { return responsive_; }
  // Once we mark a device unresponsive, it cannot correctly transition back to responsive state.
  void set_unresponsive() { responsive_ = false; }
  void CompleteCreateRingBuffer(fuchsia_hardware_audio::DriverError error =
                                    fuchsia_hardware_audio::DriverError::kNotSupported);
  std::optional<bool> health_state() const { return healthy_; }
  void set_health_state(std::optional<bool> healthy) { healthy_ = healthy; }

  void set_device_manufacturer(std::optional<std::string> mfgr) { manufacturer_ = std::move(mfgr); }
  void set_device_product(std::optional<std::string> product) { product_ = std::move(product); }
  void set_stream_unique_id(std::optional<UniqueId> uid) {
    if (uid) {
      std::memcpy(uid_->data(), uid->data(), sizeof(*uid));
    } else {
      uid_.reset();
    }
  }
  void set_clock_domain(std::optional<ClockDomain> clock_domain) { clock_domain_ = clock_domain; }

  bool is_element_type(ElementId element_id,
                       fuchsia_hardware_audio_signalprocessing::ElementType element_type) const {
    for (auto& element_iter : elements_) {
      if (element_iter.first == element_id) {
        return element_iter.second.element.type() == element_type;
      }
    }
    return false;  // We didn't find the element.
  }

  // These rely on the RingBuffer being created; do not use them to pre-configure the RingBuffer.
  uint64_t RingBufferActiveChannelsBitmask(ElementId element_id) const;
  zx::time RingBufferSetActiveChannelsCompletedAt(ElementId element_id) const;
  bool RingBufferStarted(ElementId element_id) const;
  zx::time RingBufferMonoStartTime(ElementId element_id) const;
  void RingBufferInjectDelayUpdate(ElementId element_id, std::optional<zx::duration> internal_delay,
                                   std::optional<zx::duration> external_delay);
  void InjectTopologyChange(std::optional<TopologyId> topology_id);
  void InjectElementStateChange(ElementId element_id,
                                fuchsia_hardware_audio_signalprocessing::ElementState new_state);

  bool PacketStreamStarted(ElementId element_id) const;
  zx::time PacketStreamMonoStartTime(ElementId element_id) const;
  std::optional<zx_rights_t> PacketStreamVmoRights(ElementId element_id, uint64_t vmo_id) const;

  // fuchsia_hardware_audio::Composite implementation
  void Reset(ResetCompleter::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                            GetRingBufferFormatsCompleter::Sync& completer) override;
  void CreateRingBuffer(CreateRingBufferRequest& request,
                        CreateRingBufferCompleter::Sync& completer) override;
  void GetPacketStreamFormats(GetPacketStreamFormatsRequest& request,
                              GetPacketStreamFormatsCompleter::Sync& completer) override;
  void CreatePacketStream(CreatePacketStreamRequest& request,
                          CreatePacketStreamCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsRequest& request,
                     GetDaiFormatsCompleter::Sync& completer) override;
  void SetDaiFormat(SetDaiFormatRequest& request, SetDaiFormatCompleter::Sync& completer) override;

  // fuchsia_hardware_audio.Health implementation
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override;

  // fuchsia_hardware_audio_signalprocessing.Connector implementation
  void SignalProcessingConnect(SignalProcessingConnectRequest& request,
                               SignalProcessingConnectCompleter::Sync& completer) override;

  // fuchsia_hardware_audio_signalprocessing::SignalProcessing implementation (including Reader)
  void GetElements(GetElementsCompleter::Sync& completer) final;
  void GetTopologies(GetTopologiesCompleter::Sync& completer) final;
  void WatchElementState(WatchElementStateRequest& request,
                         WatchElementStateCompleter::Sync& completer) final;
  void WatchTopology(WatchTopologyCompleter::Sync& completer) final;
  void SetElementState(SetElementStateRequest& request,
                       SetElementStateCompleter::Sync& completer) final;
  void SetTopology(SetTopologyRequest& request, SetTopologyCompleter::Sync& completer) final;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio_signalprocessing::SignalProcessing>
          metadata,
      fidl::UnknownMethodCompleter::Sync& completer) final;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) final;

 private:
  friend FakeCompositeRingBuffer;
  friend FakeCompositePacketStream;

  static constexpr std::string_view kClassName = "FakeComposite";

  struct FakeElementRecord {
    fuchsia_hardware_audio_signalprocessing::Element element;
    fuchsia_hardware_audio_signalprocessing::ElementState state;
    bool state_has_changed = true;  // immediately complete the first WatchElementState request
    std::optional<WatchElementStateCompleter::Async> watch_completer;
  };
  void SetupElementsMap();

  // Internal implementation methods/members
  static bool DaiFormatIsSupported(ElementId element_id,
                                   const fuchsia_hardware_audio::DaiFormat& format);

  static void MaybeCompleteWatchElementState(FakeElementRecord& element_record);
  void MaybeCompleteWatchTopology();

  async_dispatcher_t* dispatcher_;
  fidl::ServerEnd<fuchsia_hardware_audio::Composite> server_end_;
  fidl::ClientEnd<fuchsia_hardware_audio::Composite> client_end_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_audio::Composite>> binding_;

  bool responsive_ = true;
  std::optional<bool> healthy_ = true;
  std::vector<GetPropertiesCompleter::Async> get_properties_completers_;
  std::vector<GetRingBufferFormatsCompleter::Async> get_ring_buffer_formats_completers_;
  std::vector<CreateRingBufferCompleter::Async> create_ring_buffer_completers_;
  std::vector<GetPacketStreamFormatsCompleter::Async> get_packet_stream_formats_completers_;
  std::vector<CreatePacketStreamCompleter::Async> create_packet_stream_completers_;
  std::vector<GetDaiFormatsCompleter::Async> get_dai_formats_completers_;
  std::vector<SetDaiFormatCompleter::Async> set_dai_format_completers_;
  std::vector<GetHealthStateCompleter::Async> get_health_state_completers_;
  std::vector<ResetCompleter::Async> reset_completers_;
  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::optional<std::string> manufacturer_ = kDefaultManufacturer;
  std::optional<std::string> product_ = kDefaultProduct;
  std::optional<UniqueId> uid_ = kDefaultUniqueInstanceId;
  std::optional<ClockDomain> clock_domain_ = kDefaultClockDomain;

  bool supports_signalprocessing_ = true;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_audio_signalprocessing::SignalProcessing>>
      signal_processing_binding_;

  std::vector<GetElementsCompleter::Async> get_elements_completers_;
  std::vector<WatchElementStateCompleter::Async> watch_element_state_completers_;
  std::vector<SetElementStateCompleter::Async> set_element_state_completers_;
  std::unordered_map<ElementId, FakeElementRecord> elements_;

  std::vector<GetTopologiesCompleter::Async> get_topologies_completers_;
  std::vector<WatchTopologyCompleter::Async> watch_topology_completers_;
  std::vector<SetTopologyCompleter::Async> set_topology_completers_;
  std::optional<TopologyId> topology_id_ = kDefaultTopologyId;
  bool topology_has_changed_ = true;

  std::unordered_map<ElementId, size_t> ring_buffer_allocation_sizes_;
  std::unordered_map<ElementId, bool> active_channels_support_overrides_;
  std::unordered_map<ElementId, std::optional<zx::duration>> turn_on_delay_overrides_;
  std::unordered_map<ElementId, zx::duration> internal_delay_overrides_;
  std::unordered_map<ElementId, std::optional<zx::duration>> external_delay_overrides_;

  std::unordered_map<ElementId, fidl::ServerBindingRef<fuchsia_hardware_audio::RingBuffer>>
      ring_buffer_bindings_;
  std::unordered_map<ElementId, std::unique_ptr<FakeCompositeRingBuffer>> ring_buffers_;

  // PacketStream support
  std::unordered_map<ElementId, fuchsia_hardware_audio::BufferType>
      inject_packet_stream_buffer_types_;
  std::unordered_map<ElementId, zx_status_t> inject_packet_stream_allocate_vmos_error_;
  std::unordered_map<ElementId, zx_status_t> inject_packet_stream_register_vmos_error_;
  std::unordered_map<ElementId, fidl::ServerBindingRef<fuchsia_hardware_audio::PacketStreamControl>>
      packet_stream_bindings_;
  std::unordered_map<ElementId, std::unique_ptr<FakeCompositePacketStream>> packet_streams_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_H_

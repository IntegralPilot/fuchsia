// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_VALIDATE_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_VALIDATE_H_

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <zircon/rights.h>

#include <unordered_map>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

// TODO(https://fxbug.dev/42068183): official frame-rate limits/expectations for audio devices.
constexpr uint32_t kMinSupportedDaiFrameRate = 1000;
constexpr uint32_t kMaxSupportedDaiFrameRate = 192000 * 8 * 64;
constexpr uint8_t kMaxSupportedDaiFormatBitsPerSlot = 64;

// We define these here only temporarily, as we do not publish frame-rate limits for audio devices.
// TODO(https://fxbug.dev/42068183): official frame-rate limits/expectations for audio devices.
const uint32_t kMinSupportedRingBufferFrameRate = 1000;
const uint32_t kMinSupportedPacketStreamFrameRate = 1000;
const uint32_t kMaxSupportedRingBufferFrameRate = 192000;
const uint32_t kMaxSupportedPacketStreamFrameRate = 192000;

constexpr zx_rights_t kRequiredVmoRightsForRead = ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_MAP;
constexpr zx_rights_t kRequiredVmoRightsForReadWrite = kRequiredVmoRightsForRead | ZX_RIGHT_WRITE;

// Utility functions to validate direct responses from audio drivers.
bool ClientIsValidForDeviceType(const fuchsia_audio_device::DeviceType& device_type,
                                const fuchsia_audio_device::DriverClient& driver_client);

bool ValidatePlugState(const fuchsia_hardware_audio::PlugState& plug_state,
                       std::optional<fuchsia_hardware_audio::PlugDetectCapabilities>
                           plug_detect_capabilities = std::nullopt);

bool ValidateCodecProperties(
    const fuchsia_hardware_audio::CodecProperties& codec_props,
    std::optional<const fuchsia_hardware_audio::PlugState> plug_state = std::nullopt);
bool ValidateCodecFormatInfo(const fuchsia_hardware_audio::CodecFormatInfo& format_info);

bool ValidateCompositeProperties(
    const fuchsia_hardware_audio::CompositeProperties& composite_props);

bool ValidateDeviceInfo(const fuchsia_audio_device::Info& device_info);

bool ValidateTopologies(
    const std::vector<fuchsia_hardware_audio_signalprocessing::Topology>& topologies,
    const std::unordered_map<ElementId, ElementRecord>& element_map);
bool ValidateTopology(const fuchsia_hardware_audio_signalprocessing::Topology& topology,
                      const std::unordered_map<ElementId, ElementRecord>& element_map);

bool ValidateElements(
    const std::vector<fuchsia_hardware_audio_signalprocessing::Element>& elements);
bool ValidateElement(const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateDaiInterconnectElement(
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateDynamicsElement(const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateEqualizerElement(const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateGainElement(const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateVendorSpecificElement(const fuchsia_hardware_audio_signalprocessing::Element& element);

bool ValidateElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateDaiInterconnectElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateDynamicsElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateEqualizerElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateGainElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateVendorSpecificElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);

bool ValidateSettableElementState(
    const fuchsia_hardware_audio_signalprocessing::SettableElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateSettableDynamicsElementState(
    const fuchsia_hardware_audio_signalprocessing::SettableElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateSettableEqualizerElementState(
    const fuchsia_hardware_audio_signalprocessing::SettableElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateSettableGainElementState(
    const fuchsia_hardware_audio_signalprocessing::SettableElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);
bool ValidateSettableVendorSpecificElementState(
    const fuchsia_hardware_audio_signalprocessing::SettableElementState& element_state,
    const fuchsia_hardware_audio_signalprocessing::Element& element);

bool ValidateRingBufferFormatSets(
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& ring_buffer_format_sets);
bool ValidateSampleFormatCompatibility(uint8_t bytes_per_sample,
                                       fuchsia_hardware_audio::SampleFormat sample_format);

bool ValidatePcmFormat(const fuchsia_hardware_audio::PcmFormat& pcm_format);
bool ValidateEncoding(const fuchsia_hardware_audio::Encoding& encoding);

bool ValidatePcmSupportedFormats(const fuchsia_hardware_audio::PcmSupportedFormats& pcm_format_set,
                                 bool is_packet_stream);
bool ValidateSupportedEncodings(
    const fuchsia_hardware_audio::SupportedEncodings& encoding_format_set);

bool ValidateDaiFormatSets(
    const std::vector<fuchsia_hardware_audio::DaiSupportedFormats>& dai_format_sets);
bool ValidateDaiFormat(const fuchsia_hardware_audio::DaiFormat& dai_format);

bool ValidateRingBufferProperties(const fuchsia_hardware_audio::RingBufferProperties& rb_props);
bool ValidateRingBufferVmo(const zx::vmo& vmo, uint32_t num_frames,
                           const fuchsia_hardware_audio::Format2& format,
                           zx_rights_t required_rights);

bool ValidatePacketStreamProperties(
    const fuchsia_hardware_audio::PacketStreamProperties& packet_stream_properties);
bool ValidatePacketStreamFormat(const fuchsia_hardware_audio::Format2& format);
bool ValidatePacketStreamFormatSets(
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& packet_stream_format_sets);
zx_status_t ValidatePacketStreamVmo(const fuchsia_hardware_audio::VmoInfo& vmo_info,
                                    zx_rights_t required_rights,
                                    std::optional<uint64_t> min_size = std::nullopt);

bool ValidateDelayInfo(const fuchsia_hardware_audio::DelayInfo& delay_info);

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_VALIDATE_H_

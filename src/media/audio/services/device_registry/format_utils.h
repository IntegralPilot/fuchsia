// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_FORMAT_UTILS_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_FORMAT_UTILS_H_

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>

#include <optional>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

// Maps fuchsia_audio::SampleType to driver-level PCM format details.
struct DriverPcmDetails {
  fuchsia_hardware_audio::SampleFormat sample_format;
  uint8_t bytes_per_sample;
  uint8_t max_valid_bits;
};

std::optional<DriverPcmDetails> MapSampleTypeToDriverPcm(fuchsia_audio::SampleType sample_type);

// Maps driver-level PCM format details to fuchsia_audio::SampleType.
std::optional<fuchsia_audio::SampleType> MapDriverPcmToSampleType(
    fuchsia_hardware_audio::SampleFormat sample_format, uint8_t bytes_per_sample);

// Returns a vector of translated PcmFormatSets from driver SupportedFormats2.
std::vector<fuchsia_audio_device::PcmFormatSet> TranslateRingBufferFormatSets(
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& ring_buffer_format_sets);

// Returns a vector of translated PacketStreamSupportedFormats from driver SupportedFormats2.
std::vector<fuchsia_audio_device::PacketStreamSupportedFormats> TranslatePacketStreamFormatSets(
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& packet_stream_format_sets);

// Maps driver-level PcmSupportedFormats to fuchsia_audio_device::PcmFormatSet.
std::optional<fuchsia_audio_device::PcmFormatSet> MapPcmSupportedFormats(
    const fuchsia_hardware_audio::PcmSupportedFormats& pcm_formats);

size_t CountFormatMatches(const std::vector<fuchsia_hardware_audio::SampleFormat>& sample_formats,
                          fuchsia_hardware_audio::SampleFormat format_to_match);

size_t CountUcharMatches(const std::vector<uint8_t>& uchars, size_t uchar_to_match);

bool DaiFormatIsSupported(
    ElementId element_id,
    const std::vector<fuchsia_audio_device::ElementDaiFormatSet>& element_dai_format_sets,
    const fuchsia_hardware_audio::DaiFormat& format);

bool RingBufferFormatIsSupported(
    ElementId element_id,
    const std::vector<fuchsia_audio_device::ElementRingBufferFormatSet>&
        element_ring_buffer_format_sets,
    const fuchsia_hardware_audio::Format2& format);

bool PacketStreamFormatIsSupported(
    ElementId element_id,
    const std::vector<fuchsia_audio_device::ElementPacketStreamFormatSet>&
        element_packet_stream_format_sets,
    const fuchsia_hardware_audio::Format2& format);

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_FORMAT_UTILS_H_

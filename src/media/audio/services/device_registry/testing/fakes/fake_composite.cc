// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/result.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_packet_stream.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_ring_buffer.h"
#include "src/media/audio/services/device_registry/testing/fakes/logging.h"
namespace media_audio {

namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace {

bool ElementStateMatchesSettableElementState(
    fuchsia_hardware_audio_signalprocessing::ElementState state,
    fuchsia_hardware_audio_signalprocessing::SettableElementState settable_state) {
  return
      // started must match
      state.started() == settable_state.started() &&
      // bypassed must match
      state.bypassed() == settable_state.bypassed() &&
      // type_specific must match ...
      (
          // ... whether it is dynamics ...
          (state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::Tag::kDynamics &&
           settable_state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::SettableTypeSpecificElementState::Tag::
                   kDynamics &&
           state.type_specific()->dynamics().value() ==
               settable_state.type_specific()->dynamics().value()) ||
          // ... or equalizer ...
          (state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::Tag::kEqualizer &&
           settable_state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::SettableTypeSpecificElementState::Tag::
                   kEqualizer &&
           state.type_specific()->equalizer().value() ==
               settable_state.type_specific()->equalizer().value()) ||
          // ... or gain ...
          (state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::Tag::kGain &&
           settable_state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::SettableTypeSpecificElementState::Tag::
                   kGain &&
           state.type_specific()->gain().value() ==
               settable_state.type_specific()->gain().value()) ||
          // ... or vendor_specific.
          (state.type_specific()->Which() == fuchsia_hardware_audio_signalprocessing::
                                                 TypeSpecificElementState::Tag::kVendorSpecific &&
           settable_state.type_specific()->Which() ==
               fuchsia_hardware_audio_signalprocessing::SettableTypeSpecificElementState::Tag::
                   kVendorSpecific &&
           state.type_specific()->vendor_specific().value() ==
               settable_state.type_specific()->vendor_specific().value())) &&
      // vendor_specific_data must match
      state.vendor_specific_data() == settable_state.vendor_specific_data();
}

bool FormatIsSupported(const fha::Format2& format,
                       const std::vector<fha::SupportedFormats2>& format_sets) {
  if (format.Which() != fha::Format2::Tag::kPcmFormat &&
      format.Which() != fha::Format2::Tag::kEncoding) {
    return false;
  }

  if (format.Which() == fha::Format2::Tag::kEncoding) {
    if (!format.encoding()->encoding_type().has_value() ||
        !format.encoding()->decoded_channel_count().has_value() ||
        !format.encoding()->average_encoding_bitrate().has_value()) {
      return false;
    }
    for (const auto& format_set : format_sets) {
      if (format_set.Which() != fha::SupportedFormats2::Tag::kSupportedEncodings) {
        continue;
      }
      const auto& supported_encodings = format_set.supported_encodings().value();
      if (!supported_encodings.encoding_types().has_value()) {
        continue;
      }
      bool type_match = false;
      for (const auto& encoding_type : supported_encodings.encoding_types().value()) {
        if (encoding_type == format.encoding()->encoding_type().value()) {
          type_match = true;
          break;
        }
      }
      if (!type_match) {
        continue;
      }

      if (supported_encodings.decoded_channel_sets().has_value()) {
        bool channel_match = false;
        for (const auto& channel_set : supported_encodings.decoded_channel_sets().value()) {
          if (channel_set.attributes()->size() ==
              format.encoding()->decoded_channel_count().value()) {
            channel_match = true;
            break;
          }
        }
        if (!channel_match) {
          continue;
        }
      }

      if (supported_encodings.decoded_frame_rates().has_value() &&
          format.encoding()->decoded_frame_rate().has_value()) {
        bool rate_match = false;
        for (const auto& frame_rate : supported_encodings.decoded_frame_rates().value()) {
          if (frame_rate == format.encoding()->decoded_frame_rate().value()) {
            rate_match = true;
            break;
          }
        }
        if (!rate_match) {
          continue;
        }
      }

      return true;
    }
    return false;
  }

  // PCM format sets
  for (const auto& format_set : format_sets) {
    if (format_set.Which() != fha::SupportedFormats2::Tag::kPcmSupportedFormats) {
      continue;
    }
    bool match = false;
    for (const auto& frame_rate : format_set.pcm_supported_formats()->frame_rates().value()) {
      if (frame_rate == format.pcm_format()->frame_rate()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (const auto& sample_format : format_set.pcm_supported_formats()->sample_formats().value()) {
      if (sample_format == format.pcm_format()->sample_format()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (const auto& channel_set : format_set.pcm_supported_formats()->channel_sets().value()) {
      if (channel_set.attributes()->size() == format.pcm_format()->number_of_channels()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (const auto& bytes_per_sample :
         format_set.pcm_supported_formats()->bytes_per_sample().value()) {
      if (bytes_per_sample == format.pcm_format()->bytes_per_sample()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (const auto& valid_bits :
         format_set.pcm_supported_formats()->valid_bits_per_sample().value()) {
      if (valid_bits == format.pcm_format()->valid_bits_per_sample()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }
    return true;
  }

  return false;
}

void on_unbind(FakeComposite* fake_composite, fidl::UnbindInfo info,
               fidl::ServerEnd<fha::Composite> server_end) {
  ADR_LOG(kLogFakeComposite) << "for FakeComposite";
  fake_composite->DropChildren();
}

}  // namespace

FakeComposite::FakeComposite(zx::channel server_end, zx::channel client_end,
                             async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher),
      server_end_(std::move(server_end)),
      client_end_(std::move(client_end)) {
  ADR_LOG_METHOD(kLogFakeComposite || kLogObjectLifetimes);

  SetupElementsMap();
}

FakeComposite::~FakeComposite() { ADR_LOG_METHOD(kLogFakeComposite || kLogObjectLifetimes); }

// From the device side, drop the Composite protocol connection as if the device has been removed.
void FakeComposite::DropComposite() {
  FX_CHECK(binding_.has_value()) << "Should not call DropComposite() twice";

  binding_->Close(ZX_ERR_PEER_CLOSED);  // This in turn will trigger on_unbind -> DropChildren().
  binding_.reset();
}

void FakeComposite::DropChildren() {
  ADR_LOG_METHOD(kLogFakeComposite);

  get_health_state_completers_.clear();
  get_properties_completers_.clear();
  get_ring_buffer_formats_completers_.clear();
  create_ring_buffer_completers_.clear();
  get_packet_stream_formats_completers_.clear();
  create_packet_stream_completers_.clear();
  get_dai_formats_completers_.clear();
  reset_completers_.clear();
  set_dai_format_completers_.clear();

  get_elements_completers_.clear();
  watch_element_state_completers_.clear();
  set_element_state_completers_.clear();
  get_topologies_completers_.clear();
  watch_topology_completers_.clear();
  set_topology_completers_.clear();

  unknown_method_completers_.clear();

  DropRingBuffers();
  DropPacketStreams();

  for (auto& element_entry_pair : elements_) {
    if (element_entry_pair.second.watch_completer.has_value()) {
      element_entry_pair.second.watch_completer.reset();
    }
  }
  if (signal_processing_binding_.has_value()) {
    signal_processing_binding_->Close(ZX_ERR_PEER_CLOSED);
    signal_processing_binding_.reset();
  }
}

// From the driver side, drop all RingBuffer protocol connections for this device.
void FakeComposite::DropRingBuffers() {
  ADR_LOG_METHOD(kLogFakeComposite);

  for (auto& binding : ring_buffer_bindings_) {
    binding.second.Unbind();
  }
}

// From the driver side, drop all PacketStream protocol connection for this device.
void FakeComposite::DropPacketStreams() {
  ADR_LOG_METHOD(kLogFakeComposite);

  for (auto& binding : packet_stream_bindings_) {
    binding.second.Unbind();
  }
}

// From the driver side, drop the RingBuffer protocol connection for this element_id.
void FakeComposite::DropRingBuffer(ElementId element_id) {
  ADR_LOG_METHOD(kLogFakeComposite) << "element_id " << element_id;

  for (auto& binding : ring_buffer_bindings_) {
    if (binding.first == element_id) {
      binding.second.Unbind();
      return;
    }
  }
  ADR_WARN_METHOD() << "No ring_buffer binding found for element_id " << element_id;
}

// From the driver side, drop the PacketStream protocol connection for this element_id.
void FakeComposite::DropPacketStream(ElementId element_id) {
  ADR_LOG_METHOD(kLogFakeComposite) << "element_id " << element_id;

  for (auto& binding : packet_stream_bindings_) {
    if (binding.first == element_id) {
      binding.second.Unbind();
      return;
    }
  }
  ADR_WARN_METHOD() << "No packet_stream binding found for element_id " << element_id;
}

// static
void FakeComposite::on_rb_unbind(FakeCompositeRingBuffer* fake_ring_buffer, fidl::UnbindInfo info,
                                 fidl::ServerEnd<fha::RingBuffer>) {
  ADR_LOG(kLogFakeComposite) << "for FakeCompositeRingBuffer";

  fake_ring_buffer->parent()->RingBufferWasDropped(fake_ring_buffer->element_id());
}

void FakeComposite::on_ps_unbind(FakeCompositePacketStream* fake_packet_stream,
                                 fidl::UnbindInfo info, fidl::ServerEnd<fha::PacketStreamControl>) {
  ADR_LOG(kLogFakeComposite) << "for FakeCompositePacketStream";

  fake_packet_stream->parent()->PacketStreamWasDropped(fake_packet_stream->element_id());
}

// The RingBuffer FIDL connection has already been dropped, so there's nothing else for the parent
// driver to do, except clean up our accounting.
void FakeComposite::RingBufferWasDropped(ElementId element_id) {
  ADR_LOG_METHOD(kLogFakeComposite) << "element_id " << element_id;

  ring_buffer_bindings_.erase(element_id);
  ring_buffers_.erase(element_id);
}

// The PacketStream FIDL connection has already been dropped, so there's nothing else for the parent
// driver to do, except clean up our accounting.
void FakeComposite::PacketStreamWasDropped(ElementId element_id) {
  ADR_LOG_METHOD(kLogFakeComposite) << "element_id " << element_id;

  packet_stream_bindings_.erase(element_id);
  packet_streams_.erase(element_id);
}

fidl::ClientEnd<fha::Composite> FakeComposite::Enable() {
  ADR_LOG_METHOD(kLogFakeComposite);
  EXPECT_TRUE(server_end_.is_valid());
  EXPECT_TRUE(client_end_.is_valid());
  EXPECT_TRUE(dispatcher_);
  EXPECT_FALSE(binding_);

  binding_ = fidl::BindServer(dispatcher_, std::move(server_end_), shared_from_this(), &on_unbind);
  EXPECT_FALSE(server_end_.is_valid());

  return std::move(client_end_);
}

void FakeComposite::SetupElementsMap() {
  elements_.insert({
      kSourceDaiElementId,
      FakeElementRecord{
          .element = kSourceDaiElement,
          .state = kSourceDaiElementInitState,
      },
  });
  elements_.insert({
      kSourceRbElementId,
      FakeElementRecord{
          .element = kSourceRbElement,
          .state = kSourceRbElementInitState,
      },
  });
  elements_.insert({
      kSourcePsElementId,
      FakeElementRecord{
          .element = kSourcePsElement,
          .state = kSourcePsElementInitState,
      },
  });
  elements_.insert({
      kSourceDualSupportPsElementId,
      FakeElementRecord{
          .element = kSourceDualSupportPsElement,
          .state = kSourceDualSupportPsElementInitState,
      },
  });
  elements_.insert({
      kDestDaiElementId,
      FakeElementRecord{
          .element = kDestDaiElement,
          .state = kDestDaiElementInitState,
      },
  });
  elements_.insert({
      kDestRbElementId,
      FakeElementRecord{
          .element = kDestRbElement,
          .state = kDestRbElementInitState,
      },
  });
  elements_.insert({
      kDestPsElementId,
      FakeElementRecord{
          .element = kDestPsElement,
          .state = kDestPsElementInitState,
      },
  });
  elements_.insert({
      kVendorSpecificElementId,
      FakeElementRecord{
          .element = kVendorSpecificElement,
          .state = kVendorSpecificElementInitState,
      },
  });
  elements_.insert({
      kDynamicsElementId,
      FakeElementRecord{
          .element = kDynamicsElement,
          .state = kDynamicsElementInitState,
      },
  });
  elements_.insert({
      kEqualizerElementId,
      FakeElementRecord{
          .element = kEqualizerElement,
          .state = kEqualizerElementInitState,
      },
  });
  elements_.insert({
      kGainElementId,
      FakeElementRecord{
          .element = kGainElement,
          .state = kGainElementInitState,
      },
  });
  elements_.insert({
      kMuteElementId,
      FakeElementRecord{
          .element = kMuteElement,
          .state = kMuteElementInitState,
      },
  });

  ASSERT_TRUE(elements_.at(kSourceDaiElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kSourceRbElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kSourcePsElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kSourceDualSupportPsElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kDestDaiElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kDestRbElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kDestPsElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kVendorSpecificElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kDynamicsElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kEqualizerElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kGainElementId).state_has_changed);
  ASSERT_TRUE(elements_.at(kMuteElementId).state_has_changed);

  ASSERT_FALSE(elements_.at(kSourceDaiElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kSourceRbElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kSourcePsElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kSourceDualSupportPsElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kDestDaiElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kDestRbElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kDestPsElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kVendorSpecificElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kDynamicsElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kEqualizerElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kGainElementId).watch_completer.has_value());
  ASSERT_FALSE(elements_.at(kMuteElementId).watch_completer.has_value());
}
void FakeComposite::GetHealthState(GetHealthStateCompleter::Sync& completer) {
  if (!responsive()) {
    get_health_state_completers_.emplace_back(completer.ToAsync());  // Just pend it; never respond.
    return;
  }

  if (healthy_.has_value()) {
    completer.Reply(fha::HealthState{{
        healthy_,
    }});
  } else {
    completer.Reply({});
  }
}

void FakeComposite::SignalProcessingConnect(SignalProcessingConnectRequest& request,
                                            SignalProcessingConnectCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeComposite);

  // If we've been instructed to be unresponsive, do nothing (no need to pend the completer).
  if (!responsive()) {
    return;
  }

  if (!supports_signalprocessing_) {
    request.protocol().Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  FX_CHECK(!signal_processing_binding_.has_value())
      << "SignalProcessing already bound (cannot have multiple clients)";
  signal_processing_binding_ = fidl::BindServer(dispatcher_, std::move(request.protocol()), this);
}

void FakeComposite::Reset(ResetCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeComposite);

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    reset_completers_.emplace_back(completer.ToAsync());
    return;
  }

  // Reset any RingBuffers (start, format)
  DropRingBuffers();

  // Reset any PacketStreams (start, format)
  DropPacketStreams();

  // Reset any DAIs (start, format)

  // Reset all signalprocessing Elements

  // Reset the signalprocessing Topology

  completer.Reply(fit::ok());
}

void FakeComposite::GetProperties(GetPropertiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeComposite);

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_properties_completers_.emplace_back(completer.ToAsync());
    return;
  }

  // Gather the properties and return them.
  fha::CompositeProperties composite_properties{};
  if (manufacturer_.has_value()) {
    composite_properties.manufacturer(manufacturer_);
  }
  if (product_.has_value()) {
    composite_properties.product(product_);
  }
  if (uid_.has_value()) {
    composite_properties.unique_id(uid_);
  }
  if (clock_domain_.has_value()) {
    composite_properties.clock_domain(clock_domain_);
  }

  completer.Reply(composite_properties);
}

void FakeComposite::GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                                         GetRingBufferFormatsCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_ring_buffer_formats_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto element_pair_iter = elements_.find(element_id);
  if (element_pair_iter == elements_.end()) {
    ADR_WARN_METHOD() << "unrecognized element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  if (*element_pair_iter->second.element.type() != fhasp::ElementType::kRingBuffer) {
    ADR_WARN_METHOD() << "wrong type for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kWrongType));
    return;
  }

  auto ring_buffer_format_sets = kDefaultRbFormatsMap.find(element_id);
  if (ring_buffer_format_sets == kDefaultRbFormatsMap.end()) {
    ADR_WARN_METHOD() << "no ring_buffer_format_sets specified for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  completer.Reply(fit::success(ring_buffer_format_sets->second));
}

void FakeComposite::GetPacketStreamFormats(GetPacketStreamFormatsRequest& request,
                                           GetPacketStreamFormatsCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_packet_stream_formats_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto element_pair_iter = elements_.find(element_id);
  if (element_pair_iter == elements_.end()) {
    ADR_WARN_METHOD() << "unrecognized element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  if (*element_pair_iter->second.element.type() != fhasp::ElementType::kPacketStream) {
    ADR_WARN_METHOD() << "wrong type for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kWrongType));
    return;
  }

  auto packet_stream_format_sets = kDefaultPsFormatsMap.find(element_id);
  if (packet_stream_format_sets == kDefaultPsFormatsMap.end()) {
    ADR_WARN_METHOD() << "no packet_stream_format_sets specified for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  completer.Reply(fit::success(packet_stream_format_sets->second));
}

void FakeComposite::ReserveRingBufferSize(ElementId element_id, size_t size) {
  ring_buffer_allocation_sizes_.insert_or_assign(element_id, size);
}

void FakeComposite::EnableActiveChannelsSupport(ElementId element_id) {
  active_channels_support_overrides_.insert_or_assign(element_id, true);
}
void FakeComposite::DisableActiveChannelsSupport(ElementId element_id) {
  active_channels_support_overrides_.insert_or_assign(element_id, false);
}
void FakeComposite::PresetTurnOnDelay(ElementId element_id,
                                      std::optional<zx::duration> turn_on_delay) {
  turn_on_delay_overrides_.insert_or_assign(element_id, turn_on_delay);
}
void FakeComposite::PresetInternalExternalDelays(ElementId element_id, zx::duration internal_delay,
                                                 std::optional<zx::duration> external_delay) {
  internal_delay_overrides_.insert_or_assign(element_id, internal_delay);
  external_delay_overrides_.insert_or_assign(element_id, external_delay);
}

void FakeComposite::CreateRingBuffer(CreateRingBufferRequest& request,
                                     CreateRingBufferCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    create_ring_buffer_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto element_pair_iter = elements_.find(element_id);
  if (element_pair_iter == elements_.end()) {
    ADR_WARN_METHOD() << "unrecognized element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  if (*element_pair_iter->second.element.type() != fhasp::ElementType::kRingBuffer) {
    ADR_WARN_METHOD() << "wrong type for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kWrongType));
    return;
  }
  auto ring_buffer_format_sets_iter = kDefaultRbFormatsMap.find(element_id);
  if (ring_buffer_format_sets_iter == kDefaultRbFormatsMap.end()) {
    ADR_WARN_METHOD() << "no ring_buffer_format_sets specified for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  const auto& ring_buffer_format_sets = ring_buffer_format_sets_iter->second;
  // Make sure the Format is OK
  if (!FormatIsSupported(request.format(), ring_buffer_format_sets)) {
    ADR_WARN_METHOD() << "ring_buffer_format not supported for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kNotSupported));
    return;
  }

  // Make sure the server_end is OK
  if (!request.ring_buffer().is_valid()) {
    ADR_WARN_METHOD() << "ring_buffer server_end is invalid";
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  size_t ring_buffer_allocated_size = kDefaultRingBufferAllocationSize;
  auto match = ring_buffer_allocation_sizes_.find(element_id);
  if (match != ring_buffer_allocation_sizes_.end()) {
    ring_buffer_allocated_size = match->second;
  } else {
    ADR_WARN_METHOD() << "ring buffer allocation size not found";
  }

  if (request.format().Which() != fha::Format2::Tag::kPcmFormat) {
    ADR_WARN_METHOD() << "ring_buffer_format not PCM for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kNotSupported));
    return;
  }

  auto ring_buffer_impl = std::make_unique<FakeCompositeRingBuffer>(
      this, element_id, request.format().pcm_format().value(), ring_buffer_allocated_size);

  auto match_active_channels = active_channels_support_overrides_.find(element_id);
  if (match_active_channels != active_channels_support_overrides_.end()) {
    if (match_active_channels->second) {
      ring_buffer_impl->enable_active_channels_support();
    } else {
      ring_buffer_impl->disable_active_channels_support();
    }
  }

  auto match_turn_on_delay = turn_on_delay_overrides_.find(element_id);
  if (match_turn_on_delay != turn_on_delay_overrides_.end()) {
    match_turn_on_delay->second ? ring_buffer_impl->set_turn_on_delay(*match_turn_on_delay->second)
                                : ring_buffer_impl->clear_turn_on_delay();
  }
  auto match_internal_delay = internal_delay_overrides_.find(element_id);
  if (match_internal_delay != internal_delay_overrides_.end()) {
    ring_buffer_impl->set_internal_delay(match_internal_delay->second);
  }
  auto match_external_delay = external_delay_overrides_.find(element_id);
  if (match_external_delay != external_delay_overrides_.end()) {
    match_external_delay->second
        ? ring_buffer_impl->set_external_delay(*match_external_delay->second)
        : ring_buffer_impl->clear_external_delay();
  }

  ring_buffers_.erase(element_id);
  ring_buffers_.insert({
      element_id,
      std::move(ring_buffer_impl),
  });
  ring_buffer_bindings_.insert_or_assign(
      element_id,
      fidl::BindServer(dispatcher_, std::move(request.ring_buffer()),
                       ring_buffers_.at(element_id).get(), &FakeComposite::on_rb_unbind));

  completer.Reply(fit::ok());
}

void FakeComposite::CompleteCreateRingBuffer(fuchsia_hardware_audio::DriverError error) {
  for (auto& completer : create_ring_buffer_completers_) {
    completer.Reply(fit::error(error));
  }
  create_ring_buffer_completers_.clear();
}

void FakeComposite::CreatePacketStream(CreatePacketStreamRequest& request,
                                       CreatePacketStreamCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    create_packet_stream_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto element_pair_iter = elements_.find(element_id);
  if (element_pair_iter == elements_.end()) {
    ADR_WARN_METHOD() << "unrecognized element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  if (*element_pair_iter->second.element.type() != fhasp::ElementType::kPacketStream) {
    ADR_WARN_METHOD() << "wrong type for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kWrongType));
    return;
  }

  auto packet_stream_format_sets_iter = kDefaultPsFormatsMap.find(element_id);
  if (packet_stream_format_sets_iter == kDefaultPsFormatsMap.end()) {
    ADR_WARN_METHOD() << "no packet_stream_format_sets specified for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }
  const auto& packet_stream_format_sets = packet_stream_format_sets_iter->second;

  if (!FormatIsSupported(request.format(), packet_stream_format_sets)) {
    ADR_WARN_METHOD() << "packet_stream_format not supported for element_id " << element_id;
    completer.Reply(fit::error(fha::DriverError::kNotSupported));
    return;
  }

  if (!request.packet_stream_control().is_valid()) {
    ADR_WARN_METHOD() << "packet_stream_control server_end is invalid";
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  auto buffer_types = fha::BufferType::kClientOwned | fha::BufferType::kDriverOwned;
  if (auto match = inject_packet_stream_buffer_types_.find(element_id);
      match != inject_packet_stream_buffer_types_.end()) {
    buffer_types = match->second;
  }

  auto packet_stream_impl =
      std::make_unique<FakeCompositePacketStream>(this, element_id, request.format(), buffer_types);

  packet_streams_.erase(element_id);
  packet_streams_.insert({
      element_id,
      std::move(packet_stream_impl),
  });
  packet_stream_bindings_.insert_or_assign(
      element_id,
      fidl::BindServer(dispatcher_, std::move(request.packet_stream_control()),
                       packet_streams_.at(element_id).get(), &FakeComposite::on_ps_unbind));

  completer.Reply(fit::ok());
}

void FakeComposite::GetDaiFormats(GetDaiFormatsRequest& request,
                                  GetDaiFormatsCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_dai_formats_completers_.emplace_back(completer.ToAsync());
    return;
  }

  if (element_id < kMinDaiElementId || element_id > kMaxDaiElementId) {
    ADR_WARN_METHOD() << "Element " << element_id << " is out of range";
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  auto dai_format_sets = kDefaultDaiFormatsMap.find(element_id);
  if (dai_format_sets == kDefaultDaiFormatsMap.end()) {
    ADR_WARN_METHOD() << "No DaiFormatSet found for element " << element_id;
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  completer.Reply(fit::success(dai_format_sets->second));
}

// static
bool FakeComposite::DaiFormatIsSupported(ElementId element_id, const fha::DaiFormat& format) {
  auto match = kDefaultDaiFormatsMap.find(element_id);
  if (match == kDefaultDaiFormatsMap.end()) {
    return false;
  }

  if (format.channels_to_use_bitmask() >= (1u << format.number_of_channels())) {
    return false;
  }

  auto dai_format_sets = match->second;
  for (const auto& dai_format_set : dai_format_sets) {
    bool match = false;
    for (auto channel_count : dai_format_set.number_of_channels()) {
      if (channel_count == format.number_of_channels()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (auto sample_format : dai_format_set.sample_formats()) {
      if (sample_format == format.sample_format()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (const auto& frame_format : dai_format_set.frame_formats()) {
      if (frame_format == format.frame_format()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (auto rate : dai_format_set.frame_rates()) {
      if (rate == format.frame_rate()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (auto bits : dai_format_set.bits_per_slot()) {
      if (bits == format.bits_per_slot()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    for (auto bits : dai_format_set.bits_per_sample()) {
      if (bits == format.bits_per_sample()) {
        match = true;
        break;
      }
    }
    if (!match) {
      continue;
    }
    // This DaiFormatSet matched all aspects.
    return true;
  }
  // None of the DaiFormatSets matched all of the aspects.
  return false;
}

void FakeComposite::SetDaiFormat(SetDaiFormatRequest& request,
                                 SetDaiFormatCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeComposite) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    set_dai_format_completers_.emplace_back(completer.ToAsync());
    return;
  }

  if (element_id < kMinDaiElementId || element_id > kMaxDaiElementId) {
    ADR_WARN_METHOD() << "Element " << element_id << " is out of range";
    completer.Reply(fit::error(fha::DriverError::kInvalidArgs));
    return;
  }

  if (!DaiFormatIsSupported(element_id, request.format())) {
    ADR_WARN_METHOD() << "Format is not supported for element " << element_id;
    completer.Reply(fit::error(fha::DriverError::kNotSupported));
    return;
  }

  completer.Reply(fit::ok());
}

// Return our static element list.
void FakeComposite::GetElements(GetElementsCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing);

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_elements_completers_.emplace_back(completer.ToAsync());
    return;
  }

  completer.Reply(fit::success(kElements));
}

// Return our static topology list.
void FakeComposite::GetTopologies(GetTopologiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing);

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_topologies_completers_.emplace_back(completer.ToAsync());
    return;
  }

  completer.Reply(fit::success(kTopologies));
}

void FakeComposite::WatchElementState(WatchElementStateRequest& request,
                                      WatchElementStateCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    watch_element_state_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto match = elements_.find(element_id);
  if (match == elements_.end()) {
    ADR_WARN_METHOD() << "Element ID " << element_id << " not found";
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  FakeElementRecord& element = match->second;

  if (element.watch_completer.has_value()) {
    ADR_WARN_METHOD() << "previous completer was still pending";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  element.watch_completer = completer.ToAsync();

  MaybeCompleteWatchElementState(element);
}

void FakeComposite::SetElementState(SetElementStateRequest& request,
                                    SetElementStateCompleter::Sync& completer) {
  auto element_id = request.processing_element_id();
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "(" << element_id << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    set_element_state_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto match = elements_.find(element_id);
  if (match == elements_.end()) {
    ADR_WARN_METHOD() << "Element ID " << element_id << " not found";
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  FakeElementRecord& element_record = match->second;
  if (ElementStateMatchesSettableElementState(element_record.state, request.state())) {
    ADR_LOG_METHOD(kLogFakeComposite)
        << "element " << element_id << " was already in this state: no change";
  } else {
    element_record.state.started() = request.state().started();
    element_record.state.bypassed() = request.state().bypassed();
    element_record.state.vendor_specific_data() = request.state().vendor_specific_data();
    if (*element_record.element.type() ==
        fuchsia_hardware_audio_signalprocessing::ElementType::kDynamics) {
      element_record.state.type_specific() =
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::WithDynamics(
              request.state().type_specific()->dynamics().value());
    } else if (*element_record.element.type() ==
               fuchsia_hardware_audio_signalprocessing::ElementType::kEqualizer) {
      element_record.state.type_specific() =
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::WithEqualizer(
              request.state().type_specific()->equalizer().value());
    } else if (*element_record.element.type() ==
               fuchsia_hardware_audio_signalprocessing::ElementType::kGain) {
      element_record.state.type_specific() =
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::WithGain(
              request.state().type_specific()->gain().value());
    } else if (*element_record.element.type() ==
               fuchsia_hardware_audio_signalprocessing::ElementType::kVendorSpecific) {
      element_record.state.type_specific() =
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::WithVendorSpecific(
              request.state().type_specific()->vendor_specific().value());
    }
    element_record.state_has_changed = true;

    MaybeCompleteWatchElementState(element_record);
  }

  completer.Reply(fit::ok());
}

void FakeComposite::InjectElementStateChange(ElementId element_id, fhasp::ElementState new_state) {
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "(" << element_id << ")";
  auto match = elements_.find(element_id);
  ASSERT_NE(match, elements_.end());
  auto& element = match->second;

  element.state = std::move(new_state);
  element.state_has_changed = true;

  if (responsive()) {
    MaybeCompleteWatchElementState(element);
  }
}

// static
void FakeComposite::MaybeCompleteWatchElementState(FakeElementRecord& element_record) {
  if (element_record.state_has_changed && element_record.watch_completer.has_value()) {
    auto completer = std::move(*element_record.watch_completer);
    element_record.watch_completer.reset();

    element_record.state_has_changed = false;

    ADR_LOG_STATIC(kLogFakeCompositeSignalProcessing)
        << "About to complete WatchElementState for element_id " << *element_record.element.id();
    completer.Reply(element_record.state);
  } else {
    ADR_LOG_STATIC(kLogFakeCompositeSignalProcessing)
        << "Not completing WatchElementState for element_id " << *element_record.element.id();
  }
}

void FakeComposite::WatchTopology(WatchTopologyCompleter::Sync& completer) {
  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "will not respond";
    watch_topology_completers_.emplace_back(completer.ToAsync());
    return;
  }

  if (!watch_topology_completers_.empty()) {
    ADR_WARN_METHOD() << "previous completer was still pending";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing);

  watch_topology_completers_.emplace_back(completer.ToAsync());

  MaybeCompleteWatchTopology();
}

void FakeComposite::SetTopology(SetTopologyRequest& request,
                                SetTopologyCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "(id: " << request.topology_id() << ")";

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    set_topology_completers_.emplace_back(completer.ToAsync());
    return;
  }

  bool topology_id_is_valid = false;
  for (const auto& topology : kTopologies) {
    if (topology.id() == request.topology_id()) {
      topology_id_is_valid = true;
      break;
    }
  }
  if (!topology_id_is_valid) {
    ADR_WARN_METHOD() << "Topology ID " << request.topology_id() << " not found";
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (topology_id_.has_value() && *topology_id_ == request.topology_id()) {
    ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing)
        << "topology was already set to " << request.topology_id() << ": no change";
  } else {
    topology_id_ = request.topology_id();
    topology_has_changed_ = true;

    MaybeCompleteWatchTopology();
  }
  completer.Reply(fit::ok());
}

// Inject std::nullopt to simulate "no topology", such as at power-up or after Reset().
void FakeComposite::InjectTopologyChange(std::optional<TopologyId> topology_id) {
  ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing)
      << "id " << (topology_id.has_value() ? std::to_string(*topology_id) : "(none)");
  topology_has_changed_ = topology_id.has_value();

  if (topology_has_changed_) {
    topology_id_ = topology_id;

    if (responsive()) {
      MaybeCompleteWatchTopology();
    }
  } else {
    topology_id_.reset();  // A new `SetTopology` call must be made
  }
}

void FakeComposite::MaybeCompleteWatchTopology() {
  if (topology_id_.has_value() && topology_has_changed_ && !watch_topology_completers_.empty()) {
    auto completer = std::move(watch_topology_completers_.front());
    watch_topology_completers_.clear();

    topology_has_changed_ = false;

    ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing)
        << "About to complete WatchTopology with topology_id " << *topology_id_;
    completer.Reply(*topology_id_);
  } else {
    ADR_LOG_METHOD(kLogFakeCompositeSignalProcessing) << "Not completing WatchTopology";
  }
}

void FakeComposite::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio_signalprocessing::SignalProcessing> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "(SignalProcessing) ordinal " << metadata.method_ordinal;
  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    unknown_method_completers_.emplace_back(completer.ToAsync());
    return;
  }
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeComposite::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "(Composite) ordinal " << metadata.method_ordinal;
  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    unknown_method_completers_.emplace_back(completer.ToAsync());
    return;
  }
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

uint64_t FakeComposite::RingBufferActiveChannelsBitmask(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kRingBuffer));
  return ring_buffers_.find(element_id)->second->active_channels_bitmask();
}

zx::time FakeComposite::RingBufferSetActiveChannelsCompletedAt(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kRingBuffer));
  return ring_buffers_.find(element_id)->second->set_active_channels_completed_at();
}

bool FakeComposite::RingBufferStarted(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id,
                           fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer));
  if (auto it = ring_buffers_.find(element_id); it != ring_buffers_.end()) {
    return it->second->started();
  }
  return false;
}

bool FakeComposite::PacketStreamStarted(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id,
                           fuchsia_hardware_audio_signalprocessing::ElementType::kPacketStream));
  if (auto it = packet_streams_.find(element_id); it != packet_streams_.end()) {
    return it->second->started();
  }
  return false;
}

zx::time FakeComposite::RingBufferMonoStartTime(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kRingBuffer));
  return ring_buffers_.find(element_id)->second->mono_start_time();
}

zx::time FakeComposite::PacketStreamMonoStartTime(ElementId element_id) const {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kPacketStream));
  if (auto it = packet_streams_.find(element_id); it != packet_streams_.end()) {
    return it->second->mono_start_time();
  }
  return zx::time::infinite_past();
}

std::optional<zx_rights_t> FakeComposite::PacketStreamVmoRights(ElementId element_id,
                                                                uint64_t vmo_id) const {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kPacketStream));
  if (auto it = packet_streams_.find(element_id); it != packet_streams_.end()) {
    return it->second->vmo_rights(vmo_id);
  }
  return std::nullopt;
}

void FakeComposite::RingBufferInjectDelayUpdate(ElementId element_id,
                                                std::optional<zx::duration> internal_delay,
                                                std::optional<zx::duration> external_delay) {
  FX_CHECK(is_element_type(element_id, fhasp::ElementType::kRingBuffer));
  ring_buffers_.find(element_id)->second->InjectDelayUpdate(internal_delay, external_delay);
}

}  // namespace media_audio

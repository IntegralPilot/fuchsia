// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/inspector.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <memory>
#include <optional>
#include <ostream>
#include <string>

#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace {

// `ElementType` is a fiexible enum, recently containing 13 distinct values. For the five types
// specified below, we define additional parameters in `Element`.
// The `.type_specific` field refers to union `TypeSpecificElement` containing these parameters.
bool TypeRequiresTypeSpecific(fhasp::ElementType type) {
  switch (type) {
    case fhasp::ElementType::kDaiInterconnect:
    case fhasp::ElementType::kDynamics:
    case fhasp::ElementType::kEqualizer:
    case fhasp::ElementType::kGain:
    case fhasp::ElementType::kVendorSpecific:
      return true;
    default:
      return false;
  }
}

// `TypeSpecificElement` is a flexible union, so we use this to future-proof against newer variants
// that we do not yet handle. If a new variant is added, this function will need to be updated.
bool TypeSpecificTagIsRecognized(fhasp::TypeSpecificElement::Tag tag) {
  switch (tag) {
    case fhasp::TypeSpecificElement::Tag::kDaiInterconnect:
    case fhasp::TypeSpecificElement::Tag::kDynamics:
    case fhasp::TypeSpecificElement::Tag::kEqualizer:
    case fhasp::TypeSpecificElement::Tag::kGain:
    case fhasp::TypeSpecificElement::Tag::kVendorSpecific:
      return true;
    default:
      return false;
  }
}

void RecordPcmFormat(inspect::Node& node, fuchsia_audio::SampleType sample_type,
                     uint32_t channel_count, uint32_t frames_per_second) {
  node.RecordUint(kChannelCount, channel_count);
  node.RecordUint(kFramesPerSecond, frames_per_second);

  std::ostringstream stream;
  stream << sample_type;
  node.RecordString(kSampleFormat, stream.str());
}

// Save a boolean to an inspect property _that might already exist_.
void SaveNodeBoolean(inspect::Node& node, std::optional<inspect::BoolProperty>& prop,
                     const std::string& key, std::optional<bool> value) {
  if (!value.has_value()) {
    return;
  }
  if (!prop) {
    prop = node.CreateBool(key, *value);
  } else {
    prop->Set(*value);
  }
}

// Save a uint64 to an inspect property _that might already exist_.
void SaveNodeUint(inspect::Node& node, std::optional<inspect::UintProperty>& prop,
                  const std::string& key, std::optional<uint64_t> value) {
  if (!value.has_value()) {
    return;
  }
  if (!prop) {
    prop = node.CreateUint(key, *value);
  } else {
    prop->Set(*value);
  }
}

// Save a int64 to an inspect property _that might already exist_.
void SaveNodeInt(inspect::Node& node, std::optional<inspect::IntProperty>& prop,
                 const std::string& key, std::optional<int64_t> value) {
  if (!value.has_value()) {
    return;
  }
  if (!prop) {
    prop = node.CreateInt(key, *value);
  } else {
    prop->Set(*value);
  }
}

// Save a double to an inspect property _that might already exist_.
void SaveNodeDouble(inspect::Node& node, std::optional<inspect::DoubleProperty>& prop,
                    const std::string& key, std::optional<double> value) {
  if (!value.has_value()) {
    return;
  }
  if (!prop) {
    prop = node.CreateDouble(key, *value);
  } else {
    prop->Set(*value);
  }
}

// Save a string to an inspect property _that might already exist_.
void SaveNodeString(inspect::Node& node, std::optional<inspect::StringProperty>& prop,
                    const std::string& key, const std::string& value) {
  if (!prop) {
    prop = node.CreateString(key, value);
  } else {
    prop->Set(value);
  }
}

// Save an optional bool to an inspect property _that might already exist_ -- as a string.
bool SaveBooleanToNodeStringProperty(inspect::Node& node,
                                     std::optional<inspect::StringProperty>& prop,
                                     const std::string& key, std::optional<bool> value,
                                     const std::string& default_str) {
  if (value.has_value()) {
    std::string value_str = *value ? "true" : "false";
    SaveNodeString(node, prop, key, value_str);
    return true;
  }
  SaveNodeString(node, prop, key, default_str);
  return false;
}

// Save an optional uint64 to an inspect property _that might already exist_ -- as a string.
bool SaveUintToNodeStringProperty(inspect::Node& node, std::optional<inspect::StringProperty>& prop,
                                  const std::string& key, std::optional<uint64_t> value,
                                  const std::string& default_str) {
  SaveNodeString(node, prop, key, value.has_value() ? std::to_string(*value) : default_str);
  return value.has_value();
}

// Save an optional int64 to an inspect property _that might already exist_ -- as a string.
bool SaveIntToNodeStringProperty(inspect::Node& node, std::optional<inspect::StringProperty>& prop,
                                 const std::string& key, std::optional<int64_t> value,
                                 const std::string& default_str) {
  SaveNodeString(node, prop, key, value.has_value() ? std::to_string(*value) : default_str);
  return value.has_value();
}

// Save an optional float to an inspect property _that might already exist_ -- as a string.
bool SaveFloatToNodeStringProperty(inspect::Node& node,
                                   std::optional<inspect::StringProperty>& prop,
                                   const std::string& key, std::optional<float> value,
                                   const std::string& default_str) {
  SaveNodeString(node, prop, key, value.has_value() ? std::to_string(*value) : default_str);
  return value.has_value();
}

}  // namespace

///////////////////////////////////////
// static members and methods
//
// This singleton handles Inspect for the entire service.
std::unique_ptr<Inspector> Inspector::singleton_ = nullptr;

void Inspector::Initialize(async_dispatcher_t* dispatcher) {
  ADR_LOG_STATIC(kTraceInspector);
  // Should only be called once.
  if (singleton_ == nullptr) {
    singleton_ = std::make_unique<Inspector>(dispatcher);
  } else {
    FX_LOGS(ERROR) << "Inspector::Initialize should only be called once";
  }
}
// end of static members and methods
///////////////////////////////////////

///////////////////////////////////////
// SetActiveChannelsInspectInstance methods
SetActiveChannelsInspectInstance::SetActiveChannelsInspectInstance(
    inspect::Node set_active_channels_node, uint64_t channel_bitmask, const zx::time& called_at,
    const zx::time& completed_at)
    : set_active_channels_node_(std::move(set_active_channels_node)) {
  ADR_LOG_METHOD(kTraceInspector);
  set_active_channels_node_.RecordInt(kCalledAt, called_at.get());
  set_active_channels_node_.RecordInt(kCompletedAt, completed_at.get());
  set_active_channels_node_.RecordUint(kChannelBitmask, channel_bitmask);
}

SetActiveChannelsInspectInstance::~SetActiveChannelsInspectInstance() {
  ADR_LOG_METHOD(kTraceInspector);
}

///////////////////////////////////////
// RunningIntervalInspectInstance methods
RunningIntervalInspectInstance::RunningIntervalInspectInstance(inspect::Node running_interval_node,
                                                               const zx::time& started_at)
    : running_interval_node_(std::move(running_interval_node)) {
  ADR_LOG_METHOD(kTraceInspector);
  running_interval_node_.RecordInt(kStartedAt, started_at.get());
}

RunningIntervalInspectInstance::~RunningIntervalInspectInstance() {
  ADR_LOG_METHOD(kTraceInspector);
}

void RunningIntervalInspectInstance::RecordStopTime(const zx::time& stopped_at) {
  ADR_LOG_METHOD(kTraceInspector) << kStoppedAt << stopped_at.get();
  running_interval_node_.RecordInt(kStoppedAt, stopped_at.get());
}

///////////////////////////////////////
// RingBufferInspectInstance methods
RingBufferInspectInstance::RingBufferInspectInstance(inspect::Node ring_buffer_instance_node,
                                                     const zx::time& created_at)
    : ring_buffer_instance_node_(std::move(ring_buffer_instance_node)) {
  ADR_LOG_METHOD(kTraceInspector);
  ring_buffer_instance_node_.RecordInt(kCreatedAt, created_at.get());
}

RingBufferInspectInstance::~RingBufferInspectInstance() { ADR_LOG_METHOD(kTraceInspector); }

void RingBufferInspectInstance::RecordDestructionTime(const zx::time& destroyed_at) {
  ADR_LOG_METHOD(kTraceInspector) << kDestroyedAt << destroyed_at.get();
  ring_buffer_instance_node_.RecordInt(kDestroyedAt, destroyed_at.get());
}

void RingBufferInspectInstance::RecordStartTime(const zx::time& started_at) {
  ADR_LOG_METHOD(kTraceInspector);
  if (running_intervals_.empty()) {
    running_intervals_root_node_ = ring_buffer_instance_node_.CreateChild(kRunningIntervals);
  }
  auto running_interval_node =
      running_intervals_root_node_.CreateChild(std::to_string(running_intervals_.size()));
  auto running_interval = std::make_shared<RunningIntervalInspectInstance>(
      std::move(running_interval_node), started_at);
  running_intervals_.push_back(running_interval);
}

void RingBufferInspectInstance::RecordStopTime(const zx::time& stopped_at) {
  ADR_LOG_METHOD(kTraceInspector) << kStoppedAt << stopped_at.get();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->RecordStopTime(stopped_at);
  }
}

std::shared_ptr<SetActiveChannelsInspectInstance>
RingBufferInspectInstance::RecordSetActiveChannelsCall(uint64_t channel_bitmask,
                                                       const zx::time& called_at,
                                                       const zx::time& completed_at) {
  ADR_LOG_METHOD(kTraceInspector);
  if (set_active_channels_calls_.empty()) {
    set_active_channels_calls_root_node_ =
        ring_buffer_instance_node_.CreateChild(kSetActiveChannelsCalls);
  }
  auto set_active_channels_instance_node = set_active_channels_calls_root_node_.CreateChild(
      std::to_string(set_active_channels_calls_.size()));
  auto set_active_channels_instance = std::make_shared<SetActiveChannelsInspectInstance>(
      std::move(set_active_channels_instance_node), channel_bitmask, called_at, completed_at);

  set_active_channels_calls_.push_back(set_active_channels_instance);
  return set_active_channels_instance;
}

void RingBufferInspectInstance::RecordBuffer(uint64_t requested_bytes, uint64_t producer_frames,
                                             uint64_t consumer_frames, uint64_t vmo_size) {
  ADR_LOG_METHOD(kTraceInspector);
  buffer_node_ = ring_buffer_instance_node_.CreateChild(kBufferProps);
  buffer_node_.RecordUint(kRequestedBytes, requested_bytes);
  buffer_node_.RecordUint(kProducerFrames, producer_frames);
  buffer_node_.RecordUint(kConsumerFrames, consumer_frames);
  buffer_node_.RecordUint(kVmoBytes, vmo_size);
}

void RingBufferInspectInstance::RecordFormat(uint32_t channel_count, uint32_t frames_per_second,
                                             fuchsia_audio::SampleType sample_type) {
  ADR_LOG_METHOD(kTraceInspector);
  format_node_ = ring_buffer_instance_node_.CreateChild(kFormatProps);
  RecordPcmFormat(format_node_, sample_type, channel_count, frames_per_second);
}

void RecordSupportedPcmFormatSets(
    inspect::Node& header_node, std::vector<SupportedPcmFormatsRecord>& records,
    const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets, std::string_view prefix) {
  for (auto i = 0u; i < format_sets.size(); ++i) {
    records.emplace_back(SupportedPcmFormatsRecord{});
    auto& pcm_format_set = records[i];
    pcm_format_set.pcm_format_set_node =
        header_node.CreateChild(std::string(prefix) + std::to_string(i));
    auto& parent_node = pcm_format_set.pcm_format_set_node;

    const auto& sample_types = *format_sets[i].sample_types();
    pcm_format_set.sample_formats =
        parent_node.CreateStringArray(kSampleFormat, sample_types.size());
    auto& sample_format_arr = pcm_format_set.sample_formats;
    for (auto j = 0u; j < sample_types.size(); ++j) {
      std::ostringstream stream;
      stream << sample_types[j];
      sample_format_arr.Set(j, stream.str());
    }

    const auto& frame_rates = *format_sets[i].frame_rates();
    pcm_format_set.frame_rates = parent_node.CreateUintArray(kFramesPerSecond, frame_rates.size());
    auto& frame_rate_arr = pcm_format_set.frame_rates;
    for (auto j = 0u; j < frame_rates.size(); ++j) {
      frame_rate_arr.Set(j, frame_rates[j]);
    }

    const auto& channel_sets = *format_sets[i].channel_sets();
    pcm_format_set.channel_sets_node = parent_node.CreateChild(kChannelCount);
    auto& channel_sets_node = pcm_format_set.channel_sets_node;
    for (auto j = 0u; j < channel_sets.size(); ++j) {
      const auto& channel_set = channel_sets[j];
      pcm_format_set.channel_sets.emplace_back(ChannelSetRecord{});
      auto& channel_set_record = pcm_format_set.channel_sets[j];
      channel_set_record.channel_set_node =
          channel_sets_node.CreateChild("channel_set_" + std::to_string(j));
      auto& channel_set_node = channel_set_record.channel_set_node;

      for (auto k = 0u; k < channel_set.attributes()->size(); ++k) {
        const auto& channel_attributes = channel_set.attributes()->at(k);
        channel_set_record.channel_nodes.emplace_back(
            channel_set_node.CreateChild("channel_" + std::to_string(k)));
        auto& channel_node = channel_set_record.channel_nodes[k];
        if (channel_attributes.min_frequency().has_value()) {
          channel_node.RecordUint(kMinFrequency, *channel_attributes.min_frequency());
        }
        if (channel_attributes.max_frequency().has_value()) {
          channel_node.RecordUint(kMaxFrequency, *channel_attributes.max_frequency());
        }
      }
    }
  }
}

void RecordSupportedEncodingSets(
    inspect::Node& header_node, std::vector<SupportedEncodingsRecord>& records,
    const std::vector<fuchsia_hardware_audio::SupportedEncodings>& encodings,
    std::string_view prefix) {
  ADR_LOG(kTraceInspector) << encodings.size() << " encoding sets";
  for (auto i = 0u; i < encodings.size(); ++i) {
    records.emplace_back(SupportedEncodingsRecord{});
    auto& encoding_set = records[i];
    encoding_set.encodings_node = header_node.CreateChild(std::string(prefix) + std::to_string(i));
    auto& parent_node = encoding_set.encodings_node;

    if (encodings[i].encoding_types().has_value()) {
      const auto& types = *encodings[i].encoding_types();
      encoding_set.encoding_types = parent_node.CreateStringArray(kEncodingType, types.size());
      for (auto j = 0u; j < types.size(); ++j) {
        std::ostringstream stream;
        stream << types[j];
        encoding_set.encoding_types.Set(j, stream.str());
      }
    }

    if (encodings[i].decoded_frame_rates().has_value()) {
      const auto& frame_rates = *encodings[i].decoded_frame_rates();
      encoding_set.decoded_frame_rates =
          parent_node.CreateUintArray(kFramesPerSecond, frame_rates.size());
      for (auto j = 0u; j < frame_rates.size(); ++j) {
        encoding_set.decoded_frame_rates.Set(j, frame_rates[j]);
      }
    }

    if (encodings[i].min_encoding_bitrate().has_value()) {
      encoding_set.min_encoding_bitrate =
          parent_node.CreateUint(kMinBitrate, *encodings[i].min_encoding_bitrate());
    }
    if (encodings[i].max_encoding_bitrate().has_value()) {
      encoding_set.max_encoding_bitrate =
          parent_node.CreateUint(kMaxBitrate, *encodings[i].max_encoding_bitrate());
    }

    if (encodings[i].decoded_channel_sets().has_value()) {
      const auto& channel_sets = *encodings[i].decoded_channel_sets();
      encoding_set.decoded_channel_sets_node = parent_node.CreateChild(kChannelCount);
      auto& channel_sets_node = encoding_set.decoded_channel_sets_node;
      for (auto j = 0u; j < channel_sets.size(); ++j) {
        const auto& channel_set = channel_sets[j];
        encoding_set.decoded_channel_sets.emplace_back(ChannelSetRecord{});
        auto& channel_set_record = encoding_set.decoded_channel_sets[j];
        channel_set_record.channel_set_node =
            channel_sets_node.CreateChild("channel_set_" + std::to_string(j));
        auto& channel_set_node = channel_set_record.channel_set_node;

        for (auto k = 0u; k < channel_set.attributes()->size(); ++k) {
          const auto& channel_attributes = channel_set.attributes()->at(k);
          channel_set_record.channel_nodes.emplace_back(
              channel_set_node.CreateChild("channel_" + std::to_string(k)));
          auto& channel_node = channel_set_record.channel_nodes[k];
          if (channel_attributes.min_frequency().has_value()) {
            channel_node.RecordUint(kMinFrequency, *channel_attributes.min_frequency());
          }
          if (channel_attributes.max_frequency().has_value()) {
            channel_node.RecordUint(kMaxFrequency, *channel_attributes.max_frequency());
          }
        }
      }
    }
  }
}

///////////////////////////////////////
// PacketStreamInspectInstance methods
PacketStreamInspectInstance::PacketStreamInspectInstance(inspect::Node packet_stream_instance_node,
                                                         const zx::time& created_at)
    : packet_stream_instance_node_(std::move(packet_stream_instance_node)) {
  ADR_LOG_METHOD(kTraceInspector);
  packet_stream_instance_node_.RecordInt(kCreatedAt, created_at.get());
}

PacketStreamInspectInstance::~PacketStreamInspectInstance() { ADR_LOG_METHOD(kTraceInspector); }

void PacketStreamInspectInstance::RecordDestructionTime(const zx::time& destroyed_at) {
  ADR_LOG_METHOD(kTraceInspector) << kDestroyedAt << destroyed_at.get();
  packet_stream_instance_node_.RecordInt(kDestroyedAt, destroyed_at.get());
}

void PacketStreamInspectInstance::RecordStartTime(const zx::time& started_at) {
  ADR_LOG_METHOD(kTraceInspector);
  if (running_intervals_.empty()) {
    running_intervals_root_node_ = packet_stream_instance_node_.CreateChild(kRunningIntervals);
  }
  auto running_interval_node =
      running_intervals_root_node_.CreateChild(std::to_string(running_intervals_.size()));
  auto running_interval = std::make_shared<RunningIntervalInspectInstance>(
      std::move(running_interval_node), started_at);
  running_intervals_.push_back(running_interval);
}

void PacketStreamInspectInstance::RecordStopTime(const zx::time& stopped_at) {
  ADR_LOG_METHOD(kTraceInspector) << kStoppedAt << stopped_at.get();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->RecordStopTime(stopped_at);
  }
}

void PacketStreamInspectInstance::RecordBuffer(
    fuchsia_hardware_audio::BufferType buffer_type,
    const std::vector<fuchsia_hardware_audio::VmoInfo>& vmo_infos) {
  ADR_LOG_METHOD(kTraceInspector);
  buffer_node_ = packet_stream_instance_node_.CreateChild(kBufferProps);

  std::ostringstream stream;
  if (buffer_type & fha::BufferType::kClientOwned) {
    stream << "|CLIENT_OWNED";
  }
  if (buffer_type & fha::BufferType::kDriverOwned) {
    stream << "|DRIVER_OWNED";
  }
  if (buffer_type & fha::BufferType::kInline) {
    stream << "|INLINE";
  }
  auto type_str = stream.str();
  buffer_node_.RecordString(kBufferType, type_str.empty() ? kNone : type_str.substr(1));

  if (!vmo_infos.empty()) {
    vmo_infos_node_ = buffer_node_.CreateChild(kVmoInfos);
    FX_DCHECK(vmo_nodes_.empty());
    uint64_t total_vmo_size = 0;
    for (auto i = 0u; i < vmo_infos.size(); ++i) {
      auto vmo_node = vmo_infos_node_.CreateChild(std::to_string(i));
      vmo_node.RecordUint(kVmoId, *vmo_infos[i].id());
      size_t vmo_size = 0;
      if (vmo_infos[i].vmo()->is_valid()) {
        vmo_infos[i].vmo()->get_size(&vmo_size);
      }
      vmo_node.RecordUint(kVmoBytes, vmo_size);
      total_vmo_size += vmo_size;
      vmo_nodes_.push_back(std::move(vmo_node));
    }
    buffer_node_.RecordUint(kVmoBytes, total_vmo_size);
  }
}

void PacketStreamInspectInstance::RecordFormat(
    const fuchsia_audio_device::PacketStreamFormat& format) {
  ADR_LOG_METHOD(kTraceInspector);
  format_node_ = packet_stream_instance_node_.CreateChild(kFormatProps);

  if (format.pcm_format()) {
    const auto& pcm = format.pcm_format().value();
    RecordPcmFormat(format_node_, pcm.sample_type().value_or(fuchsia_audio::SampleType::Unknown()),
                    pcm.channel_count().value_or(0), pcm.frames_per_second().value_or(0));
  } else if (format.encoding()) {
    const auto& enc = format.encoding().value();
    if (enc.decoded_channel_count()) {
      format_node_.RecordUint(kChannelCount, *enc.decoded_channel_count());
    }
    if (enc.decoded_frame_rate()) {
      format_node_.RecordUint(kFramesPerSecond, *enc.decoded_frame_rate());
    }

    if (enc.encoding_type()) {
      std::ostringstream stream;
      stream << *enc.encoding_type();
      format_node_.RecordString(kEncodingType, stream.str());
    }
  } else {
    format_node_.RecordUint("unknown_union_tag", static_cast<uint32_t>(format.Which()));
  }
}

///////////////////////////////////////
// IoNode methods
IoNode::IoNode(inspect::Node node, ElementId element_id,
               const std::optional<std::string>& element_name)
    : node_(std::move(node)), element_id_(element_id) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  node_.RecordUint(kElementId, element_id);
  if (element_name.has_value()) {
    node_.RecordString(kDescription, *element_name);
  }
}

IoNode::~IoNode() { ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_; }

///////////////////////////////////////
// RingBuffer methods
RingBuffer::RingBuffer(inspect::Node ring_buffer_node, ElementId element_id,
                       const std::optional<std::string>& element_name)
    : IoNode(std::move(ring_buffer_node), element_id, element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id << ", '" << element_name.value_or("")
                                  << "'";
  // Consider recording an 'is_input' bool, indicating dataflow direction (derived from Topology?).
}

RingBuffer::~RingBuffer() { ADR_LOG_METHOD(kTraceInspector) << "element " << element_id(); }

void RingBuffer::RecordSupportedFormatSets(
    const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id();

  format_sets_header_node() = inspect_node().CreateChild(kSupportedFormats);
  RecordSupportedPcmFormatSets(format_sets_header_node(), supported_pcm_formats_sets_, format_sets,
                               "rb_format_set_");
}

std::shared_ptr<RingBufferInspectInstance> RingBuffer::RecordRingBufferInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id() << ", instance "
                                  << ring_buffer_instances_.size();

  auto ring_buffer_instance_node = inspect_node().CreateChild(
      std::string("instance_") + std::to_string(ring_buffer_instances_.size()));
  auto ring_buffer_instance =
      std::make_shared<RingBufferInspectInstance>(std::move(ring_buffer_instance_node), created_at);
  ADR_LOG_METHOD(kTraceInspector) << "returning " << ring_buffer_instance;

  ring_buffer_instances_.push_back(ring_buffer_instance);
  return ring_buffer_instance;
}

///////////////////////////////////////
// PacketStream methods
PacketStream::PacketStream(inspect::Node packet_stream_node, ElementId element_id,
                           const std::optional<std::string>& element_name)
    : IoNode(std::move(packet_stream_node), element_id, element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id << ", '" << element_name.value_or("")
                                  << "'";
}

PacketStream::~PacketStream() { ADR_LOG_METHOD(kTraceInspector) << "element " << element_id(); }

void PacketStream::RecordSupportedFormatSets(
    const std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>& format_sets) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id();

  format_sets_header_node() = inspect_node().CreateChild(kSupportedFormats);

  std::vector<fuchsia_audio_device::PcmFormatSet> pcm_format_sets;
  std::vector<fuchsia_hardware_audio::SupportedEncodings> encoding_sets;

  for (const auto& format_set : format_sets) {
    if (format_set.pcm_format().has_value()) {
      pcm_format_sets.push_back(format_set.pcm_format().value());
    }
    if (format_set.supported_encodings().has_value()) {
      encoding_sets.push_back(format_set.supported_encodings().value());
    }
  }

  RecordSupportedPcmFormatSets(format_sets_header_node(), supported_pcm_formats_sets_,
                               pcm_format_sets, "ps_pcm_format_set_");
  RecordSupportedEncodingSets(format_sets_header_node(), supported_encodings_, encoding_sets,
                              "ps_encoding_set_");
}

std::shared_ptr<PacketStreamInspectInstance> PacketStream::RecordPacketStreamInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id() << ", instance "
                                  << packet_stream_instances_.size();

  auto packet_stream_instance_node = inspect_node().CreateChild(
      std::string("instance_") + std::to_string(packet_stream_instances_.size()));
  auto packet_stream_instance = std::make_shared<PacketStreamInspectInstance>(
      std::move(packet_stream_instance_node), created_at);
  ADR_LOG_METHOD(kTraceInspector) << "returning " << packet_stream_instance;

  packet_stream_instances_.push_back(packet_stream_instance);
  return packet_stream_instance;
}

///////////////////////////////////////
// Dai methods
Dai::Dai(inspect::Node dai_node, ElementId element_id,
         const std::optional<std::string>& element_name)
    : IoNode(std::move(dai_node), element_id, element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id;
}

Dai::~Dai() { ADR_LOG_METHOD(kTraceInspector) << "element " << element_id(); }

void Dai::RecordSupportedFormatSets(
    const std::vector<fuchsia_hardware_audio::DaiSupportedFormats>& format_sets) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id();

  format_sets_header_node() = inspect_node().CreateChild(kSupportedFormats);
  dai_format_sets_.clear();
  for (auto i = 0u; i < format_sets.size(); ++i) {
    dai_format_sets_.emplace_back(DaiFormatSetRecord{});
    auto& dai_format_set = dai_format_sets_[i];
    dai_format_set.dai_format_set_node =
        format_sets_header_node().CreateChild("dai_format_set_" + std::to_string(i));

    const auto& channel_counts = format_sets[i].number_of_channels();
    dai_format_set.dai_format_set_channel_counts =
        dai_format_set.dai_format_set_node.CreateUintArray(kChannelCount, channel_counts.size());
    for (auto j = 0u; j < channel_counts.size(); ++j) {
      dai_format_set.dai_format_set_channel_counts.Set(j, channel_counts[j]);
    }

    const auto& sample_formats = format_sets[i].sample_formats();
    dai_format_set.dai_format_set_sample_formats =
        dai_format_set.dai_format_set_node.CreateStringArray(kSampleFormat, sample_formats.size());
    for (auto j = 0u; j < sample_formats.size(); ++j) {
      std::ostringstream stream;
      stream << sample_formats[j];
      dai_format_set.dai_format_set_sample_formats.Set(j, stream.str());
    }

    const auto& frame_formats = format_sets[i].frame_formats();
    dai_format_set.dai_format_set_frame_formats =
        dai_format_set.dai_format_set_node.CreateStringArray(kFrameFormat, frame_formats.size());
    for (auto j = 0u; j < frame_formats.size(); ++j) {
      std::ostringstream stream;
      stream << frame_formats[j];
      dai_format_set.dai_format_set_frame_formats.Set(j, stream.str());
    }

    const auto& frame_rates = format_sets[i].frame_rates();
    dai_format_set.dai_format_set_frame_rates =
        dai_format_set.dai_format_set_node.CreateUintArray(kFramesPerSecond, frame_rates.size());
    for (auto j = 0u; j < frame_rates.size(); ++j) {
      dai_format_set.dai_format_set_frame_rates.Set(j, frame_rates[j]);
    }

    const auto& frame_sizes = format_sets[i].bits_per_slot();
    dai_format_set.dai_format_set_frame_sizes =
        dai_format_set.dai_format_set_node.CreateUintArray(kBitsPerFrame, frame_sizes.size());
    for (auto j = 0u; j < frame_sizes.size(); ++j) {
      dai_format_set.dai_format_set_frame_sizes.Set(j, frame_sizes[j]);
    }

    const auto& sample_sizes = format_sets[i].bits_per_sample();
    dai_format_set.dai_format_set_sample_sizes =
        dai_format_set.dai_format_set_node.CreateUintArray(kBitsPerSample, sample_sizes.size());
    for (auto j = 0u; j < sample_sizes.size(); ++j) {
      dai_format_set.dai_format_set_sample_sizes.Set(j, sample_sizes[j]);
    }
  }
}

void Dai::RecordSetDaiFormat(const zx::time& set_at,
                             const fuchsia_hardware_audio::DaiFormat& dai_format) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id();
  format_node() = inspect_node().CreateChild(kFormatProps);

  format_node().RecordUint(kBitsPerFrame, dai_format.bits_per_slot());
  format_node().RecordUint(kBitsPerSample, dai_format.bits_per_sample());
  format_node().RecordUint(kChannelCount, dai_format.number_of_channels());
  format_node().RecordUint(kChannelBitmask, dai_format.channels_to_use_bitmask());
  format_node().RecordUint(kFramesPerSecond, dai_format.frame_rate());

  std::ostringstream format_stream;
  format_stream << dai_format.frame_format();
  format_node().RecordString(kFrameFormat, format_stream.str());

  format_stream.str("");
  format_stream.clear();
  format_stream << dai_format.sample_format();
  format_node().RecordString(kSampleFormat, format_stream.str());
}

///////////////////////////////////////
// Element methods
Element::Element(inspect::Node element_node, ElementId element_id, const fhasp::Element& element)
    : element_node_(std::move(element_node)), element_id_(element_id) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  element_node_.RecordUint(kElementId, element_id);
  props_node_ = element_node_.CreateChild(kProperties);

  if (element.type().has_value()) {
    element_type_ = *element.type();

    std::ostringstream stream;
    stream << *element.type();
    props_node_.RecordString(kType, stream.str());
  } else {
    props_node_.RecordString(kType, kNoneNonCompliant);
    ADR_WARN_METHOD() << "No element_type for element " << element_id_;
  }

  if (element.description().has_value()) {
    props_node_.RecordString(kDescription, *element.description());
  }

  if (element.can_stop().has_value()) {
    props_node_.RecordString(kCanStop, *element.can_stop() ? "true" : "false");
  } else {
    props_node_.RecordString(kCanStop, kNone + " (cannot be stopped)");
  }

  if (element.can_bypass().has_value()) {
    if (*element.can_bypass() && (*element.type() == fhasp::ElementType::kDaiInterconnect ||
                                  *element.type() == fhasp::ElementType::kPacketStream ||
                                  *element.type() == fhasp::ElementType::kRingBuffer)) {
      props_node_.RecordString(kCanBypass, "true" + kNonCompliant);
    } else {
      props_node_.RecordString(kCanBypass, *element.can_bypass() ? "true" : "false");
    }
  } else {
    props_node_.RecordString(kCanBypass, kNone + " (cannot be bypassed)");
  }
  RecordTypeSpecificElement(*element.type(), element.type_specific());
}

Element::~Element() { ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_; }

void Element::SaveBoolean(std::optional<inspect::BoolProperty>& prop, const std::string& key,
                          std::optional<bool> value) {
  SaveNodeBoolean(state_node_, prop, key, value);
}

void Element::SaveUint(std::optional<inspect::UintProperty>& prop, const std::string& key,
                       std::optional<uint64_t> value) {
  SaveNodeUint(state_node_, prop, key, value);
}

void Element::SaveInt(std::optional<inspect::IntProperty>& prop, const std::string& key,
                      std::optional<int64_t> value) {
  SaveNodeInt(state_node_, prop, key, value);
}

void Element::SaveString(std::optional<inspect::StringProperty>& prop, const std::string& key,
                         const std::string& value) {
  SaveNodeString(state_node_, prop, key, value);
}

// Helper to record an optional bool as a string ("true", "false", or default_val).
bool Element::SaveBooleanToStringProperty(std::optional<inspect::StringProperty>& prop,
                                          const std::string& key, std::optional<bool> value,
                                          const std::string& default_str) {
  return SaveBooleanToNodeStringProperty(state_node_, prop, key, value, default_str);
}

// Helper to record an optional int64 as a string or default_val.
bool Element::SaveIntToStringProperty(std::optional<inspect::StringProperty>& prop,
                                      const std::string& key, std::optional<int64_t> value,
                                      const std::string& default_str) {
  return SaveIntToNodeStringProperty(state_node_, prop, key, value, default_str);
}

void Element::RecordElementState(
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  if (!state_node_) {
    state_node_ = element_node_.CreateChild(kState);
  }

  if (!SaveBooleanToStringProperty(started_prop_, std::string(kStarted), element_state.started(),
                                   kNoneNonCompliant)) {
    ADR_WARN_METHOD() << "No element_state.started for element " << element_id_;
  }
  SaveBooleanToStringProperty(bypassed_prop_, std::string(kBypassed), element_state.bypassed(),
                              kNone + " (not bypassed)");

  SaveIntToStringProperty(turn_on_delay_prop_, std::string(kTurnOnDelay),
                          element_state.turn_on_delay(), kNone);
  SaveIntToStringProperty(turn_off_delay_prop_, std::string(kTurnOffDelay),
                          element_state.turn_off_delay(), kNone);
  SaveIntToStringProperty(processing_delay_prop_, std::string(kProcessingDelay),
                          element_state.processing_delay(), kNone);

  if (element_state.vendor_specific_data().has_value()) {
    std::string value_str =
        "uint8[" + std::to_string(element_state.vendor_specific_data()->size()) + "]";
    SaveString(vendor_specific_data_prop_, std::string(kVendorSpecificData), value_str);
    ADR_LOG_METHOD(kTraceInspector) << "Received vendor_specific_data " << value_str;
  } else {
    if (element_type_ == fhasp::ElementType::kVendorSpecific) {
      SaveString(vendor_specific_data_prop_, std::string(kVendorSpecificData), kNoneNonCompliant);
      ADR_WARN_METHOD() << "No element_state.vendor_specific_data for element " << element_id_;
    }
  }

  RecordTypeSpecificElementState(element_state.type_specific());
}

void Element::RecordTypeSpecificElement(
    fhasp::ElementType type, const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  if (!type_specific.has_value()) {
    if (TypeRequiresTypeSpecific(type)) {
      props_node_.RecordString(kTypeSpecific, kNoneNonCompliant);
    }
    return;
  }
  if (!TypeSpecificTagIsRecognized(type_specific->Which())) {
    props_node_.RecordString(kTypeSpecific, "Unknown TypeSpecific variant");
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  type_specific_node_ = props_node_.CreateChild(kTypeSpecific);
  switch (type_specific->Which()) {
    case fhasp::TypeSpecificElement::Tag::kDaiInterconnect: {
      RecordDaiInterconnectElement(type, *type_specific);
      break;
    }
    case fhasp::TypeSpecificElement::Tag::kDynamics: {
      RecordDynamicsElement(type, *type_specific);
      break;
    }

    case fhasp::TypeSpecificElement::Tag::kEqualizer: {
      RecordEqualizerElement(type, *type_specific);
      break;
    }

    case fhasp::TypeSpecificElement::Tag::kGain: {
      RecordGainElement(type, *type_specific);
      break;
    }

    case fhasp::TypeSpecificElement::Tag::kVendorSpecific: {
      RecordVendorSpecificElement(type, *type_specific);
      break;
    }

    default:
      __UNREACHABLE;
  }
}

void Element::RecordTypeSpecificElementState(
    const std::optional<fhasp::TypeSpecificElementState>& type_specific_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  if (!type_specific_state.has_value()) {
    type_specific_state_node_.reset();
    if (TypeRequiresTypeSpecific(*element_type_)) {
      state_node_.RecordString(kTypeSpecific, kNoneNonCompliant);
    }
    return;
  }

  if (!type_specific_state_node_.has_value()) {
    type_specific_state_node_ = state_node_.CreateChild(kTypeSpecific);
  }
  // Check that the type_specific element's type matches the element's type.
  switch (type_specific_state->Which()) {
    case fhasp::TypeSpecificElementState::Tag::kDaiInterconnect: {
      RecordDaiInterconnectElementState(type_specific_state->dai_interconnect().value());
      break;
    }

    case fhasp::TypeSpecificElementState::Tag::kDynamics: {
      RecordDynamicsElementState(type_specific_state->dynamics().value());
      break;
    }

    case fhasp::TypeSpecificElementState::Tag::kEqualizer: {
      RecordEqualizerElementState(type_specific_state->equalizer().value());
      break;
    }

    case fhasp::TypeSpecificElementState::Tag::kGain: {
      RecordGainElementState(type_specific_state->gain().value());
      break;
    }

    case fhasp::TypeSpecificElementState::Tag::kVendorSpecific: {
      RecordVendorSpecificElementState(type_specific_state->vendor_specific().value());
      break;
    }

    default: {
      type_specific_state_node_.reset();
      state_node_.RecordString(kTypeSpecific, "Unknown TypeSpecific variant");
      return;
    }
  }
}

void Element::RecordDaiInterconnectElement(
    fhasp::ElementType type, const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  std::ostringstream stream;
  if (type != fhasp::ElementType::kDaiInterconnect) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << type
                      << " with TypeSpecific::DaiInterconnect";
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  stream << type_specific->dai_interconnect()->plug_detect_capabilities();
  type_specific_node_->RecordString(kPlugDetectCapabilities, stream.str());
}

void Element::RecordDaiInterconnectElementState(
    const fuchsia_hardware_audio_signalprocessing::DaiInterconnectElementState&
        dai_interconnect_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  if (*element_type_ != fhasp::ElementType::kDaiInterconnect) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << *element_type_
                      << " with TypeSpecific::kDaiInterconnectState";
    return;
  }

  if (dai_interconnect_state.plug_state().has_value()) {
    if (!plug_state_node_) {
      plug_state_node_ = type_specific_state_node_->CreateChild(kPlugState);
    }

    if (dai_interconnect_state.plug_state()->plugged().has_value()) {
      std::string plugged_str = *dai_interconnect_state.plug_state()->plugged()
                                    ? std::string(kPluggedInStr)
                                    : std::string(kUnpluggedStr);
      SaveNodeString(*plug_state_node_, plug_state_prop_, std::string(kPlugged), plugged_str);
    } else {
      SaveNodeString(*plug_state_node_, plug_state_prop_, std::string(kPlugged),
                     kNoneNonCompliant + " (plugged-in)");
    }

    SaveUintToNodeStringProperty(
        *plug_state_node_, plug_state_time_prop_, std::string(kPlugStateTime),
        dai_interconnect_state.plug_state()->plug_state_time(), kNoneNonCompliant);
  } else {
    plug_state_node_.reset();
    plug_state_prop_ = {};
    plug_state_time_prop_ = {};
  }

  SaveIntToNodeStringProperty(*type_specific_state_node_, external_delay_prop_,
                              std::string(kExternalDelay), dai_interconnect_state.external_delay(),
                              kNone);
}

void Element::RecordDynamicsElement(
    fhasp::ElementType type, const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  std::ostringstream stream;
  if (type != fhasp::ElementType::kDynamics) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << type
                      << " with TypeSpecific::Dynamics";
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  const auto& bands = type_specific->dynamics()->bands();
  if (bands.has_value() && !bands->empty()) {
    bands_arr_ = type_specific_node_->CreateUintArray(kBands, bands->size());
    dynamics_band_state_props_.resize(bands->size());
    for (size_t i = 0; i < bands->size(); ++i) {
      if (bands->at(i).id().has_value()) {
        bands_arr_->Set(i, *bands->at(i).id());
      } else {
        ADR_WARN_METHOD() << "No band_id for Dynamics element " << element_id_ << ", band[" << i
                          << "]";
      }
    }
  } else {
    bands_arr_ = type_specific_node_->CreateUintArray(kBands, 0);
    ADR_WARN_METHOD() << "No bands for Dynamics element " << element_id_;
  }

  if (type_specific->dynamics()->supported_controls().has_value()) {
    stream << *type_specific->dynamics()->supported_controls();
    type_specific_node_->RecordString(kSupportedControls, stream.str());
  }
}

void Element::RecordDynamicsElementState(
    const fuchsia_hardware_audio_signalprocessing::DynamicsElementState& dynamics_element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  if (*element_type_ != fhasp::ElementType::kDynamics) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << *element_type_
                      << " with TypeSpecific::kDynamicsState";
    return;
  }

  if (!dynamics_element_state.band_states().has_value() ||
      dynamics_element_state.band_states()->empty()) {
    type_specific_state_node_->RecordString(kBandStates, kNoneNonCompliant);
    return;
  }

  if (!dyn_band_states_node_) {
    dyn_band_states_node_ = type_specific_state_node_->CreateChild(kBandStates);
  }

  const auto& band_states = *dynamics_element_state.band_states();
  if (band_states.size() != dynamics_band_state_props_.size()) {
    ADR_WARN_METHOD() << "Band states size mismatch: " << band_states.size() << " vs "
                      << dynamics_band_state_props_.size();
    return;
  }

  for (size_t i = 0; i < band_states.size(); ++i) {
    const auto& band_state = band_states.at(i);
    auto& props = dynamics_band_state_props_.at(i);

    if (!props.band_node) {
      props.band_node = dyn_band_states_node_->CreateChild(std::to_string(i));
    }
    auto& node = *props.band_node;

    // Band ID
    if (!SaveUintToNodeStringProperty(node, props.band_id, std::string(kBandId), band_state.id(),
                                      kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No band id for element " << element_id_ << ", band[" << i << "]";
    }

    // Min Frequency
    if (!SaveUintToNodeStringProperty(node, props.min_frequency, std::string(kMinFrequency),
                                      band_state.min_frequency(), kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No min_frequency for element " << element_id_ << ", band[" << i << "]";
    }
    // Max Frequency
    if (!SaveUintToNodeStringProperty(node, props.max_frequency, std::string(kMaxFrequency),
                                      band_state.max_frequency(), kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No max_frequency for element " << element_id_ << ", band[" << i << "]";
    }

    // Threshold
    if (!SaveFloatToNodeStringProperty(node, props.threshold_db, std::string(kThresholdDb),
                                       band_state.threshold_db(), kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No threshold_db for element " << element_id_ << ", band[" << i << "]";
    }

    // Threshold Type
    if (band_state.threshold_type().has_value()) {
      std::ostringstream stream;
      stream << *band_state.threshold_type();
      SaveNodeString(node, props.threshold_type, std::string(kThresholdType), stream.str());
    } else {
      SaveNodeString(node, props.threshold_type, std::string(kThresholdType), kNoneNonCompliant);
      ADR_WARN_METHOD() << "No threshold_type for element " << element_id_ << ", band[" << i << "]";
    }

    // Ratio
    if (!SaveFloatToNodeStringProperty(node, props.ratio, std::string(kRatio), band_state.ratio(),
                                       kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No ratio for element " << element_id_ << ", band[" << i << "]";
    }

    // Knee Width
    SaveNodeDouble(node, props.knee_width_db, std::string(kKneeWidthDb),
                   band_state.knee_width_db());

    // Attack
    SaveNodeInt(node, props.attack_ns, std::string(kAttackNs), band_state.attack());

    // Release
    SaveNodeInt(node, props.release_ns, std::string(kReleaseNs), band_state.release());

    // Output Gain
    SaveNodeDouble(node, props.output_gain_db, std::string(kOutputGainDb),
                   band_state.output_gain_db());

    // Input Gain
    SaveNodeDouble(node, props.input_gain_db, std::string(kInputGainDb),
                   band_state.input_gain_db());

    // Level Type
    if (band_state.level_type().has_value()) {
      std::ostringstream stream;
      stream << *band_state.level_type();
      std::string level_type_str = stream.str();
      SaveNodeString(node, props.level_type, std::string(kLevelType), level_type_str);
    }

    // Lookahead
    SaveNodeInt(node, props.lookahead_ns, std::string(kLookaheadNs), band_state.lookahead());

    // Linked Channels
    SaveNodeBoolean(node, props.linked_channels, std::string(kLinkedChannels),
                    band_state.linked_channels());
  }
}

void Element::RecordEqualizerElement(
    fhasp::ElementType type, const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  std::ostringstream stream;
  if (type != fhasp::ElementType::kEqualizer) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << type
                      << " with TypeSpecific::Equalizer";
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  const auto& bands = type_specific->equalizer()->bands();
  if (bands.has_value() && !bands->empty()) {
    bands_arr_ = type_specific_node_->CreateUintArray(kBands, bands->size());
    equalizer_band_state_props_.resize(bands->size());
    for (size_t i = 0; i < bands->size(); ++i) {
      if (bands->at(i).id().has_value()) {
        bands_arr_->Set(i, *bands->at(i).id());
      } else {
        ADR_WARN_METHOD() << "No band_id for Equalizer element " << element_id_ << ", band[" << i
                          << "]";
      }
    }
  } else {
    bands_arr_ = type_specific_node_->CreateUintArray(kBands, 0);
    ADR_WARN_METHOD() << "No bands for Equalizer element " << element_id_;
  }

  bool uses_min_max_gain_db = false;
  if (type_specific->equalizer()->supported_controls().has_value()) {
    stream << *type_specific->equalizer()->supported_controls();
    type_specific_node_->RecordString(kSupportedControls, stream.str());

    // We need this later. Calculate this here while we are looking at supported_controls.
    uses_min_max_gain_db = *type_specific->equalizer()->supported_controls() &
                               fhasp::EqualizerSupportedControls::kSupportsTypePeak ||
                           *type_specific->equalizer()->supported_controls() &
                               fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf ||
                           *type_specific->equalizer()->supported_controls() &
                               fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf;
  }
  if (type_specific->equalizer()->can_disable_bands().has_value()) {
    type_specific_node_->RecordBool(kCanDisableBands,
                                    *type_specific->equalizer()->can_disable_bands());
  }
  if (type_specific->equalizer()->min_frequency().has_value()) {
    type_specific_node_->RecordString(kMinFrequency,
                                      std::to_string(*type_specific->equalizer()->min_frequency()));
  } else {
    type_specific_node_->RecordString(kMinFrequency, kNoneNonCompliant);
    ADR_WARN_METHOD() << "No min_frequency for Equalizer element " << element_id_;
  }
  if (type_specific->equalizer()->max_frequency().has_value()) {
    type_specific_node_->RecordString(kMaxFrequency,
                                      std::to_string(*type_specific->equalizer()->max_frequency()));
  } else {
    type_specific_node_->RecordString(kMaxFrequency, kNoneNonCompliant);
    ADR_WARN_METHOD() << "No max_frequency for Equalizer element " << element_id_;
  }
  if (type_specific->equalizer()->max_q().has_value()) {
    type_specific_node_->RecordDouble(kMaxQ, *type_specific->equalizer()->max_q());
  }
  if (type_specific->equalizer()->min_gain_db().has_value()) {
    type_specific_node_->RecordDouble(kMinGainDb, *type_specific->equalizer()->min_gain_db());
    if (!uses_min_max_gain_db) {
      ADR_WARN_METHOD()
          << "No supported_control requires min_gain_db; it should be omitted (element "
          << element_id_ << ")";
    }
  } else {
    if (uses_min_max_gain_db) {
      ADR_WARN_METHOD() << "min_gain_db was omitted, but a supported_control requires it (element "
                        << element_id_ << ")";
    }
  }
  if (type_specific->equalizer()->max_gain_db().has_value()) {
    type_specific_node_->RecordDouble(kMaxGainDb, *type_specific->equalizer()->max_gain_db());
    if (!uses_min_max_gain_db) {
      ADR_WARN_METHOD()
          << "No supported_control requires max_gain_db; it should be omitted (element "
          << element_id_ << ")";
    }
  } else {
    if (uses_min_max_gain_db) {
      ADR_WARN_METHOD() << "max_gain_db was omitted, but a supported_control requires it (element "
                        << element_id_ << ")";
    }
  }
}

void Element::RecordEqualizerElementState(
    const fuchsia_hardware_audio_signalprocessing::EqualizerElementState& equalizer_element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  if (*element_type_ != fhasp::ElementType::kEqualizer) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << *element_type_
                      << " with TypeSpecific::kEqualizerState";
    return;
  }

  if (!equalizer_element_state.band_states().has_value() ||
      equalizer_element_state.band_states()->empty()) {
    type_specific_state_node_->RecordString(kBandStates, kNoneNonCompliant);
    return;
  }

  if (!eq_band_states_node_) {
    eq_band_states_node_ = type_specific_state_node_->CreateChild(kBandStates);
  }

  const auto& band_states = *equalizer_element_state.band_states();
  if (band_states.size() != equalizer_band_state_props_.size()) {
    ADR_WARN_METHOD() << "Band states size mismatch: " << band_states.size() << " vs "
                      << equalizer_band_state_props_.size();
    return;
  }

  for (size_t i = 0; i < band_states.size(); ++i) {
    const auto& band_state = band_states.at(i);
    auto& props = equalizer_band_state_props_.at(i);

    if (!props.band_node) {
      props.band_node = eq_band_states_node_->CreateChild(std::to_string(i));
    }
    auto& node = *props.band_node;

    // Band ID (Required)
    if (!SaveUintToNodeStringProperty(node, props.band_id, std::string(kBandId), band_state.id(),
                                      kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No ID for Equalizer band[" << i << "] in element " << element_id_;
    }

    // Type (Required)
    if (band_state.type().has_value()) {
      std::ostringstream stream;
      stream << *band_state.type();
      std::string band_type_str = stream.str();
      SaveNodeString(node, props.type, std::string(kType), band_type_str);
    } else {
      SaveNodeString(node, props.type, std::string(kType), kNoneNonCompliant);
      ADR_WARN_METHOD() << "No type for Equalizer band[" << i << "] in element " << element_id_;
    }

    // Frequency (Required)
    if (!SaveUintToNodeStringProperty(node, props.frequency, std::string(kFrequency),
                                      band_state.frequency(), kNoneNonCompliant)) {
      ADR_WARN_METHOD() << "No frequency for Equalizer band[" << i << "] in element "
                        << element_id_;
    }

    // Q
    SaveNodeDouble(node, props.q, std::string(kQ), band_state.q());

    // Gain DB
    std::string gain_db_str;
    if (band_state.gain_db().has_value()) {
      gain_db_str = std::to_string(*band_state.gain_db());
      // Should not have been set, if Notch or Cut
      if (band_state.type().has_value() &&
          (*band_state.type() == fhasp::EqualizerBandType::kNotch ||
           *band_state.type() == fhasp::EqualizerBandType::kLowCut ||
           *band_state.type() == fhasp::EqualizerBandType::kHighCut)) {
        gain_db_str += kNonCompliant;
        ADR_WARN_METHOD() << "gain_db set for Equalizer band[" << i << "] in element "
                          << element_id_;
      }
    } else {
      gain_db_str = kNone;
      // Should have been set, if Peak or Shelf
      if (band_state.type().has_value() &&
          (*band_state.type() == fhasp::EqualizerBandType::kPeak ||
           *band_state.type() == fhasp::EqualizerBandType::kLowShelf ||
           *band_state.type() == fhasp::EqualizerBandType::kHighShelf)) {
        gain_db_str += kNonCompliant;
        ADR_WARN_METHOD() << "No gain_db for Equalizer band[" << i << "] in element "
                          << element_id_;
      }
    }
    SaveNodeString(node, props.gain_db, std::string(kGainDb), gain_db_str);

    // Enabled
    if (band_state.enabled().has_value()) {
      std::string enabled_str = *band_state.enabled() ? "true" : "false";
      SaveNodeString(node, props.enabled, std::string(kEnabled), enabled_str);
    }
  }
}

void Element::RecordGainElement(fhasp::ElementType type,
                                const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  std::ostringstream stream;
  if (type != fhasp::ElementType::kGain) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << type << " with TypeSpecific::Gain";
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  stream << type_specific->gain()->type();
  type_specific_node_->RecordString(kGainType, stream.str());

  stream.str("");
  stream.clear();
  stream << type_specific->gain()->domain();
  type_specific_node_->RecordString(kGainDomain, stream.str());

  if (type_specific->gain()->min_gain().has_value()) {
    type_specific_node_->RecordString(kMinGain, std::to_string(*type_specific->gain()->min_gain()));
  } else {
    type_specific_node_->RecordString(kMinGain, kNoneNonCompliant);
  }

  if (type_specific->gain()->max_gain().has_value()) {
    type_specific_node_->RecordString(kMaxGain, std::to_string(*type_specific->gain()->max_gain()));
  } else {
    type_specific_node_->RecordString(kMaxGain, kNoneNonCompliant);
  }

  if (type_specific->gain()->min_gain_step().has_value()) {
    type_specific_node_->RecordString(kMinGainStep,
                                      std::to_string(*type_specific->gain()->min_gain_step()));
  } else {
    type_specific_node_->RecordString(kMinGainStep, kNoneNonCompliant);
  }
}

void Element::RecordGainElementState(
    const fuchsia_hardware_audio_signalprocessing::GainElementState& gain_element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  if (*element_type_ != fhasp::ElementType::kGain) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << *element_type_
                      << " with TypeSpecific::kGainState";
    return;
  }

  if (!SaveFloatToNodeStringProperty(*type_specific_state_node_, gain_db_prop_,
                                     std::string(kGainDb), gain_element_state.gain(),
                                     kNoneNonCompliant)) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": GainElementState has no gain_db";
  }
}

void Element::RecordVendorSpecificElement(
    fhasp::ElementType type,
    [[maybe_unused]] const std::optional<fhasp::TypeSpecificElement>& type_specific) {
  std::ostringstream stream;
  if (type != fhasp::ElementType::kVendorSpecific) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << type
                      << " with TypeSpecific::VendorSpecific";
    return;
  }
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;

  // Nothing else VendorSpecific-specific to capture!
}

void Element::RecordVendorSpecificElementState(
    [[maybe_unused]] const fuchsia_hardware_audio_signalprocessing::VendorSpecificState&
        vendor_specific_element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "element " << element_id_;
  if (*element_type_ != fhasp::ElementType::kVendorSpecific) {
    ADR_WARN_METHOD() << "element " << element_id_ << ": " << *element_type_
                      << " with TypeSpecific::kVendorSpecificState";
    return;
  }

  // Nothing else VendorSpecific-specific to capture!
}

///////////////////////////////////////
// Edge methods
Edge::Edge(inspect::Node edge_node, ElementId from_element_id, ElementId to_element_id)
    : edge_node_(std::move(edge_node)),
      from_element_id_(from_element_id),
      to_element_id_(to_element_id) {
  ADR_LOG_METHOD(kTraceInspector) << from_element_id_ << " -> " << to_element_id_;
  edge_node_.RecordUint(kEdgeFromElementId, from_element_id_);
  edge_node_.RecordUint(kEdgeToElementId, to_element_id_);
}

Edge::~Edge() { ADR_LOG_METHOD(kTraceInspector) << from_element_id_ << " -> " << to_element_id_; }

///////////////////////////////////////
// Topology methods
Topology::Topology(inspect::Node topology_node, TopologyId topology_id,
                   const std::vector<fhasp::EdgePair>& edge_pairs)
    : topology_node_(std::move(topology_node)), topology_id_(topology_id) {
  ADR_LOG_METHOD(kTraceInspector) << "id " << topology_id_ << ", edge_pairs[" << edge_pairs.size()
                                  << "]";

  topology_node_.RecordUint(kTopologyId, topology_id);
  edges_node_ = topology_node_.CreateChild(kEdgePairs);
  for (const auto& edge : edge_pairs) {
    edges_.emplace_back(edges_node_.CreateChild(std::to_string(edges_.size())),
                        edge.processing_element_id_from(), edge.processing_element_id_to());
  }
}

Topology::~Topology() { ADR_LOG_METHOD(kTraceInspector) << "id " << topology_id_; }

///////////////////////////////////////
// DeviceInspectInstance methods
DeviceInspectInstance::DeviceInspectInstance(inspect::Node device_node, std::string device_name,
                                             fuchsia_audio_device::DeviceType device_type,
                                             const zx::time& added_at, const std::string& added_by)
    : device_node_(std::move(device_node)), name_(std::move(device_name)) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  std::ostringstream device_type_ss;
  device_node_.RecordInt(kAddedAt, added_at.get());
  device_node_.RecordString(kAddedBy, added_by);

  device_type_ss << device_type;
  device_node_.RecordString(kDeviceType, device_type_ss.str());

  count_timeout_ = device_node_.CreateUint(kDriverTimeout, 0);
  count_late_response_ = device_node_.CreateUint(kDriverLateResponse, 0);
}

DeviceInspectInstance::~DeviceInspectInstance() {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
}

void DeviceInspectInstance::RecordTokenId(TokenId token_id) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "': token " << token_id;
  device_node_.RecordUint(kTokenId, token_id);
}

void DeviceInspectInstance::RecordDeviceHealthOk() {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  healthy_ = device_node_.CreateBool(kHealthy, true);
}

void DeviceInspectInstance::RecordProperties(std::optional<bool> is_input,
                                             std::optional<std::string> manufacturer,
                                             std::optional<std::string> product,
                                             std::optional<std::string> unique_instance_id,
                                             std::optional<ClockDomain> clock_domain) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  if (is_input.has_value()) {
    device_node_.RecordBool(kIsInput, *is_input);
  }
  if (manufacturer.has_value()) {
    device_node_.RecordString(kManufacturer, *manufacturer);
  }
  if (product.has_value()) {
    device_node_.RecordString(kProduct, *product);
  }
  if (unique_instance_id.has_value()) {
    device_node_.RecordString(kUniqueId, *unique_instance_id);
  }
  if (clock_domain.has_value()) {
    std::string domain_str = std::to_string(*clock_domain);
    if (*clock_domain == fuchsia_hardware_audio::kClockDomainMonotonic) {
      domain_str += " (CLOCK_DOMAIN_MONOTONIC)";
    } else if (*clock_domain == fuchsia_hardware_audio::kClockDomainExternal) {
      domain_str += " (CLOCK_DOMAIN_EXTERNAL)";
    }
    device_node_.RecordString(kClockDomain, domain_str);
  }
}

std::shared_ptr<Dai> DeviceInspectInstance::RecordDai(
    ElementId element_id, const std::optional<std::string>& element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  if (dais_.empty()) {
    dais_root_node_ = device_node_.CreateChild(kDAIs);
  }
  auto dai_node = dais_root_node_.CreateChild(std::to_string(dais_.size()));
  auto dai = std::make_shared<Dai>(std::move(dai_node), element_id, element_name);

  dais_.push_back(dai);
  return dai;
}

std::shared_ptr<RingBuffer> DeviceInspectInstance::RecordRingBuffer(
    ElementId element_id, const std::optional<std::string>& element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  if (ring_buffers_.empty()) {
    ring_buffers_root_node_ = device_node_.CreateChild(kRingBuffers);
  }
  auto ring_buffer_node = ring_buffers_root_node_.CreateChild(std::to_string(ring_buffers_.size()));
  auto ring_buffer =
      std::make_shared<RingBuffer>(std::move(ring_buffer_node), element_id, element_name);

  ring_buffers_.push_back(ring_buffer);
  return ring_buffer;
}

std::shared_ptr<PacketStream> DeviceInspectInstance::RecordPacketStream(
    ElementId element_id, const std::optional<std::string>& element_name) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  if (packet_streams_.empty()) {
    packet_streams_root_node_ = device_node_.CreateChild(kPacketStreams);
  }
  auto packet_stream_node =
      packet_streams_root_node_.CreateChild(std::to_string(packet_streams_.size()));
  auto packet_stream =
      std::make_shared<PacketStream>(std::move(packet_stream_node), element_id, element_name);

  packet_streams_.push_back(packet_stream);
  return packet_stream;
}

void DeviceInspectInstance::RecordRingBufferSupportedFormatSets(
    ElementId element_id, const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  auto found = std::ranges::find_if(ring_buffers_, [element_id](const auto& rb_ptr) {
    return (rb_ptr->element_id() == element_id);
  });
  if (found == ring_buffers_.end()) {
    ADR_WARN_OBJECT() << "Cannot record supported format sets: RingBuffer element_id " << element_id
                      << " not found";
  } else {
    found->get()->RecordSupportedFormatSets(format_sets);
  }
}

void DeviceInspectInstance::RecordPacketStreamSupportedFormatSets(
    ElementId element_id,
    const std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>& format_sets) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  auto found = std::ranges::find_if(packet_streams_.begin(), packet_streams_.end(),
                                    [element_id](const std::shared_ptr<PacketStream>& ps_ptr) {
                                      return (ps_ptr->element_id() == element_id);
                                    });
  if (found == packet_streams_.end()) {
    ADR_WARN_OBJECT() << "Cannot record supported format sets: PacketStream element_id "
                      << element_id << " not found";
  } else {
    found->get()->RecordSupportedFormatSets(format_sets);
  }
}

std::shared_ptr<RingBufferInspectInstance> DeviceInspectInstance::RecordRingBufferInstance(
    ElementId element_id, const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  auto found = std::ranges::find_if(ring_buffers_, [element_id](const auto& rb_ptr) {
    return (rb_ptr->element_id() == element_id);
  });
  if (found == ring_buffers_.end()) {
    ADR_WARN_OBJECT() << "Cannot create RingBuffer inspect instance: element_id " << element_id
                      << " not found";
    return nullptr;
  }
  return found->get()->RecordRingBufferInstance(created_at);
}

std::shared_ptr<PacketStreamInspectInstance> DeviceInspectInstance::RecordPacketStreamInstance(
    ElementId element_id, const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "', element " << element_id;
  auto found = std::ranges::find_if(packet_streams_, [element_id](const auto& ps_ptr) {
    return (ps_ptr->element_id() == element_id);
  });
  if (found == packet_streams_.end()) {
    ADR_WARN_OBJECT() << "Cannot create PacketStream inspect instance: element_id " << element_id
                      << " not found";
    return nullptr;
  }
  return found->get()->RecordPacketStreamInstance(created_at);
}

void DeviceInspectInstance::RecordCommandTimeout(const std::string& cmd_tag,
                                                 const zx::duration& expected,
                                                 std::optional<zx::duration> actual) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  if (actual.has_value()) {
    count_late_response_.Add(1);
    ADR_LOG_METHOD(kTraceInspector)
        << "Driver command '" << cmd_tag << "' expected in " << expected.to_usecs()
        << " usec, received in " << actual->to_usecs() << " usec.";
  } else {
    count_timeout_.Add(1);
    ADR_LOG_METHOD(kTraceInspector) << "Driver command '" << cmd_tag << "' expected in "
                                    << expected.to_usecs() << " usec, not yet received.";
  }
}

void DeviceInspectInstance::RecordError(const zx::time& failed_at) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  healthy_ = device_node_.CreateBool(kHealthy, false);
  device_node_.RecordInt(kFailedAt, failed_at.get());
}

void DeviceInspectInstance::RecordRemoval(const zx::time& removed_at) {
  ADR_LOG_METHOD(kTraceInspector) << "'" << name_ << "'";
  device_node_.RecordInt(kRemovedAt, removed_at.get());
}

std::shared_ptr<Topology> DeviceInspectInstance::RecordTopology(
    fhasp::TopologyId topology_id, const std::vector<fhasp::EdgePair>& edge_pairs) {
  ADR_LOG_METHOD(kTraceInspector) << "id " << topology_id;

  if (topologies_.empty()) {
    topologies_root_node_ = device_node_.CreateChild(kTopologies);
  }
  auto topology_node = topologies_root_node_.CreateChild(std::to_string(topologies_.size()));
  auto topology_node_ptr =
      std::make_shared<Topology>(std::move(topology_node), topology_id, edge_pairs);

  topologies_.push_back(topology_node_ptr);
  return topology_node_ptr;
}

std::shared_ptr<Element> DeviceInspectInstance::RecordElement(fhasp::ElementId element_id,
                                                              const fhasp::Element& element) {
  ADR_LOG_METHOD(kTraceInspector) << "id " << element_id;

  if (elements_.empty()) {
    elements_root_node_ = device_node_.CreateChild(kElements);
  }
  auto element_node = elements_root_node_.CreateChild(std::to_string(elements_.size()));
  auto element_node_ptr = std::make_shared<Element>(std::move(element_node), element_id, element);

  elements_.push_back(element_node_ptr);
  return element_node_ptr;
}

void DeviceInspectInstance::RecordActiveTopology(fhasp::TopologyId topology_id) {
  ADR_LOG_METHOD(kTraceInspector) << "id " << topology_id;
  if (!current_topology_id_) {
    device_node_.RecordUint(kInitialTopologyId, topology_id);
    // Create the property once here, then we'll update it subsequently.
    current_topology_id_ = device_node_.CreateUint(kCurrentTopologyId, topology_id);
  } else {
    current_topology_id_.Set(topology_id);
  }
}

void DeviceInspectInstance::RecordElementState(
    fuchsia_hardware_audio_signalprocessing::ElementId element_id,
    const fuchsia_hardware_audio_signalprocessing::ElementState& element_state) {
  ADR_LOG_METHOD(kTraceInspector) << "id " << element_id;
  auto element = std::ranges::find_if(
      elements_, [element_id](const auto& e) { return (e->element_id() == element_id); });
  if (element == elements_.end()) {
    ADR_WARN_OBJECT() << "Cannot record element state: element_id " << element_id << " not found";
    return;
  }
  element->get()->RecordElementState(element_state);
}

///////////////////////////////////////
// FidlServerInspectInstance methods
FidlServerInspectInstance::FidlServerInspectInstance(inspect::Node instance_node,
                                                     const zx::time& created_at)
    : instance_node_(std::move(instance_node)) {
  ADR_LOG_METHOD(kTraceInspector);
  instance_node_.RecordInt(kCreatedAt, created_at.get());
}

FidlServerInspectInstance::~FidlServerInspectInstance() { ADR_LOG_METHOD(kTraceInspector); }

void FidlServerInspectInstance::RecordDestructionTime(const zx::time& destroyed_at) {
  ADR_LOG_METHOD(kTraceInspector);
  instance_node_.RecordInt(kDestroyedAt, destroyed_at.get());
}

///////////////////////////////////////
// ProviderInspectInstance methods
ProviderInspectInstance::ProviderInspectInstance(inspect::Node provider_instance_node,
                                                 const zx::time& created_at)
    : FidlServerInspectInstance(std::move(provider_instance_node), created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  provider_devices_root_node_ = instance_node().CreateChild(kAddedDevices);
}

ProviderInspectInstance::~ProviderInspectInstance() { ADR_LOG_METHOD(kTraceInspector); }

void ProviderInspectInstance::RecordAddedDevice(const std::string& device_name,
                                                const fuchsia_audio_device::DeviceType& device_type,
                                                const zx::time& added_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node = provider_devices_root_node_.CreateChild(device_name);
  auto instance = std::make_shared<DeviceInspectInstance>(std::move(instance_node), device_name,
                                                          device_type, added_at, "Provider");
  provider_devices_.push_back(instance);
}

///////////////////////////////////////
// Inspector methods
Inspector::Inspector(async_dispatcher_t* dispatcher)
    : component_inspector_(
          std::make_unique<inspect::ComponentInspector>(dispatcher, inspect::PublishOptions{})),
      inspect_root_(component_inspector_->root()) {
  ADR_LOG_METHOD(kTraceInspector);
  component_inspector_->Health().StartingUp();

  devices_root_ = inspect_root_.CreateChild(kDevices);
  fidl_servers_root_ = inspect_root_.CreateChild(kFidlServers);

  registry_servers_root_ = fidl_servers_root_.CreateChild(kRegistryServerInstances);
  observer_servers_root_ = fidl_servers_root_.CreateChild(kObserverServerInstances);
  control_creator_servers_root_ = fidl_servers_root_.CreateChild(kControlCreatorServerInstances);
  control_servers_root_ = fidl_servers_root_.CreateChild(kControlServerInstances);
  ring_buffer_servers_root_ = fidl_servers_root_.CreateChild(kRingBufferServerInstances);
  packet_stream_servers_root_ = fidl_servers_root_.CreateChild(kPacketStreamServerInstances);

  provider_servers_root_ = fidl_servers_root_.CreateChild(kProviderServerInstances);

  count_device_failed_to_connect_ = inspect_root_.CreateUint(kDetectionConnectionErrors, 0);
  count_device_watcher_failures_ = inspect_root_.CreateUint(kDetectionOtherErrors, 0);
  count_detected_unsupported_device_type_ =
      inspect_root_.CreateUint(kDetectionUnsupportedDevices, 0);
}

Inspector::~Inspector() { ADR_LOG_METHOD(kTraceInspector); }

void Inspector::RecordHealthOk() {
  ADR_LOG_METHOD(kTraceInspector);
  component_inspector_->Health().Ok();
}

void Inspector::RecordUnhealthy(const std::string& health_message) {
  ADR_LOG_METHOD(kTraceInspector);
  component_inspector_->Health().Unhealthy(health_message);
}

void Inspector::RecordDetectionFailureToConnect() {
  ADR_LOG_METHOD(kTraceInspector);
  count_device_failed_to_connect_.Add(1);
}

void Inspector::RecordDetectionFailureOther() {
  ADR_LOG_METHOD(kTraceInspector);
  count_device_watcher_failures_.Add(1);
}

void Inspector::RecordDetectionFailureUnsupported() {
  ADR_LOG_METHOD(kTraceInspector);
  count_detected_unsupported_device_type_.Add(1);
}

std::shared_ptr<DeviceInspectInstance> Inspector::RecordDeviceInitializing(
    const std::string& device_name, fuchsia_audio_device::DeviceType device_type,
    const zx::time& added_at, const std::string& added_by) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node = devices_root_.CreateChild(device_name);
  auto instance = std::make_shared<DeviceInspectInstance>(std::move(instance_node), device_name,
                                                          device_type, added_at, added_by);
  device_instances_.push_back(instance);
  return instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordRegistryInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node =
      registry_servers_root_.CreateChild(std::to_string(registry_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  registry_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordObserverInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node =
      observer_servers_root_.CreateChild(std::to_string(observer_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  observer_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordControlCreatorInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node = control_creator_servers_root_.CreateChild(
      std::to_string(control_creator_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  control_creator_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordControlInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node =
      control_servers_root_.CreateChild(std::to_string(control_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  control_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordRingBufferInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node =
      ring_buffer_servers_root_.CreateChild(std::to_string(ring_buffer_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  ring_buffer_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<FidlServerInspectInstance> Inspector::RecordPacketStreamInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node = packet_stream_servers_root_.CreateChild(
      std::to_string(packet_stream_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<FidlServerInspectInstance>(std::move(instance_node), created_at);
  packet_stream_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

std::shared_ptr<ProviderInspectInstance> Inspector::RecordProviderInspectInstance(
    const zx::time& created_at) {
  ADR_LOG_METHOD(kTraceInspector);
  auto instance_node =
      provider_servers_root_.CreateChild(std::to_string(provider_server_instances_.size()));
  auto fidl_instance =
      std::make_shared<ProviderInspectInstance>(std::move(instance_node), created_at);
  provider_server_instances_.push_back(fidl_instance);
  return fidl_instance;
}

}  // namespace media_audio

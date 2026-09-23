// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_INSPECTOR_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_INSPECTOR_H_

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/async/dispatcher.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>

#include <optional>
#include <string>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

static constexpr std::string_view kDetectionConnectionErrors =
    "Device_detection_connection_failure_count";
static constexpr std::string_view kDetectionOtherErrors =
    "Device_detection_unclassified_error_count";
static constexpr std::string_view kDetectionUnsupportedDevices =
    "Device_detection_unsupported_device_count";

static constexpr std::string_view kDevices = "Devices";
static constexpr std::string_view kAddedAt = "added_at";
static constexpr std::string_view kAddedBy = "added_by";
static constexpr std::string_view kFailedAt = "failed_at";
static constexpr std::string_view kRemovedAt = "removed_at";
static constexpr std::string_view kDeviceType = "device_type";
static constexpr std::string_view kTokenId = "token_id";
static constexpr std::string_view kHealthy = "healthy";
static constexpr std::string_view kIsInput = "is_input";
static constexpr std::string_view kManufacturer = "manufacturer";
static constexpr std::string_view kProduct = "product";
static constexpr std::string_view kUniqueId = "unique_id";
static constexpr std::string_view kClockDomain = "clock_domain";
static constexpr std::string_view kDriverTimeout = "driver_timeouts";
static constexpr std::string_view kDriverLateResponse = "driver_late_responses";

static constexpr std::string_view kPluggedInStr = "plugged-in";
static constexpr std::string_view kUnpluggedStr = "unplugged";

static constexpr std::string_view kTopologies = "Topologies";
static constexpr std::string_view kTopologyId = "topology_id";
static constexpr std::string_view kInitialTopologyId = "initial_topology_id";
static constexpr std::string_view kCurrentTopologyId = "current_topology_id";
static constexpr std::string_view kEdgePairs = "edge_pairs";
static constexpr std::string_view kEdgeFromElementId = "from_element_id";
static constexpr std::string_view kEdgeToElementId = "to_element_id";

static constexpr std::string_view kElements = "Elements";
static constexpr std::string_view kProperties = "properties";
static constexpr std::string_view kType = "type";
static constexpr std::string_view kElementId = "element_id";
static constexpr std::string_view kDescription = "description";
static constexpr std::string_view kTypeSpecific = "type_specific";
static constexpr std::string_view kCanStop = "can_stop";
static constexpr std::string_view kCanBypass = "can_bypass";

static constexpr std::string_view kState = "state";
static constexpr std::string_view kVendorSpecificData = "vendor_specific_data";
static constexpr std::string_view kStarted = "started";
static constexpr std::string_view kBypassed = "bypassed";
static constexpr std::string_view kTurnOnDelay = "turn_on_delay_ns";
static constexpr std::string_view kTurnOffDelay = "turn_off_delay_ns";
static constexpr std::string_view kProcessingDelay = "processing_delay_ns";

// DAI-specific
static constexpr std::string_view kPlugDetectCapabilities = "plug_detect_capabilities";
static constexpr std::string_view kExternalDelay = "external_delay_ns";
static constexpr std::string_view kPlugState = "plug_state";
static constexpr std::string_view kPlugged = "plugged";
static constexpr std::string_view kPlugStateTime = "plug_state_time";
// Dynamics-specific
static constexpr std::string_view kThresholdDb = "threshold_db";
static constexpr std::string_view kThresholdType = "threshold_type";
static constexpr std::string_view kRatio = "ratio";
static constexpr std::string_view kKneeWidthDb = "knee_width_db";
static constexpr std::string_view kAttackNs = "attack_ns";
static constexpr std::string_view kReleaseNs = "release_ns";
static constexpr std::string_view kOutputGainDb = "output_gain_db";
static constexpr std::string_view kInputGainDb = "input_gain_db";
static constexpr std::string_view kLevelType = "level_type";
static constexpr std::string_view kLookaheadNs = "lookahead_ns";
static constexpr std::string_view kLinkedChannels = "linked_channels";
// Dynamics- and Equalizer-specific
static constexpr std::string_view kBands = "bands";
static constexpr std::string_view kBandId = "band_id";
static constexpr std::string_view kSupportedControls = "supported_controls";
static constexpr std::string_view kBandStates = "band_states";
// Equalizer-specific
static constexpr std::string_view kCanDisableBands = "can_disable_bands";
static constexpr std::string_view kMaxQ = "max_q";
static constexpr std::string_view kMinGainDb = "min_gain_db";
static constexpr std::string_view kMaxGainDb = "max_gain_db";
static constexpr std::string_view kFrequency = "frequency";
static constexpr std::string_view kQ = "q_factor";
static constexpr std::string_view kEnabled = "enabled";
// Gain-specific
static constexpr std::string_view kGainType = "gain_type";
static constexpr std::string_view kGainDomain = "gain_domain";
static constexpr std::string_view kMinGain = "min_gain";
static constexpr std::string_view kMaxGain = "max_gain";
static constexpr std::string_view kMinGainStep = "min_gain_step";
static constexpr std::string_view kGainDb = "gain_db";
// Vendor-specific element fields are all custom.

static constexpr std::string_view kDAIs = "DAIs";
static constexpr std::string_view kRingBuffers = "RingBuffers";
static constexpr std::string_view kPacketStreams = "PacketStreams";

static constexpr std::string_view kSupportedFormats = "supported_format_sets";
static constexpr std::string_view kFormatProps = "format";
static constexpr std::string_view kBitsPerFrame = "bits_per_frame";
static constexpr std::string_view kBitsPerSample = "bits_per_sample";
static constexpr std::string_view kChannelBitmask = "channel_bitmask";
static constexpr std::string_view kChannelCount = "channel_count";
static constexpr std::string_view kFramesPerSecond = "frames_per_second";
static constexpr std::string_view kFrameFormat = "frame_format";
static constexpr std::string_view kSampleFormat = "sample_format";
static constexpr std::string_view kEncodingType = "encoding_type";
static constexpr std::string_view kMinBitrate = "min_bitrate";
static constexpr std::string_view kMaxBitrate = "max_bitrate";
static constexpr std::string_view kMinFrequency = "min_frequency";
static constexpr std::string_view kMaxFrequency = "max_frequency";

static constexpr std::string_view kBufferProps = "buffer";
static constexpr std::string_view kBufferType = "buffer_type";
static constexpr std::string_view kVmoId = "vmo_id";
static constexpr std::string_view kVmoInfos = "vmo_infos";
static constexpr std::string_view kVmoBytes = "allocated_vmo_bytes";
static constexpr std::string_view kRequestedBytes = "client_request_bytes";
static constexpr std::string_view kConsumerFrames = "consumer_frames";
static constexpr std::string_view kProducerFrames = "producer_frames";

static constexpr std::string_view kRunningIntervals = "Running_intervals";
static constexpr std::string_view kStartedAt = "started_at";
static constexpr std::string_view kStoppedAt = "stopped_at";

static constexpr std::string_view kSetActiveChannelsCalls = "SetActiveChannels_calls";
static constexpr std::string_view kCalledAt = "called_at";
static constexpr std::string_view kCompletedAt = "completed_at";

static constexpr std::string_view kFidlServers = "FIDL_servers";
static constexpr std::string_view kRegistryServerInstances = "RegistryServer_instances";
static constexpr std::string_view kObserverServerInstances = "ObserverServer_instances";
static constexpr std::string_view kControlCreatorServerInstances = "ControlCreatorServer_instances";
static constexpr std::string_view kControlServerInstances = "ControlServer_instances";
static constexpr std::string_view kRingBufferServerInstances = "RingBufferServer_instances";
static constexpr std::string_view kPacketStreamServerInstances = "PacketStreamServer_instances";
static constexpr std::string_view kProviderServerInstances = "ProviderServer_instances";
static constexpr std::string_view kCreatedAt = "created_at";
static constexpr std::string_view kDestroyedAt = "destroyed_at";
static constexpr std::string_view kAddedDevices = "Added_devices";

static constexpr std::string kNone = "<none>";
static constexpr std::string kNonCompliant = " (non-compliant)";
static constexpr std::string kNoneNonCompliant = kNone + kNonCompliant;

//
// These classes encapsulate the creation and update of the fuchsia Inspect data store. The parent
// Inspector singleton entails the system-provided ComponentInspector and offers methods that enable
// ADR to chronicle objects without integrating directly with Inspect.
//
// Each class handles Inspect for an associated entity (which may not be an actual ADR object).

// This represents a single call to SetActiveChannels.
class SetActiveChannelsInspectInstance {
 public:
  SetActiveChannelsInspectInstance(inspect::Node set_active_channels_node, uint64_t channel_bitmask,
                                   const zx::time& called_at, const zx::time& completed_at);
  ~SetActiveChannelsInspectInstance();

 private:
  static constexpr std::string_view kClassName = "SetActiveChannelsInspectInstance";

  inspect::Node set_active_channels_node_;
};

// This represents a single Start/Stop segment.
class RunningIntervalInspectInstance {
 public:
  RunningIntervalInspectInstance(inspect::Node running_interval_node, const zx::time& started_at);
  ~RunningIntervalInspectInstance();

  void RecordStopTime(const zx::time& stopped_at);

 private:
  static constexpr std::string_view kClassName = "RunningIntervalInspectInstance";

  inspect::Node running_interval_node_;
};

// This represents an active instance of the audio driver RingBuffer protocol.
class RingBufferInspectInstance {
 public:
  RingBufferInspectInstance(inspect::Node ring_buffer_instance_node, const zx::time& created_at);
  ~RingBufferInspectInstance();

  void RecordDestructionTime(const zx::time& destroyed_at);

  void RecordStartTime(const zx::time& started_at);
  void RecordStopTime(const zx::time& stopped_at);

  inspect::Node& inspect_node() { return ring_buffer_instance_node_; }
  std::shared_ptr<SetActiveChannelsInspectInstance> RecordSetActiveChannelsCall(
      uint64_t channel_bitmask, const zx::time& called_at, const zx::time& completed_at);

  void RecordBuffer(uint64_t requested_bytes, uint64_t producer_frames, uint64_t consumer_frames,
                    uint64_t vmo_size);
  void RecordFormat(uint32_t channel_count, uint32_t frames_per_second,
                    fuchsia_audio::SampleType sample_type);

 private:
  static constexpr std::string_view kClassName = "RingBufferInspectInstance";

  inspect::Node ring_buffer_instance_node_;

  inspect::Node set_active_channels_calls_root_node_;
  std::vector<std::shared_ptr<SetActiveChannelsInspectInstance>> set_active_channels_calls_;

  inspect::Node running_intervals_root_node_;
  std::vector<std::shared_ptr<RunningIntervalInspectInstance>> running_intervals_;

  inspect::Node buffer_node_;
  inspect::Node format_node_;
};

// This represents an active instance of the audio driver PacketStream protocol.
class PacketStreamInspectInstance {
 public:
  PacketStreamInspectInstance(inspect::Node packet_stream_instance_node,
                              const zx::time& created_at);
  ~PacketStreamInspectInstance();

  void RecordDestructionTime(const zx::time& destroyed_at);

  void RecordStartTime(const zx::time& started_at);
  void RecordStopTime(const zx::time& stopped_at);

  inspect::Node& inspect_node() { return packet_stream_instance_node_; }

  void RecordBuffer(fuchsia_hardware_audio::BufferType buffer_type,
                    const std::vector<fuchsia_hardware_audio::VmoInfo>& vmo_infos);
  void RecordFormat(const fuchsia_audio_device::PacketStreamFormat& format);

 private:
  static constexpr std::string_view kClassName = "PacketStreamInspectInstance";

  inspect::Node packet_stream_instance_node_;

  inspect::Node running_intervals_root_node_;
  std::vector<std::shared_ptr<RunningIntervalInspectInstance>> running_intervals_;

  inspect::Node buffer_node_;
  inspect::Node vmo_infos_node_;
  std::vector<inspect::Node> vmo_nodes_;
  inspect::Node format_node_;
};

// rb_format_set_0
//   channel_count
//     [0]
//       channel_0
//         min_frequency:  0
//         max_frequency:  48000
struct ChannelSetRecord {
  inspect::Node channel_set_node;
  std::vector<inspect::Node> channel_nodes;
};

struct SupportedPcmFormatsRecord {
  inspect::Node pcm_format_set_node;
  inspect::Node channel_sets_node;
  std::vector<ChannelSetRecord> channel_sets;
  inspect::UintArray frame_rates;
  inspect::StringArray sample_formats;
};
struct SupportedEncodingsRecord {
  inspect::Node encodings_node;
  inspect::Node decoded_channel_sets_node;
  std::vector<ChannelSetRecord> decoded_channel_sets;
  inspect::UintArray decoded_frame_rates;
  inspect::UintProperty min_encoding_bitrate;
  inspect::UintProperty max_encoding_bitrate;
  inspect::StringArray encoding_types;
};

struct DaiFormatSetRecord {
  inspect::Node dai_format_set_node;
  inspect::UintArray dai_format_set_channel_counts;
  inspect::StringArray dai_format_set_sample_formats;
  inspect::StringArray dai_format_set_frame_formats;
  inspect::UintArray dai_format_set_frame_rates;
  inspect::UintArray dai_format_set_frame_sizes;
  inspect::UintArray dai_format_set_sample_sizes;
};

void RecordSupportedPcmFormatSets(
    inspect::Node& header_node, std::vector<SupportedPcmFormatsRecord>& records,
    const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets, std::string_view prefix);

void RecordSupportedEncodingSets(
    inspect::Node& header_node, std::vector<SupportedEncodingsRecord>& records,
    const std::vector<fuchsia_hardware_audio::SupportedEncodings>& encodings,
    std::string_view prefix);

// IoNode represents a DAI/RingBuffer/PacketStream as expressed in the hardware topology.
// Conceptually these are the _only places that audio frames enter or leave the topology._ IoNodes
// report all properties/state NOT conveyed through the signalprocessing protocol (format support,
// creation of RingBuffer/PacketStream instances, buffer sizes, timestamp details, etc). "IoNode"
// naming explicitly avoids "Element" because all signalprocessing aspects are handled elsewhere.
class IoNode {
 public:
  IoNode(inspect::Node node, ElementId element_id, const std::optional<std::string>& element_name);
  ~IoNode();

  inspect::Node& inspect_node() { return node_; }
  ElementId element_id() const { return element_id_; }

 protected:
  inspect::Node& format_sets_header_node() { return format_sets_header_node_; }
  inspect::Node& format_node() { return format_node_; }

 private:
  static constexpr std::string_view kClassName = "IoNode";
  inspect::Node node_;
  inspect::Node format_sets_header_node_;
  inspect::Node format_node_;
  ElementId element_id_;
};

// This represents the functionality of a ring buffer as expressed in the hardware topology. This
// object will have RingBufferInspectInstance children, if a client connects to the RingBuffer API.
class RingBuffer : public IoNode {
 public:
  RingBuffer(inspect::Node ring_buffer_node, ElementId element_id,
             const std::optional<std::string>& element_name);
  ~RingBuffer();

  std::shared_ptr<RingBufferInspectInstance> RecordRingBufferInstance(const zx::time& created_at);
  void RecordSupportedFormatSets(
      const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets);

 private:
  static constexpr std::string_view kClassName = "RingBuffer";
  std::vector<SupportedPcmFormatsRecord> supported_pcm_formats_sets_;
  std::vector<std::shared_ptr<RingBufferInspectInstance>> ring_buffer_instances_;
};

// This represents the functionality of a packet stream as expressed in the hardware topology. This
// object have PacketStreamInspectInstance children, if a client connects to the PacketStream API.
class PacketStream : public IoNode {
 public:
  PacketStream(inspect::Node packet_stream_node, ElementId element_id,
               const std::optional<std::string>& element_name);
  ~PacketStream();

  std::shared_ptr<PacketStreamInspectInstance> RecordPacketStreamInstance(
      const zx::time& created_at);
  void RecordSupportedFormatSets(
      const std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>& format_sets);

 private:
  static constexpr std::string_view kClassName = "PacketStream";
  std::vector<SupportedPcmFormatsRecord> supported_pcm_formats_sets_;
  std::vector<SupportedEncodingsRecord> supported_encodings_;
  std::vector<std::shared_ptr<PacketStreamInspectInstance>> packet_stream_instances_;
};

// This represents the functionality of a DAI as expressed in the hardware topology.
class Dai : public IoNode {
 public:
  Dai(inspect::Node dai_node, ElementId element_id, const std::optional<std::string>& element_name);
  ~Dai();
  void RecordSetDaiFormat(const zx::time& set_at,
                          const fuchsia_hardware_audio::DaiFormat& dai_format);
  void RecordSupportedFormatSets(
      const std::vector<fuchsia_hardware_audio::DaiSupportedFormats>& format_sets);

 private:
  static constexpr std::string_view kClassName = "Dai";

  std::vector<DaiFormatSetRecord> dai_format_sets_;
};

// This represents an EdgePair within a hardware topology.
class Edge {
 public:
  Edge(inspect::Node edge_node, ElementId from_element_id, ElementId to_element_id);
  ~Edge();

  Edge(Edge&& other) = default;
  Edge& operator=(Edge&& other) = default;

  inspect::Node& inspect_node() { return edge_node_; }
  ElementId from_element_id() const { return from_element_id_; }
  ElementId to_element_id() const { return to_element_id_; }

 private:
  static constexpr std::string_view kClassName = "Edge";

  inspect::Node edge_node_;
  ElementId from_element_id_;
  ElementId to_element_id_;
};

struct DynamicsBandStateProps {
  std::optional<inspect::Node> band_node;
  std::optional<inspect::StringProperty> band_id;
  std::optional<inspect::StringProperty> min_frequency;
  std::optional<inspect::StringProperty> max_frequency;
  std::optional<inspect::StringProperty> threshold_db;
  std::optional<inspect::StringProperty> threshold_type;
  std::optional<inspect::StringProperty> ratio;
  std::optional<inspect::DoubleProperty> knee_width_db;
  std::optional<inspect::IntProperty> attack_ns;
  std::optional<inspect::IntProperty> release_ns;
  std::optional<inspect::DoubleProperty> output_gain_db;
  std::optional<inspect::DoubleProperty> input_gain_db;
  std::optional<inspect::StringProperty> level_type;
  std::optional<inspect::IntProperty> lookahead_ns;
  std::optional<inspect::BoolProperty> linked_channels;
};

struct EqualizerBandStateProps {
  std::optional<inspect::Node> band_node;
  std::optional<inspect::StringProperty> band_id;
  std::optional<inspect::StringProperty> type;
  std::optional<inspect::StringProperty> frequency;
  std::optional<inspect::DoubleProperty> q;
  std::optional<inspect::StringProperty> gain_db;
  std::optional<inspect::StringProperty> enabled;
};

// This represents a hardware element as expressed in the signalprocessing API.
class Element {
 public:
  Element(inspect::Node element_node, ElementId element_id,
          const fuchsia_hardware_audio_signalprocessing::Element& element);
  ~Element();

  inspect::Node& inspect_node() { return element_node_; }
  ElementId element_id() const { return element_id_; }

  void RecordElementState(
      const fuchsia_hardware_audio_signalprocessing::ElementState& element_state);

 protected:
  void RecordTypeSpecificElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);
  void RecordDaiInterconnectElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);
  void RecordDynamicsElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);
  void RecordEqualizerElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);
  void RecordGainElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);
  void RecordVendorSpecificElement(
      fuchsia_hardware_audio_signalprocessing::ElementType type,
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElement>&
          type_specific);

  void RecordTypeSpecificElementState(
      const std::optional<fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState>&
          type_specific_state);
  void RecordDaiInterconnectElementState(
      const fuchsia_hardware_audio_signalprocessing::DaiInterconnectElementState&
          dai_interconnect_state);
  void RecordDynamicsElementState(
      const fuchsia_hardware_audio_signalprocessing::DynamicsElementState& dynamics_element_state);
  void RecordEqualizerElementState(
      const fuchsia_hardware_audio_signalprocessing::EqualizerElementState&
          equalizer_element_state);
  void RecordGainElementState(
      const fuchsia_hardware_audio_signalprocessing::GainElementState& gain_element_state);
  void RecordVendorSpecificElementState(
      const fuchsia_hardware_audio_signalprocessing::VendorSpecificState&
          vendor_specific_element_state);

  void SaveUint(std::optional<inspect::UintProperty>& prop, const std::string& key,
                std::optional<uint64_t> value);
  void SaveString(std::optional<inspect::StringProperty>& prop, const std::string& key,
                  const std::string& value);

  void SaveBoolean(std::optional<inspect::BoolProperty>& prop, const std::string& key,
                   std::optional<bool> value);
  bool SaveBooleanToStringProperty(std::optional<inspect::StringProperty>& prop,
                                   const std::string& key, std::optional<bool> value,
                                   const std::string& default_str);

  void SaveInt(std::optional<inspect::IntProperty>& prop, const std::string& key,
               std::optional<int64_t> value);
  bool SaveIntToStringProperty(std::optional<inspect::StringProperty>& prop, const std::string& key,
                               std::optional<int64_t> value, const std::string& default_str);

 private:
  static constexpr std::string_view kClassName = "Element";

  inspect::Node element_node_;
  inspect::Node props_node_;
  std::optional<inspect::Node> type_specific_node_ = std::nullopt;
  std::optional<inspect::UintArray> bands_arr_;

  inspect::Node state_node_;
  std::optional<inspect::StringProperty> started_prop_;
  std::optional<inspect::StringProperty> bypassed_prop_;
  std::optional<inspect::StringProperty> turn_on_delay_prop_;
  std::optional<inspect::StringProperty> turn_off_delay_prop_;
  std::optional<inspect::StringProperty> processing_delay_prop_;
  std::optional<inspect::StringProperty> vendor_specific_data_prop_;
  std::optional<inspect::Node> type_specific_state_node_ = std::nullopt;

  std::optional<inspect::StringProperty> external_delay_prop_;
  std::optional<inspect::Node> plug_state_node_;
  std::optional<inspect::StringProperty> plug_state_prop_;
  std::optional<inspect::StringProperty> plug_state_time_prop_;

  std::optional<inspect::Node> dyn_band_states_node_;
  std::vector<DynamicsBandStateProps> dynamics_band_state_props_;

  std::optional<inspect::Node> eq_band_states_node_;
  std::vector<EqualizerBandStateProps> equalizer_band_state_props_;

  std::optional<inspect::StringProperty> gain_db_prop_;

  ElementId element_id_;
  std::optional<fuchsia_hardware_audio_signalprocessing::ElementType> element_type_ = std::nullopt;
};

// This represents a hardware topology as expressed in the signalprocessing API.
class Topology {
 public:
  Topology(inspect::Node topology_node, TopologyId topology_id,
           const std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>& edge_pairs);
  ~Topology();

 private:
  static constexpr std::string_view kClassName = "Topology";

  inspect::Node topology_node_;
  TopologyId topology_id_;

  inspect::Node edges_node_;
  std::vector<Edge> edges_;
};

// This represents an audio driver and its device. It is created when an audio device is detected in
// DevFs or added via Provider/AddDevice.
class DeviceInspectInstance {
 public:
  DeviceInspectInstance(inspect::Node device_node, std::string device_name,
                        fuchsia_audio_device::DeviceType device_type, const zx::time& added_at,
                        const std::string& added_by);
  ~DeviceInspectInstance();

  inspect::Node& inspect_node() { return device_node_; }

  void RecordTokenId(TokenId token_id);
  void RecordDeviceHealthOk();
  void RecordProperties(std::optional<bool> is_input, std::optional<std::string> manufacturer,
                        std::optional<std::string> product,
                        std::optional<std::string> unique_instance_id,
                        std::optional<ClockDomain> clock_domain);

  std::shared_ptr<Topology> RecordTopology(
      fuchsia_hardware_audio_signalprocessing::TopologyId topology_id,
      const std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>& edge_pairs);
  std::shared_ptr<Element> RecordElement(
      fuchsia_hardware_audio_signalprocessing::ElementId element_id,
      const fuchsia_hardware_audio_signalprocessing::Element& element);
  void RecordActiveTopology(fuchsia_hardware_audio_signalprocessing::TopologyId topology_id);
  void RecordElementState(
      fuchsia_hardware_audio_signalprocessing::ElementId element_id,
      const fuchsia_hardware_audio_signalprocessing::ElementState& element_state);

  std::shared_ptr<Dai> RecordDai(ElementId element_id,
                                 const std::optional<std::string>& element_name);

  std::shared_ptr<RingBuffer> RecordRingBuffer(ElementId element_id,
                                               const std::optional<std::string>& element_name);
  void RecordRingBufferSupportedFormatSets(
      ElementId element_id, const std::vector<fuchsia_audio_device::PcmFormatSet>& format_sets);
  std::shared_ptr<RingBufferInspectInstance> RecordRingBufferInstance(ElementId element_id,
                                                                      const zx::time& created_at);

  std::shared_ptr<PacketStream> RecordPacketStream(ElementId element_id,
                                                   const std::optional<std::string>& element_name);
  void RecordPacketStreamSupportedFormatSets(
      ElementId element_id,
      const std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>& format_sets);
  void RecordPacketStreamSupportedFormatSets(
      ElementId element_id,
      const std::vector<fuchsia_hardware_audio::SupportedEncodings>& format_sets);
  std::shared_ptr<PacketStreamInspectInstance> RecordPacketStreamInstance(
      ElementId element_id, const zx::time& created_at);

  void RecordCommandTimeout(const std::string& cmd_tag, const zx::duration& expected,
                            std::optional<zx::duration> actual);
  void RecordError(const zx::time& failed_at);
  void RecordRemoval(const zx::time& removed_at);

 private:
  static constexpr std::string_view kClassName = "DeviceInspectInstance";

  inspect::Node device_node_;
  std::string name_;

  inspect::Node topologies_root_node_;
  inspect::Node elements_root_node_;

  inspect::Node dais_root_node_;
  inspect::Node ring_buffers_root_node_;
  inspect::Node packet_streams_root_node_;

  inspect::BoolProperty healthy_;
  inspect::UintProperty count_timeout_;
  inspect::UintProperty count_late_response_;
  inspect::UintProperty current_topology_id_;

  std::vector<std::shared_ptr<Dai>> dais_;
  std::vector<std::shared_ptr<RingBuffer>> ring_buffers_;
  std::vector<std::shared_ptr<PacketStream>> packet_streams_;
  std::vector<std::shared_ptr<Topology>> topologies_;
  std::vector<std::shared_ptr<Element>> elements_;
};

// This represents a client connection to one of the seven ADR FIDL protocols:
// Registry, Observer, ControlCreator, Control, RingBuffer, PacketStream or Provider.
class FidlServerInspectInstance {
 public:
  FidlServerInspectInstance(inspect::Node instance_node, const zx::time& created_at);
  ~FidlServerInspectInstance();

  void RecordDestructionTime(const zx::time& destroyed_at);

 protected:
  inspect::Node& instance_node() { return instance_node_; }

 private:
  static constexpr std::string_view kClassName = "FidlServerInspectInstance";

  inspect::Node instance_node_;
};

// We save additional information or each client Provider instance: the devices that it has added.
// We reuse DeviceInspectInstance but use only a subset (name, type, added_at).
class ProviderInspectInstance : public FidlServerInspectInstance {
 public:
  ProviderInspectInstance(inspect::Node provider_instance_node, const zx::time& created_at);
  ~ProviderInspectInstance();

  void RecordAddedDevice(const std::string& device_name,
                         const fuchsia_audio_device::DeviceType& device_type,
                         const zx::time& added_at);

 private:
  static constexpr std::string_view kClassName = "ProviderInspectInstance";

  inspect::Node provider_devices_root_node_;
  std::vector<std::shared_ptr<DeviceInspectInstance>> provider_devices_;
};

// This singleton manages (and owns, from a lifetime mgmt standpoint) Inspect for the entire ADR.
// Inspect for core/audio_device_registry contains two sections: Devices and FIDL Servers.
// The Devices section contains info on devices that have been detected and presented to clients.
// The FIDL Servers section contains info on client instances of the six ADR FIDL protocols.
class Inspector {
 public:
  static void Initialize(async_dispatcher_t* dispatcher);
  static std::unique_ptr<Inspector>& Singleton() { return singleton_; }
  std::unique_ptr<inspect::ComponentInspector>& component_inspector() {
    return component_inspector_;
  }

  explicit Inspector(async_dispatcher_t* dispatcher);
  ~Inspector();

  void RecordHealthOk();
  void RecordUnhealthy(const std::string& health_message);

  void RecordDetectionFailureToConnect();
  void RecordDetectionFailureOther();
  void RecordDetectionFailureUnsupported();

  std::shared_ptr<DeviceInspectInstance> RecordDeviceInitializing(
      const std::string& device_name, fuchsia_audio_device::DeviceType device_type,
      const zx::time& added_at, const std::string& added_by);

  // Create an Inspect node for the instance (e.g. a child of control_servers_root_ if this is a
  // Control instance), wrap it in a FidlServerInspectInstance object, and return a shared_ptr to
  // it. This class (via control_server_instances_ etc.) owns the instance node; the child
  // FidlServerInspectInstance creates nodes or properties attached to the instance node.
  std::shared_ptr<FidlServerInspectInstance> RecordRegistryInstance(const zx::time& created_at);
  std::shared_ptr<FidlServerInspectInstance> RecordObserverInstance(const zx::time& created_at);
  std::shared_ptr<FidlServerInspectInstance> RecordControlCreatorInstance(
      const zx::time& created_at);
  std::shared_ptr<FidlServerInspectInstance> RecordControlInstance(const zx::time& created_at);
  std::shared_ptr<FidlServerInspectInstance> RecordRingBufferInstance(const zx::time& created_at);
  std::shared_ptr<FidlServerInspectInstance> RecordPacketStreamInstance(const zx::time& created_at);

  std::shared_ptr<ProviderInspectInstance> RecordProviderInspectInstance(
      const zx::time& created_at);

 private:
  static constexpr std::string_view kClassName = "Inspector";

  static std::unique_ptr<Inspector> singleton_;

  std::unique_ptr<inspect::ComponentInspector> component_inspector_;
  inspect::Node& inspect_root_;

  inspect::UintProperty count_device_failed_to_connect_;
  inspect::UintProperty count_device_watcher_failures_;
  inspect::UintProperty count_detected_unsupported_device_type_;

  // The top-level "Devices" parent node
  inspect::Node devices_root_;

  // Each entry in this vector represents an audio driver and device.
  // Note that entries are not removed when a device encounters an error or is removed.
  std::vector<std::shared_ptr<DeviceInspectInstance>> device_instances_;

  // The top-level "FIDL Servers" parent node
  inspect::Node fidl_servers_root_;

  inspect::Node registry_servers_root_;
  inspect::Node observer_servers_root_;
  inspect::Node control_creator_servers_root_;
  inspect::Node control_servers_root_;
  inspect::Node ring_buffer_servers_root_;
  inspect::Node packet_stream_servers_root_;

  inspect::Node provider_servers_root_;

  // Each entry in these vectors represents a client instance of one of the ADR protocols and
  // includes an Inspect node for that instance as well as timestamps for creation and destruction.
  // Note that entries are not removed when a client disconnects.
  std::vector<std::shared_ptr<FidlServerInspectInstance>> registry_server_instances_;
  std::vector<std::shared_ptr<FidlServerInspectInstance>> observer_server_instances_;
  std::vector<std::shared_ptr<FidlServerInspectInstance>> control_creator_server_instances_;
  std::vector<std::shared_ptr<FidlServerInspectInstance>> control_server_instances_;
  std::vector<std::shared_ptr<FidlServerInspectInstance>> ring_buffer_server_instances_;
  std::vector<std::shared_ptr<FidlServerInspectInstance>> packet_stream_server_instances_;

  std::vector<std::shared_ptr<ProviderInspectInstance>> provider_server_instances_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_INSPECTOR_H_

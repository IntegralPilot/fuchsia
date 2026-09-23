// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/audio_core/reporter.h"

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.metrics/cpp/natural_types.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <queue>
#include <vector>

#include "src/media/audio/audio_core/media_metrics_registry.cb.h"
#include "src/media/audio/audio_core/metrics/metrics_impl.h"

namespace media::audio {

////////////////////////////////////////////////////////////////////////////////
// Singletons

namespace {
using AudioDriverInfo = Reporter::AudioDriverInfo;

std::mutex singleton_mutex;
Reporter* const singleton_nop = new Reporter();
Reporter* singleton_real;

class TokenBucket {
 public:
  TokenBucket(zx::duration period, size_t tokens_per_period)
      : period_(period),
        tokens_per_period_(tokens_per_period),
        start_time_(zx::clock::get_monotonic()),
        tokens_(tokens_per_period) {}

  bool Acquire() {
    std::lock_guard<std::mutex> lock(mutex_);
    auto now = zx::clock::get_monotonic();
    if (now - start_time_ >= period_) {
      start_time_ = now;
      tokens_ = tokens_per_period_;
    }
    if (tokens_ == 0) {
      return false;
    }
    --tokens_;
    return true;
  }

 private:
  const zx::duration period_;
  const size_t tokens_per_period_;

  std::mutex mutex_;
  zx::time start_time_ FXL_GUARDED_BY(mutex_);
  size_t tokens_ FXL_GUARDED_BY(mutex_);
};

// To avoid overloading cobalt, throttle cobalt RPCs. See https://fxbug.dev/42146328.
// In a typical worst case, we might expect about 1 RPC every 10ms, or 6000 RPCs per minute.
// Throttle to 30 per minute.
TokenBucket* const cobalt_token_bucket = new TokenBucket(zx::min(1), 30);
}  // namespace

////////////////////////////////////////////////////////////////////////////////
// No-op implementations, used before the Reporter has been initialized

namespace {
class OutputDeviceNop : public Reporter::OutputDevice {
 public:
  void Destroy() override {}

  void StartSession(zx::time start_time) override {}
  void StopSession(zx::time stop_time) override {}

  void SetDriverInfo(const AudioDriverInfo& driver) override {}
  void SetGainInfo(const fuchsia::media::AudioGainInfo& gain_info,
                   fuchsia::media::AudioGainValidFlags set_flags) override {}
  void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                    std::optional<zx::duration> external_delay) override {}
  void DeviceUnderflow(zx::time start_time, zx::time end_time) override {}
  void PipelineUnderflow(zx::time start_time, zx::time end_time) override {}
};

class InputDeviceNop : public Reporter::InputDevice {
 public:
  void Destroy() override {}

  void StartSession(zx::time start_time) override {}
  void StopSession(zx::time stop_time) override {}

  void SetDriverInfo(const AudioDriverInfo& driver) override {}
  void SetGainInfo(const fuchsia::media::AudioGainInfo& gain_info,
                   fuchsia::media::AudioGainValidFlags set_flags) override {}
  void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                    std::optional<zx::duration> external_delay) override {}
};

class RendererNop : public Reporter::Renderer {
 public:
  void Destroy() override {}

  void StartSession(zx::time start_time) override {}
  void StopSession(zx::time stop_time) override {}

  void SetUsage(RenderUsage usage) override {}
  void SetFormat(const Format& format) override {}

  void SetGain(float gain_db) override {}
  void SetMute(bool muted) override {}
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) override {}
  void SetCompleteGain(float complete_gain_db) override {}

  void SetInitialMinLeadTime(zx::duration initial_min_lead_time) override {}
  void UpdateMinLeadTime(zx::duration new_min_lead_time,
                         zx::time time_of_min_lead_time_change) override {}

  void SetPtsContinuityThreshold(float threshold_seconds) override {}
  void SetPtsUnits(uint32_t numerator, uint32_t denominator) override {}

  void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) override {}
  void RemovePayloadBuffer(uint32_t buffer_id) override {}
  void SendPacket(const fuchsia::media::StreamPacket& packet) override {}

  void PacketQueueUnderflow(zx::time start_time, zx::time end_time) override {}
  void ContinuityUnderflow(zx::time start_time, zx::time end_time) override {}
  void TimestampUnderflow(zx::time start_time, zx::time end_time) override {}
};

class CapturerNop : public Reporter::Capturer {
 public:
  void Destroy() override {}

  void StartSession(zx::time start_time) override {}
  void StopSession(zx::time stop_time) override {}

  void SetUsage(CaptureUsage usage) override {}
  void SetFormat(const Format& format) override {}

  void SetGain(float gain_db) override {}
  void SetMute(bool muted) override {}
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) override {}
  void SetCompleteGain(float complete_gain_db) override {}

  void SetInitialPresentationDelay(zx::duration initial_presentation_delay) override {}
  void UpdatePresentationDelay(zx::duration new_presentation_delay,
                               zx::time time_of_presentation_delay_change) override {}

  void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) override {}
  void SendPacket(const fuchsia::media::StreamPacket& packet) override {}
  void Overflow(zx::time start_time, zx::time end_time) override {}
};

class VolumeControlNop : public Reporter::VolumeControl {
 public:
  void Destroy() override {}

  void SetVolumeMute(float volume, bool mute) override {}
  void AddBinding(std::string name) override {}
};
}  // namespace

////////////////////////////////////////////////////////////////////////////////
// Real implementations of reporting objects.

class Reporter::OverflowUnderflowTracker {
 public:
  // Trackers begin in a "stopped" state and must move to a "started" state
  // before metrics can be reported. The Start/Stop events are intended to
  // mirror higher-level Play/Pause or Record/Stop events. If a session is not
  // stopped explicitly, it's stopped automatically by our destructor.
  void StartSession(zx::time start_time);
  void StopSession(zx::time stop_time);

  // Report an event with the given start and end times.
  void Report(zx::time start_time, zx::time end_time);

  struct Args {
    uint32_t component;
    std::string_view event_name;
    inspect::Node& parent_node;
    Reporter::Impl& impl;
    bool is_underflow;
    uint32_t cobalt_component_id;
  };
  explicit OverflowUnderflowTracker(Args args);
  ~OverflowUnderflowTracker();

 private:
  void RestartSession();
  zx::duration ComputeDurationOfAllSessions();
  void LogCobaltDuration(uint32_t metric_id, std::vector<uint32_t> event_codes, zx::duration d);

  std::mutex mutex_;

  enum class State : uint8_t { Stopped, Started };
  State state_ FXL_GUARDED_BY(mutex_) = State::Stopped;

  // Ideally we'd record final cobalt metrics when the component exits, however we can't
  // be notified of component exit until we've switched to Components v2. In the interim,
  // we automatically restart sessions every hour. Inspect metrics don't have this limitation
  // and can use the "real" session times.
  static constexpr auto kMaxSessionDuration = zx::hour(1);
  async::TaskClosureMethod<OverflowUnderflowTracker, &OverflowUnderflowTracker::RestartSession>
      restart_session_timer_ FXL_GUARDED_BY(mutex_){this};

  zx::time last_event_time_ FXL_GUARDED_BY(mutex_);             // for cobalt
  zx::time session_start_time_ FXL_GUARDED_BY(mutex_);          // for cobalt
  zx::time session_real_start_time_ FXL_GUARDED_BY(mutex_);     // for inspect
  zx::duration past_sessions_duration_ FXL_GUARDED_BY(mutex_);  // for inspect

  inspect::Node node_;
  inspect::UintProperty event_count_;
  inspect::UintProperty event_duration_;
  inspect::UintProperty session_count_;
  inspect::LazyNode total_duration_;

  Reporter::Impl& impl_;
  const uint32_t cobalt_component_id_;
  const uint32_t cobalt_event_duration_metric_id_;
  const uint32_t cobalt_time_since_last_event_or_session_start_metric_id_;
};

class Reporter::ObjectTracker {
 public:
  ObjectTracker(Reporter::Impl& impl,
                AudioObjectsCreatedMigratedMetricDimensionObjectType object_type)
      : impl_(impl), object_type_(object_type) {}

  void SetFormat(const Format& f) { format_ = f; }

  // Marks the object "enabled", which triggers a cobalt metric increment.
  void Enable() {
    // Ignore when cobalt is disabled.
    auto& logger = impl_.metrics_impl;
    if (!logger) {
      return;
    }

    // Log exactly once. Don't bother throttling: if we're creating objects quickly enough
    // to overload cobalt with these RPCs, we'll hit many other problems first.
    if (enabled_ || !format_) {
      return;
    }
    enabled_ = true;

    AudioObjectsCreatedMigratedMetricDimensionSampleFormat sample_format;
    switch (format_->sample_format()) {
      case fuchsia::media::AudioSampleFormat::UNSIGNED_8:
        sample_format = AudioObjectsCreatedMigratedMetricDimensionSampleFormat::Uint8;
        break;
      case fuchsia::media::AudioSampleFormat::SIGNED_16:
        sample_format = AudioObjectsCreatedMigratedMetricDimensionSampleFormat::Int16;
        break;
      case fuchsia::media::AudioSampleFormat::SIGNED_24_IN_32:
        sample_format = AudioObjectsCreatedMigratedMetricDimensionSampleFormat::Int24;
        break;
      case fuchsia::media::AudioSampleFormat::FLOAT:
        sample_format = AudioObjectsCreatedMigratedMetricDimensionSampleFormat::Float32;
        break;
      default:
        sample_format = AudioObjectsCreatedMigratedMetricDimensionSampleFormat::Other;
        break;
    }
    AudioObjectsCreatedMigratedMetricDimensionChannels channels;
    switch (format_->channels()) {
      case 1:
        channels = AudioObjectsCreatedMigratedMetricDimensionChannels::Chan1;
        break;
      case 2:
        channels = AudioObjectsCreatedMigratedMetricDimensionChannels::Chan2;
        break;
      case 4:
        channels = AudioObjectsCreatedMigratedMetricDimensionChannels::Chan4;
        break;
      default:
        channels = AudioObjectsCreatedMigratedMetricDimensionChannels::Other;
        break;
    }
    AudioObjectsCreatedMigratedMetricDimensionFrameRate frame_rate;
    switch (format_->frames_per_second()) {
      case 16000:
        frame_rate = AudioObjectsCreatedMigratedMetricDimensionFrameRate::Rate16000;
        break;
      case 44100:
        frame_rate = AudioObjectsCreatedMigratedMetricDimensionFrameRate::Rate44100;
        break;
      case 48000:
        frame_rate = AudioObjectsCreatedMigratedMetricDimensionFrameRate::Rate48000;
        break;
      case 96000:
        frame_rate = AudioObjectsCreatedMigratedMetricDimensionFrameRate::Rate96000;
        break;
      default:
        frame_rate = AudioObjectsCreatedMigratedMetricDimensionFrameRate::Other;
        break;
    }
    std::vector<fuchsia_metrics::MetricEvent> events = {fuchsia_metrics::MetricEvent{
        kAudioObjectsCreatedMigratedMetricId,
        {object_type_, sample_format, channels, frame_rate},
        fuchsia_metrics::MetricEventPayload::WithCount(1),
    }};
    logger->LogMetricEvents(std::move(events));
  }

 private:
  Reporter::Impl& impl_;
  AudioObjectsCreatedMigratedMetricDimensionObjectType object_type_;
  std::optional<Format> format_;
  bool enabled_ = false;
};

class FormatInfo {
 public:
  FormatInfo(inspect::Node& parent_node, const std::string_view& name)
      : node_(parent_node.CreateChild(name)),
        sample_format_(node_.CreateString(kSampleFormat, kUnknown)),
        channels_(node_.CreateUint(kChannels, 0)),
        frames_per_second_(node_.CreateUint(kFramesPerSecond, 0)) {}

  void Set(const Format& f) {
    switch (f.sample_format()) {
      case fuchsia::media::AudioSampleFormat::UNSIGNED_8:
        sample_format_.Set(kSampleFormatUint8);
        break;
      case fuchsia::media::AudioSampleFormat::SIGNED_16:
        sample_format_.Set(kSampleFormatInt16);
        break;
      case fuchsia::media::AudioSampleFormat::SIGNED_24_IN_32:
        sample_format_.Set(kSampleFormatInt24In32);
        break;
      case fuchsia::media::AudioSampleFormat::FLOAT:
        sample_format_.Set(kSampleFormatFloat32);
        break;
      default:
        sample_format_.Set(kUnknown);
        FX_LOGS(ERROR) << "Unhandled sample stream_type type "
                       << fidl::ToUnderlying(f.sample_format());
        break;
    }
    channels_.Set(f.channels());
    frames_per_second_.Set(f.frames_per_second());
  }

 private:
  inspect::Node node_;
  inspect::StringProperty sample_format_;
  inspect::UintProperty channels_;
  inspect::UintProperty frames_per_second_;
};

class Reporter::DeviceDriverInfo {
 public:
  DeviceDriverInfo(inspect::Node& parent_node, ObjectTracker&& object_tracker)
      : node_(parent_node.CreateChild(kDriver)),
        name_(node_.CreateString(kName, kUnknown)),
        initial_internal_delay_(node_.CreateUint(kInitialInternalDelayNsec, 0)),
        current_internal_delay_(node_.CreateUint(kCurrentInternalDelayNsec, 0)),
        internal_delay_change_time_(zx::time(0)),
        internal_delay_change_time_ns_(
            node_.CreateInt(kInternalDelayChangedAt, internal_delay_change_time_.get())),
        initial_external_delay_(node_.CreateUint(kInitialExternalDelayNsec, 0)),
        current_external_delay_(node_.CreateUint(kCurrentExternalDelayNsec, 0)),
        external_delay_change_time_(zx::time(0)),
        external_delay_change_time_ns_(
            node_.CreateInt(kExternalDelayChangedAt, external_delay_change_time_.get())),
        driver_transfer_bytes_(node_.CreateUint(kDriverTransferBytes, 0)),
        format_(parent_node, kFormat),
        object_tracker_(object_tracker) {}

  void Set(const AudioDriverInfo& d) {
    name_.Set(d.manufacturer_name + ' ' + d.product_name);
    initial_internal_delay_.Set(d.internal_delay.get());
    current_internal_delay_.Set(d.internal_delay.get());
    initial_external_delay_.Set(d.external_delay.get());
    current_external_delay_.Set(d.external_delay.get());
    driver_transfer_bytes_.Set(d.driver_transfer_bytes);
    if (d.format.has_value()) {
      format_.Set(*d.format);
      object_tracker_.SetFormat(*d.format);
      object_tracker_.Enable();
    }
  }

  void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                    std::optional<zx::duration> external_delay) {
    if (time_of_update > internal_delay_change_time_) {
      current_internal_delay_.Set(internal_delay.get());
      SetInternalDelayChangeTime(time_of_update);
    }
    if (external_delay.has_value() && time_of_update > external_delay_change_time_) {
      current_external_delay_.Set(external_delay->get());
      SetExternalDelayChangeTime(time_of_update);
    }
  }

 private:
  // Use separate methods to update these members, to ensure they stay in sync.
  void SetInternalDelayChangeTime(zx::time internal_delay_change_time) {
    internal_delay_change_time_ = internal_delay_change_time;
    internal_delay_change_time_ns_.Set(internal_delay_change_time_.get());
  }
  void SetExternalDelayChangeTime(zx::time external_delay_change_time) {
    external_delay_change_time_ = external_delay_change_time;
    external_delay_change_time_ns_.Set(external_delay_change_time_.get());
  }

  inspect::Node node_;
  inspect::StringProperty name_;
  inspect::UintProperty initial_internal_delay_;
  inspect::UintProperty current_internal_delay_;
  zx::time internal_delay_change_time_;  // Needed because we cannot compare based on the prop.
  inspect::IntProperty internal_delay_change_time_ns_;
  inspect::UintProperty initial_external_delay_;
  inspect::UintProperty current_external_delay_;
  zx::time external_delay_change_time_;  // Needed because we cannot compare based on the prop.
  inspect::IntProperty external_delay_change_time_ns_;
  inspect::UintProperty driver_transfer_bytes_;
  FormatInfo format_;
  ObjectTracker object_tracker_;
};

class DeviceGainInfo {
 public:
  explicit DeviceGainInfo(inspect::Node& parent)
      : node_(parent.CreateChild(kDeviceGain)),
        gain_db_(node_.CreateDouble(kGainDb, 0.0)),
        muted_(node_.CreateBool(kMuted, false)),
        agc_supported_(node_.CreateBool(kAgcSupported, false)),
        agc_enabled_(node_.CreateBool(kAgcEnabled, false)) {}

  void Set(const fuchsia::media::AudioGainInfo& gain_info,
           fuchsia::media::AudioGainValidFlags set_flags) {
    if ((set_flags & fuchsia::media::AudioGainValidFlags::GAIN_VALID) ==
        fuchsia::media::AudioGainValidFlags::GAIN_VALID) {
      gain_db_.Set(gain_info.gain_db);
    }

    if ((set_flags & fuchsia::media::AudioGainValidFlags::MUTE_VALID) ==
        fuchsia::media::AudioGainValidFlags::MUTE_VALID) {
      muted_.Set((gain_info.flags & fuchsia::media::AudioGainInfoFlags::MUTE) ==
                 fuchsia::media::AudioGainInfoFlags::MUTE);
    }

    if ((set_flags & fuchsia::media::AudioGainValidFlags::AGC_VALID) ==
        fuchsia::media::AudioGainValidFlags::AGC_VALID) {
      agc_supported_.Set((gain_info.flags & fuchsia::media::AudioGainInfoFlags::AGC_SUPPORTED) ==
                         fuchsia::media::AudioGainInfoFlags::AGC_SUPPORTED);
      agc_enabled_.Set((gain_info.flags & fuchsia::media::AudioGainInfoFlags::AGC_ENABLED) ==
                       fuchsia::media::AudioGainInfoFlags::AGC_ENABLED);
    }
  }

 private:
  inspect::Node node_;
  inspect::DoubleProperty gain_db_;
  inspect::BoolProperty muted_;
  inspect::BoolProperty agc_supported_;
  inspect::BoolProperty agc_enabled_;
};

class Reporter::ThermalStateTransition {
 public:
  ThermalStateTransition(inspect::Node& parent, const std::string& name, uint32_t state)
      : node_(parent.CreateChild(name)),
        state_(node_.CreateString(kState, state ? std::to_string(state) : kNormal)),
        active_(node_.CreateBool(kActive, true)),
        duration_(node_.CreateLazyValues(
            kThermalStateTransitionDuration,
            [this] {
              std::lock_guard<std::mutex> lock(mutex_);
              inspect::Inspector i;
              i.GetRoot().CreateUint(
                  kDurationNsec,
                  (alive_ ? zx::clock::get_monotonic() - start_time_ : past_duration_).get(), &i);
              return fpromise::make_ok_promise(std::move(i));
            })),
        start_time_(zx::clock::get_monotonic()) {}

  void Destroy() {
    std::lock_guard<std::mutex> lock(mutex_);
    past_duration_ = zx::clock::get_monotonic() - start_time_;
    alive_ = false;
    active_.Set(false);
  }

 private:
  inspect::Node node_;
  inspect::StringProperty state_;
  inspect::BoolProperty active_;
  inspect::LazyNode duration_;

  const zx::time start_time_;

  std::mutex mutex_;
  bool alive_ FXL_GUARDED_BY(mutex_) = true;
  zx::duration past_duration_ FXL_GUARDED_BY(mutex_);
};

class Reporter::ThermalStateTracker {
 public:
  explicit ThermalStateTracker(Reporter::Impl& impl);

  void SetNumThermalStates(size_t num);
  void SetThermalState(uint32_t state);

 private:
  using CobaltStateTransition = AudioThermalStateTransitionsMigratedMetricDimensionStateTransition;
  static constexpr std::array kCobaltStateTransitions = {
      CobaltStateTransition::Normal,
      CobaltStateTransition::State1,
      CobaltStateTransition::State2,
      CobaltStateTransition::State3,
  };
  // Ideally we'd record final cobalt metrics when the component exits, however we can't
  // be notified of component exit until we've switched to Components v2. In the interim,
  // we automatically restart sessions every 5 min.
  static constexpr auto kCobaltDataCollectionInterval = zx::min(5);

  struct State {
    // Total duration this state has been active, not counting
    // the current activation if this is the current state.
    zx::duration past_duration;
    // Set if this is the current thermal state.
    std::optional<zx::time> current_activation_time;
    std::optional<zx::time> interval_start_time;  // Cobalt
    // Running total of transitions to State.
    uint32_t total_transitions;

    inspect::Node node;
    inspect::LazyNode duration;

    void Activate();
    void Deactivate();
  };

  void LogCobaltStateTransition(State& state, CobaltStateTransition event) FXL_REQUIRE(mutex_);
  void LogCobaltStateDuration(State& old_state, State& new_state) FXL_REQUIRE(mutex_);
  void ResetInterval();

  Reporter::Impl& impl_;
  inspect::Node root_;
  inspect::UintProperty num_thermal_states_;

  inspect::Node transitions_node_;
  Container<ThermalStateTransition, kThermalStatesToCache> thermal_state_transitions_;

  std::mutex mutex_;
  uint32_t active_state_ FXL_GUARDED_BY(mutex_);
  std::unordered_map<uint32_t, State> states_ FXL_GUARDED_BY(mutex_);
  async::TaskClosureMethod<ThermalStateTracker, &ThermalStateTracker::ResetInterval>
      restart_interval_timer_ FXL_GUARDED_BY(mutex_){this};

  uint64_t next_thermal_transition_id_ FXL_GUARDED_BY(mutex_) = 0;
  Container<ThermalStateTransition, kThermalStatesToCache>::Ptr last_transition_
      FXL_GUARDED_BY(mutex_);
};

namespace {
std::string UsageBehaviorToString(fuchsia::media::Behavior behavior) {
  switch (behavior) {
    case fuchsia::media::Behavior::NONE:
      return kNone;
    case fuchsia::media::Behavior::DUCK:
      return kDuck;
    case fuchsia::media::Behavior::MUTE:
      return kMute;
    default:
      FX_DCHECK(false) << "Invalid fuchsia::media::Behavior: " << static_cast<int>(behavior);
      return kUnknown;
  }
}
}  // namespace

class Reporter::ActiveUsagePolicy {
 public:
  ActiveUsagePolicy(inspect::Node& parent, const std::string& name,
                    const std::vector<fuchsia::media::Usage2>& active_usages,
                    const AudioAdmin::RendererPolicies& render_usage_behaviors,
                    const AudioAdmin::CapturerPolicies& capture_usage_behaviors)
      : node_(parent.CreateChild(name)), active_(node_.CreateBool(kActive, true)) {
    for (auto& active_usage : active_usages) {
      if (active_usage.is_render_usage()) {
        auto usage = StreamUsage::WithRenderUsage(active_usage.render_usage()).ToString();
        auto behavior = UsageBehaviorToString(
            render_usage_behaviors[static_cast<int>(active_usage.render_usage())]);
        renderer_policies_.emplace(node_.CreateString(usage, behavior));
      } else {
        auto usage = StreamUsage::WithCaptureUsage(active_usage.capture_usage()).ToString();
        auto behavior = UsageBehaviorToString(
            capture_usage_behaviors[static_cast<int>(active_usage.capture_usage())]);
        capturer_policies_.emplace(node_.CreateString(usage, behavior));
      }
    }
  }

  void Destroy() { active_.Set(false); }

 private:
  inspect::Node node_;
  inspect::BoolProperty active_;

  std::queue<inspect::StringProperty> renderer_policies_;
  std::queue<inspect::StringProperty> capturer_policies_;
};

class Reporter::ActiveUsagePolicyTracker {
 public:
  explicit ActiveUsagePolicyTracker(Reporter::Impl& impl)
      : node_(impl.inspector->root().CreateChild(kActiveUsagePolicies)),
        none_gain_(node_.CreateDouble(kNoneGainDb, 0.0)),
        duck_gain_(node_.CreateDouble(kDuckGainDb, 0.0)),
        mute_gain_(node_.CreateDouble(kMuteGainDb, 0.0)),
        last_policy_(active_usage_policies_.New(new ActiveUsagePolicy(
            node_, std::to_string(++next_active_usage_policy_id_),
            std::vector<fuchsia::media::Usage2>(), AudioAdmin::RendererPolicies(),
            AudioAdmin::CapturerPolicies()))) {}

  void SetAudioPolicyBehaviorGain(AudioAdmin::BehaviorGain behavior_gain) {
    none_gain_.Set(behavior_gain.none_gain_db);
    duck_gain_.Set(behavior_gain.duck_gain_db);
    mute_gain_.Set(behavior_gain.mute_gain_db);
  }

  void UpdateActiveUsagePolicy(const std::vector<fuchsia::media::Usage2>& active_usages,
                               const AudioAdmin::RendererPolicies& renderer_policies,
                               const AudioAdmin::CapturerPolicies& capturer_policies) {
    std::lock_guard<std::mutex> lock(mutex_);
    last_policy_ = active_usage_policies_.New(
        new ActiveUsagePolicy(node_, std::to_string(++next_active_usage_policy_id_), active_usages,
                              renderer_policies, capturer_policies));
  }

 private:
  inspect::Node node_;
  inspect::DoubleProperty none_gain_;
  inspect::DoubleProperty duck_gain_;
  inspect::DoubleProperty mute_gain_;

  Container<ActiveUsagePolicy, kActiveUsagePoliciesToCache> active_usage_policies_;

  std::mutex mutex_;
  uint64_t next_active_usage_policy_id_ FXL_GUARDED_BY(mutex_) = 0;
  Container<ActiveUsagePolicy, kActiveUsagePoliciesToCache>::Ptr last_policy_
      FXL_GUARDED_BY(mutex_);
};

class Reporter::VolumeSetting {
 public:
  explicit VolumeSetting(inspect::Node& parent, const std::string& name, float volume, bool muted)
      : node_(parent.CreateChild(name)),
        active_(node_.CreateBool(kActive, true)),
        volume_(node_.CreateDouble(kVolume, volume)),
        muted_(node_.CreateBool(kMuted, muted)) {}

  void Destroy() { active_.Set(false); }

 private:
  inspect::Node node_;
  inspect::BoolProperty active_;
  inspect::DoubleProperty volume_;
  inspect::BoolProperty muted_;
};

class Reporter::VolumeControlImpl : public Reporter::VolumeControl {
 public:
  explicit VolumeControlImpl(Reporter::Impl& impl)
      : node_(impl.volume_controls_node.CreateChild(impl.NextVolumeControlIdStr())),
        volume_settings_node_(node_.CreateChild(kVolumeSettings)),
        name_(node_.CreateString(kName, kUnknownNoClients)),
        client_count_(node_.CreateUint(kClientCount, 0)),
        last_volume_setting_(volume_settings_.New(new VolumeSetting(
            volume_settings_node_, std::to_string(++next_volume_setting_id_), 0.0, false))) {}

  void Destroy() override {}

  void SetVolumeMute(float volume, bool mute) override {
    std::lock_guard<std::mutex> lock(mutex_);
    last_volume_setting_ = volume_settings_.New(new VolumeSetting(
        volume_settings_node_, std::to_string(++next_volume_setting_id_), volume, mute));
  }

  void AddBinding(std::string name) override {
    name_.Set(name);
    client_count_.Add(1);
  }

 private:
  static constexpr size_t kVolumeSettingsToCache = 10;

  inspect::Node node_;
  inspect::Node volume_settings_node_;
  inspect::StringProperty name_;
  inspect::UintProperty client_count_;

  Container<VolumeSetting, kVolumeSettingsToCache> volume_settings_;

  std::mutex mutex_;
  uint64_t next_volume_setting_id_ FXL_GUARDED_BY(mutex_) = 0;
  Container<VolumeSetting, kVolumeSettingsToCache>::Ptr last_volume_setting_ FXL_GUARDED_BY(mutex_);
};

class Reporter::OutputDeviceImpl : public Reporter::OutputDevice {
 public:
  OutputDeviceImpl(Reporter::Impl& impl, const std::string& name, const std::string& thread_name)
      : node_(impl.outputs_node.CreateChild(name)),
        thread_name_(node_.CreateString(kMixerThreadName, thread_name)),
        driver_info_(node_,
                     ObjectTracker(
                         impl, AudioObjectsCreatedMigratedMetricDimensionObjectType::OutputDevice)),
        gain_info_(node_),
        device_underflows_(
            std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
                .event_name = kDeviceUnderflows,
                .parent_node = node_,
                .impl = impl,
                .is_underflow = true,
                .cobalt_component_id =
                    AudioSessionDurationMigratedMetricDimensionComponent::OutputDevice,
            })),
        pipeline_underflows_(
            std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
                .event_name = kPipelineUnderflows,
                .parent_node = node_,
                .impl = impl,
                .is_underflow = true,
                .cobalt_component_id =
                    AudioSessionDurationMigratedMetricDimensionComponent::OutputPipeline,
            })) {
    time_since_death_ = node_.CreateLazyValues(kOutputDeviceTimeSinceDeath, [this] {
      inspect::Inspector i;
      i.GetRoot().CreateUint(
          kTimeSinceDeathNsec,
          time_of_death_ ? (zx::clock::get_monotonic() - time_of_death_.value()).get() : 0, &i);
      return fpromise::make_ok_promise(std::move(i));
    });
  }

  void Destroy() override { time_of_death_ = zx::clock::get_monotonic(); }

  void StartSession(zx::time start_time) override {
    device_underflows_->StartSession(start_time);
    pipeline_underflows_->StartSession(start_time);
  }

  void StopSession(zx::time stop_time) override {
    device_underflows_->StopSession(stop_time);
    pipeline_underflows_->StopSession(stop_time);
  }

  void SetDriverInfo(const AudioDriverInfo& driver) override { driver_info_.Set(driver); }

  void SetGainInfo(const fuchsia::media::AudioGainInfo& gain_info,
                   fuchsia::media::AudioGainValidFlags set_flags) override {
    gain_info_.Set(gain_info, set_flags);
  }

  void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                    std::optional<zx::duration> external_delay) override {
    driver_info_.UpdateDelays(time_of_update, internal_delay, external_delay);
  }

  void DeviceUnderflow(zx::time start_time, zx::time end_time) override {
    device_underflows_->Report(start_time, end_time);
  }

  void PipelineUnderflow(zx::time start_time, zx::time end_time) override {
    pipeline_underflows_->Report(start_time, end_time);
  }

 private:
  inspect::Node node_;
  inspect::LazyNode time_since_death_;
  inspect::StringProperty thread_name_;
  DeviceDriverInfo driver_info_;
  DeviceGainInfo gain_info_;
  std::unique_ptr<OverflowUnderflowTracker> device_underflows_;
  std::unique_ptr<OverflowUnderflowTracker> pipeline_underflows_;
  std::optional<zx::time> time_of_death_;
};

class Reporter::InputDeviceImpl : public Reporter::InputDevice {
 public:
  InputDeviceImpl(Reporter::Impl& impl, const std::string& name, const std::string& thread_name)
      : node_(impl.inputs_node.CreateChild(name)),
        thread_name_(node_.CreateString(kMixerThreadName, thread_name)),
        driver_info_(
            node_,
            ObjectTracker(impl, AudioObjectsCreatedMigratedMetricDimensionObjectType::InputDevice)),
        gain_info_(node_) {
    time_since_death_ = node_.CreateLazyValues(kInputDeviceTimeSinceDeath, [this] {
      inspect::Inspector i;
      i.GetRoot().CreateUint(
          kTimeSinceDeathNsec,
          time_of_death_ ? (zx::clock::get_monotonic() - time_of_death_.value()).get() : 0, &i);
      return fpromise::make_ok_promise(std::move(i));
    });
  }

  void Destroy() override { time_of_death_ = zx::clock::get_monotonic(); }

  void StartSession(zx::time start_time) override {}
  void StopSession(zx::time stop_time) override {}

  void SetDriverInfo(const AudioDriverInfo& driver) override { driver_info_.Set(driver); }

  void SetGainInfo(const fuchsia::media::AudioGainInfo& gain_info,
                   fuchsia::media::AudioGainValidFlags set_flags) override {
    gain_info_.Set(gain_info, set_flags);
  }

  void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                    std::optional<zx::duration> external_delay) override {
    driver_info_.UpdateDelays(time_of_update, internal_delay, external_delay);
  }

 private:
  inspect::Node node_;
  inspect::LazyNode time_since_death_;
  inspect::StringProperty thread_name_;
  DeviceDriverInfo driver_info_;
  DeviceGainInfo gain_info_;
  std::optional<zx::time> time_of_death_;
};

// This groups together stream-specific gain metrics, for all renderer and capturer instances.
class ClientGainInfo {
 public:
  ClientGainInfo(inspect::Node& parent_node, const std::string_view& name)
      : node_(parent_node.CreateChild(name)),
        gain_db_(node_.CreateDouble(kGainDb, 0.0)),
        muted_(node_.CreateBool(kMuted, false)),
        calls_to_set_gain_with_ramp_(node_.CreateUint(kCallsToSetGainWithRamp, 0)),
        complete_gain_db_(node_.CreateDouble(kCompleteStreamGainDb, 0.0)) {}

  void SetGain(float gain_db) { gain_db_.Set(gain_db); }
  void SetMute(bool muted) { muted_.Set(muted); }
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) {
    calls_to_set_gain_with_ramp_.Add(1);
  }
  void SetCompleteGain(float complete_gain_db) { complete_gain_db_.Set(complete_gain_db); }

 private:
  inspect::Node node_;
  inspect::DoubleProperty gain_db_;
  inspect::BoolProperty muted_;
  inspect::UintProperty calls_to_set_gain_with_ramp_;
  inspect::DoubleProperty complete_gain_db_;
};

// This groups together metrics related to presentation timestamps on renderer packets.
class ClientTimingInfo {
 public:
  ClientTimingInfo(inspect::Node& parent_node, const std::string_view& name)
      : node_(parent_node.CreateChild(name)),
        pts_continuity_threshold_seconds_(node_.CreateDouble(kPtsContinuityThresholdSec, 0.0)),
        pts_units_per_second_numerator_(node_.CreateUint(kPtsUnitsNumerator, 1'000'000'000)),
        pts_units_per_second_denominator_(node_.CreateUint(kPtsUnitsDenominator, 1)) {}

  void SetPtsContinuityThreshold(float threshold_seconds) {
    pts_continuity_threshold_seconds_.Set(threshold_seconds);
  }
  void SetPtsUnits(uint32_t numerator, uint32_t denominator) {
    pts_units_per_second_numerator_.Set(numerator);
    pts_units_per_second_denominator_.Set(denominator);
  }

 private:
  inspect::Node node_;
  inspect::UintProperty initial_min_offset_time_ns_;
  zx::time min_offset_time_change_time_;
  inspect::DoubleProperty pts_continuity_threshold_seconds_;
  inspect::UintProperty pts_units_per_second_numerator_;
  inspect::UintProperty pts_units_per_second_denominator_;
};

// This is not a node grouping; it represents metrics that are directly set by client calls.
class Reporter::ClientPort {
 public:
  ClientPort(inspect::Node& node, ObjectTracker&& object_tracker)
      : node_(node),
        object_tracker_(object_tracker),
        format_(node_, kFormat),
        payload_buffers_node_(node_.CreateChild(kPayloadBuffers)),
        gain_info_(node_, kGain) {}

  // This is only relevant for renderers, so we defer construction until called by RendererImpl.
  void SetupPtsInfo() { timing_info_ = ClientTimingInfo(node_, kPresentationTimestamps); }

  void SetFormat(const Format& format) {
    object_tracker_.SetFormat(format);
    format_.Set(format);
  }

  void SetGain(float gain_db) { gain_info_.SetGain(gain_db); }
  void SetMute(bool muted) { gain_info_.SetMute(muted); }
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) {
    gain_info_.SetGainWithRamp(gain_db, ramp_duration, ramp_type);
  }
  void SetCompleteGain(float complete_gain_db) { gain_info_.SetCompleteGain(complete_gain_db); }

  void SetPtsContinuityThreshold(float threshold_seconds) {
    FX_DCHECK(timing_info_.has_value());
    timing_info_->SetPtsContinuityThreshold(threshold_seconds);
  }
  void SetPtsUnits(uint32_t numerator, uint32_t denominator) {
    FX_DCHECK(timing_info_.has_value());
    timing_info_->SetPtsUnits(numerator, denominator);
  }

  void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) {
    std::lock_guard<std::mutex> lock(mutex_);
    payload_buffers_.emplace(
        buffer_id,
        PayloadBuffer(payload_buffers_node_.CreateChild(std::to_string(buffer_id)), size));
  }

  void RemovePayloadBuffer(uint32_t buffer_id) {
    std::lock_guard<std::mutex> lock(mutex_);
    payload_buffers_.erase(buffer_id);
  }

  void SendPacket(const fuchsia::media::StreamPacket& packet) {
    object_tracker_.Enable();
    std::lock_guard<std::mutex> lock(mutex_);
    auto b = payload_buffers_.find(packet.payload_buffer_id);
    if (b == payload_buffers_.end()) {
      FX_LOGS(ERROR) << "Specified payload buffer not found";
      return;
    }
    b->second.packets_.Add(1);
  }

 private:
  inspect::Node& node_;
  ObjectTracker object_tracker_;
  FormatInfo format_;
  inspect::Node payload_buffers_node_;

  struct PayloadBuffer {
    PayloadBuffer(inspect::Node node, uint64_t size)
        : node_(std::move(node)),
          size_(node_.CreateUint(kSize, size)),
          packets_(node_.CreateUint(kPackets, 0)) {}

    inspect::Node node_;
    inspect::UintProperty size_;
    inspect::UintProperty packets_;
  };
  std::mutex mutex_;
  std::unordered_map<uint32_t, PayloadBuffer> payload_buffers_ FXL_GUARDED_BY(mutex_);

  ClientGainInfo gain_info_;
  std::optional<ClientTimingInfo> timing_info_;
};

class Reporter::RendererImpl : public Reporter::Renderer {
 public:
  explicit RendererImpl(Reporter::Impl& impl)
      : node_(impl.renderers_node.CreateChild(impl.NextRendererIdStr())),
        client_port_(
            node_,
            ObjectTracker(impl, AudioObjectsCreatedMigratedMetricDimensionObjectType::Renderer)),
        initial_min_lead_time_ns_(node_.CreateUint(kInitialMinLeadTimeNsec, 0)),
        current_min_lead_time_ns_(node_.CreateUint(kCurrentMinLeadTimeNsec, 0)),
        time_of_min_lead_time_change_(zx::time(0)),
        time_of_min_lead_time_change_ns_(
            node_.CreateInt(kMinLeadTimeChangedAt, time_of_min_lead_time_change_.get())),
        usage_(node_.CreateString(kUsage, kDefault)),
        packet_queue_underflows_(
            std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
                .event_name = kPacketQueueUnderflows,
                .parent_node = node_,
                .impl = impl,
                .is_underflow = true,
                .cobalt_component_id =
                    AudioSessionDurationMigratedMetricDimensionComponent::Renderer,
            })),
        continuity_underflows_(
            std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
                .event_name = kContinuityUnderflows,
                .parent_node = node_,
                .impl = impl,
                .is_underflow = true,
                .cobalt_component_id =
                    AudioSessionDurationMigratedMetricDimensionComponent::Renderer,
            })),
        timestamp_underflows_(
            std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
                .event_name = kTimestampUnderflows,
                .parent_node = node_,
                .impl = impl,
                .is_underflow = true,
                .cobalt_component_id =
                    AudioSessionDurationMigratedMetricDimensionComponent::Renderer,
            })) {
    client_port_.SetupPtsInfo();

    time_since_death_ = node_.CreateLazyValues(kRendererTimeSinceDeath, [this] {
      inspect::Inspector i;
      i.GetRoot().CreateUint(
          kTimeSinceDeathNsec,
          time_of_death_ ? (zx::clock::get_monotonic() - time_of_death_.value()).get() : 0, &i);
      return fpromise::make_ok_promise(std::move(i));
    });
  }

  void Destroy() override { time_of_death_ = zx::clock::get_monotonic(); }

  void StartSession(zx::time start_time) override {
    packet_queue_underflows_->StartSession(start_time);
    continuity_underflows_->StartSession(start_time);
    timestamp_underflows_->StartSession(start_time);
  }
  void StopSession(zx::time stop_time) override {
    packet_queue_underflows_->StopSession(stop_time);
    continuity_underflows_->StopSession(stop_time);
    timestamp_underflows_->StopSession(stop_time);
  }

  void SetUsage(RenderUsage usage) override { usage_.Set(ToString(usage)); }
  void SetFormat(const Format& format) override { client_port_.SetFormat(format); }

  void SetGain(float gain_db) override { client_port_.SetGain(gain_db); }
  void SetMute(bool muted) override { client_port_.SetMute(muted); }
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) override {
    client_port_.SetGainWithRamp(gain_db, ramp_duration, ramp_type);
  }
  void SetCompleteGain(float complete_gain_db) override {
    client_port_.SetCompleteGain(complete_gain_db);
  }

  void SetInitialMinLeadTime(zx::duration initial_min_lead_time) override {
    initial_min_lead_time_ns_.Set(initial_min_lead_time.to_nsecs());
    current_min_lead_time_ns_.Set(initial_min_lead_time.to_nsecs());
    SetTimeOfMinLeadTimeChange(zx::time(0));
  }
  void UpdateMinLeadTime(zx::duration new_min_lead_time,
                         zx::time time_of_min_lead_time_change) override {
    if (time_of_min_lead_time_change > time_of_min_lead_time_change_) {
      current_min_lead_time_ns_.Set(new_min_lead_time.to_nsecs());
      SetTimeOfMinLeadTimeChange(time_of_min_lead_time_change);
    }
  }

  void SetPtsContinuityThreshold(float threshold_seconds) override {
    client_port_.SetPtsContinuityThreshold(threshold_seconds);
  }
  void SetPtsUnits(uint32_t numerator, uint32_t denominator) override {
    client_port_.SetPtsUnits(numerator, denominator);
  }

  void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) override {
    client_port_.AddPayloadBuffer(buffer_id, size);
  }
  void RemovePayloadBuffer(uint32_t buffer_id) override {
    client_port_.RemovePayloadBuffer(buffer_id);
  }
  void SendPacket(const fuchsia::media::StreamPacket& packet) override {
    client_port_.SendPacket(packet);
  }

  void PacketQueueUnderflow(zx::time start_time, zx::time end_time) override {
    packet_queue_underflows_->Report(start_time, end_time);
  }
  void ContinuityUnderflow(zx::time start_time, zx::time end_time) override {
    continuity_underflows_->Report(start_time, end_time);
  }
  void TimestampUnderflow(zx::time start_time, zx::time end_time) override {
    timestamp_underflows_->Report(start_time, end_time);
  }

 private:
  // Use a separate method to update these members, to ensure they stay in sync.
  void SetTimeOfMinLeadTimeChange(zx::time time_of_min_lead_time_change) {
    time_of_min_lead_time_change_ = time_of_min_lead_time_change;
    time_of_min_lead_time_change_ns_.Set(time_of_min_lead_time_change_.get());
  }

  inspect::Node node_;
  ClientPort client_port_;
  inspect::LazyNode time_since_death_;
  inspect::UintProperty initial_min_lead_time_ns_;
  inspect::UintProperty current_min_lead_time_ns_;
  zx::time time_of_min_lead_time_change_;  // Needed because we cannot compare based on the prop.
  inspect::IntProperty time_of_min_lead_time_change_ns_;
  inspect::StringProperty usage_;
  std::unique_ptr<OverflowUnderflowTracker> packet_queue_underflows_;
  std::unique_ptr<OverflowUnderflowTracker> continuity_underflows_;
  std::unique_ptr<OverflowUnderflowTracker> timestamp_underflows_;
  std::optional<zx::time> time_of_death_;
};

class Reporter::CapturerImpl : public Reporter::Capturer {
 public:
  CapturerImpl(Reporter::Impl& impl, const std::string& thread_name)
      : node_(impl.capturers_node.CreateChild(impl.NextCapturerIdStr())),
        client_port_(
            node_,
            ObjectTracker(impl, AudioObjectsCreatedMigratedMetricDimensionObjectType::Capturer)),
        initial_presentation_delay_ns_(node_.CreateUint(kInitialPresentationDelayNsec, 0)),
        current_presentation_delay_ns_(node_.CreateUint(kCurrentPresentationDelayNsec, 0)),
        time_of_presentation_delay_change_(zx::time(0)),
        time_of_presentation_delay_change_ns_(node_.CreateInt(kPresentationDelayChangedAt, 0)),
        usage_(node_.CreateString(kUsage, kDefault)),
        thread_name_(node_.CreateString(kMixerThreadName, thread_name)),
        overflows_(std::make_unique<OverflowUnderflowTracker>(OverflowUnderflowTracker::Args{
            .event_name = kCaptureOverflows,
            .parent_node = node_,
            .impl = impl,
            .is_underflow = false,
            .cobalt_component_id = AudioSessionDurationMigratedMetricDimensionComponent::Capturer,
        })) {
    time_since_death_ = node_.CreateLazyValues(kCapturerTimeSinceDeath, [this] {
      inspect::Inspector i;
      i.GetRoot().CreateUint(
          kTimeSinceDeathNsec,
          time_of_death_ ? (zx::clock::get_monotonic() - time_of_death_.value()).get() : 0, &i);
      return fpromise::make_ok_promise(std::move(i));
    });
  }

  void Destroy() override { time_of_death_ = zx::clock::get_monotonic(); }

  void StartSession(zx::time start_time) override { overflows_->StartSession(start_time); }
  void StopSession(zx::time stop_time) override { overflows_->StopSession(stop_time); }

  void SetUsage(CaptureUsage usage) override { usage_.Set(ToString(usage)); }
  void SetFormat(const Format& format) override { client_port_.SetFormat(format); }

  void SetGain(float gain_db) override { client_port_.SetGain(gain_db); }
  void SetMute(bool muted) override { client_port_.SetMute(muted); }
  void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                       fuchsia::media::audio::RampType ramp_type) override {
    client_port_.SetGainWithRamp(gain_db, ramp_duration, ramp_type);
  }
  void SetCompleteGain(float complete_gain_db) override {
    client_port_.SetCompleteGain(complete_gain_db);
  }

  void SetInitialPresentationDelay(zx::duration initial_presentation_delay) override {
    initial_presentation_delay_ns_.Set(initial_presentation_delay.to_nsecs());
    current_presentation_delay_ns_.Set(initial_presentation_delay.to_nsecs());
    SetTimeOfPresentationDelayChange(zx::time(0));
  }
  void UpdatePresentationDelay(zx::duration new_presentation_delay,
                               zx::time time_of_presentation_delay_change) override {
    if (time_of_presentation_delay_change > time_of_presentation_delay_change_) {
      current_presentation_delay_ns_.Set(new_presentation_delay.to_nsecs());
      SetTimeOfPresentationDelayChange(time_of_presentation_delay_change);
    }
  }

  void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) override {
    client_port_.AddPayloadBuffer(buffer_id, size);
  }
  void SendPacket(const fuchsia::media::StreamPacket& packet) override {
    client_port_.SendPacket(packet);
  }
  void Overflow(zx::time start_time, zx::time end_time) override {
    overflows_->Report(start_time, end_time);
  }

 private:
  // Use a separate method to update these members, to ensure they stay in sync.
  void SetTimeOfPresentationDelayChange(zx::time time_of_presentation_delay_change) {
    time_of_presentation_delay_change_ = time_of_presentation_delay_change;
    time_of_presentation_delay_change_ns_.Set(time_of_presentation_delay_change_.get());
  }

  inspect::Node node_;
  ClientPort client_port_;
  inspect::LazyNode time_since_death_;
  inspect::UintProperty initial_presentation_delay_ns_;
  inspect::UintProperty current_presentation_delay_ns_;
  zx::time time_of_presentation_delay_change_;  // Needed: we cannot compare based on the prop.
  inspect::IntProperty time_of_presentation_delay_change_ns_;
  inspect::StringProperty usage_;
  inspect::StringProperty thread_name_;
  std::unique_ptr<OverflowUnderflowTracker> overflows_;
  std::optional<zx::time> time_of_death_;
};

////////////////////////////////////////////////////////////////////////////////
// Reporter implementation

// static
Reporter& Reporter::Singleton() {
  std::lock_guard<std::mutex> lock(singleton_mutex);
  if (singleton_real) {
    return *singleton_real;
  }
  FX_LOGS_FIRST_N(INFO, 1)
      << "Creating reporting objects before the Reporter singleton has been initialized";
  return *singleton_nop;
}

// static
void Reporter::InitializeSingleton(sys::ComponentContext& component_context,
                                   async_dispatcher_t* fidl_dispatcher,
                                   async_dispatcher_t* io_dispatcher, bool enable_cobalt) {
  std::lock_guard<std::mutex> lock(singleton_mutex);
  if (singleton_real) {
    FX_LOGS(ERROR) << "Reporter::Singleton double initialized";
    return;
  }
  singleton_real = new Reporter(component_context, fidl_dispatcher, io_dispatcher, enable_cobalt);
}

Reporter::Reporter(sys::ComponentContext& component_context, async_dispatcher_t* fidl_dispatcher,
                   async_dispatcher_t* io_dispatcher, bool enable_cobalt)
    : impl_(std::make_unique<Impl>(component_context, fidl_dispatcher, io_dispatcher)) {
  // This lock isn't necessary, but the lock analysis can't tell that.
  std::lock_guard<std::mutex> lock(mutex_);
  InitInspect();
  if (enable_cobalt) {
    InitCobalt();
  }
}

void Reporter::InitInspect() {
  impl_->inspector = std::make_unique<inspect::ComponentInspector>(impl_->fidl_dispatcher,
                                                                   inspect::PublishOptions{});
  inspect::Node& root_node = impl_->inspector->root();

  impl_->failed_to_connect_to_device_count = root_node.CreateUint(kConnectToDeviceFailureCount, 0);
  impl_->failed_to_obtain_stream_channel_count =
      root_node.CreateUint(kObtainDeviceStreamChannelFailureCount, 0);
  impl_->failed_to_start_device_count = root_node.CreateUint(kStartDeviceFailureCount, 0);

  impl_->failed_to_apply_scheduler_profile_count =
      root_node.CreateUint(kApplySchedulerProfileFailureCount, 0);
  impl_->failed_to_apply_memory_profile_count =
      root_node.CreateUint(kApplyMemoryProfileFailureCount, 0);

  impl_->mixer_clock_skew_discontinuities =
      root_node.CreateLinearIntHistogram(kMixerClockSkewDiscontinuitiesNsec,
                                         ZX_MSEC(-100),  // floor
                                         ZX_MSEC(2),     // step size
                                         100);           // buckets range from -100ms to +100ms

  impl_->outputs_node = root_node.CreateChild(kOutputDevices);
  impl_->inputs_node = root_node.CreateChild(kInputDevices);
  impl_->renderers_node = root_node.CreateChild(kRenderers);
  impl_->capturers_node = root_node.CreateChild(kCapturers);
  impl_->volume_controls_node = root_node.CreateChild(kVolumeControls);

  impl_->thermal_state_tracker = std::make_unique<ThermalStateTracker>(*impl_);
  impl_->active_usage_policy_tracker = std::make_unique<ActiveUsagePolicyTracker>(*impl_);
}

void Reporter::InitCobalt() {
  impl_->metrics_impl = std::make_unique<media::audio::MetricsImpl>(
      impl_->fidl_dispatcher,
      fidl::ClientEnd<fuchsia_io::Directory>(component::OpenServiceRoot()->TakeChannel()),
      kProjectId);
}

Reporter::Container<Reporter::OutputDevice, Reporter::kObjectsToCache>::Ptr
Reporter::CreateOutputDevice(const std::string& name, const std::string& thread_name) {
  std::lock_guard<std::mutex> lock(mutex_);
  return outputs_.New(
      impl_ ? static_cast<OutputDevice*>(new OutputDeviceImpl(*impl_, name, thread_name))
            : static_cast<OutputDevice*>(new OutputDeviceNop));
}

Reporter::Container<Reporter::InputDevice, Reporter::kObjectsToCache>::Ptr
Reporter::CreateInputDevice(const std::string& name, const std::string& thread_name) {
  std::lock_guard<std::mutex> lock(mutex_);
  return inputs_.New(impl_
                         ? static_cast<InputDevice*>(new InputDeviceImpl(*impl_, name, thread_name))
                         : static_cast<InputDevice*>(new InputDeviceNop));
}

Reporter::Container<Reporter::Renderer, Reporter::kObjectsToCache>::Ptr Reporter::CreateRenderer() {
  std::lock_guard<std::mutex> lock(mutex_);
  return renderers_.New(impl_ ? static_cast<Renderer*>(new RendererImpl(*impl_))
                              : static_cast<Renderer*>(new RendererNop));
}

Reporter::Container<Reporter::Capturer, Reporter::kObjectsToCache>::Ptr Reporter::CreateCapturer(
    const std::string& thread_name) {
  std::lock_guard<std::mutex> lock(mutex_);
  return capturers_.New(impl_ ? static_cast<Capturer*>(new CapturerImpl(*impl_, thread_name))
                              : static_cast<Capturer*>(new CapturerNop));
}

Reporter::Container<Reporter::VolumeControl, Reporter::kVolumeControlsToCache>::Ptr
Reporter::CreateVolumeControl() {
  std::lock_guard<std::mutex> lock(mutex_);
  return volume_controls_.New(impl_ ? static_cast<VolumeControl*>(new VolumeControlImpl(*impl_))
                                    : static_cast<VolumeControl*>(new VolumeControlNop()));
}

void Reporter::FailedToConnectToDevice(const std::string& name, bool is_input, zx_status_t status) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->failed_to_connect_to_device_count.Add(1);
}

void Reporter::FailedToObtainStreamChannel(const std::string& name, bool is_input,
                                           zx_status_t status) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->failed_to_obtain_stream_channel_count.Add(1);
}

void Reporter::FailedToStartDevice(const std::string& name) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->failed_to_start_device_count.Add(1);
}

void Reporter::FailedToApplySchedulerProfile(const std::string& profile, zx_status_t status) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->failed_to_apply_scheduler_profile_count.Add(1);
}

void Reporter::FailedToApplyMemoryProfile(const std::string& profile, zx_status_t status) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->failed_to_apply_memory_profile_count.Add(1);
}

void Reporter::MixerClockSkewDiscontinuity(zx::duration clock_error) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->mixer_clock_skew_discontinuities.Insert(clock_error.get());
}

void Reporter::SetNumThermalStates(size_t num) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->thermal_state_tracker->SetNumThermalStates(num);
}

void Reporter::SetThermalState(uint32_t state) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->thermal_state_tracker->SetThermalState(state);
}

void Reporter::SetAudioPolicyBehaviorGain(AudioAdmin::BehaviorGain behavior_gain) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->active_usage_policy_tracker->SetAudioPolicyBehaviorGain(behavior_gain);
}

void Reporter::UpdateActiveUsagePolicy(const std::vector<fuchsia::media::Usage2>& active_usages,
                                       const AudioAdmin::RendererPolicies& renderer_policies,
                                       const AudioAdmin::CapturerPolicies& capturer_policies) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!impl_) {
    return;
  }
  impl_->active_usage_policy_tracker->UpdateActiveUsagePolicy(active_usages, renderer_policies,
                                                              capturer_policies);
}

Reporter::Impl::Impl(sys::ComponentContext& cc, async_dispatcher_t* fidl_dispatcher,
                     async_dispatcher_t* io_dispatcher)
    : component_context(cc), fidl_dispatcher(fidl_dispatcher), io_dispatcher(io_dispatcher) {}

Reporter::Impl::~Impl() {}

//////////////////////////////////////////////////////////////////////////////////
// OverflowUnderflowTracker implementation

namespace {
constexpr auto kEventSessionStart = 0;
constexpr auto kEventOverflowUnderflow = 1;
static_assert(
    AudioTimeSinceLastOverflowOrSessionStartMigratedMetricDimensionLastEvent::SessionStart == 0);
static_assert(
    AudioTimeSinceLastUnderflowOrSessionStartMigratedMetricDimensionLastEvent::SessionStart == 0);
static_assert(AudioTimeSinceLastOverflowOrSessionStartMigratedMetricDimensionLastEvent::Overflow ==
              1);
static_assert(
    AudioTimeSinceLastUnderflowOrSessionStartMigratedMetricDimensionLastEvent::Underflow == 1);
}  // namespace

Reporter::OverflowUnderflowTracker::OverflowUnderflowTracker(Args args)
    : impl_(args.impl),
      cobalt_component_id_(args.cobalt_component_id),
      cobalt_event_duration_metric_id_(args.is_underflow ? kAudioUnderflowDurationMigratedMetricId
                                                         : kAudioOverflowDurationMigratedMetricId),
      cobalt_time_since_last_event_or_session_start_metric_id_(
          args.is_underflow ? kAudioTimeSinceLastUnderflowOrSessionStartMigratedMetricId
                            : kAudioTimeSinceLastOverflowOrSessionStartMigratedMetricId) {
  node_ = args.parent_node.CreateChild(args.event_name);
  event_count_ = node_.CreateUint(kCount, 0);
  event_duration_ = node_.CreateUint(kDurationNsec, 0);
  session_count_ = node_.CreateUint(kSessionCount, 0);
  total_duration_ = node_.CreateLazyValues(kAllSessionsDuration, [this] {
    inspect::Inspector i;
    i.GetRoot().CreateUint(kTotalDurationOfAllParentSessionsNsec,
                           ComputeDurationOfAllSessions().get(), &i);
    return fpromise::make_ok_promise(std::move(i));
  });
}

Reporter::OverflowUnderflowTracker::~OverflowUnderflowTracker() {
  bool started;
  {
    // Technically a lock is not required here, but the lock analysis can't tell that.
    std::lock_guard<std::mutex> lock(mutex_);
    started = (state_ == State::Started);
  }
  if (started) {
    StopSession(zx::clock::get_monotonic());
  }
}

void Reporter::OverflowUnderflowTracker::StartSession(zx::time start_time) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (state_ == State::Started) {
    FX_LOGS(ERROR) << "StartSession called on session that is already started";
    return;
  }

  session_count_.Add(1);

  state_ = State::Started;
  last_event_time_ = start_time;
  session_start_time_ = start_time;
  session_real_start_time_ = start_time;
  restart_session_timer_.PostDelayed(impl_.io_dispatcher, kMaxSessionDuration);
}

void Reporter::OverflowUnderflowTracker::StopSession(zx::time stop_time) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (state_ == State::Stopped) {
    FX_LOGS(ERROR) << "StopSession called on session that is already stopped";
    return;
  }

  LogCobaltDuration(kAudioSessionDurationMigratedMetricId, {cobalt_component_id_},
                    stop_time - session_start_time_);
  LogCobaltDuration(cobalt_time_since_last_event_or_session_start_metric_id_,
                    {cobalt_component_id_, kEventSessionStart}, stop_time - last_event_time_);

  state_ = State::Stopped;
  past_sessions_duration_ += stop_time - session_real_start_time_;
  restart_session_timer_.Cancel();
}

void Reporter::OverflowUnderflowTracker::RestartSession() {
  auto stop_time = zx::clock::get_monotonic();

  std::lock_guard<std::mutex> lock(mutex_);
  if (state_ == State::Stopped) {
    return;  // must have been stopped concurrently
  }

  LogCobaltDuration(kAudioSessionDurationMigratedMetricId, {cobalt_component_id_},
                    stop_time - session_start_time_);
  LogCobaltDuration(cobalt_time_since_last_event_or_session_start_metric_id_,
                    {cobalt_component_id_, kEventSessionStart}, stop_time - last_event_time_);

  last_event_time_ = stop_time;
  session_start_time_ = stop_time;
  restart_session_timer_.PostDelayed(impl_.io_dispatcher, kMaxSessionDuration);
}

zx::duration Reporter::OverflowUnderflowTracker::ComputeDurationOfAllSessions() {
  std::lock_guard<std::mutex> lock(mutex_);
  auto dt = past_sessions_duration_;
  if (state_ == State::Started) {
    dt += zx::clock::get_monotonic() - session_real_start_time_;
  }
  return dt;
}

void Reporter::OverflowUnderflowTracker::Report(zx::time start_time, zx::time end_time) {
  if (end_time < start_time) {
    FX_LOGS(ERROR) << "Reported overflow/underflow with negative duration: " << start_time.get()
                   << " to " << end_time.get();
  }

  std::lock_guard<std::mutex> lock(mutex_);

  auto event_duration = end_time - start_time;
  event_count_.Add(1);
  event_duration_.Add(event_duration.get());

  LogCobaltDuration(cobalt_event_duration_metric_id_, {cobalt_component_id_}, event_duration);

  if (state_ != State::Started) {
    // This can happen because reporting can race with session boundaries. If we detect an underflow
    // as the client concurrently pauses the renderer, the Report and StopSession calls will race.
    // Also, data discontinuities can occur before a stream starts: when a client submits
    // timestamped packets before starting a renderer, an overlap in packet timestamp ranges leads
    // to data loss ("timestamp underflow"). In that case, Report is called before StartSession.
    //
    // We still track all overflows/underflows, even if the session is not currently running. We
    // mention this possibility only because it may cause the total time of overflow/underflow to
    // seem large, compared to the "total duration of all parent sessions" field that essentially
    // tracks the lifetimes of renderers/capturers/outputs/inputs.
    FX_LOGS_FIRST_N(INFO, 20)
        << "Overflow/Underflow event arrived when the session is stopped: start "
        << start_time.get() << ", end " << end_time.get();
    return;
  }

  LogCobaltDuration(cobalt_time_since_last_event_or_session_start_metric_id_,
                    {cobalt_component_id_, kEventOverflowUnderflow}, start_time - last_event_time_);
  last_event_time_ = end_time;
}

void Reporter::OverflowUnderflowTracker::LogCobaltDuration(uint32_t metric_id,
                                                           std::vector<uint32_t> event_codes,
                                                           zx::duration d) {
  auto& logger = impl_.metrics_impl;
  if (!logger || !cobalt_token_bucket->Acquire()) {
    return;
  }

  std::vector<fuchsia_metrics::MetricEvent> events = {fuchsia_metrics::MetricEvent{
      metric_id,
      std::move(event_codes),
      fuchsia_metrics::MetricEventPayload::WithIntegerValue(d.to_nsecs()),
  }};
  logger->LogMetricEvents(std::move(events));
}

//////////////////////////////////////////////////////////////////////////////////
// ThermalStateTracker implementation

Reporter::ThermalStateTracker::ThermalStateTracker(Reporter::Impl& impl)
    : impl_(impl),
      root_(impl.inspector->root().CreateChild(kThermalState)),
      num_thermal_states_(root_.CreateUint(kThermalStateCount, 1)),
      transitions_node_(impl.inspector->root().CreateChild(kThermalStateTransitions)),
      active_state_(0),
      last_transition_(thermal_state_transitions_.New(new ThermalStateTransition(
          transitions_node_, std::to_string(++next_thermal_transition_id_), active_state_))) {
  if (active_state_ >= kCobaltStateTransitions.size()) {
    FX_LOGS(ERROR) << "active_state_ is out of range: kCobaltStateTransitions must be updated.";
  }
  states_[active_state_] = State({
      .node = root_.CreateChild(kNormal),
  });
  states_[active_state_].Activate();
  LogCobaltStateTransition(states_[active_state_], kCobaltStateTransitions[active_state_]);
  LogCobaltStateDuration(states_[active_state_], states_[active_state_]);
}

void Reporter::ThermalStateTracker::SetNumThermalStates(size_t num) {
  num_thermal_states_.Set(num);
}

void Reporter::ThermalStateTracker::SetThermalState(uint32_t state) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (active_state_ == state) {
    return;
  }
  if (state >= kCobaltStateTransitions.size()) {
    FX_LOGS(ERROR) << "new state is out of range: kCobaltStateTransitions must be updated.";
  }
  states_[active_state_].Deactivate();

  if (!states_.contains(state)) {
    states_[state] = State({
        .node = root_.CreateChild(state ? std::to_string(state) : kNormal),
    });
  }
  states_[state].Activate();
  last_transition_ = thermal_state_transitions_.New(new ThermalStateTransition(
      transitions_node_, std::to_string(++next_thermal_transition_id_), state));
  LogCobaltStateTransition(states_[state], kCobaltStateTransitions[state]);
  LogCobaltStateDuration(states_[active_state_], states_[state]);

  // Set |active_state_| after all activation steps are complete.
  active_state_ = state;
}

void Reporter::ThermalStateTracker::State::Activate() {
  current_activation_time = zx::clock::get_monotonic();

  // If state has never been activated, set duration LazyNode.
  if (!past_duration.get()) {
    duration = node.CreateLazyValues(kTotalThermalStateDuration, [this] {
      inspect::Inspector i;
      i.GetRoot().CreateUint(kTotalDurationNsec,
                             (current_activation_time ? past_duration + zx::clock::get_monotonic() -
                                                            current_activation_time.value()
                                                      : past_duration)
                                 .get(),
                             &i);
      return fpromise::make_ok_promise(std::move(i));
    });
  }
}

void Reporter::ThermalStateTracker::State::Deactivate() {
  past_duration += (zx::clock::get_monotonic() - current_activation_time.value());
  current_activation_time = std::nullopt;
}

////////////////////////////////////////////////////////////////////////////////////////////////
// Thermal State Cobalt Metrics
//
// `LogCobaltStateTransition`
// A per-device histogram is generated for each thermal state. Transitions into |state| are
// counted by marking "1" in each bucket up to N total transitions since boot.
//
// For example, if a device has N transitions to state 2 since boot, the state 2 histogram
// will be:
//
//     value: [1] ... [1]  [0]  [0] ...
//    bucket:  0  ...  N   N+1  N+2 ...
//
// For each thermal state, Cobalt will aggregate the per-device histograms into one for all
// devices, such that each bucket counts the number of devices that have reached that bucket's
// thermal state count. Post-processing in the custom dashboard will reduce these histograms to
// give an accurate representation of the total number of devices that have reached each thermal
// state transition count.
//
// `LogCobaltStateDuration`
// In addition, the total time spent in |old_state| is recorded (in ns).
////////////////////////////////////////////////////////////////////////////////////////////////
void Reporter::ThermalStateTracker::LogCobaltStateTransition(State& state,
                                                             CobaltStateTransition event) {
  auto& logger = impl_.metrics_impl;
  if (!logger) {
    return;
  }

  if (auto transition_count = ++state.total_transitions; transition_count > 50) {
    FX_LOGS_FIRST_N(INFO, 10)
        << "Cobalt logging of audio_thermal_state_transitions has exceeded maximum of 50: " << event
        << " = " << transition_count << "transitions";
  } else if (cobalt_token_bucket->Acquire()) {
    std::vector<fuchsia_metrics::HistogramBucket> histogram{
        fuchsia_metrics::HistogramBucket(transition_count, 1),
    };

    logger->LogIntegerHistogram(kAudioThermalStateTransitionsMigratedMetricId, histogram, {event});
  }
}

void Reporter::ThermalStateTracker::LogCobaltStateDuration(State& old_state, State& new_state) {
  auto& logger = impl_.metrics_impl;
  if (!logger) {
    return;
  }

  auto now = zx::clock::get_monotonic();

  // Record duration of |old_state|, if interval has started.
  if (auto start_time = old_state.interval_start_time;
      start_time && cobalt_token_bucket->Acquire()) {
    std::vector<fuchsia_metrics::MetricEvent> events = {fuchsia_metrics::MetricEvent{
        kAudioThermalStateDurationMigratedMetricId,
        {active_state_},
        fuchsia_metrics::MetricEventPayload::WithIntegerValue(
            (now - start_time.value()).to_nsecs()),
    }};
    logger->LogMetricEvents(std::move(events));
  }

  // End |old_state| and start |new_state| logging interval.
  old_state.interval_start_time = std::nullopt;
  new_state.interval_start_time = now;
  restart_interval_timer_.PostDelayed(impl_.io_dispatcher, kCobaltDataCollectionInterval);
}

void Reporter::ThermalStateTracker::ResetInterval() {
  std::lock_guard<std::mutex> lock(mutex_);
  LogCobaltStateDuration(states_[active_state_], states_[active_state_]);
}

}  // namespace media::audio

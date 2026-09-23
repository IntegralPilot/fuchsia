// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_REPORTER_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_REPORTER_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/zx/time.h>

#include <memory>
#include <mutex>
#include <optional>
#include <queue>
#include <set>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/media/audio/audio_core/audio_admin.h"
#include "src/media/audio/audio_core/metrics/metrics_impl.h"
#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/lib/format/format.h"

namespace media::audio {

////////////////////////////////////////////////////////////////////////////////
// string_view keys

// Top-level Inspect values (cannot be grouped with a specific device or client instance)
constexpr std::string_view kConnectToDeviceFailureCount = "count of failures to connect to device";
constexpr std::string_view kObtainDeviceStreamChannelFailureCount =
    "count of failures to obtain device stream channel";
constexpr std::string_view kStartDeviceFailureCount = "count of failures to start a device";
constexpr std::string_view kApplySchedulerProfileFailureCount =
    "count of failures to apply a Scheduler Profile";
constexpr std::string_view kApplyMemoryProfileFailureCount =
    "count of failures to apply a Memory Profile";

// Top-level Inspect groups
constexpr std::string_view kCapturers = "capturers";
constexpr std::string_view kRenderers = "renderers";
constexpr std::string_view kInputDevices = "input devices";
constexpr std::string_view kOutputDevices = "output devices";
constexpr std::string_view kVolumeControls = "volume controls";
constexpr std::string_view kActiveUsagePolicies = "active usage policies";
constexpr std::string_view kThermalState = "thermal state";

// Device keys
constexpr std::string_view kDriver = "driver";
constexpr std::string_view kDeviceGain = "device gain";
constexpr std::string_view kAgcEnabled = "agc enabled";
constexpr std::string_view kAgcSupported = "agc supported";
constexpr std::string_view kDriverTransferBytes = "driver transfer (bytes)";
constexpr std::string_view kInitialInternalDelayNsec = "initial internal delay (ns)";
constexpr std::string_view kInitialExternalDelayNsec = "initial external delay (ns)";
constexpr std::string_view kCurrentInternalDelayNsec = "current internal delay (ns)";
constexpr std::string_view kCurrentExternalDelayNsec = "current external delay (ns)";
constexpr std::string_view kInternalDelayChangedAt = "time of latest internal delay change";
constexpr std::string_view kExternalDelayChangedAt = "time of latest external delay change";

// Client (renderer/capturer) keys
constexpr std::string_view kUsage = "usage";
constexpr std::string_view kPackets = "packets";
constexpr std::string_view kPayloadBuffers = "payload buffers";
constexpr std::string_view kInitialMinLeadTimeNsec = "initial min lead time (ns)";
constexpr std::string_view kCurrentMinLeadTimeNsec = "current min lead time (ns)";
constexpr std::string_view kMinLeadTimeChangedAt = "time of latest min lead time change";
constexpr std::string_view kInitialPresentationDelayNsec = "initial presentation delay (ns)";
constexpr std::string_view kCurrentPresentationDelayNsec = "current presentation delay (ns)";
constexpr std::string_view kPresentationDelayChangedAt = "time of latest presentation delay change";
constexpr std::string_view kPresentationTimestamps = "presentation timestamps";
constexpr std::string_view kPtsUnitsNumerator = "pts units numerator";
constexpr std::string_view kPtsUnitsDenominator = "pts units denominator";
constexpr std::string_view kPtsContinuityThresholdSec = "pts continuity threshold (s)";

// Volume/gain keys
constexpr std::string_view kGain = "gain";
constexpr std::string_view kVolume = "volume";
constexpr std::string_view kGainDb = "gain db";
constexpr std::string_view kClientCount = "client count";
constexpr std::string_view kVolumeSettings = "volume settings";
constexpr std::string_view kCallsToSetGainWithRamp = "calls to SetGainWithRamp";
constexpr std::string_view kCompleteStreamGainDb = "complete stream gain (post-volume) dbfs";

// Policy keys
constexpr std::string_view kNoneGainDb = "none gain db";
constexpr std::string_view kDuckGainDb = "duck gain db";
constexpr std::string_view kMuteGainDb = "mute gain db";

// Thermal keys
constexpr std::string_view kThermalStateCount = "num thermal states";
constexpr std::string_view kThermalStateTransitions = "thermal state transitions";

// Discontinuity (underflow/overflow) keys
constexpr std::string_view kMixerThreadName = "mixer thread name";
constexpr std::string_view kMixerClockSkewDiscontinuitiesNsec =
    "mixer clock skew discontinuities (error in ns)";
constexpr std::string_view kDeviceUnderflows = "device underflows";
constexpr std::string_view kPipelineUnderflows = "pipeline underflows";
constexpr std::string_view kPacketQueueUnderflows = "packet queue underflows";
constexpr std::string_view kContinuityUnderflows = "continuity underflows";
constexpr std::string_view kTimestampUnderflows = "timestamp underflows";
constexpr std::string_view kCaptureOverflows = "overflows";
constexpr std::string_view kSessionCount = "session count";
constexpr std::string_view kTotalDurationOfAllParentSessionsNsec =
    "total duration of all parent sessions (ns)";

// Shared keys
constexpr std::string_view kSize = "size";
constexpr std::string_view kName = "name";
constexpr std::string_view kCount = "count";
constexpr std::string_view kMuted = "muted";
constexpr std::string_view kState = "state";
constexpr std::string_view kActive = "active";
constexpr std::string_view kFormat = "format";
constexpr std::string_view kChannels = "channels";
constexpr std::string_view kSampleFormat = "sample format";
constexpr std::string_view kFramesPerSecond = "frames per second";
constexpr std::string_view kDurationNsec = "duration (ns)";
constexpr std::string_view kTotalDurationNsec = "total duration (ns)";
constexpr std::string_view kTimeSinceDeathNsec = "time since death (ns)";

// wrapper identifiers for closures - not visible in inspect UI
constexpr std::string_view kThermalStateTransitionDuration = "ThermalStateTransitionDuration";
constexpr std::string_view kTotalThermalStateDuration = "TotalThermalStateDuration";
constexpr std::string_view kOutputDeviceTimeSinceDeath = "OutputDeviceTimeSinceDeath";
constexpr std::string_view kInputDeviceTimeSinceDeath = "InputDeviceTimeSinceDeath";
constexpr std::string_view kRendererTimeSinceDeath = "RendererTimeSinceDeath";
constexpr std::string_view kCapturerTimeSinceDeath = "CapturerTimeSinceDeath";
constexpr std::string_view kAllSessionsDuration = "@wrapper";

// string values
constexpr std::string kUnknown = "unknown";
constexpr std::string kUnknownNoClients = "unknown - no clients";
constexpr std::string kNormal = "normal";
constexpr std::string kDefault = "default";
constexpr std::string kSampleFormatUint8 = "UNSIGNED_8";
constexpr std::string kSampleFormatInt16 = "SIGNED_16";
constexpr std::string kSampleFormatInt24In32 = "SIGNED_24_IN_32";
constexpr std::string kSampleFormatFloat32 = "FLOAT";

constexpr std::string kNone = "NONE";
constexpr std::string kDuck = "DUCK";
constexpr std::string kMute = "MUTE";

// A singleton instance of |Reporter| handles instrumentation concerns (e.g.
// exposing information via inspect, cobalt, etc) for an audio_core instance.
// The idea is to make instrumentation as simple as possible for the code that
// does the real work. The singleton can be accessed via
//
//   Reporter::Singleton()
//
// Given a Reporter, reporting objects can be created through the Create*()
// methods. Each reporting object is intended to mirror a single object within
// audio_core, such as an AudioRenderer -- the reporting object should live
// exactly as long as its parent audio_core object. In addition to Create*()
// methods, there are FailedTo*() methods that report when an object could not
// be created.
//
// The singleton object always exists: it does not need to be created. However,
// the singleton needs to be initialized, via Reporter::InitializeSingleton().
// Before that static method is called, all reporting objects created by the
// singleton will be no-ops.
//
// The lifetime of each reporting object is divided into sessions. Roughly
// speaking, a session corresponds to a contiguous time spent processing audio.
// For example, for an AudioRenderer, this is the time between Play and Pause events.
// Session lifetimes are controlled by StartSession and StopSession methods.
//
// All times are relative to the system monotonic clock.
//
// This class is fully thread safe, including all static methods and all methods
// on reporting objects.
//
class Reporter {
 public:
  static Reporter& Singleton();
  static void InitializeSingleton(sys::ComponentContext& component_context,
                                  async_dispatcher_t* fidl_dispatcher,
                                  async_dispatcher_t* io_dispatcher, bool enable_cobalt);

  struct AudioDriverInfo {
    std::string manufacturer_name;
    std::string product_name;
    zx::duration internal_delay;
    zx::duration external_delay;
    int64_t driver_transfer_bytes;
    std::optional<Format> format;
  };

  class Device {
   public:
    virtual ~Device() = default;

    virtual void Destroy() = 0;

    virtual void StartSession(zx::time start_time) = 0;
    virtual void StopSession(zx::time stop_time) = 0;

    virtual void SetDriverInfo(const AudioDriverInfo& driver) = 0;
    virtual void SetGainInfo(const fuchsia::media::AudioGainInfo& gain_info,
                             fuchsia::media::AudioGainValidFlags set_flags) = 0;
    virtual void UpdateDelays(zx::time time_of_update, zx::duration internal_delay,
                              std::optional<zx::duration> external_delay) = 0;
  };

  class OutputDevice : public Device {
   public:
    virtual void DeviceUnderflow(zx::time start_time, zx::time end_time) = 0;
    virtual void PipelineUnderflow(zx::time start_time, zx::time end_time) = 0;
  };

  class InputDevice : public Device {};

  class Renderer {
   public:
    virtual ~Renderer() = default;

    virtual void Destroy() = 0;

    virtual void StartSession(zx::time start_time) = 0;
    virtual void StopSession(zx::time stop_time) = 0;

    virtual void SetUsage(RenderUsage usage) = 0;
    virtual void SetFormat(const Format& format) = 0;

    virtual void SetGain(float gain_db) = 0;
    virtual void SetMute(bool muted) = 0;
    virtual void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                                 fuchsia::media::audio::RampType ramp_type) = 0;
    virtual void SetCompleteGain(float complete_gain_db) = 0;

    virtual void SetInitialMinLeadTime(zx::duration initial_min_lead_time) = 0;
    virtual void UpdateMinLeadTime(zx::duration new_min_lead_time,
                                   zx::time time_of_min_lead_time_change) = 0;
    virtual void SetPtsContinuityThreshold(float threshold_seconds) = 0;
    virtual void SetPtsUnits(uint32_t numerator, uint32_t denominator) = 0;

    virtual void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) = 0;
    virtual void RemovePayloadBuffer(uint32_t buffer_id) = 0;
    virtual void SendPacket(const fuchsia::media::StreamPacket& packet) = 0;

    virtual void PacketQueueUnderflow(zx::time start_time, zx::time end_time) = 0;
    virtual void ContinuityUnderflow(zx::time start_time, zx::time end_time) = 0;
    virtual void TimestampUnderflow(zx::time start_time, zx::time end_time) = 0;
  };

  class Capturer {
   public:
    virtual ~Capturer() = default;

    virtual void Destroy() = 0;

    virtual void StartSession(zx::time start_time) = 0;
    virtual void StopSession(zx::time stop_time) = 0;

    virtual void SetUsage(CaptureUsage usage) = 0;
    virtual void SetFormat(const Format& format) = 0;

    virtual void SetGain(float gain_db) = 0;
    virtual void SetMute(bool muted) = 0;
    virtual void SetGainWithRamp(float gain_db, zx::duration ramp_duration,
                                 fuchsia::media::audio::RampType ramp_type) = 0;
    virtual void SetCompleteGain(float complete_gain_db) = 0;

    virtual void SetInitialPresentationDelay(zx::duration initial_presentation_delay) = 0;
    virtual void UpdatePresentationDelay(zx::duration new_presentation_delay,
                                         zx::time time_of_presentation_delay_change) = 0;

    virtual void AddPayloadBuffer(uint32_t buffer_id, uint64_t size) = 0;
    virtual void SendPacket(const fuchsia::media::StreamPacket& packet) = 0;
    virtual void Overflow(zx::time start_time, zx::time end_time) = 0;
  };

  class VolumeControl {
   public:
    virtual ~VolumeControl() = default;

    virtual void Destroy() = 0;

    virtual void SetVolumeMute(float volume, bool mute) = 0;
    virtual void AddBinding(std::string name) = 0;
  };

  // This class is an implementation detail.
  // Container::Ptr is a smart pointer that calls T::Destroy() when the Ptr is destructed.
  // The underlying object may be cached for some time afterwards.
  // ObjectsToCache is the number of destroyed objects to cache, in addition to the
  // current alive object.
  template <typename T, size_t ObjectsToCache>
  class Container {
   public:
    class Ptr {
     public:
      Ptr(Container<T, ObjectsToCache>* c, std::shared_ptr<T> p) : container_(c), ptr_(p) {}
      Ptr(const Ptr&) = delete;
      Ptr(Ptr&&) = default;
      ~Ptr() { Drop(); }

      Ptr& operator=(Ptr&& rhs) noexcept {
        Drop();
        ptr_ = std::move(rhs.ptr_);
        container_ = rhs.container_;
        rhs.container_ = nullptr;
        return *this;
      }

      T& operator*() const { return *ptr_; }
      T* operator->() const { return ptr_.get(); }

      void Drop() {
        if (ptr_) {
          ptr_->Destroy();
          container_->Kill(ptr_);
          ptr_ = nullptr;
        }
      }

     private:
      Container<T, ObjectsToCache>* container_ = nullptr;
      std::shared_ptr<T> ptr_;
    };

   private:
    friend class Reporter;
    friend class Ptr;

    Ptr New(T* object) {
      std::shared_ptr<T> ptr(object);
      std::lock_guard<std::mutex> lock(mutex_);
      alive_.insert(ptr);
      return Ptr(this, ptr);
    }

    void Kill(const std::shared_ptr<T>& ptr) {
      std::lock_guard<std::mutex> lock(mutex_);
      alive_.erase(ptr);
      while (dead_.size() >= ObjectsToCache) {
        dead_.pop();
      }
      dead_.push(ptr);
    }

    std::mutex mutex_;
    std::set<std::shared_ptr<T>> alive_ FXL_GUARDED_BY(mutex_);
    std::queue<std::shared_ptr<T>> dead_ FXL_GUARDED_BY(mutex_);
  };

  Reporter() = default;
  Reporter(sys::ComponentContext& component_context, async_dispatcher_t* fidl_dispatcher,
           async_dispatcher_t* io_dispatcher, bool enable_cobalt);

  static constexpr size_t kObjectsToCache = 4;
  static constexpr size_t kVolumeControlsToCache = 10;
  static constexpr size_t kActiveUsagePoliciesToCache = 10;

  Container<OutputDevice, kObjectsToCache>::Ptr CreateOutputDevice(const std::string& name,
                                                                   const std::string& thread_name);
  Container<InputDevice, kObjectsToCache>::Ptr CreateInputDevice(const std::string& name,
                                                                 const std::string& thread_name);
  Container<Renderer, kObjectsToCache>::Ptr CreateRenderer();
  Container<Capturer, kObjectsToCache>::Ptr CreateCapturer(const std::string& thread_name);
  Container<VolumeControl, kVolumeControlsToCache>::Ptr CreateVolumeControl();

  // Thermal state of Audio system.
  void SetNumThermalStates(size_t num);
  void SetThermalState(uint32_t state);

  // Audio policy logging of usage activity and behavior (none|duck|mute).
  void SetAudioPolicyBehaviorGain(AudioAdmin::BehaviorGain behavior_gain);
  void UpdateActiveUsagePolicy(const std::vector<fuchsia::media::Usage2>& active_usages,
                               const AudioAdmin::RendererPolicies& renderer_policies,
                               const AudioAdmin::CapturerPolicies& capturer_policies);

  // Device creation failures.
  void FailedToConnectToDevice(const std::string& name, bool is_input, zx_status_t status);
  void FailedToObtainStreamChannel(const std::string& name, bool is_input, zx_status_t status);
  void FailedToStartDevice(const std::string& name);

  // Mixer events which are not easily tied to a specific device or client.
  void MixerClockSkewDiscontinuity(zx::duration abs_clock_error);

  // Failures when calling RoleManager (to set thread priority or pin processwide memory).
  void FailedToApplySchedulerProfile(const std::string& profile, zx_status_t status);
  void FailedToApplyMemoryProfile(const std::string& profile, zx_status_t status);

  // Exported for tests.
  const inspect::Inspector& inspector() {
    std::lock_guard<std::mutex> lock(mutex_);
    return impl_->inspector->inspector();
  }

 private:
  static constexpr size_t kThermalStatesToCache = 8;

  class OverflowUnderflowTracker;
  class ObjectTracker;
  class DeviceDriverInfo;
  class ThermalStateTransition;
  class ThermalStateTracker;
  class OutputDeviceImpl;
  class InputDeviceImpl;
  class ClientPort;
  class RendererImpl;
  class CapturerImpl;
  class VolumeControlImpl;
  class VolumeSetting;
  class ActiveUsagePolicy;
  class ActiveUsagePolicyTracker;
  struct Impl;

  friend class OverflowUnderflowTracker;
  friend class ObjectTracker;
  friend class OutputDeviceImpl;
  friend class InputDeviceImpl;
  friend class RendererImpl;
  friend class CapturerImpl;

  void InitInspect() FXL_EXCLUSIVE_LOCKS_REQUIRED(mutex_);
  void InitCobalt() FXL_EXCLUSIVE_LOCKS_REQUIRED(mutex_);

  // This object contains internal state shared by multiple reporting objects.
  struct Impl {
    sys::ComponentContext& component_context;
    async_dispatcher_t* fidl_dispatcher;
    async_dispatcher_t* io_dispatcher;
    std::unique_ptr<inspect::ComponentInspector> inspector;
    std::unique_ptr<media::audio::MetricsImpl> metrics_impl;

    inspect::UintProperty failed_to_connect_to_device_count;
    inspect::UintProperty failed_to_obtain_stream_channel_count;
    inspect::UintProperty failed_to_start_device_count;
    inspect::UintProperty failed_to_apply_scheduler_profile_count;
    inspect::UintProperty failed_to_apply_memory_profile_count;
    inspect::LinearIntHistogram mixer_clock_skew_discontinuities;
    inspect::Node outputs_node;
    inspect::Node inputs_node;
    inspect::Node renderers_node;
    inspect::Node capturers_node;
    inspect::Node thermal_state_transitions_node;
    inspect::Node volume_controls_node;

    std::unique_ptr<ThermalStateTracker> thermal_state_tracker;
    std::unique_ptr<ActiveUsagePolicyTracker> active_usage_policy_tracker;

    // These could be guarded by Reporter::mutex_, but clang's thread safety
    // analysis cannot represent that relationship.
    std::mutex mutex;
    uint64_t next_renderer_id FXL_GUARDED_BY(mutex) = 0;
    uint64_t next_capturer_id FXL_GUARDED_BY(mutex) = 0;
    uint64_t next_thermal_transition_id FXL_GUARDED_BY(mutex) = 0;
    uint64_t next_volume_control_id FXL_GUARDED_BY(mutex) = 0;

    Impl(sys::ComponentContext& cc, async_dispatcher_t* fidl_dispatcher,
         async_dispatcher_t* io_dispatcher);
    ~Impl();

    std::string NextRendererIdStr() {
      std::lock_guard<std::mutex> lock(mutex);
      return std::to_string(++next_renderer_id);
    }
    std::string NextCapturerIdStr() {
      std::lock_guard<std::mutex> lock(mutex);
      return std::to_string(++next_capturer_id);
    }
    std::string NextThermalTransitionIdStr() {
      std::lock_guard<std::mutex> lock(mutex);
      return std::to_string(++next_thermal_transition_id);
    }
    std::string NextVolumeControlIdStr() {
      std::lock_guard<std::mutex> lock(mutex);
      return std::to_string(++next_volume_control_id);
    }
  };

  std::mutex mutex_;
  std::unique_ptr<Impl> impl_ FXL_GUARDED_BY(mutex_);

  // Caches of allocated objects so they can live beyond destruction.
  Container<OutputDevice, kObjectsToCache> outputs_;
  Container<InputDevice, kObjectsToCache> inputs_;
  Container<Renderer, kObjectsToCache> renderers_;
  Container<Capturer, kObjectsToCache> capturers_;
  Container<ThermalStateTransition, kThermalStatesToCache> thermal_state_transitions_;
  Container<VolumeControl, kVolumeControlsToCache> volume_controls_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_REPORTER_H_

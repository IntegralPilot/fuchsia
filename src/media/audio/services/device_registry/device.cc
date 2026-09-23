// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/device.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.audio/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/async/cpp/time.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <string_view>

#include "src/media/audio/lib/clock/real_clock.h"
#include "src/media/audio/lib/timeline/timeline_rate.h"
#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/device_presence_watcher.h"
#include "src/media/audio/services/device_registry/format_utils.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/observer_notify.h"
#include "src/media/audio/services/device_registry/signal_processing_utils.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace {

TokenId NextTokenId() {
  static TokenId token_id = 0;
  return token_id++;
}

}  // namespace

void Device::SetCommandTimeout(const zx::duration& budget, std::string cmd_tag) {
  auto now = zx::clock::get_monotonic();
  ADR_LOG_METHOD(kLogDriverCommandTimeouts)
      << "(" << budget.to_msecs() << " msec, '" << cmd_tag << "') at " << now.to_timespec().tv_sec;
  if (recovering_from_late_response_) {
    ADR_LOG_METHOD(kLogDriverCommandTimeouts)
        << " while trying to recover from previous driver cmd timeout";
  }

  if (driver_cmd_waiting()) {
    ADR_WARN_METHOD()
        << __func__
        << " while already waiting for driver response. Cancelling existing cmd_timeout '"
        << pending_driver_cmd_->tag << "' and setting new one '" << cmd_tag << "'.";
    auto status = timeout_task_.Cancel();
    ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status << ", prev_deadline "
                                              << timeout_task_.last_deadline().get();
    pending_driver_cmd_.reset();
    SetDriverCommandState(DriverCommandState::Idle);

    // Don't return -- fall-thru into the next Idle section.
  }
  if (driver_cmd_idle()) {
    pending_driver_cmd_ = CommandCountdown{
        .tag = std::move(cmd_tag),
        .deadline = now + budget,
        .budget = budget,
    };
    zx_status_t status = timeout_task_.PostDelayed(dispatcher_, budget);
    ADR_LOG_METHOD(kLogDriverCommandTimeouts)
        << "PostDelayed(" << budget.get() << ") returned " << status;
    SetDriverCommandState(DriverCommandState::Waiting);
    return;
  }

  if (driver_cmd_overdue()) {
    auto status = timeout_task_.Cancel();
    ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status << ", prev_deadline "
                                              << timeout_task_.last_deadline().get();
    pending_driver_cmd_.reset();
    OnError(ZX_ERR_IO_MISSED_DEADLINE);
    SetDriverCommandState(DriverCommandState::Unresponsive);
    return;
  }

  // if (driver_cmd_unresponsive() { /* do nothing */ return; }
}

void Device::ClearCommandTimeout() {
  if (recovering_from_late_response_) {
    ADR_LOG_METHOD(kLogDriverCommandTimeouts)
        << " while trying to recover from previous driver cmd timeout";
  }

  auto now = zx::clock::get_monotonic();
  zx::duration elapsed = now - pending_driver_cmd_->deadline + pending_driver_cmd_->budget;

  // if (driver_cmd_idle() { /* do nothing */ return; }

  if (driver_cmd_waiting()) {
    auto status = timeout_task_.Cancel();
    ADR_LOG_METHOD(kLogDriverCommandTimeouts)
        << " for cmd '" << pending_driver_cmd_->tag << "' (deadline "
        << timeout_task_.last_deadline().get() << ") after "
        << static_cast<float>(elapsed.to_usecs()) / 1000.0f << " msec (limit "
        << pending_driver_cmd_->budget.to_msecs() << ") at " << now.to_timespec().tv_sec;
    ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status << ", prev_deadline "
                                              << timeout_task_.last_deadline().get();
    pending_driver_cmd_.reset();
    SetDriverCommandState(DriverCommandState::Idle);
    return;
  }

  if (driver_cmd_overdue()) {
    LogCommandTimeout(pending_driver_cmd_->tag, pending_driver_cmd_->budget, elapsed);
    recovering_from_late_response_ = true;
    auto status = timeout_task_.Cancel();
    ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status << ", prev_deadline "
                                              << timeout_task_.last_deadline().get();
    pending_driver_cmd_.reset();
    SetDriverCommandState(DriverCommandState::Idle);
    return;
  }

  // if (driver_cmd_unresponsive() { /* do nothing */ return; }
}

void Device::DriverCommandTimedOut() {
  ADR_LOG_METHOD(kLogDriverCommandTimeouts) << timeout_task_.last_deadline().get();
  if (recovering_from_late_response_) {
    ADR_WARN_METHOD() << " while trying to recover from previous driver cmd timeout";
  }

  if (driver_cmd_idle()) {
    FX_CHECK(!pending_driver_cmd_.has_value());
    FX_CHECK(!driver_cmd_idle()) << "received " << __func__ << " without a waiting driver cmd";
    return;
  }

  if (driver_cmd_waiting()) {
    LogCommandTimeout(pending_driver_cmd_->tag, pending_driver_cmd_->budget, std::nullopt);
    SetDriverCommandState(DriverCommandState::Overdue);
    return;
  }

  if (driver_cmd_overdue()) {
    auto status = timeout_task_.Cancel();
    ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status << ", prev_deadline "
                                              << timeout_task_.last_deadline().get();
    pending_driver_cmd_.reset();
    OnError(ZX_ERR_IO_MISSED_DEADLINE);
    SetDriverCommandState(DriverCommandState::Unresponsive);
    return;
  }

  if (driver_cmd_unresponsive()) {
    FX_CHECK(!pending_driver_cmd_.has_value());
    FX_CHECK(!driver_cmd_unresponsive())
        << "received " << __func__ << " after previous driver cmd timed out";
    return;
  }

  FX_LOGS(FATAL) << "Unknown driver command state";
}

void Device::LogCommandTimeout(const std::string& cmd_tag, zx::duration expected,
                               std::optional<zx::duration> actual) {
  std::ostringstream stream;
  stream << "Driver response to command '" << cmd_tag << "' was expected in " << expected.to_msecs()
         << " msec or less. ";

  if (actual.has_value()) {
    stream << "Response was received in " << static_cast<float>(actual->to_usecs()) / 1000.0f
           << " msec.";
  } else {
    stream << "No response received yet.";
  }
  ADR_WARN_METHOD() << stream.str();
  inspect()->RecordCommandTimeout(cmd_tag, expected, actual);
  // Also TRACE?

  //// TODO(https://fxbug.dev/42114915): Log a cobalt metric for this.
}

// Invoked when a RingBuffer channel drops. Device state previously was Configured/Paused/Started.
template <>
void Device::RingBufferFidlErrorHandler<fha::RingBuffer>::on_fidl_error(fidl::UnbindInfo info) {
  ADR_LOG_METHOD(kLogRingBufferFidlResponses || kLogObjectLifetimes) << "(RingBuffer)";
  if (device()->has_error()) {
    ADR_WARN_METHOD() << "device already has an error; no device state to unwind";
  } else if (info.is_peer_closed() || info.is_user_initiated()) {
    ADR_LOG_METHOD(kLogRingBufferFidlResponses) << name() << " disconnected: " << info;
    device()->DeviceDroppedRingBuffer(element_id_);
  } else {
    ADR_WARN_METHOD() << name() << " disconnected: " << info;
    device()->OnError(info.status());
  }
  auto& ring_buffer = device()->ring_buffer_map_.find(element_id_)->second;
  ring_buffer.ring_buffer_client.reset();

  device()->ring_buffer_map_.erase(element_id_);
}

// Invoked when a PacketStream channel drops. Device state previously was Configured/Paused/Started.
template <>
void Device::PacketStreamFidlErrorHandler<fha::PacketStreamControl>::on_fidl_error(
    fidl::UnbindInfo info) {
  ADR_LOG_METHOD(kLogPacketStreamFidlResponses || kLogObjectLifetimes) << "(PacketStream)";
  if (device()->has_error()) {
    ADR_WARN_METHOD() << "device already has an error; no device state to unwind";
  } else if (info.is_peer_closed() || info.is_user_initiated()) {
    ADR_LOG_METHOD(kLogPacketStreamFidlResponses) << name() << " disconnected: " << info;
    device()->DeviceDroppedPacketStream(element_id_);
  } else {
    ADR_WARN_METHOD() << name() << " disconnected: " << info;
    device()->OnError(info.status());
  }
  auto& packet_stream = device()->packet_stream_map_.find(element_id_)->second;
  packet_stream.packet_stream_client.reset();

  device()->packet_stream_map_.erase(element_id_);
}

// Invoked when a SignalProcessing channel drops. This can occur during device initialization.
template <>
void Device::FidlErrorHandler<fhasp::SignalProcessing>::on_fidl_error(fidl::UnbindInfo info) {
  ADR_LOG_METHOD(kLogRingBufferFidlResponses || kLogObjectLifetimes) << "(SignalProcessing)";
  // If a device already encountered some other error, disconnects like this are not unexpected.
  if (device_->has_error()) {
    ADR_WARN_METHOD() << "(signalprocessing) device already has error; no state to unwind";
    return;
  }
  // Check whether this occurred as a normal part of checking for signalprocessing support.
  if (info.is_peer_closed() && info.status() == ZX_ERR_NOT_SUPPORTED) {
    ADR_LOG_METHOD(kLogSignalProcessingFidlResponses || kLogDeviceState)
        << name_ << ": signalprocessing not supported: " << info;
    device_->SetSignalProcessingSupported(false);
    return;
  }
  // Otherwise, we might just be unwinding this device (device-removal, or service shut-down).
  if (info.is_peer_closed() || info.is_user_initiated()) {
    ADR_LOG_METHOD(kLogSignalProcessingFidlResponses || kLogDeviceState)
        << name_ << "(signalprocessing) disconnected: " << info;
    device_->SetSignalProcessingSupported(false);
    return;
  }
  // If none of the above, this driver's FIDL connection is now in doubt. Mark it as in ERROR.
  ADR_WARN_METHOD() << name_ << "(signalprocessing) disconnected: " << info;
  device_->SetSignalProcessingSupported(false);
  device_->OnError(info.status());

  device_->sig_proc_client_.reset();
}

// Invoked when the underlying driver disconnects.
template <typename T>
void Device::FidlErrorHandler<T>::on_fidl_error(fidl::UnbindInfo info) {
  if (!info.is_peer_closed() && !info.is_user_initiated() && !info.is_dispatcher_shutdown()) {
    ADR_WARN_METHOD() << name_ << " disconnected: " << info;
    device_->OnError(info.status());
  } else {
    ADR_LOG_METHOD(kLogCodecFidlResponses || kLogCompositeFidlResponses || kLogObjectLifetimes ||
                   kLogDeviceAddErrorRemove)
        << name_ << " disconnected: " << info;
  }
  device_->OnRemoval();
}

// static
std::shared_ptr<Device> Device::Create(std::weak_ptr<DevicePresenceWatcher> presence_watcher,
                                       async_dispatcher_t* dispatcher, std::string_view name,
                                       fad::DeviceType device_type, fad::DriverClient driver_client,
                                       const std::string& added_by) {
  ADR_LOG_STATIC(kLogObjectLifetimes);

  // The constructor is private, forcing clients to use Device::Create().
  class MakePublicCtor : public Device {
   public:
    MakePublicCtor(std::weak_ptr<DevicePresenceWatcher> presence_watcher,
                   async_dispatcher_t* dispatcher, std::string_view name,
                   fad::DeviceType device_type, fad::DriverClient driver_client,
                   const std::string& added_by)
        : Device(std::move(presence_watcher), dispatcher, name, device_type,
                 std::move(driver_client), added_by) {}
  };

  return std::make_shared<MakePublicCtor>(std::move(presence_watcher), dispatcher, name,
                                          device_type, std::move(driver_client), added_by);
}

// Device notifies presence_watcher when it is available (Initialized), unhealthy (Error) or
// removed. The dispatcher member is needed for Device to create client connections to protocols
// such as fuchsia.hardware.audio.signalprocessing.Reader or fuchsia.hardware.audio.RingBuffer.
Device::Device(std::weak_ptr<DevicePresenceWatcher> presence_watcher,
               async_dispatcher_t* dispatcher, std::string_view name, fad::DeviceType device_type,
               fad::DriverClient driver_client, const std::string& added_by)
    : presence_watcher_(std::move(presence_watcher)),
      dispatcher_(dispatcher),
      name_(name.substr(0, name.find('\0'))),
      device_type_(device_type),
      driver_client_(std::move(driver_client)),
      token_id_(NextTokenId()),
      state_(State::Initializing) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  device_inspect_instance_ = Inspector::Singleton()->RecordDeviceInitializing(
      name_, device_type_, zx::clock::get_monotonic(), added_by);
  inspect()->RecordTokenId(token_id_);

  ++count_;
  LogObjectCounts();

  switch (device_type_) {
    case fad::DeviceType::kCodec:
      codec_handler_ = {this, "Codec"};
      codec_client_ = {driver_client_.codec().take().value(), dispatcher, &codec_handler_};
      break;
    case fad::DeviceType::kComposite:
      composite_handler_ = {this, "Composite"};
      composite_client_ = {driver_client_.composite().take().value(), dispatcher,
                           &composite_handler_};
      break;
    default:
      ADR_WARN_METHOD() << "Unknown DeviceType" << device_type_;
      OnError(ZX_ERR_WRONG_TYPE);
      return;
  }

  // Upon creation, automatically kick off initialization. This will complete asynchronously,
  // notifying the presence_watcher at that time by calling DeviceIsReady().
  Initialize();
}

Device::~Device() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  --count_;
  LogObjectCounts();
}

// Called during initialization, when we know definitively whether the driver supports the protocol.
// It might also be called later, if there is an error related to the signalprocessing protocol.
// If this is the first time this is called, then call `OnInitializationResponse` to unblock the
// signalprocessing-related aspect of the "wait for multiple responses" state machine.
void Device::SetSignalProcessingSupported(bool is_supported) {
  ADR_LOG_METHOD(kLogSignalProcessingState);

  auto first_set_of_signalprocessing_support = !supports_signalprocessing_.has_value();
  supports_signalprocessing_ = is_supported;

  // Only poke the initialization state machine the FIRST time this is called.
  if (first_set_of_signalprocessing_support) {
    // Composite drivers must support signalprocessing.
    if (is_composite() && !is_supported) {
      // If we have not already registered an error for this device, then do so now.
      OnError(ZX_ERR_NOT_SUPPORTED);
    }
    OnInitializationResponse();
  }

  if constexpr (kLogSignalProcessingState || kLogDeviceState) {
    if (!first_set_of_signalprocessing_support && is_composite() && !is_supported) {
      // If supports_signalprocessing_ changed AFTER we first checked, this is probably a device
      // removal. Let OnRemoval() clean things up, instead of marking the device as error.
      ADR_LOG_METHOD(kLogSignalProcessingState || kLogDeviceState) << "interpreting as a removal";
    }
  }
}

void Device::OnRemoval() {
  ADR_LOG_METHOD(kLogDeviceState);
  inspect()->RecordRemoval(zx::clock::get_monotonic());

  auto status = timeout_task_.Cancel();
  ADR_LOG_METHOD(kLogDriverCommandTimeouts) << "Cancel returned " << status;

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; no device state to unwind";
    --unhealthy_count_;
  } else if (is_operational()) {
    --initialized_count_;

    DropControl();  // Probably unneeded (the Device is going away) but makes unwind "complete".
    ADR_LOG_OBJECT(kLogNotifyMethods) << "ForEachObserver => DeviceIsRemoved()" << token_id_ << ")";
    ForEachObserver(
        [](const auto& obs) { obs->DeviceIsRemoved(); });  // Our control is also an observer.
  }

  ring_buffer_map_.clear();
  sig_proc_client_.reset();
  codec_client_.reset();

  LogDeviceRemoval(info());
  LogObjectCounts();

  // Regardless of whether device was pending / operational / unhealthy, notify the state watcher.
  if (std::shared_ptr<DevicePresenceWatcher> pw = presence_watcher_.lock(); pw) {
    pw->DeviceIsRemoved(shared_from_this());
  }
}

void Device::ForEachObserver(fit::function<void(std::shared_ptr<ObserverNotify>)> action) {
  for (auto weak_obs = observers_.begin(); weak_obs < observers_.end(); ++weak_obs) {
    if (auto observer = weak_obs->lock(); observer) {
      action(observer);
    }
  }
}

// Unwind any operational state or configuration the device might be in, and remove this device.
void Device::OnError(zx_status_t error) {
  ADR_LOG_METHOD(kLogDeviceState);
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; ignoring subsequent error (" << error << ")";
    return;
  }

  FX_PLOGS(WARNING, error) << __func__;
  inspect()->RecordError(zx::clock::get_monotonic());

  if (is_operational()) {
    --initialized_count_;

    DropControl();
    ADR_LOG_OBJECT(kLogNotifyMethods) << "ForEachObserver => DeviceHasError";
    ForEachObserver([](const auto& obs) { obs->DeviceHasError(); });
  }
  ++unhealthy_count_;
  SetError(error);
  LogDeviceError(info());
  LogObjectCounts();

  if (std::shared_ptr<DevicePresenceWatcher> pw = presence_watcher_.lock(); pw) {
    pw->DeviceHasError(shared_from_this());
  }
}
bool Device::IsInitializationComplete() {
  switch (device_type_) {
    case fad::DeviceType::kCodec:
      return has_codec_properties() && has_health_state() && dai_format_sets_retrieved() &&
             has_plug_state();
    case fad::DeviceType::kComposite:
      return has_composite_properties() && has_health_state() && checked_for_signalprocessing() &&
             dai_format_sets_retrieved() && ring_buffer_format_sets_retrieved() &&
             packet_stream_format_sets_retrieved();
    default:
      ADR_WARN_METHOD() << "Invalid device_type_";
      return false;
  }
}

// An initialization command returned a successful response. Is initialization complete?
void Device::OnInitializationResponse() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device has already encountered a problem; ignoring this";
    return;
  }

  if (is_operational()) {
    ADR_WARN_METHOD() << "unexpected device initialization response when not Initializing";
  }

  switch (device_type_) {
    case fad::DeviceType::kCodec:
      ADR_LOG_METHOD(kLogDeviceInitializationProgress)
          << " (RECEIVED|pending)"                                                 //
          << "   " << (has_codec_properties() ? "PROPS" : "props")                 //
          << "   " << (has_health_state() ? "HEALTH" : "health")                   //
          << "   " << (dai_format_sets_retrieved() ? "DAIFORMATS" : "daiformats")  //
          << "   " << (has_plug_state() ? "PLUG" : "plug");
      break;
    case fad::DeviceType::kComposite:
      ADR_LOG_METHOD(kLogDeviceInitializationProgress)
          << " (RECEIVED|pending)"                                                         //
          << "   " << (has_composite_properties() ? "PROPS" : "props")                     //
          << "   " << (has_health_state() ? "HEALTH" : "health")                           //
          << "   " << (checked_for_signalprocessing() ? "SIGPROC" : "sigproc")             //
          << "   " << (dai_format_sets_retrieved() ? "DAIFORMATS" : "daiformats")          //
          << "   " << (ring_buffer_format_sets_retrieved() ? "RB_FORMATS" : "rb_formats")  //
          << "   " << (packet_stream_format_sets_retrieved() ? "PS_FORMATS" : "ps_formats");
      break;
    default:
      ADR_WARN_METHOD() << "Invalid device_type_";
      return;
  }

  if (IsInitializationComplete()) {
    // This clears the "meta-command" timeout set earlier in Initialize, covering GetProperties,
    // GetTopologies, GetElements, GetRingBufferFormats, GetDaiFormats and initial GetHealthInfo.
    ClearCommandTimeout();

    ++initialized_count_;
    SetDeviceInfo();
    LogDeviceAddition(*info());
    OnInitializationComplete();
    LogObjectCounts();

    if (std::shared_ptr<DevicePresenceWatcher> pw = presence_watcher_.lock(); pw) {
      pw->DeviceIsReady(shared_from_this());
    }
  }
}

// Returns true if the specified entity successfully asserts control of this device.
bool Device::SetControl(std::shared_ptr<ControlNotify> control_notify_to_set) {
  ADR_LOG_METHOD(kLogDeviceState || kLogNotifyMethods);

  if (has_error()) {
    ADR_WARN_METHOD() << "device has an error; cannot set control";
    return false;
  }
  FX_CHECK(is_operational());

  if (GetControlNotify()) {
    ADR_WARN_METHOD() << "already controlled";
    return false;
  }

  control_notify_ = control_notify_to_set;
  AddObserver(std::move(control_notify_to_set));

  // For this new control, "catch it up" with notifications about the device's current state.
  // We do this to explicitly confirm that a Device persists its set DaiFormats and CodecStart/Stop
  // state even after its Control is dropped.
  if (auto notify = GetControlNotify(); notify) {
    // DaiFormat(s)
    if (is_codec()) {
      if (codec_format_) {
        notify->DaiFormatIsChanged(fad::kDefaultDaiInterconnectElementId, codec_format_->dai_format,
                                   codec_format_->codec_format_info);
      } else {
        notify->DaiFormatIsChanged(fad::kDefaultDaiInterconnectElementId, std::nullopt,
                                   std::nullopt);
      }
      // Codec start state
      codec_start_state_.started ? notify->CodecIsStarted(codec_start_state_.start_stop_time)
                                 : notify->CodecIsStopped(codec_start_state_.start_stop_time);
    } else if (is_composite()) {
      for (auto [element_id, dai_format] : composite_dai_formats_) {
        notify->DaiFormatIsChanged(element_id, dai_format, std::nullopt);
      }
    }
  }

  LogObjectCounts();
  return true;
}

// Returns true if this device previously had a controlling entity.
bool Device::DropControl() {
  ADR_LOG_METHOD(kLogDeviceMethods || kLogNotifyMethods);

  auto control_notify = GetControlNotify();
  if (!control_notify) {
    ADR_LOG_METHOD(kLogNotifyMethods) << "already not controlled";
    return false;
  }

  // Need to drop ring buffers here?

  control_notify_.reset();
  // We don't remove our ControlNotify from the observer list: we wait for it to self-invalidate.

  return true;
}

bool Device::AddObserver(const std::shared_ptr<ObserverNotify>& observer_to_add) {
  ADR_LOG_METHOD(kLogDeviceMethods || kLogNotifyMethods) << " (" << observer_to_add << ")";

  if (has_error()) {
    ADR_WARN_METHOD() << ": unhealthy, cannot be observed";
    return false;
  }
  FX_CHECK(is_operational());

  for (auto weak_obs = observers_.begin(); weak_obs < observers_.end(); ++weak_obs) {
    if (auto observer = weak_obs->lock(); observer) {
      if (observer == observer_to_add) {
        // Because observers are not explicitly dropped (they are weakly held and allowed to
        // self-invalidate), this can occur if a Control is dropped then immediately readded.
        // This is OK because the outcome is ultimately correct (it is still in the vector).
        ADR_WARN_METHOD() << "Device(" << this << ")::AddObserver: observer cannot be re-added";
        return false;
      }
    }
  }
  observers_.push_back(observer_to_add);

  // For this new observer, "catch it up" with notifications about the device's current state.
  // This may include:
  //
  // (1) current signalprocessing topology.
  if (current_topology_id_.has_value()) {
    observer_to_add->TopologyIsChanged(*current_topology_id_);
  }

  // (2) ElementState, for each signalprocessing element where this has already been retrieved.
  for (const auto& element_record : sig_proc_element_map_) {
    if (element_record.second.state.has_value()) {
      observer_to_add->ElementStateIsChanged(element_record.first, *element_record.second.state);
    }
  }

  // (4) PlugState (if Codec).
  if (is_codec()) {
    observer_to_add->PlugStateIsChanged(
        *plug_state_->plugged() ? fad::PlugState::kPlugged : fad::PlugState::kUnplugged,
        zx::time(*plug_state_->plug_state_time()));
  }

  LogObjectCounts();

  return true;
}

// The Device dropped the driver RingBuffer FIDL. Notify any clients.
void Device::DeviceDroppedRingBuffer(ElementId element_id) {
  ADR_LOG_METHOD(kLogDeviceState);

  // This is distinct from DropRingBuffer in case we must notify our RingBufferServer (via our
  // Control). We do so if we have 1) a Control and 2) a driver_format_ (thus a client-configured
  // RingBufferServer).
  if (auto notify = GetControlNotify(); notify) {
    if (auto rb_pair = ring_buffer_map_.find(element_id);
        rb_pair != ring_buffer_map_.end() && rb_pair->second.driver_format.has_value())
      notify->DeviceDroppedRingBuffer(element_id);
  }
  DropRingBuffer(element_id);
}

void Device::DeviceDroppedPacketStream(ElementId element_id) {
  ADR_LOG_METHOD(kLogDeviceState);

  // This is distinct from DropPacketStream in case we must notify our PacketStreamServer (via our
  // Control). We do so if we have 1) a Control and 2) a driver_format_ (thus a client-configured
  // PacketStreamServer).
  if (auto notify = GetControlNotify(); notify) {
    if (auto ps_pair = packet_stream_map_.find(element_id);
        ps_pair != packet_stream_map_.end() && ps_pair->second.driver_format.has_value()) {
      notify->DeviceDroppedPacketStream(element_id);
    }
  }
  DropPacketStream(element_id);
}

// Whether client- or device-originated, reset any state associated with an active RingBuffer.
void Device::DropRingBuffer(ElementId element_id) {
  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot DropRingBuffer";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogDeviceState);

  auto rb_pair = ring_buffer_map_.find(element_id);
  if (rb_pair == ring_buffer_map_.end()) {
    ADR_LOG_METHOD(kLogRingBufferMethods) << "element_id " << element_id << " not found";
    return;
  }

  // If we've already cleaned out any state with the underlying driver RingBuffer, then we're done.
  auto& rb_record = rb_pair->second;

  // This can be called before the RingBufferRecord is entirely populated, if failure occurs after
  // the initial Connect (such as unsupported format). We can only record the 'destroyed_at'
  // timestamp if the Inspect instance exists.
  if (rb_record.inspect_instance) {
    rb_record.inspect_instance->RecordDestructionTime(zx::clock::get_monotonic());
  } else {
    ADR_WARN_METHOD()
        << "cannot RecordDestructionTime: no RB inspect node (CreateRingBuffer must have failed)";
  }

  if (!rb_record.ring_buffer_client.has_value() || !rb_record.ring_buffer_client->is_valid()) {
    ADR_LOG_METHOD(kLogRingBufferMethods) << "Driver RingBuffer connection is already dropped";
    return;
  }

  // Revert all configuration state related to the ring buffer.
  //
  rb_record.start_time.reset();  // Pause, if we are started.

  rb_record.requested_ring_buffer_bytes.reset();  // User must call CreateRingBuffer again ...
  rb_record.create_ring_buffer_callback = nullptr;
  rb_record.inspect_instance = nullptr;

  rb_record.driver_format.reset();  // ... making us re-call ConnectToRingBufferFidl ...
  rb_record.vmo_format = {};
  rb_record.num_ring_buffer_frames.reset();  // ... and GetRingBufferVmo ...
  rb_record.ring_buffer_vmo.reset();

  rb_record.ring_buffer_properties.reset();  // ... and GetProperties ...

  rb_record.delay_info.reset();   // ... and WatchDelayInfo ...
  rb_record.bytes_per_frame = 0;  // (.. which calls CalculateRequiredRingBufferSizes ...)
  rb_record.requested_ring_buffer_frames = 0u;
  rb_record.ring_buffer_producer_bytes = 0;
  rb_record.ring_buffer_consumer_bytes = 0;

  rb_record.active_channels_bitmask.reset();  // ... and SetActiveChannels.
  rb_record.set_active_channels_completed_at.reset();

  // Clear our FIDL connection to the driver RingBuffer.
  (void)rb_record.ring_buffer_client->UnbindMaybeGetEndpoint();
  rb_record.ring_buffer_client.reset();
}

void Device::DropPacketStream(ElementId element_id) {
  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot DropPacketStream";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogDeviceState);

  auto ps_pair = packet_stream_map_.find(element_id);
  if (ps_pair == packet_stream_map_.end()) {
    ADR_LOG_METHOD(kLogPacketStreamMethods) << "element_id " << element_id << " not found";
    return;
  }

  // If we've already cleaned out any state with the underlying driver PacketStream, then we're
  // done.
  auto& ps_record = ps_pair->second;

  // This can be called before the PacketStreamRecord is entirely populated, if failure occurs after
  // the initial Connect (such as unsupported format). We can only record the 'destroyed_at'
  // timestamp if the Inspect instance exists.
  if (ps_record.inspect_instance) {
    ps_record.inspect_instance->RecordDestructionTime(zx::clock::get_monotonic());
  } else {
    ADR_LOG_METHOD(kLogPacketStreamMethods)
        << "cannot RecordDestructionTime: no PS inspect node (CreatePacketStream must have failed)";
  }

  if (!ps_record.packet_stream_client.has_value() || !ps_record.packet_stream_client->is_valid()) {
    ADR_LOG_METHOD(kLogPacketStreamMethods) << "Driver PacketStream connection is already dropped";
    return;
  }

  // Revert all configuration state related to the PacketStream.
  //
  ps_record.start_time.reset();  // Pause, if we are started.

  ps_record.create_packet_stream_callback = nullptr;
  ps_record.inspect_instance = nullptr;

  ps_record.driver_format.reset();  // ... making us re-call ConnectToPacketStreamFidl ...
  ps_record.configured_buffer_type = std::nullopt;
  ps_record.configured_format = fuchsia_audio_device::PacketStreamFormat::WithPcmFormat({});

  ps_record.packet_stream_properties.reset();  // ... and GetProperties ...

  ps_record.requested_packet_stream_frames = 0u;

  // Clear our FIDL connection to the driver PacketStream.
  (void)ps_record.packet_stream_client->UnbindMaybeGetEndpoint();
  ps_record.packet_stream_client.reset();
}

void Device::SetError(zx_status_t error) {
  ADR_LOG_METHOD(kLogDeviceState) << ": " << error;
  state_ = State::Error;
}

void Device::OnInitializationComplete() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; ignoring this";
    return;
  }

  ADR_LOG_METHOD(kLogDeviceState);
  state_ = State::Initialized;
}

void Device::SetRingBufferState(ElementId element_id, RingBufferState ring_buffer_state) {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; ignoring this";
    return;
  }

  auto rb_pair = ring_buffer_map_.find(element_id);
  if (rb_pair == ring_buffer_map_.end()) {
    ADR_WARN_METHOD() << "couldn't find RingBuffer for element_id " << element_id;
    return;
  }

  ADR_LOG_METHOD(kLogRingBufferState) << ring_buffer_state;
  rb_pair->second.ring_buffer_state = ring_buffer_state;
}

void Device::SetPacketStreamState(ElementId element_id, PacketStreamState packet_stream_state) {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; ignoring this";
    return;
  }

  auto ps_pair = packet_stream_map_.find(element_id);
  if (ps_pair == packet_stream_map_.end()) {
    ADR_WARN_METHOD() << "couldn't find PacketStream for element_id " << element_id;
    return;
  }

  ADR_LOG_METHOD(kLogPacketStreamState) << packet_stream_state;
  ps_pair->second.packet_stream_state = packet_stream_state;
}

void Device::Initialize() {
  ADR_LOG_METHOD(kLogDeviceMethods);
  FX_CHECK(!has_error());
  FX_CHECK(!is_operational());

  if (is_codec()) {
    RetrieveDeviceProperties();
    RetrieveHealthState();
    RetrieveDaiFormatSets();
    RetrievePlugState();
  } else if (is_composite()) {
    // We use kDefaultLongCmdTimeout here, as we don't clear the timeout until GetProperties,
    // GetTopologies, GetElements, GetRingBufferFormats, GetPacketStreamFormats, GetDaiFormats and
    // the initial GetHealthInfo call have all completed.
    SetCommandTimeout(kDefaultLongCmdTimeout, "Device::Initialize (multi-command)");

    RetrieveDeviceProperties();
    RetrieveHealthState();
    RetrieveSignalProcessingState();  // On completion, this starts format-set-retrieval.
  } else {
    ADR_WARN_METHOD() << "Different device type: " << device_type_;
  }
}

// Use this when the Result might contain a domain_error or framework_error.
template <typename ResultT>
bool Device::SetDeviceErrorOnFidlError(const ResultT& result, const char* debug_context) {
  if (has_error()) {
    ADR_WARN_METHOD() << debug_context << ": device already has an error";
    return true;
  }
  if (result.is_error()) {
    if (result.error_value().is_framework_error()) {
      if (result.error_value().framework_error().is_canceled() ||
          result.error_value().framework_error().is_peer_closed()) {
        ADR_LOG_METHOD(kLogCodecFidlResponses || kLogCompositeFidlResponses)
            << debug_context << ": will take no action on " << result.error_value();
      } else {
        FX_LOGS(ERROR) << debug_context << " failed: " << result.error_value() << ")";
        OnError(result.error_value().framework_error().status());
      }
    } else {
      FX_LOGS(ERROR) << debug_context << " failed: " << result.error_value() << ")";
      OnError(ZX_ERR_INTERNAL);
    }
  }
  return result.is_error();
}

// Use this when the Result error is allowed to be a domain error (which will be ignored),
// but we want to take the device into an error state if it's a framework error.
// Returns true if a framework error was found (and handled by this function).
template <typename ResultT>
bool Device::SetDeviceErrorOnFidlFrameworkError(const ResultT& result, const char* debug_context) {
  if (has_error()) {
    ADR_WARN_METHOD() << debug_context << ": device already has an error";
    return true;
  }
  if (result.is_error()) {
    fidl::Status framework_status = fidl::Status::Ok();
    if constexpr (requires { result.error_value().is_framework_error(); }) {
      if (!result.error_value().is_framework_error()) {
        return false;
      }
      framework_status = result.error_value().framework_error();
    } else {
      framework_status = result.error_value();
    }

    if (framework_status.is_canceled() || framework_status.is_peer_closed()) {
      ADR_LOG_METHOD(kLogCodecFidlResponses || kLogCompositeFidlResponses)
          << debug_context << ": will take no action on " << framework_status;
    } else {
      FX_LOGS(ERROR) << debug_context << " failed: " << framework_status.status() << " ("
                     << framework_status << ")";
      OnError(framework_status.status());
    }
    return true;
  }
  return false;
}

void Device::RetrieveDeviceProperties() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  if (is_codec()) {
    RetrieveCodecProperties();
  } else if (is_composite()) {
    RetrieveCompositeProperties();
  }
}

void Device::RetrieveCodecProperties() {
  ADR_LOG_METHOD(kLogCodecFidlCalls);
  codec_client_.value()->GetProperties().Then(
      [this](fidl::Result<fha::Codec::GetProperties>& result) {
        if (SetDeviceErrorOnFidlFrameworkError(result, "GetProperties response")) {
          return;
        }

        ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/GetProperties: success";
        if (!ValidateCodecProperties(result->properties())) {
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        FX_CHECK(!has_codec_properties())
            << "Codec/GetProperties response: codec_properties_ already set";
        codec_properties_ = result->properties();
        SanitizeCodecPropertiesStrings(codec_properties_);

        inspect()->RecordProperties(codec_properties_->is_input(),
                                    codec_properties_->manufacturer(), codec_properties_->product(),
                                    codec_properties_->unique_id().has_value()
                                        ? std::optional(UidToString(codec_properties_->unique_id()))
                                        : std::nullopt,
                                    /* clock_domain */ std::nullopt);

        OnInitializationResponse();
      });
}

void Device::RetrieveCompositeProperties() {
  ADR_LOG_METHOD(kLogCompositeFidlCalls);

  composite_client_.value()->GetProperties().Then(
      [this](fidl::Result<fha::Composite::GetProperties>& result) {
        if (SetDeviceErrorOnFidlFrameworkError(result, "GetProperties response")) {
          return;
        }

        ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "Composite/GetProperties: success";
        if (!ValidateCompositeProperties(result->properties())) {
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        FX_CHECK(!has_composite_properties())
            << "Composite/GetProperties response: composite_properties_ already set";
        composite_properties_ = result->properties();
        SanitizeCompositePropertiesStrings(composite_properties_);
        // We have our clock domain now. Create the device clock.
        CreateDeviceClock();

        inspect()->RecordProperties(
            /* is_input */ std::nullopt, composite_properties_->manufacturer(),
            composite_properties_->product(),
            composite_properties_->unique_id().has_value()
                ? std::optional(UidToString(composite_properties_->unique_id()))
                : std::nullopt,
            composite_properties_->clock_domain());

        OnInitializationResponse();
      });
}

void Device::SanitizeCodecPropertiesStrings(std::optional<fha::CodecProperties>& codec_properties) {
  if (!codec_properties.has_value()) {
    FX_LOGS(ERROR) << __func__ << " called with unspecified CodecProperties";
    return;
  }

  if (codec_properties->manufacturer()) {
    codec_properties->manufacturer(codec_properties->manufacturer()->substr(
        0, std::min<uint64_t>(codec_properties->manufacturer()->find('\0'),
                              fha::kMaxUiStringSize - 1)));
  }
  if (codec_properties->product()) {
    codec_properties->product(codec_properties->product()->substr(
        0, std::min<uint64_t>(codec_properties->product()->find('\0'), fha::kMaxUiStringSize - 1)));
  }
}

void Device::SanitizeCompositePropertiesStrings(
    std::optional<fha::CompositeProperties>& composite_properties) {
  if (!composite_properties.has_value()) {
    FX_LOGS(ERROR) << __func__ << " called with unspecified CompositeProperties";
    return;
  }

  if (composite_properties->manufacturer()) {
    composite_properties->manufacturer(composite_properties->manufacturer()->substr(
        0, std::min<uint64_t>(composite_properties->manufacturer()->find('\0'),
                              fha::kMaxUiStringSize - 1)));
  }
  if (composite_properties->product()) {
    composite_properties->product(composite_properties->product()->substr(
        0, std::min<uint64_t>(composite_properties->product()->find('\0'),
                              fha::kMaxUiStringSize - 1)));
  }
}

// TODO(https://fxbug.dev/42068381): Decide when we proactively call GetHealthState, if at all.
void Device::RetrieveHealthState() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error; ignoring this";
    return;
  }
  ADR_LOG_METHOD(kLogCodecFidlCalls || kLogCompositeFidlCalls);

  if (is_codec()) {
    codec_client_.value()->GetHealthState().Then(
        [this](fidl::Result<fha::Codec::GetHealthState>& result) {
          if (SetDeviceErrorOnFidlFrameworkError(result, "HealthState response")) {
            return;
          }
          ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/GetHealthState: success";
          SetHealthState(result->state().healthy());
        });
  } else if (is_composite()) {
    if (has_health_state()) {
      SetCommandTimeout(kDefaultShortCmdTimeout, "GetHealthState");
    }
    composite_client_.value()->GetHealthState().Then(
        [this](fidl::Result<fha::Composite::GetHealthState>& result) {
          if (has_health_state()) {
            ClearCommandTimeout();
          }

          if (has_error()) {
            ADR_WARN_OBJECT() << "device already has an error; ignoring this";
            return;
          }
          if (SetDeviceErrorOnFidlFrameworkError(result, "HealthState response")) {
            return;
          }
          ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "Composite/GetHealthState: success";
          SetHealthState(result->state().healthy());
        });
  }
}

void Device::SetHealthState(std::optional<bool> is_healthy) {
  auto preexisting_health_state = health_state_;

  // An empty health state is permitted; it still indicates that the driver is responsive.
  health_state_ = is_healthy.value_or(true);
  // ...but if the driver actually self-reported as unhealthy, this is a problem.
  if (!*health_state_) {
    ADR_WARN_METHOD() << "RetrieveHealthState response: .healthy is FALSE (unhealthy)";
    OnError(ZX_ERR_IO);
    return;
  }

  inspect()->RecordDeviceHealthOk();

  ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "RetrieveHealthState response: healthy";
  if (!preexisting_health_state.has_value()) {
    OnInitializationResponse();
  }
}

void Device::RetrieveSignalProcessingState() {
  ADR_LOG_METHOD(kLogDeviceMethods || kLogSignalProcessingState);

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  auto [sig_proc_client_end, sig_proc_server_end] =
      fidl::Endpoints<fhasp::SignalProcessing>::Create();

  if (is_codec()) {
    ADR_LOG_METHOD(kLogCodecFidlCalls) << "calling SignalProcessingConnect";
    if (!codec_client_->is_valid()) {
      return;
    }
    auto status = codec_client_.value()->SignalProcessingConnect(std::move(sig_proc_server_end));

    if (status.is_error()) {
      if (status.error_value().is_canceled()) {
        // These indicate that we are already shutting down, so they aren't error conditions.
        ADR_LOG_METHOD(kLogCodecFidlResponses)
            << "Codec/SignalProcessingConnect response will take no action on error "
            << status.error_value().FormatDescription();
        return;
      }

      FX_PLOGS(ERROR, status.error_value().status()) << __func__ << " returned error:";
      OnError(status.error_value().status());
      return;
    }
    ADR_LOG_METHOD(kLogCodecFidlResponses) << "success";
  } else if (is_composite()) {
    ADR_LOG_METHOD(kLogCompositeFidlCalls) << "calling SignalProcessingConnect";
    auto status =
        composite_client_.value()->SignalProcessingConnect(std::move(sig_proc_server_end));

    if (status.is_error()) {
      if (status.error_value().is_canceled()) {
        // These indicate that we are already shutting down, so they aren't error conditions.
        ADR_LOG_METHOD(kLogCompositeFidlResponses)
            << "Composite/SignalProcessingConnect response will take no action on error "
            << status.error_value().FormatDescription();
        return;
      }

      FX_PLOGS(ERROR, status.error_value().status()) << __func__ << " returned error:";
      OnError(status.error_value().status());
      return;
    }
    ADR_LOG_METHOD(kLogCompositeFidlResponses) << "success";
  }

  sig_proc_client_.emplace();
  sig_proc_handler_ = {this, "SignalProcessing"};
  sig_proc_client_->Bind(std::move(sig_proc_client_end), dispatcher_, &sig_proc_handler_);

  RetrieveSignalProcessingElements();
  RetrieveSignalProcessingTopologies();
}

void Device::RetrieveSignalProcessingElements() {
  // Must be false -- THIS is what leads to supports_signalprocessing being set.
  if (supports_signalprocessing()) {
    ADR_WARN_METHOD() << "unexpected signalprocessing query (already supported)";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  // Another signalprocessing query (retrieve topologies) must have returned NOT_SUPPORTED.
  if (checked_for_signalprocessing()) {
    ADR_LOG_METHOD(kLogSignalProcessingState)
        << "ignoring signalprocessing query (already checked)";
    return;
  }
  FX_CHECK(!is_operational());  // This is part of initialization; we shouldn't be operational yet.
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  FX_CHECK(sig_proc_client_->is_valid());
  sig_proc_client_.value()->GetElements().Then(
      [this](fidl::Result<fhasp::SignalProcessing::GetElements>& result) {
        std::string context("signalprocessing::GetElements response");
        if (result.is_error() && result.error_value().is_domain_error() &&
            result.error_value().domain_error() == ZX_ERR_NOT_SUPPORTED) {
          ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context << ": NOT_SUPPORTED";

          SetSignalProcessingSupported(false);
          return;
        }
        if (SetDeviceErrorOnFidlError(result, context.c_str())) {
          return;
        }

        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;
        if (!ValidateElements(result->processing_elements())) {
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        sig_proc_elements_ = result->processing_elements();
        sig_proc_element_map_ = MapElements(sig_proc_elements_);
        if (sig_proc_element_map_.empty()) {
          ADR_WARN_OBJECT() << "Empty element map";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }
        for (const auto& [element_id, element] : sig_proc_element_map_) {
          element_ids_.insert(element_id);

          auto element_ptr = inspect()->RecordElement(element_id, element.element);
          if (!element_ptr) {
            ADR_WARN_OBJECT() << "Failed to record element";
            continue;
          }
        }
        OnSignalProcessingInitializationResponse();

        // Now that we know we support signalprocessing, query the current element states.
        // These initial states are not required for our DeviceInfo struct; the queries can
        // complete after we mark the device as operational and make it available to clients.
        RetrieveSignalProcessingElementStates();

        if (is_composite()) {
          // Now we know our element IDs, we can query DAI, RingBuffer and PacketStream elements.
          RetrieveDaiFormatSets();
          RetrieveRingBufferFormatSets();
          RetrievePacketStreamFormatSets();
        }
      });
}

void Device::RetrieveSignalProcessingTopologies() {
  // Must be false -- THIS is what leads to supports_signalprocessing being set.
  if (supports_signalprocessing()) {
    ADR_WARN_METHOD() << "unexpected signalprocessing query (already supported)";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  // Another signalprocessing query (retrieve elements) must have returned NOT_SUPPORTED.
  if (checked_for_signalprocessing()) {
    ADR_LOG_METHOD(kLogSignalProcessingState)
        << "ignoring signalprocessing query (already checked)";
    return;
  }
  FX_CHECK(!is_operational());  // This is part of initialization; we shouldn't be operational yet.
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  FX_CHECK(sig_proc_client_->is_valid());
  sig_proc_client_.value()->GetTopologies().Then(
      [this](fidl::Result<fhasp::SignalProcessing::GetTopologies>& result) {
        std::string context("signalprocessing::GetTopologies response");
        if (result.is_error() && result.error_value().is_domain_error() &&
            result.error_value().domain_error() == ZX_ERR_NOT_SUPPORTED) {
          SetSignalProcessingSupported(false);
          return;
        }
        if (SetDeviceErrorOnFidlError(result, context.c_str())) {
          return;
        }

        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;
        if (!ValidateTopologies(result->topologies(), sig_proc_element_map_)) {
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        sig_proc_topologies_ = result->topologies();
        sig_proc_topology_map_ = MapTopologies(sig_proc_topologies_);
        if (sig_proc_topology_map_.empty()) {
          ADR_WARN_OBJECT() << "Empty topology map";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        for (const auto& [topology_id, edge_pairs] : sig_proc_topology_map_) {
          topology_ids_.insert(topology_id);
          auto topology_ptr = inspect()->RecordTopology(topology_id, edge_pairs);
          if (!topology_ptr) {
            ADR_WARN_OBJECT() << "Failed to record topology (id " << topology_id << ")";
          }
        }
        OnSignalProcessingInitializationResponse();

        // Now that we know we support signalprocessing, query the current topology.
        // The initial topology is not required for our DeviceInfo struct; the query can complete
        // after we mark the device as operational and make it available to clients.
        RetrieveSignalProcessingTopology();
      });
}

// An initial signalprocessing query returned successfully. Is initialization complete?
void Device::OnSignalProcessingInitializationResponse() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device has already encountered a problem; ignoring this";
    return;
  }

  if (checked_for_signalprocessing()) {
    ADR_WARN_METHOD() << "unexpected signalprocessing query response (already checked)";
    return;
  }

  if (is_operational()) {
    ADR_WARN_METHOD() << "unexpected signalprocessing query response (not Initializing)";
  }
  ADR_LOG_METHOD(kLogSignalProcessingState);

  if (!sig_proc_element_map_.empty() && !sig_proc_topology_map_.empty()) {
    SetSignalProcessingSupported(true);
    OnInitializationResponse();
  }
}

// We support signalprocessing; we know our element list. For each, retrieve its initial state.
void Device::RetrieveSignalProcessingElementStates() {
  // We know that `GetElements` completed successfully but `GetTopologies` might still be pending.
  // Only fail/exit if it DID complete and told us that we do NOT support signalprocessing.
  if (checked_for_signalprocessing() && !supports_signalprocessing()) {
    ADR_WARN_METHOD() << "device does not support signalprocessing";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  // Although this is one of the initial driver queries, it is not a prerequisite for transitioning
  // the Device from Initializing to Initialized. So don't check is_operational() here.
  ADR_LOG_METHOD(kLogSignalProcessingState);

  // `RetrieveSignalProcessingElements` has completed successfully, so we have the info needed for
  // the DeviceInfo struct. We need not wait for these WatchElementState calls complete: if a client
  // retrieves ElementState, it will simply pend until this underlying Device retrieval completes.
  for (auto& [element_id, _] : sig_proc_element_map_) {
    RetrieveSignalProcessingElementState(element_id);
  }
}

void Device::RetrieveSignalProcessingElementState(ElementId element_id) {
  auto element_match = sig_proc_element_map_.find(element_id);
  FX_CHECK(element_match != sig_proc_element_map_.end());
  auto& element = element_match->second.element;

  // Although this is one of the initial driver queries, it is not a prerequisite for transitioning
  // the Device from Initializing to Initialized. Furthermore it can be called after initialization.
  // So don't check is_operational() here.
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  FX_CHECK(sig_proc_client_->is_valid());
  sig_proc_client_.value()
      ->WatchElementState({element_id})
      .Then([this, element_id,
             element](fidl::Result<fhasp::SignalProcessing::WatchElementState>& result) {
        std::string context("signalprocessing::WatchElementState response: element_id ");
        context.append(std::to_string(element_id));
        if (SetDeviceErrorOnFidlFrameworkError(result, context.c_str())) {
          return;
        }
        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;

        auto element_state = result->state();
        if (!ValidateElementState(element_state, element)) {
          // Either (a) sig_proc_topology_map_.at(element_id).element is incorrect,
          //     or (b) the driver is behaving incorrectly.
          FX_LOGS(ERROR) << context << ": not found";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        // Save the state and notify Observers, but only if this is a CHANGE in element state.
        auto element_record = sig_proc_element_map_.find(*element.id());
        if (element_record->second.state.has_value() &&
            *element_record->second.state == element_state) {
          ADR_LOG_OBJECT(kLogNotifyMethods)
              << "Not sending ElementStateIsChanged: state is unchanged.";
        } else {
          element_record->second.state = element_state;
          inspect()->RecordElementState(element_id, element_state);
          // Notify any Observers of this change in element state.
          ADR_LOG_OBJECT(kLogNotifyMethods)
              << "ForEachObserver => ElementStateIsChanged(" << element_id << ")";
          ForEachObserver([element_id, element_state](const auto& obs) {
            obs->ElementStateIsChanged(element_id, element_state);
          });
        }

        // Register for any future change in element state, whether this was a change or not.
        RetrieveSignalProcessingElementState(element_id);
      });
}

// If the method does not return ZX_OK, then the driver was not called.
zx_status_t Device::SetElementState(ElementId element_id,
                                    const fhasp::SettableElementState& element_state) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device must be controlled before this method can be called";
    return ZX_ERR_ACCESS_DENIED;
  }

  if (!supports_signalprocessing()) {
    ADR_WARN_METHOD() << "device does not support signalprocessing";
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return ZX_ERR_IO;
  }
  FX_CHECK(is_operational());

  if (!sig_proc_element_map_.contains(element_id)) {
    ADR_WARN_METHOD() << "invalid element_id " << element_id;
    return ZX_ERR_INVALID_ARGS;
  }

  if (!ValidateSettableElementState(element_state, sig_proc_element_map_.at(element_id).element)) {
    ADR_WARN_METHOD() << "invalid ElementState for element_id " << element_id;
    return ZX_ERR_INVALID_ARGS;
  }
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  // We don't check/prevent "no-change" here, since sig_proc_element_map_ may not reflect in-flight
  // updates. We update sig_proc_element_map_ and call ObserverNotify::ElementStateIsChanged (or
  // not, if no change) at only one place: the WatchElementState response handler.

  FX_CHECK(sig_proc_client_->is_valid());
  SetCommandTimeout(kDefaultShortCmdTimeout, "SetElementState");
  sig_proc_client_.value()
      ->SetElementState({element_id, element_state})
      .Then([this, element_id](fidl::Result<fhasp::SignalProcessing::SetElementState>& result) {
        ClearCommandTimeout();

        std::string context("SigProc::SetElementState response: element_id ");
        context.append(std::to_string(element_id));
        if (SetDeviceErrorOnFidlError(result, context.c_str())) {
          return;
        }

        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;
        // Our hanging WatchElementState call will complete now, updating our cached state and
        // calling ObserverNotify::ElementStateIsChanged (or not, if no change).
      });

  return ZX_OK;
}

void Device::RetrieveSignalProcessingTopology() {
  // We know that `GetTopologies` completed successfully but `GetElements` might still be pending.
  // Only fail/exit if it DID complete and told us that we do NOT support signalprocessing.
  if (checked_for_signalprocessing() && !supports_signalprocessing()) {
    ADR_WARN_METHOD() << "device does not support signalprocessing";
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  // Although this is one of the initial driver queries, it is not a prerequisite for transitioning
  // the Device from Initializing to Initialized. Furthermore it can be called after initialization.
  // So don't check is_operational() here.
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  FX_CHECK(sig_proc_client_->is_valid());
  sig_proc_client_.value()->WatchTopology().Then(
      [this](fidl::Result<fhasp::SignalProcessing::WatchTopology>& result) {
        std::string context("signalprocessing::WatchTopology response");
        if (SetDeviceErrorOnFidlFrameworkError(result, context.c_str())) {
          return;
        }
        TopologyId topology_id = result->topology_id();
        context.append(": topology_id ").append(std::to_string(topology_id));
        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;

        // Either (a) sig_proc_topology_map_ is incorrect, or (b) the driver is poorly-behaved.
        if (!sig_proc_topology_map_.contains(topology_id)) {
          FX_LOGS(ERROR) << context << ": not found";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        // Save the topology and notify Observers, but only if this is a change in topology.
        if (!current_topology_id_.has_value() || *current_topology_id_ != topology_id) {
          inspect()->RecordActiveTopology(topology_id);
          current_topology_id_ = topology_id;
          ADR_LOG_OBJECT(kLogNotifyMethods)
              << "ForEachObserver => TopologyIsChanged: " << topology_id;
          ForEachObserver([topology_id](const auto& obs) { obs->TopologyIsChanged(topology_id); });
        }

        // Register for any future change in topology, whether this was a change or not.
        RetrieveSignalProcessingTopology();
      });
}

// If the method does not return ZX_OK, then the driver was not called.
zx_status_t Device::SetTopology(uint64_t topology_id) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot SetTopology";
    return ZX_ERR_ACCESS_DENIED;
  }

  if (!supports_signalprocessing()) {
    ADR_WARN_METHOD() << "device does not support signalprocessing";
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return ZX_ERR_IO;
  }
  FX_CHECK(is_operational());

  if (!sig_proc_topology_map_.contains(topology_id)) {
    ADR_WARN_METHOD() << "invalid topology_id " << topology_id;
    return ZX_ERR_INVALID_ARGS;
  }
  ADR_LOG_METHOD(kLogSignalProcessingFidlCalls);

  // We don't check/prevent "no-change" here, since current_topology_id_ may not reflect in-flight
  // updates. We update current_topology_id_ and call ObserverNotify::TopologyIsChanged (or
  // not, if no change) at only one place: the WatchTopology response handler.

  FX_CHECK(sig_proc_client_->is_valid());
  SetCommandTimeout(kDefaultShortCmdTimeout, "SetTopology");
  sig_proc_client_.value()
      ->SetTopology(topology_id)
      .Then([this, topology_id](fidl::Result<fhasp::SignalProcessing::SetTopology>& result) {
        ClearCommandTimeout();

        std::string context("SigProc::SetTopology response: topology_id ");
        context.append(std::to_string(topology_id));
        if (SetDeviceErrorOnFidlError(result, context.c_str())) {
          return;
        }

        ADR_LOG_OBJECT(kLogSignalProcessingFidlResponses) << context;
        // Our hanging WatchTopology will complete, IF this changes the topology. That completer
        // (not this one) updates topology_id_ and calls ObserverNotify::TopologyIsChanged.
      });

  return ZX_OK;
}

void Device::RetrieveDaiFormatSets() {
  ADR_LOG_METHOD(kLogCodecFidlCalls || kLogCompositeFidlCalls);

  dai_ids_.clear();
  if (is_codec()) {
    dai_ids_.insert(fad::kDefaultDaiInterconnectElementId);
  } else if (is_composite()) {
    dai_ids_ = dais(sig_proc_element_map_);
  }
  auto remaining_dai_ids = std::make_shared<std::unordered_set<ElementId>>(dai_ids_);
  element_dai_format_sets_.clear();
  dai_inspect_nodes_.clear();
  for (auto element_id : *remaining_dai_ids) {
    auto element_node =
        inspect()->RecordDai(element_id, sig_proc_element_map_[element_id].element.description());
    dai_inspect_nodes_.insert({element_id, element_node});
    GetDaiFormatSets(element_id, [this, remaining_dai_ids](
                                     ElementId element_id,
                                     const std::vector<fuchsia_hardware_audio::DaiSupportedFormats>&
                                         dai_format_sets) mutable {
      ADR_LOG_OBJECT(kLogCodecFidlResponses || kLogCompositeFidlResponses)
          << "GetDaiFormats(token " << token_id_ << ", element " << element_id << "): success";
      auto& element_node = dai_inspect_nodes_[element_id];

      element_dai_format_sets_.push_back({{
          .element_id = element_id,
          .format_sets = dai_format_sets,
      }});
      element_node->RecordSupportedFormatSets(dai_format_sets);
      remaining_dai_ids->erase(element_id);
      if (remaining_dai_ids->empty()) {
        dai_format_sets_retrieved_ = true;
        OnInitializationResponse();
      }
    });
  }
  if (remaining_dai_ids->empty()) {
    dai_format_sets_retrieved_ = true;
    OnInitializationResponse();
  }
}

void Device::GetDaiFormatSets(
    ElementId element_id,
    fit::callback<void(ElementId, const std::vector<fuchsia_hardware_audio::DaiSupportedFormats>&)>
        dai_format_sets_callback) {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke dai_format_sets_callback: this device is in the process of being unwound.
    // The client has already received a HasError notification for this device.
    return;
  }
  // This is part of the initialization process, but might be called afterward as well,
  // so we can't check is_operational().

  ADR_LOG_METHOD(kLogCodecFidlCalls || kLogCompositeFidlCalls)
      << "token " << token_id_ << ", element " << element_id;
  if (is_codec()) {
    FX_CHECK(element_id == fad::kDefaultDaiInterconnectElementId);

    codec_client_.value()->GetDaiFormats().Then(
        [this, element_id, get_dai_format_sets_callback = std::move(dai_format_sets_callback)](
            fidl::Result<fha::Codec::GetDaiFormats>& result) mutable {
          if (SetDeviceErrorOnFidlError(result, "Codec/GetDaiFormats response")) {
            // We need not invoke the callback: this device is being unwound.
            return;
          }
          if (has_error()) {
            ADR_WARN_OBJECT() << "device already has an error";
            // We need not invoke the callback: this device is being unwound.
            return;
          }
          ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/GetDaiFormats: success";
          if (!ValidateDaiFormatSets(result->formats())) {
            OnError(ZX_ERR_INVALID_ARGS);
            // We need not invoke the callback: this device is being unwound.
            return;
          }
          get_dai_format_sets_callback(element_id, result->formats());
        });
  } else if (is_composite()) {
    composite_client_.value()
        ->GetDaiFormats(element_id)
        .Then(
            [this, element_id, get_dai_format_sets_callback = std::move(dai_format_sets_callback)](
                fidl::Result<fha::Composite::GetDaiFormats>& result) mutable {
              if (SetDeviceErrorOnFidlError(result, "Composite/GetDaiFormats response")) {
                // We need not invoke the callback: this device is  being unwound.
                return;
              }
              if (has_error()) {
                ADR_WARN_OBJECT() << "device already has an error";
                // We need not invoke the callback: this device is  being unwound.
                return;
              }
              ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "Composite/GetDaiFormats: success";
              if (!ValidateDaiFormatSets(result->dai_formats())) {
                OnError(ZX_ERR_INVALID_ARGS);
                // We need not invoke the callback: this device is  being unwound.
                return;
              }
              get_dai_format_sets_callback(element_id, result->dai_formats());
            });
  }
}

void Device::RetrieveRingBufferFormatSets() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  if (is_composite()) {
    ADR_LOG_METHOD(kLogRingBufferMethods);
    ring_buffer_ids_ = ring_buffers(sig_proc_element_map_);
  }
  element_ring_buffer_format_sets_.clear();
  auto remaining_ring_buffer_ids =
      std::make_shared<std::unordered_set<ElementId>>(ring_buffer_ids_);

  for (auto id : ring_buffer_ids_) {
    inspect()->RecordRingBuffer(id, sig_proc_element_map_[id].element.description());

    if (is_composite()) {
      ADR_LOG_METHOD(kLogCompositeFidlCalls) << " GetRingBufferFormats (element " << id << ")";
      composite_client_.value()->GetRingBufferFormats(id).Then(
          [this, id, remaining_ring_buffer_ids](
              fidl::Result<fha::Composite::GetRingBufferFormats>& result) mutable {
            if (SetDeviceErrorOnFidlError(result, "Composite/GetRingBufferFormats response")) {
              // We need not call AddRingBufferFormatSet: this device is in the process of being
              // unwound. The client has already received a HasError notification for this device.
              return;
            }
            ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "Composite/GetRingBufferFormats: success";
            AddRingBufferFormatSet(id, remaining_ring_buffer_ids, result->ring_buffer_formats());
            inspect()->RecordRingBufferSupportedFormatSets(
                id, TranslateRingBufferFormatSets(result->ring_buffer_formats()));
          });
    }
  }
  if (remaining_ring_buffer_ids->empty()) {
    ring_buffer_format_sets_retrieved_ = true;
    OnInitializationResponse();
  }
}

void Device::AddRingBufferFormatSet(
    ElementId id, std::shared_ptr<std::unordered_set<ElementId>>& remaining_ring_buffer_ids,
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& format_set) {
  std::string context{"GetRingBufferFormats (element "};
  context.append(std::to_string(id)).append(") response");
  if (has_error()) {
    ADR_WARN_OBJECT() << "device already has error during " << context;
    return;
  }
  if (!ValidateRingBufferFormatSets(format_set)) {
    OnError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto translated_ring_buffer_format_sets = TranslateRingBufferFormatSets(format_set);
  if (translated_ring_buffer_format_sets.empty()) {
    ADR_WARN_OBJECT() << "Failed to translate " << context;
    OnError(ZX_ERR_INVALID_ARGS);
    return;
  }
  ADR_LOG_METHOD(kLogCompositeFidlResponses) << context;

  element_driver_ring_buffer_format_sets_.emplace_back(id, format_set);
  element_ring_buffer_format_sets_.push_back({{
      .element_id = id,
      .format_sets = translated_ring_buffer_format_sets,
  }});
  remaining_ring_buffer_ids->erase(id);
  if (remaining_ring_buffer_ids->empty()) {
    ADR_LOG_OBJECT(kLogCompositeFidlResponses) << context << ": success";
    ring_buffer_format_sets_retrieved_ = true;
    OnInitializationResponse();
  }
}

void Device::RetrievePacketStreamFormatSets() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }

  FX_CHECK(is_composite());

  ADR_LOG_METHOD(kLogCompositeFidlCalls);
  packet_stream_ids_ = packet_streams(sig_proc_element_map_);
  element_packet_stream_format_sets_.clear();
  auto remaining_packet_stream_ids =
      std::make_shared<std::unordered_set<ElementId>>(packet_stream_ids_);

  for (auto id : packet_stream_ids_) {
    inspect()->RecordPacketStream(id, sig_proc_element_map_[id].element.description());

    ADR_LOG_METHOD(kLogCompositeFidlCalls) << " GetPacketStreamFormats (element " << id << ")";
    composite_client_.value()->GetPacketStreamFormats(id).Then(
        [this, id, remaining_packet_stream_ids](
            fidl::Result<fha::Composite::GetPacketStreamFormats>& result) mutable {
          if (SetDeviceErrorOnFidlError(result, "Composite/GetPacketStreamFormats response")) {
            // We need not call AddPacketStreamFormatSet: this device is in the process of being
            // unwound. The client has already received a HasError notification for this device.
            return;
          }
          AddPacketStreamFormatSet(id, remaining_packet_stream_ids,
                                   result->packet_stream_formats());
          inspect()->RecordPacketStreamSupportedFormatSets(
              id, TranslatePacketStreamFormatSets(result->packet_stream_formats()));
        });
  }
  if (remaining_packet_stream_ids->empty()) {
    packet_stream_format_sets_retrieved_ = true;
    OnInitializationResponse();
  }
}

void Device::AddPacketStreamFormatSet(
    ElementId id, std::shared_ptr<std::unordered_set<ElementId>>& remaining_packet_stream_ids,
    const std::vector<fuchsia_hardware_audio::SupportedFormats2>& format_set) {
  std::string context{"GetPacketStreamFormats (element "};
  context.append(std::to_string(id)).append(") response");
  if (has_error()) {
    return;
  }
  if (!ValidatePacketStreamFormatSets(format_set)) {
    OnError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto translated_formats = TranslatePacketStreamFormatSets(format_set);
  if (translated_formats.empty()) {
    ADR_WARN_OBJECT() << "Failed to translate " << context;
    OnError(ZX_ERR_INVALID_ARGS);
    return;
  }
  ADR_LOG_METHOD(kLogCompositeFidlResponses) << context;

  fuchsia_audio_device::ElementPacketStreamFormatSet element_packet_stream_format_set;
  element_packet_stream_format_set.element_id(id);
  element_packet_stream_format_set.format_sets(translated_formats);

  element_driver_packet_stream_format_sets_.emplace_back(id, format_set);
  element_packet_stream_format_sets_.push_back(std::move(element_packet_stream_format_set));
  remaining_packet_stream_ids->erase(id);
  if (remaining_packet_stream_ids->empty()) {
    ADR_LOG_OBJECT(kLogCompositeFidlResponses) << context << ": success";
    packet_stream_format_sets_retrieved_ = true;
    OnInitializationResponse();
  }
}

void Device::RetrievePlugState() {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  ADR_LOG_METHOD(kLogCodecFidlCalls);

  if (is_codec()) {
    codec_client_.value()->WatchPlugState().Then(
        [this](fidl::Result<fha::Codec::WatchPlugState>& result) {
          if (SetDeviceErrorOnFidlFrameworkError(result, "Codec/PlugState response")) {
            return;
          }
          ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/WatchPlugState success";

          std::optional<fha::PlugDetectCapabilities> plug_detect_capabilities =
              has_codec_properties() ? codec_properties_->plug_detect_capabilities() : std::nullopt;
          SetPlugState(result->plug_state(), plug_detect_capabilities);
        });
  }
}

void Device::SetPlugState(const fuchsia_hardware_audio::PlugState& plug_state,
                          std::optional<fha::PlugDetectCapabilities> plug_detect_capabilities) {
  if (!ValidatePlugState(plug_state, plug_detect_capabilities)) {
    OnError(ZX_ERR_INVALID_ARGS);
    return;
  }

  auto preexisting_plug_state = plug_state_;
  plug_state_ = plug_state;

  if (!preexisting_plug_state.has_value()) {
    ADR_LOG_OBJECT(kLogCodecFidlResponses) << "WatchPlugState received initial value";
    OnInitializationResponse();
  } else {
    ADR_LOG_OBJECT(kLogCodecFidlResponses) << "WatchPlugState received update";
    ADR_LOG_OBJECT(kLogNotifyMethods) << "ForEachObserver => PlugStateIsChanged";
    ForEachObserver([plug_state = *plug_state_](const auto& obs) {
      obs->PlugStateIsChanged(plug_state.plugged().value_or(true) ? fad::PlugState::kPlugged
                                                                  : fad::PlugState::kUnplugged,
                              zx::time(*plug_state.plug_state_time()));
    });
  }
  // Kick off the next watch.
  RetrievePlugState();
}

// Return a fuchsia_audio_device/Info object based on this device's member values.
// Required fields (guaranteed for the caller) include: token_id, device_type, device_name.
// Other fields are required for some driver types but optional or absent for others.
fad::Info Device::CreateDeviceInfo() {
  ADR_LOG_METHOD(kLogDeviceMethods);

  auto info = fad::Info{{
      // Required for all device types:
      .token_id = token_id_,
      .device_type = device_type_,
      .device_name = name_,
  }};
  // Required for Composite; optional for Codec:
  if (supports_signalprocessing()) {
    info.signal_processing_elements(sig_proc_elements_);
    info.signal_processing_topologies(sig_proc_topologies_);
  }
  if (is_codec()) {
    // Optional for all device types.
    info.manufacturer(codec_properties_->manufacturer())
        .product(codec_properties_->product())
        // Optional for Codec:
        .is_input(codec_properties_->is_input())
        // Required for Codec; optional for Composite:
        .dai_format_sets(dai_format_sets())
        // Required for Codec; absent for Composite:
        .plug_detect_caps(*codec_properties_->plug_detect_capabilities() ==
                                  fha::PlugDetectCapabilities::kHardwired
                              ? fad::PlugDetectCapabilities::kHardwired
                              : fad::PlugDetectCapabilities::kPluggable);
    // Codec properties stores unique_id as a string, so we must handle as a special case.
    if (codec_properties_->unique_id().has_value()) {
      std::array<unsigned char, fad::kUniqueInstanceIdSize> uid{};
      memcpy(uid.data(), codec_properties_->unique_id()->data(), fad::kUniqueInstanceIdSize);
      info.unique_instance_id(uid);
    }
  } else if (is_composite()) {
    // Optional for all device types:
    info.manufacturer(composite_properties_->manufacturer())
        .product(composite_properties_->product())
        .unique_instance_id(composite_properties_->unique_id())
        // Optional for Composite; absent for Codec:
        .ring_buffer_format_sets(element_ring_buffer_format_sets_)
        .packet_stream_format_sets(element_packet_stream_format_sets_)
        // Required for Codec; optional for Composite:
        .dai_format_sets(element_dai_format_sets_)
        // Required for Composite; absent for Codec:
        .clock_domain(composite_properties_->clock_domain());
  }

  return info;
}

void Device::SetDeviceInfo() {
  ADR_LOG_METHOD(kLogDeviceMethods);

  device_info_ = CreateDeviceInfo();

  if (!sig_proc_element_map_.empty()) {
    LogElementMap(sig_proc_element_map_);
  }

  (void)ValidateDeviceInfo(*device_info_);
}

void Device::CreateDeviceClock() {
  ADR_LOG_METHOD(kLogDeviceMethods);

  ClockDomain clock_domain;
  if (is_composite()) {
    FX_CHECK(composite_properties_->clock_domain().has_value()) << "Clock domain is required";
    clock_domain = composite_properties_->clock_domain().value_or(fha::kClockDomainMonotonic);
  } else {
    ADR_WARN_METHOD() << "Cannot create a device clock for device_type " << device_type();
    return;
  }

  device_clock_ = RealClock::CreateFromMonotonic("'" + name_ + "' device clock", clock_domain,
                                                 (clock_domain != fha::kClockDomainMonotonic));
}

// Create a duplicate handle to our clock with limited rights. We can transfer it to a client who
// can only read and duplicate. Specifically, they cannot change this clock's rate or offset.
zx::result<zx::clock> Device::GetReadOnlyClock() const {
  ADR_LOG_METHOD(kLogDeviceMethods);

  if (!device_clock_) {
    // This device type does not expose a clock.
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto dupe_clock = device_clock_->DuplicateZxClockReadOnly();
  if (!dupe_clock.has_value()) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }

  return zx::ok(std::move(*dupe_clock));
}

// Determine the full fuchsia_hardware_audio::Format needed for ConnectRingBufferFidl.
// This method expects that the required fields are present.
std::optional<fha::Format2> Device::SupportedRingBufferDriverFormatForClientFormat(
    ElementId element_id, const fuchsia_audio::Format& client_format) {
  auto driver_pcm = MapSampleTypeToDriverPcm(*client_format.sample_type());
  if (!driver_pcm) {
    FX_CHECK(false) << "Unhandled fuchsia_audio::SampleType: "
                    << static_cast<uint32_t>(*client_format.sample_type());
  }

  uint32_t channel_count = *client_format.channel_count();
  uint32_t frame_rate = *client_format.frames_per_second();

  std::vector<fha::SupportedFormats2> driver_ring_buffer_format_sets;
  for (const auto& element_entry_pair : element_driver_ring_buffer_format_sets_) {
    if (element_entry_pair.first == element_id) {
      driver_ring_buffer_format_sets = element_entry_pair.second;
      break;
    }
  }
  if (driver_ring_buffer_format_sets.empty()) {
    ADR_WARN_METHOD() << "no driver ring_buffer_format_set found for element_id " << element_id;
    return {};
  }

  fha::PcmFormat format_template{{
      .number_of_channels = static_cast<uint8_t>(channel_count),
      .sample_format = driver_pcm->sample_format,
      .bytes_per_sample = driver_pcm->bytes_per_sample,
      .valid_bits_per_sample = driver_pcm->max_valid_bits,
      .frame_rate = frame_rate,
  }};

  auto best_pcm_format = MatchPcmFormatSet(driver_ring_buffer_format_sets, format_template);
  if (!best_pcm_format) {
    ADR_WARN_METHOD() << "no intersection for client format: "
                      << static_cast<uint16_t>(channel_count) << "-chan " << frame_rate << "hz "
                      << *client_format.sample_type();
    return {};
  }

  ADR_LOG_METHOD(kLogRingBufferMethods)
      << "successful match for client format: " << channel_count << "-chan " << frame_rate << "hz "
      << *client_format.sample_type() << " (valid_bits "
      << static_cast<uint16_t>(best_pcm_format->valid_bits_per_sample()) << ")";

  return fha::Format2::WithPcmFormat(*best_pcm_format);
}

// Determine the full fuchsia_hardware_audio::Format2 needed for ConnectPacketStreamFidl.
// This method expects that the required fields are present.
std::optional<fha::Format2> Device::SupportedPacketStreamDriverFormatForClientFormat(
    ElementId element_id, const fad::PacketStreamFormat& client_format) {
  std::vector<fha::SupportedFormats2> driver_packet_stream_format_sets;
  for (const auto& element_entry_pair : element_driver_packet_stream_format_sets_) {
    if (element_entry_pair.first == element_id) {
      driver_packet_stream_format_sets = element_entry_pair.second;
      break;
    }
  }
  if (driver_packet_stream_format_sets.empty()) {
    ADR_WARN_METHOD() << "no driver packet_stream_format_set found for element_id " << element_id;
    return {};
  }

  if (client_format.pcm_format()) {
    const auto& pcm_format = client_format.pcm_format().value();
    auto driver_pcm = MapSampleTypeToDriverPcm(*pcm_format.sample_type());
    if (!driver_pcm) {
      FX_CHECK(false) << "Unhandled fuchsia_audio::SampleType: "
                      << static_cast<uint32_t>(*pcm_format.sample_type());
    }
    uint32_t channel_count = *pcm_format.channel_count();
    uint32_t frame_rate = *pcm_format.frames_per_second();

    fha::PcmFormat format_template{{
        .number_of_channels = static_cast<uint8_t>(channel_count),
        .sample_format = driver_pcm->sample_format,
        .bytes_per_sample = driver_pcm->bytes_per_sample,
        .valid_bits_per_sample = driver_pcm->max_valid_bits,
        .frame_rate = frame_rate,
    }};

    auto best_pcm_format = MatchPcmFormatSet(driver_packet_stream_format_sets, format_template);
    if (!best_pcm_format) {
      ADR_WARN_METHOD() << "no supported driver format found for client format";
      return {};
    }

    return fha::Format2::WithPcmFormat(*best_pcm_format);
  }

  if (!client_format.encoding()) {
    ADR_WARN_METHOD() << "client format missing both pcm_format and encoding";
    return {};
  }
  const auto& encoding = client_format.encoding().value();

  auto matched_encoding = MatchEncodingSet(driver_packet_stream_format_sets, encoding);
  if (matched_encoding) {
    return fha::Format2::WithEncoding(*matched_encoding);
  }
  ADR_WARN_METHOD() << "no supported driver encoding found for client encoding";
  return {};
}

// static
std::optional<fha::PcmFormat> Device::MatchPcmFormatSet(
    const std::vector<fha::SupportedFormats2>& supported_formats,
    const fha::PcmFormat& format_template) {
  std::optional<fha::PcmFormat> best_pcm_format;
  for (const auto& supported_format : supported_formats) {
    if (supported_format.Which() != fha::SupportedFormats2::Tag::kPcmSupportedFormats) {
      continue;
    }
    const auto& pcm_formats = supported_format.pcm_supported_formats().value();
    const auto& sample_formats = *pcm_formats.sample_formats();
    const auto& bytes_samples = *pcm_formats.bytes_per_sample();
    const auto& frame_rates = *pcm_formats.frame_rates();
    const auto& channel_sets = *pcm_formats.channel_sets();

    if (std::count(sample_formats.begin(), sample_formats.end(), format_template.sample_format()) &&
        std::count(bytes_samples.begin(), bytes_samples.end(),
                   format_template.bytes_per_sample()) &&
        std::count(frame_rates.begin(), frame_rates.end(), format_template.frame_rate()) &&
        std::count_if(
            channel_sets.begin(), channel_sets.end(),
            [channel_count = format_template.number_of_channels()](const fha::ChannelSet& cs) {
              return cs.attributes()->size() == channel_count;
            })) {
      uint8_t best_valid_bits = 0;
      for (auto v_bits : *pcm_formats.valid_bits_per_sample()) {
        if (v_bits <= format_template.valid_bits_per_sample()) {
          best_valid_bits = std::max(best_valid_bits, v_bits);
        }
      }
      if (best_valid_bits > 0) {
        if (!best_pcm_format || best_valid_bits > best_pcm_format->valid_bits_per_sample()) {
          best_pcm_format = format_template;
          best_pcm_format->valid_bits_per_sample(best_valid_bits);
        }
      }
    }
  }
  return best_pcm_format;
}

// static
std::optional<fha::Encoding> Device::MatchEncodingSet(
    const std::vector<fha::SupportedFormats2>& supported_formats, const fha::Encoding& encoding) {
  for (const auto& supported_format : supported_formats) {
    if (supported_format.Which() != fha::SupportedFormats2::Tag::kSupportedEncodings) {
      continue;
    }
    const auto& encodings = supported_format.supported_encodings().value();

    bool match = false;
    if (encodings.encoding_types()) {
      for (auto enc_type : *encodings.encoding_types()) {
        if (enc_type == encoding.encoding_type()) {
          match = true;
          break;
        }
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    if (encodings.decoded_channel_sets()) {
      for (const auto& channel_set : *encodings.decoded_channel_sets()) {
        if (channel_set.attributes()->size() == encoding.decoded_channel_count()) {
          match = true;
          break;
        }
      }
    }
    if (!match) {
      continue;
    }

    match = false;
    if (encodings.decoded_frame_rates()) {
      for (auto rate : *encodings.decoded_frame_rates()) {
        if (rate == encoding.decoded_frame_rate()) {
          match = true;
          break;
        }
      }
    }
    if (!match) {
      continue;
    }

    return encoding;
  }
  return std::nullopt;
}

// If the optional<weak_ptr> is set AND the weak_ptr can be locked to its shared_ptr, then the
// resulting shared_ptr is returned. Otherwise, nullptr is returned, after first resetting the
// optional `control_notify` if it is set but the weak_ptr is no longer valid.
std::shared_ptr<ControlNotify> Device::GetControlNotify() {
  if (!control_notify_) {
    return nullptr;
  }

  auto sh_ptr_control = control_notify_->lock();
  if (!sh_ptr_control) {
    control_notify_.reset();
    LogObjectCounts();
  }

  return sh_ptr_control;
}

////////////////
// Subsequent methods require the device to have a ControlNotify set. As a general pattern, before
// executing a method we check for the ControlNotify, then check for the appropriate device type,
// then check for device error, then check for the presence/validity of required parameters.

// This method guarantees that ControlNotify's DaiFormatIsChanged or DaiFormatIsNotChanged will
// eventually be called, either immediately or asynchronously when driver SetDaiFormat concludes.
void Device::SetDaiFormat(ElementId element_id, const fha::DaiFormat& dai_format) {
  auto notify = GetControlNotify();
  if (!notify) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot SetDaiFormat";
    return;
  }

  if (!is_codec() && !is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot SetDaiFormat";
    notify->DaiFormatIsNotChanged(element_id, dai_format,
                                  fad::ControlSetDaiFormatError::kWrongDeviceType);
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error: cannot SetDaiFormat";
    // We need not invoke DaiFormatIsNotChanged: this device is in the process of being unwound. The
    // client has already received a HasError notification for this device.
    return;
  }
  FX_CHECK(is_operational());

  if (is_codec() ? (element_id != fad::kDefaultDaiInterconnectElementId)
                 : (!dai_ids_.contains(element_id))) {
    ADR_WARN_METHOD()
        << "element_id not found, or not a DaiInterconnect element: cannot SetDaiFormat";
    notify->DaiFormatIsNotChanged(element_id, dai_format,
                                  fad::ControlSetDaiFormatError::kInvalidElementId);
    return;
  }

  if (!ValidateDaiFormat(dai_format)) {
    ADR_WARN_METHOD() << "Invalid dai_format: cannot SetDaiFormat";
    notify->DaiFormatIsNotChanged(element_id, dai_format,
                                  fad::ControlSetDaiFormatError::kInvalidDaiFormat);
    return;
  }

  if (!DaiFormatIsSupported(element_id, dai_format_sets(), dai_format)) {
    ADR_WARN_METHOD() << "Unsupported dai_format: cannot SetDaiFormat";
    notify->DaiFormatIsNotChanged(element_id, dai_format,
                                  fad::ControlSetDaiFormatError::kFormatMismatch);
    return;
  }

  // Check for no-change

  ADR_LOG_METHOD(kLogCodecFidlCalls || kLogCompositeFidlCalls);

  if (dai_inspect_nodes_.contains(element_id)) {
    dai_inspect_nodes_[element_id]->RecordSetDaiFormat(zx::clock::get_monotonic(), dai_format);
  } else {
    ADR_WARN_METHOD() << "No DAI inspect node found for element_id " << element_id;
  }

  if (is_codec()) {
    codec_client_.value()
        ->SetDaiFormat(dai_format)
        .Then([this, element_id, dai_format](fidl::Result<fha::Codec::SetDaiFormat>& result) {
          std::string context("Codec/SetDaiFormat response: ");
          auto notify = GetControlNotify();
          if (!notify) {
            ADR_WARN_OBJECT() << context
                              << "device must be controlled before SetDaiFormat can be called";
            return;
          }

          if (has_error()) {
            ADR_WARN_OBJECT() << context << "device already has an error";
            // We need not call DaiFormatIsNotChanged: device is in the process of being unwound.
            return;
          }

          if (!result.is_ok()) {
            fad::ControlSetDaiFormatError error;
            // These types of errors don't lead us to mark the device as in Error state.
            if (result.error_value().is_domain_error() &&
                (result.error_value().domain_error() == ZX_ERR_INVALID_ARGS ||
                 result.error_value().domain_error() == ZX_ERR_NOT_SUPPORTED)) {
              ADR_WARN_OBJECT() << context << "ZX_ERR_INVALID_ARGS or ZX_ERR_NOT_SUPPORTED";
              error = (result.error_value().domain_error() == ZX_ERR_NOT_SUPPORTED
                           ? fad::ControlSetDaiFormatError::kFormatMismatch
                           : fad::ControlSetDaiFormatError::kInvalidDaiFormat);
              notify->DaiFormatIsNotChanged(element_id, dai_format, error);
            } else {
              (void)SetDeviceErrorOnFidlError(result, context.c_str());
              // We need not call DaiFormatIsNotChanged: device is in the process of being unwound.
            }
            return;
          }

          ADR_LOG_OBJECT(kLogCodecFidlResponses) << context << "success";
          if (!ValidateCodecFormatInfo(result->state())) {
            FX_LOGS(ERROR) << context << "error " << result.error_value();
            OnError(ZX_ERR_INVALID_ARGS);
            // We need not call DaiFormatIsNotChanged: device is in the process of being unwound.
            return;
          }

          if (codec_format_.has_value() && codec_format_->dai_format == dai_format &&
              codec_format_->codec_format_info == result->state()) {
            // No DaiFormat change for this element
            notify->DaiFormatIsNotChanged(element_id, dai_format, fad::ControlSetDaiFormatError(0));
            return;
          }

          // Reset DaiFormat (and Start state if it's a change). Notify our controlling entity.
          codec_format_ = CodecFormat{
              .dai_format = dai_format,
              .codec_format_info = result->state(),
          };
          if (codec_start_state_.started) {
            codec_start_state_ = CodecStartState{
                .started = false,
                .start_stop_time = zx::clock::get_monotonic(),
            };
            notify->CodecIsStopped(codec_start_state_.start_stop_time);
          }
          notify->DaiFormatIsChanged(element_id, codec_format_->dai_format,
                                     codec_format_->codec_format_info);
        });
  } else {
    SetCommandTimeout(kDefaultShortCmdTimeout, "SetDaiFormat");
    composite_client_.value()
        ->SetDaiFormat({element_id, dai_format})
        .Then([this, element_id, dai_format](fidl::Result<fha::Composite::SetDaiFormat>& result) {
          ClearCommandTimeout();

          std::string context("Composite/SetDaiFormat response: ");
          auto notify = GetControlNotify();
          if (!notify) {
            ADR_WARN_OBJECT() << context
                              << "device must be controlled before SetDaiFormat can be called";
            return;
          }

          if (has_error()) {
            ADR_WARN_OBJECT() << context << "device already has an error";
            // We need not call DaiFormatIsNotChanged: device is in the process of being unwound.
            return;
          }

          if (!result.is_ok()) {
            fad::ControlSetDaiFormatError error;
            // These types of errors don't lead us to mark the device as in Error state.
            if (result.error_value().is_domain_error() &&
                (result.error_value().domain_error() == fha::DriverError::kInvalidArgs)) {
              ADR_WARN_OBJECT() << context << "kInvalidArgs";
              error = fad::ControlSetDaiFormatError::kInvalidDaiFormat;
            } else if (result.error_value().is_domain_error() &&
                       result.error_value().domain_error() == fha::DriverError::kNotSupported) {
              ADR_WARN_OBJECT() << context << "kNotSupported";
              error = fad::ControlSetDaiFormatError::kFormatMismatch;
            } else {
              (void)SetDeviceErrorOnFidlError(result, context.c_str());
              // We need not call DaiFormatIsNotChanged: device is in the process of being unwound.
              return;
            }
            notify->DaiFormatIsNotChanged(element_id, dai_format, error);
            return;
          }

          ADR_LOG_OBJECT(kLogCompositeFidlResponses) << context << "success";
          if (auto match = composite_dai_formats_.find(element_id);
              match != composite_dai_formats_.end() && match->second == dai_format) {
            // No DaiFormat change for this element
            notify->DaiFormatIsNotChanged(element_id, dai_format, fad::ControlSetDaiFormatError(0));
            return;
          }

          // Unlike Codec, Composite DAI elements don't generate `CodecIsStopped` notifications.
          composite_dai_formats_.insert_or_assign(element_id, dai_format);
          notify->DaiFormatIsChanged(element_id, dai_format, std::nullopt);
        });
  }
}

// If true is returned, then we guarantee to call ControlNotify::DeviceIsReset (and CodecIsStopped
// if the Start/Stop state is changed, and DaiFormatIsChanged if the DaiFormat is changed).
// If false is returned, then no change occurs and these notifications will not be called.
bool Device::Reset() {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot Reset";
    return false;
  }

  if (!is_codec() && !is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot Reset";
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "Device already has an error: cannot Reset";
    return false;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogCodecFidlCalls || kLogCompositeFidlCalls);

  if (is_codec()) {
    codec_client_.value()->Reset().Then([this](fidl::Result<fha::Codec::Reset>& result) {
      if (SetDeviceErrorOnFidlFrameworkError(result, "Codec/Reset")) {
        // We need not call any notifications: device is in the process of being unwound.
        return;
      }
      ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/Reset: success";

      auto notify = GetControlNotify();

      // Reset to Stopped (if Started), even if no ControlNotify listens for notifications.
      if (codec_start_state_.started) {
        codec_start_state_.started = false;
        codec_start_state_.start_stop_time = zx::clock::get_monotonic();
        if (notify) {
          notify->CodecIsStopped(codec_start_state_.start_stop_time);
        }
      }

      // Reset our DaiFormat, even if no ControlNotify listens for notifications.
      if (codec_format_.has_value()) {
        codec_format_.reset();
        if (notify) {
          notify->DaiFormatIsChanged(fad::kDefaultDaiInterconnectElementId, std::nullopt,
                                     std::nullopt);
        }
      }

      // If a ControlNotify is listening, notify it that the Reset has now completed.
      if (notify) {
        notify->DeviceIsReset();
      }
    });
  }
  if (is_composite()) {
    SetCommandTimeout(kDefaultShortCmdTimeout, "Reset");
    composite_client_.value()->Reset().Then([this](fidl::Result<fha::Composite::Reset>& result) {
      ClearCommandTimeout();

      if (SetDeviceErrorOnFidlError(result, "Composite/Reset")) {
        // We need not call any notifications: device is in the process of being unwound.
        return;
      }
      ADR_LOG_OBJECT(kLogCompositeFidlResponses) << "Composite/Reset: success";

      auto notify = GetControlNotify();

      // We expect DeviceDroppedRingBuffer(element_id) from the driver, for all RingBuffers.
      // We shouldn't need to expressly change their state in any way, reset any hanging gets
      // or clear 'ring_buffer_map_'. Upon receiving the DeviceDroppedRingBuffer notifications,
      // we destroy the client RingBuffer connections, and they will re-create them.
      //
      // We expect the same for PacketStreams: DeviceDroppedPacketStream(element_id) should be
      // received from the driver for all PacketStreams. Upon receiving these notifications, we
      // destroy the client PacketStream connections, and they will re-create them.

      // For each DAI node with DaiFormat set, explicitly reset its state. ADR does this because the
      // driver has no way of conveying that a DaiFormat has changed (or is no longer set).
      if (!composite_dai_formats_.empty()) {
        // If a ControlNotify is listening, notify it about each DAI node.
        if (notify) {
          for (const auto& [element_id, _] : composite_dai_formats_) {
            notify->DaiFormatIsChanged(element_id, std::nullopt, std::nullopt);
          }
        }
        composite_dai_formats_.clear();
      }

      // We expect WatchTopology and WatchElementState notifications from the driver -- but ONLY
      // IF these were set to something other than the power-on defaults (and thus change as a
      // result of Composite::Reset). This is why we never need to convey that the Topology or
      // ElementState are indeterminate (thus ObserverNotify need not emit OPTIONALs for
      // TopologyIsChanged and ElementStateIsChanged).
      // We shouldn't need to expressly touch the hardware in any way; the client will receive
      // these notifications and re-establish the Topology and ElementStates.

      // If a ControlNotify is listening, notify it that the Reset has now completed.
      if (notify) {
        notify->DeviceIsReset();
      }
    });
  }

  return true;
}

// If true is returned, then we guarantee to call CodecIsStarted/CodecIsNotStarted on ControlNotify.
// If false is returned, then this notification will not be called.
bool Device::CodecStart() {
  auto notify = GetControlNotify();
  if (!notify) {
    ADR_WARN_METHOD() << "Codec is not yet controlled: cannot CodecStart";
    return false;
  }

  if (!is_codec()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot CodecStart";
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error: cannot CodecStart";
    return false;
  }
  FX_CHECK(is_operational());

  if (!codec_format_.has_value()) {
    ADR_WARN_METHOD() << "Format is not yet set: cannot CodecStart";
    return false;
  }

  if (codec_start_state_.started) {
    ADR_LOG_METHOD(kLogCodecFidlCalls) << "Codec already started; immediately completing";
    notify->CodecIsStarted(codec_start_state_.start_stop_time);
    return true;
  }
  ADR_LOG_METHOD(kLogCodecFidlCalls);

  codec_client_.value()->Start().Then([this](fidl::Result<fha::Codec::Start>& result) {
    auto notify = GetControlNotify();
    if (SetDeviceErrorOnFidlFrameworkError(result, "Codec/Start response")) {
      // We need not call CodecIsNotStarted: this device is in the process of being unwound.
      return;
    }

    ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/Start: success";

    // Notify our controlling entity, if this was a change.
    if (!codec_start_state_.started ||
        codec_start_state_.start_stop_time.get() <= result->start_time()) {
      codec_start_state_.started = true;
      codec_start_state_.start_stop_time = zx::time(result->start_time());
      if (notify) {
        notify->CodecIsStarted(codec_start_state_.start_stop_time);
      }
    }
  });

  return true;
}

// If true is returned, then we guarantee to call CodecIsStopped or CodecIsNotStopped on
// ControlNotify. If false is returned, then this notification will not be called.
bool Device::CodecStop() {
  auto notify = GetControlNotify();
  if (!notify) {
    ADR_WARN_METHOD() << "Codec is not yet controlled: cannot CodecStop";
    return false;
  }

  if (!is_codec()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot CodecStop";
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error: cannot CodecStop";
    return false;
  }
  FX_CHECK(is_operational());

  if (!codec_format_.has_value()) {
    ADR_WARN_METHOD() << "Format is not yet set: cannot CodecStop";
    return false;
  }

  if (!codec_start_state_.started) {
    ADR_LOG_METHOD(kLogCodecFidlCalls) << "Codec already stopped; immediately completing";
    notify->CodecIsStopped(codec_start_state_.start_stop_time);
    return true;
  }
  ADR_LOG_METHOD(kLogCodecFidlCalls);

  codec_client_.value()->Stop().Then([this](fidl::Result<fha::Codec::Stop>& result) {
    auto notify = GetControlNotify();
    if (SetDeviceErrorOnFidlFrameworkError(result, "Codec/Stop response")) {
      // We need not call CodecIsNotStopped: this device is in the process of being unwound.
      return;
    }

    ADR_LOG_OBJECT(kLogCodecFidlResponses) << "Codec/Stop: success";

    // Notify our controlling entity, if this was a change.
    if (codec_start_state_.started ||
        codec_start_state_.start_stop_time.get() <= result->stop_time()) {
      codec_start_state_.started = false;
      codec_start_state_.start_stop_time = zx::time(result->stop_time());
      if (notify) {
        notify->CodecIsStopped(codec_start_state_.start_stop_time);
      }
    }
  });

  return true;
}

// Expects the caller to have already checked if the RingBuffer has already been created,
// or if a Create request is already pending, and to have validated the format arguments.
// This method guarantees to call create_ring_buffer_callback (immediately or asynchronously).
// For certain unrecoverable errors, OnError is called as well.
bool Device::CreateRingBuffer(
    ElementId element_id, const fha::Format2& format, uint32_t requested_ring_buffer_bytes,
    fit::callback<void(fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>)>
        create_ring_buffer_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot CreateRingBuffer";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kOther));
    return false;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot CreateRingBuffer";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kWrongDeviceType));
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke create_ring_buffer_callback: this device is in the process of being
    // unwound. The client has already received a HasError notification for this device.
    return false;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogRingBufferMethods);

  if (!ring_buffer_ids_.contains(element_id)) {
    ADR_WARN_METHOD() << "No RingBuffer element found for id " << element_id
                      << ": cannot CreateRingBuffer";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kInvalidElementId));
    return false;
  }

  // Here, we detect all the error cases that we can, before calling into the driver.
  if (format.Which() == fha::Format2::Tag::kEncoding) {
    ADR_WARN_METHOD() << "RingBuffers do not support Encoding-based formats";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kInvalidFormat));
    return false;
  }
  if (format.Which() != fha::Format2::Tag::kPcmFormat) {
    ADR_WARN_METHOD() << "Unknown Format2 union variant";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kInvalidFormat));
    return false;
  }
  if (!ValidatePcmFormat(format.pcm_format().value())) {
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kInvalidFormat));
    return false;
  }

  auto bytes_per_sample = format.pcm_format().value().bytes_per_sample();
  auto sample_format = format.pcm_format().value().sample_format();
  if (!ValidateSampleFormatCompatibility(bytes_per_sample, sample_format)) {
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kInvalidFormat));
    return false;
  }

  if (!RingBufferFormatIsSupported(element_id, element_ring_buffer_format_sets_, format)) {
    ADR_WARN_METHOD() << "RingBuffer format not supported";
    create_ring_buffer_callback(fit::error(fad::ControlCreateRingBufferError::kFormatMismatch));
    return false;
  }

  ring_buffer_map_.erase(element_id);

  // This method creates the ring_buffer map entry, upon success.
  ConnectRingBufferFidl(
      element_id, format,
      [this, element_id, requested_ring_buffer_bytes, cb = std::move(create_ring_buffer_callback)](
          fuchsia_audio_device::ControlCreateRingBufferError status) mutable {
        if (status != fad::ControlCreateRingBufferError(0)) {
          cb(fit::error(status));
          return;
        }

        auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
        ring_buffer.requested_ring_buffer_bytes = requested_ring_buffer_bytes;
        ring_buffer.create_ring_buffer_callback = std::move(cb);

        RetrieveRingBufferProperties(element_id);
        RetrieveRingBufferDelayInfo(element_id);
      });

  return true;
}

// Expects the caller to have already checked if the PacketStream has already been created,
// or if a Create request is already pending, and to have validated the format arguments.
// This method guarantees to call create_packet_stream_callback (immediately or asynchronously).
// For certain unrecoverable errors, OnError is called as well.
bool Device::CreatePacketStream(
    ElementId element_id, const fha::Format2& format,
    fit::callback<void(fit::result<fad::ControlCreatePacketStreamError, Device::PacketStreamInfo>)>
        create_packet_stream_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot CreatePacketStream";
    create_packet_stream_callback(fit::error(fad::ControlCreatePacketStreamError::kOther));
    return false;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot CreatePacketStream";
    create_packet_stream_callback(
        fit::error(fad::ControlCreatePacketStreamError::kWrongDeviceType));
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke create_packet_stream_callback: this device is in the process of being
    // unwound. The client has already received a HasError notification for this device.
    return false;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogPacketStreamMethods);

  if (!packet_stream_ids_.contains(element_id)) {
    ADR_WARN_METHOD() << "No PacketStream element found for id " << element_id
                      << ": cannot CreatePacketStream";
    create_packet_stream_callback(
        fit::error(fad::ControlCreatePacketStreamError::kInvalidElementId));
    return false;
  }

  // Here, we detect all the error cases that we can, before calling into the driver.
  if (!ValidatePacketStreamFormat(format)) {
    create_packet_stream_callback(fit::error(fad::ControlCreatePacketStreamError::kInvalidFormat));
    return false;
  }

  if (format.pcm_format().has_value()) {
    auto bytes_per_sample = format.pcm_format()->bytes_per_sample();
    auto sample_format = format.pcm_format()->sample_format();
    if (!ValidateSampleFormatCompatibility(bytes_per_sample, sample_format)) {
      create_packet_stream_callback(
          fit::error(fad::ControlCreatePacketStreamError::kInvalidFormat));
      return false;
    }
  }

  if (!PacketStreamFormatIsSupported(element_id, element_packet_stream_format_sets_, format)) {
    ADR_WARN_METHOD() << "PacketStream format not supported";
    create_packet_stream_callback(fit::error(fad::ControlCreatePacketStreamError::kFormatMismatch));
    return false;
  }

  packet_stream_map_.erase(element_id);

  // This method creates the packet_stream map entry, upon success.
  ConnectPacketStreamFidl(element_id, format,
                          [this, element_id, cb = std::move(create_packet_stream_callback)](
                              fuchsia_audio_device::ControlCreatePacketStreamError status) mutable {
                            if (status != fad::ControlCreatePacketStreamError(0)) {
                              cb(fit::error(status));
                              return;
                            }

                            auto& packet_stream = packet_stream_map_.find(element_id)->second;
                            packet_stream.create_packet_stream_callback = std::move(cb);

                            RetrievePacketStreamProperties(element_id);
                          });

  return true;
}

void Device::ConnectRingBufferFidl(
    ElementId element_id, fha::Format2 driver_format,
    fit::callback<void(fad::ControlCreateRingBufferError status)> callback) {
  ADR_LOG_METHOD(kLogRingBufferMethods || kLogRingBufferFidlCalls);

  // We use kDefaultLongCmdTimeout here, as we don't clear the timeout until CreateRingBuffer,
  // GetProperties, GetRingBufferVmo and the initial WatchDelayInfo call have all completed.
  SetCommandTimeout(kDefaultLongCmdTimeout, "CreateRingBuffer (multi-command)");

  auto [client_end, server_end] = fidl::Endpoints<fha::RingBuffer>::Create();

  fha::Format2 format2 = driver_format;
  composite_client_.value()
      ->CreateRingBuffer({element_id, std::move(format2), std::move(server_end)})
      .Then([this, element_id, driver_format, client_end = std::move(client_end),
             callback = std::move(callback)](
                fidl::Result<fha::Composite::CreateRingBuffer>& result) mutable {
        std::string context{"Composite/CreateRingBuffer response"};
        if (result.is_error()) {
          ClearCommandTimeout();

          fad::ControlCreateRingBufferError error;
          bool is_device_error = false;

          // The `fuchsia.hardware.audio.DriverError` is mapped to
          // `fuchsia.audio.device.ControlCreateRingBufferError` here.
          // `kFatalError` is mapped to `kDeviceError`, which is not retryable.
          // All other errors are considered retryable.
          if (result.error_value().is_domain_error()) {
            switch (result.error_value().domain_error()) {
              case fha::DriverError::kInvalidArgs:
                error = fad::ControlCreateRingBufferError::kBadRingBufferOption;
                break;
              case fha::DriverError::kNotSupported:
                error = fad::ControlCreateRingBufferError::kFormatMismatch;
                break;
              case fha::DriverError::kWrongType:
                error = fad::ControlCreateRingBufferError::kWrongDeviceType;
                break;
              case fha::DriverError::kFatalError:
                is_device_error = SetDeviceErrorOnFidlError(result, context.c_str());
                FX_CHECK(is_device_error)
                    << "Invalid error interpretation. Fatal error should be considered a device error.";
                error = fad::ControlCreateRingBufferError::kDeviceError;
                break;
              case fha::DriverError::kShouldWait:
                [[fallthrough]];
              case fha::DriverError::kInternalError:
                [[fallthrough]];
              default:
                error = fad::ControlCreateRingBufferError::kOther;
                break;
            }
          } else {
            // For a framework error, let the generic handler deal with it.
            is_device_error = SetDeviceErrorOnFidlError(result, context.c_str());
            if (is_device_error) {
              error = fad::ControlCreateRingBufferError::kDeviceError;
            } else {
              error = fad::ControlCreateRingBufferError::kOther;
            }
          }

          ADR_WARN_OBJECT() << "Failed to create ring buffer: " << result.error_value();
          callback(error);
          return;
        }

        ADR_LOG_OBJECT(kLogCompositeFidlResponses || kLogRingBufferFidlResponses)
            << "Composite/CreateRingBuffer: success";

        // Success path
        auto bytes_per_sample = driver_format.pcm_format()->bytes_per_sample();
        auto sample_format = driver_format.pcm_format()->sample_format();
        std::optional<fuchsia_audio::SampleType> client_sample_type;
        if (bytes_per_sample == 1 && sample_format == fha::SampleFormat::kPcmUnsigned) {
          client_sample_type = fuchsia_audio::SampleType::kUint8;
        } else if (bytes_per_sample == 2 && sample_format == fha::SampleFormat::kPcmSigned) {
          client_sample_type = fuchsia_audio::SampleType::kInt16;
        } else if (bytes_per_sample == 4 && sample_format == fha::SampleFormat::kPcmSigned) {
          client_sample_type = fuchsia_audio::SampleType::kInt32;
        } else if (bytes_per_sample == 4 && sample_format == fha::SampleFormat::kPcmFloat) {
          client_sample_type = fuchsia_audio::SampleType::kFloat32;
        } else if (bytes_per_sample == 8 && sample_format == fha::SampleFormat::kPcmFloat) {
          client_sample_type = fuchsia_audio::SampleType::kFloat64;
        }
        FX_CHECK(client_sample_type.has_value())
            << "Invalid sample format was not detected in ValidateSampleFormatCompatibility";

        RingBufferRecord ring_buffer_record{
            .ring_buffer_state = RingBufferState::NotCreated,
            .ring_buffer_handler = std::make_unique<RingBufferFidlErrorHandler<fha::RingBuffer>>(
                this, element_id, "RingBuffer"),
            .vmo_format =
                {
                    {
                        .sample_type = client_sample_type,
                        .channel_count = driver_format.pcm_format()->number_of_channels(),
                        .frames_per_second = driver_format.pcm_format()->frame_rate(),
                        // TODO(https://fxbug.dev/42168795): handle channel_layout when communicated
                        // from driver.
                    },
                },
            .driver_format = driver_format,
        };

        ring_buffer_record.ring_buffer_client = fidl::Client<fha::RingBuffer>(
            std::move(client_end), dispatcher_, ring_buffer_record.ring_buffer_handler.get());

        ring_buffer_map_.insert_or_assign(element_id, std::move(ring_buffer_record));
        SetRingBufferState(element_id, RingBufferState::Creating);

        callback(fad::ControlCreateRingBufferError(0));
      });
}

void Device::ConnectPacketStreamFidl(
    ElementId element_id, fha::Format2 driver_format,
    fit::callback<void(fad::ControlCreatePacketStreamError status)> callback) {
  ADR_LOG_METHOD(kLogPacketStreamMethods || kLogPacketStreamFidlCalls);

  // We use kDefaultLongCmdTimeout here, as we don't clear the timeout until CreatePacketStream,
  // GetProperties and GetPacketStreamSink (for outgoing streams) calls have all completed.
  SetCommandTimeout(kDefaultLongCmdTimeout, "CreatePacketStream (multi-command)");

  auto [client_end, server_end] = fidl::Endpoints<fha::PacketStreamControl>::Create();

  fha::Format2 format2 = driver_format;
  composite_client_.value()
      ->CreatePacketStream({element_id, std::move(format2), std::move(server_end)})
      .Then([this, element_id, driver_format, client_end = std::move(client_end),
             callback = std::move(callback)](
                fidl::Result<fha::Composite::CreatePacketStream>& result) mutable {
        std::string context{"Composite/CreatePacketStream response"};
        if (result.is_error()) {
          ClearCommandTimeout();

          fad::ControlCreatePacketStreamError error;
          bool is_device_error = false;

          // The `fuchsia.hardware.audio.DriverError` is mapped to
          // `fuchsia.audio.device.ControlCreatePacketStreamError` here.
          // `kFatalError` is mapped to `kDeviceError`, which is not retryable.
          // All other errors are considered retryable.
          if (result.error_value().is_domain_error()) {
            switch (result.error_value().domain_error()) {
              case fha::DriverError::kInvalidArgs:
                error = fad::ControlCreatePacketStreamError::kBadPacketStreamOption;
                break;
              case fha::DriverError::kNotSupported:
                error = fad::ControlCreatePacketStreamError::kFormatMismatch;
                break;
              case fha::DriverError::kWrongType:
                error = fad::ControlCreatePacketStreamError::kWrongDeviceType;
                break;
              case fha::DriverError::kFatalError:
                is_device_error = SetDeviceErrorOnFidlError(result, context.c_str());
                FX_CHECK(is_device_error)
                    << "Invalid error interpretation. Fatal error should be considered a device error.";
                error = fad::ControlCreatePacketStreamError::kDeviceError;
                break;
              case fha::DriverError::kShouldWait:
                [[fallthrough]];
              case fha::DriverError::kInternalError:
                [[fallthrough]];
              default:
                error = fad::ControlCreatePacketStreamError::kOther;
                break;
            }
          } else {
            // For a framework error, let the generic handler deal with it.
            is_device_error = SetDeviceErrorOnFidlError(result, context.c_str());
            if (is_device_error) {
              error = fad::ControlCreatePacketStreamError::kDeviceError;
            } else {
              error = fad::ControlCreatePacketStreamError::kOther;
            }
          }

          ADR_WARN_OBJECT() << "Failed to create packet stream: " << result.error_value();
          callback(error);
          return;
        }

        // Success path
        uint32_t channel_count;
        uint32_t frames_per_second;
        std::optional<fuchsia_audio::SampleType> client_sample_type;

        if (driver_format.pcm_format().has_value()) {
          client_sample_type =
              MapDriverPcmToSampleType(driver_format.pcm_format()->sample_format(),
                                       driver_format.pcm_format()->bytes_per_sample());
          FX_CHECK(client_sample_type.has_value())
              << "Invalid sample format was not detected in ConnectPacketStreamFidl.";

          channel_count = driver_format.pcm_format()->number_of_channels();
          frames_per_second = driver_format.pcm_format()->frame_rate();
        } else {
          FX_CHECK(driver_format.encoding().has_value());
          const auto& encoding = driver_format.encoding().value();
          channel_count = *encoding.decoded_channel_count();
          frames_per_second = *encoding.decoded_frame_rate();
        }

        auto configured_format = [&]() -> fuchsia_audio_device::PacketStreamFormat {
          if (client_sample_type.has_value()) {
            return fuchsia_audio_device::PacketStreamFormat::WithPcmFormat({{
                .sample_type = client_sample_type,
                .channel_count = channel_count,
                .frames_per_second = frames_per_second,
                // TODO(https://fxbug.dev/42168795): handle channel_layout when communicated from
                // driver.
            }});
          }
          return fuchsia_audio_device::PacketStreamFormat::WithEncoding(
              driver_format.encoding().value());
        }();

        PacketStreamRecord packet_stream_record{
            .packet_stream_state = PacketStreamState::NotCreated,
            .packet_stream_handler =
                std::make_unique<PacketStreamFidlErrorHandler<fha::PacketStreamControl>>(
                    this, element_id, "PacketStream"),
            .configured_format = std::move(configured_format),
            .driver_format = driver_format,
        };

        packet_stream_record.packet_stream_client = fidl::Client<fha::PacketStreamControl>(
            std::move(client_end), dispatcher_, packet_stream_record.packet_stream_handler.get());

        packet_stream_map_.insert_or_assign(element_id, std::move(packet_stream_record));
        SetPacketStreamState(element_id, PacketStreamState::Creating);

        callback(fad::ControlCreatePacketStreamError(0));
      });
}

void Device::RetrieveRingBufferProperties(ElementId element_id) {
  ADR_LOG_METHOD(kLogRingBufferMethods || kLogRingBufferFidlCalls);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());

  ring_buffer.ring_buffer_client.value()->GetProperties().Then(
      [this, &ring_buffer, element_id](fidl::Result<fha::RingBuffer::GetProperties>& result) {
        if (SetDeviceErrorOnFidlFrameworkError(result, "RingBuffer/GetProperties response")) {
          return;
        }

        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/GetProperties: success";
        if (!ValidateRingBufferProperties(result->properties())) {
          FX_LOGS(ERROR) << "RingBuffer/GetProperties error: " << result.error_value();
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        ring_buffer.ring_buffer_properties = result->properties();
        CheckForRingBufferReady(element_id);
      });
}

void Device::RetrievePacketStreamProperties(ElementId element_id) {
  ADR_LOG_METHOD(kLogPacketStreamMethods || kLogPacketStreamFidlCalls);

  auto& packet_stream = packet_stream_map_.find(element_id)->second;
  FX_CHECK(packet_stream.packet_stream_client.has_value() &&
           packet_stream.packet_stream_client->is_valid());

  packet_stream.packet_stream_client.value()->GetProperties().Then(
      [this, &packet_stream,
       element_id](fidl::Result<fha::PacketStreamControl::GetProperties>& result) {
        if (SetDeviceErrorOnFidlFrameworkError(result, "PacketStream/GetProperties response")) {
          return;
        }

        ADR_LOG_OBJECT(kLogPacketStreamFidlResponses) << "PacketStream/GetProperties: success";
        if (!ValidatePacketStreamProperties(result->properties())) {
          FX_LOGS(ERROR) << "PacketStream/GetProperties properties validation failed";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        packet_stream.packet_stream_properties = result->properties();
        CheckForPacketStreamReady(element_id);
      });

  FX_CHECK(current_topology_id_.has_value());
  bool expects_data_sink =
      ElementHasOutgoingEdges(sig_proc_topology_map_.at(*current_topology_id_), element_id);

  if (expects_data_sink) {
    packet_stream.packet_stream_client.value()->GetPacketStreamSink().Then(
        [this, &packet_stream,
         element_id](fidl::Result<fha::PacketStreamControl::GetPacketStreamSink>& result) {
          if (SetDeviceErrorOnFidlFrameworkError(result,
                                                 "PacketStream/GetPacketStreamSink response")) {
            return;
          }

          ADR_LOG_OBJECT(kLogPacketStreamFidlResponses)
              << "PacketStream/GetPacketStreamSink: success";
          if (!result->stream()) {
            FX_LOGS(ERROR) << "PacketStream/GetPacketStreamSink: missing 'stream'";
            OnError(ZX_ERR_INVALID_ARGS);
            return;
          }

          packet_stream.data_sink = std::move(result->stream().value());
          CheckForPacketStreamReady(element_id);
        });
  }
}

void Device::RetrieveRingBufferDelayInfo(ElementId element_id) {
  ADR_LOG_METHOD(kLogRingBufferMethods || kLogRingBufferFidlCalls);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());

  ring_buffer.ring_buffer_client.value()->WatchDelayInfo().Then(
      [this, &ring_buffer, element_id](fidl::Result<fha::RingBuffer::WatchDelayInfo>& result) {
        if (SetDeviceErrorOnFidlFrameworkError(result, "RingBuffer/WatchDelayInfo response")) {
          return;
        }
        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/WatchDelayInfo: success";

        if (!ValidateDelayInfo(result->delay_info())) {
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }

        ring_buffer.delay_info = result->delay_info();
        // If requested_ring_buffer_bytes_ is already set, but num_ring_buffer_frames_ isn't,
        // then we're getting delay info as part of creating a ring buffer. Otherwise,
        // requested_ring_buffer_bytes_ must be set separately before calling GetRingBufferVmo.
        if (ring_buffer.requested_ring_buffer_bytes && !ring_buffer.num_ring_buffer_frames) {
          // Needed, to set requested_ring_buffer_frames_ before calling GetRingBufferVmo.
          CalculateRequiredRingBufferSizes(element_id);

          FX_CHECK(device_info_->clock_domain().has_value());
          const auto clock_position_notifications_per_ring =
              *device_info_->clock_domain() == fha::kClockDomainMonotonic ? 0 : 2;
          GetRingBufferVmo(element_id,
                           static_cast<uint32_t>(ring_buffer.requested_ring_buffer_frames),
                           clock_position_notifications_per_ring);
        }

        // Notify our controlling entity, if we have one.
        if (auto notify = GetControlNotify(); notify) {
          notify->DelayInfoIsChanged(element_id,
                                     {{
                                         .internal_delay = ring_buffer.delay_info->internal_delay(),
                                         .external_delay = ring_buffer.delay_info->external_delay(),
                                     }});
        }
        RetrieveRingBufferDelayInfo(element_id);
      });
}

void Device::GetRingBufferVmo(ElementId element_id, uint32_t min_frames,
                              uint32_t position_notifications_per_ring) {
  ADR_LOG_METHOD(kLogRingBufferMethods || kLogRingBufferFidlCalls);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());
  FX_CHECK(ring_buffer.driver_format.has_value());

  ring_buffer.ring_buffer_client.value()
      ->GetVmo({{
          .min_frames = min_frames,
          .clock_recovery_notifications_per_ring = position_notifications_per_ring,
      }})
      .Then([this, &ring_buffer, element_id](fidl::Result<fha::RingBuffer::GetVmo>& result) {
        if (SetDeviceErrorOnFidlError(result, "RingBuffer/GetVmo response")) {
          return;
        }
        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/GetVmo: success";

        bool vmo_is_incoming;
        if (current_topology_id_.has_value()) {
          vmo_is_incoming =
              ElementHasIncomingEdges(sig_proc_topology_map_.at(*current_topology_id_), element_id);
        } else {
          vmo_is_incoming = info()->is_input().value_or(true);
        }
        zx_rights_t required_rights =
            vmo_is_incoming ? kRequiredVmoRightsForRead : kRequiredVmoRightsForReadWrite;
        if (!ValidateRingBufferVmo(result->ring_buffer(), result->num_frames(),
                                   *ring_buffer.driver_format, required_rights)) {
          FX_PLOGS(ERROR, ZX_ERR_INVALID_ARGS) << "Error in RingBuffer/GetVmo response";
          OnError(ZX_ERR_INVALID_ARGS);
          return;
        }
        ring_buffer.ring_buffer_vmo = std::move(result->ring_buffer());
        ring_buffer.num_ring_buffer_frames = result->num_frames();

        uint64_t total_ring_buffer_bytes =
            *ring_buffer.num_ring_buffer_frames * ring_buffer.bytes_per_frame;

        // Re-calculate the size of the client "zone" (if the driver over-allocates, this is
        // considered client space).
        if (vmo_is_incoming) {
          if (total_ring_buffer_bytes < ring_buffer.ring_buffer_producer_bytes) {
            FX_PLOGS(ERROR, ZX_ERR_INVALID_ARGS)
                << "RingBuffer/GetVmo: num_frames (" << *ring_buffer.num_ring_buffer_frames
                << ") is smaller than driver_transfer_bytes implies";
            OnError(ZX_ERR_INVALID_ARGS);
            return;
          }
          uint64_t previous_consumer_bytes = ring_buffer.ring_buffer_consumer_bytes;
          ring_buffer.ring_buffer_consumer_bytes =
              total_ring_buffer_bytes - ring_buffer.ring_buffer_producer_bytes;
          if (previous_consumer_bytes != ring_buffer.ring_buffer_consumer_bytes) {
            ADR_WARN_OBJECT() << "ring_buffer.ring_buffer_consumer_bytes was "
                              << previous_consumer_bytes << ", is now "
                              << ring_buffer.ring_buffer_consumer_bytes;
          }
        } else {
          if (total_ring_buffer_bytes < ring_buffer.ring_buffer_consumer_bytes) {
            FX_PLOGS(ERROR, ZX_ERR_INVALID_ARGS)
                << "RingBuffer/GetVmo: num_frames (" << *ring_buffer.num_ring_buffer_frames
                << ") is smaller than driver_transfer_bytes implies";
            OnError(ZX_ERR_INVALID_ARGS);
            return;
          }
          uint64_t previous_producer_bytes = ring_buffer.ring_buffer_producer_bytes;
          ring_buffer.ring_buffer_producer_bytes =
              total_ring_buffer_bytes - ring_buffer.ring_buffer_consumer_bytes;
          if (previous_producer_bytes != ring_buffer.ring_buffer_producer_bytes) {
            ADR_WARN_OBJECT() << "ring_buffer.ring_buffer_producer_bytes was "
                              << previous_producer_bytes << ", is now "
                              << ring_buffer.ring_buffer_producer_bytes;
          }
        }
        CheckForRingBufferReady(element_id);
      });
}

// RingBuffer FIDL successful-response handlers.
void Device::CheckForRingBufferReady(ElementId element_id) {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  ADR_LOG_METHOD(kLogRingBufferState);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  // Check whether we are tearing down, or conversely have already set up the ring buffer.
  if (ring_buffer.ring_buffer_state != RingBufferState::Creating) {
    return;
  }

  // We're creating the ring buffer but don't have all our prerequisites yet.
  if (!ring_buffer.ring_buffer_properties.has_value() || !ring_buffer.delay_info.has_value() ||
      !ring_buffer.num_ring_buffer_frames.has_value()) {
    return;
  }

  auto ref_clock = GetReadOnlyClock();
  if (!ref_clock.is_ok()) {
    ADR_WARN_METHOD() << "reference clock is not ok";
    ring_buffer.create_ring_buffer_callback(
        fit::error(fad::ControlCreateRingBufferError::kDeviceError));
    ring_buffer.create_ring_buffer_callback = nullptr;
    OnError(ZX_ERR_INTERNAL);
    return;
  }

  SetRingBufferState(element_id, RingBufferState::Stopped);

  FX_CHECK(ring_buffer.create_ring_buffer_callback);
  // This clears the "meta-command" timeout set earlier in ConnectRingBufferFidl, covering the
  // CreateRingBuffer, GetProperties, GetRingBufferVmo and initial WatchDelayInfo calls.
  ClearCommandTimeout();

  auto now = zx::clock::get_monotonic();
  FX_LOGS(INFO) << "RingBuffer: total frames " << *ring_buffer.num_ring_buffer_frames
                << " (producer "
                << ring_buffer.ring_buffer_producer_bytes / ring_buffer.bytes_per_frame
                << ", consumer "
                << ring_buffer.ring_buffer_consumer_bytes / ring_buffer.bytes_per_frame << ")";
  ring_buffer.create_ring_buffer_callback(fit::success(Device::RingBufferInfo{
      .ring_buffer = fuchsia_audio::RingBuffer{{
          .buffer = fuchsia_mem::Buffer{{
              .vmo = std::move(ring_buffer.ring_buffer_vmo),
              .size = *ring_buffer.num_ring_buffer_frames * ring_buffer.bytes_per_frame,
          }},
          .format = ring_buffer.vmo_format,
          .producer_bytes = ring_buffer.ring_buffer_producer_bytes,
          .consumer_bytes = ring_buffer.ring_buffer_consumer_bytes,
          .reference_clock = std::move(*ref_clock),
          .reference_clock_domain = device_info_->clock_domain(),
      }},
      .properties = fad::RingBufferProperties{{
          .valid_bits_per_sample = valid_bits_per_sample(element_id),
          .turn_on_delay = ring_buffer.ring_buffer_properties->turn_on_delay().value_or(0),
      }},
  }));
  ring_buffer.create_ring_buffer_callback = nullptr;

  ring_buffer.inspect_instance = inspect()->RecordRingBufferInstance(element_id, now);
  ring_buffer.inspect_instance->RecordFormat(
      ring_buffer.vmo_format.channel_count().value_or(0u),
      ring_buffer.vmo_format.frames_per_second().value_or(0u),
      ring_buffer.vmo_format.sample_type().value_or(fuchsia_audio::SampleType::Unknown()));
  ring_buffer.inspect_instance->RecordBuffer(
      ring_buffer.requested_ring_buffer_bytes.value_or(0ull),
      ring_buffer.ring_buffer_producer_bytes / ring_buffer.bytes_per_frame,
      ring_buffer.ring_buffer_consumer_bytes / ring_buffer.bytes_per_frame,
      *ring_buffer.num_ring_buffer_frames * ring_buffer.bytes_per_frame);
}

void Device::CheckForPacketStreamReady(ElementId element_id) {
  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    return;
  }
  ADR_LOG_METHOD(kLogPacketStreamState);

  auto& packet_stream = packet_stream_map_.find(element_id)->second;
  // Check whether we are tearing down, or conversely have already set up the packet stream.
  if (packet_stream.packet_stream_state != PacketStreamState::Creating) {
    return;
  }

  FX_CHECK(current_topology_id_.has_value());
  bool expects_data_sink =
      ElementHasOutgoingEdges(sig_proc_topology_map_.at(*current_topology_id_), element_id);

  // We're creating the packet stream but don't have all our prerequisites yet.
  if (!packet_stream.packet_stream_properties.has_value() ||
      (expects_data_sink && !packet_stream.data_sink.has_value())) {
    return;
  }

  SetPacketStreamState(element_id, PacketStreamState::Stopped);

  FX_CHECK(packet_stream.create_packet_stream_callback);
  // This clears the "meta-command" timeout set earlier in ConnectPacketStreamFidl, covering the
  // CreatePacketStream, GetProperties and (if applicable) GetPacketStreamSink calls.
  ClearCommandTimeout();

  auto now = zx::clock::get_monotonic();
  fad::PacketStreamProperties properties;
  properties.format(packet_stream.configured_format);
  if (packet_stream.data_sink.has_value()) {
    properties.data_sink(std::move(packet_stream.data_sink.value()));
  }
  if (packet_stream.driver_format && packet_stream.driver_format->pcm_format()) {
    properties.valid_bits_per_sample(
        packet_stream.driver_format->pcm_format()->valid_bits_per_sample());
  }
  properties.supported_buffer_types(
      packet_stream.packet_stream_properties->supported_buffer_types());

  packet_stream.inspect_instance = inspect()->RecordPacketStreamInstance(element_id, now);
  packet_stream.inspect_instance->RecordFormat(packet_stream.configured_format);

  packet_stream.create_packet_stream_callback(fit::success(Device::PacketStreamInfo{
      .properties = std::move(properties),
  }));
  packet_stream.create_packet_stream_callback = nullptr;
}

// Expects the caller to have already created the RingBuffer.
// Returns TRUE if it actually calls out to the driver. We avoid doing so if we already know
// that this RingBuffer does not support SetActiveChannels.
bool Device::SetActiveChannels(
    ElementId element_id, uint64_t channel_bitmask,
    fit::callback<void(zx::result<zx::time>)> set_active_channels_callback) {
  TRACE_DURATION("power-audio", "ADR::Device::SetActiveChannels", "bitmask", channel_bitmask);
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot SetActiveChannels";
    TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
                  "reason", "Device is not yet controlled", "bitmask", channel_bitmask);
    set_active_channels_callback(zx::error(ZX_ERR_INTERNAL));
    return false;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot SetActiveChannels";
    TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
                  "reason", "Incorrect device type", "bitmask", channel_bitmask);
    set_active_channels_callback(zx::error(ZX_ERR_INTERNAL));
    return false;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke set_active_channels_callback: this device is in the process of being
    // unwound. The client has already received a HasError notification for this device.
    TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
                  "reason", "Device already has an error", "bitmask", channel_bitmask);
    return false;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogRingBufferFidlCalls) << "(0x" << std::hex << channel_bitmask << ")";

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());

  // If we already know this device doesn't support SetActiveChannels, do nothing.
  if (!ring_buffer.supports_set_active_channels.value_or(true)) {
    TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels exit", TRACE_SCOPE_PROCESS,
                  "reason", "Device does not support SetActiveChannels", "bitmask",
                  channel_bitmask);
    return false;
  }
  SetCommandTimeout(kDefaultShortCmdTimeout, "SetActiveChannels");
  ring_buffer.ring_buffer_client.value()
      ->SetActiveChannels({{
          .active_channels_bitmask = channel_bitmask,
      }})
      .Then([this, &ring_buffer, channel_bitmask,
             callback = std::move(set_active_channels_callback)](
                fidl::Result<fha::RingBuffer::SetActiveChannels>& result) mutable {
        ClearCommandTimeout();

        if (result.is_error() && result.error_value().is_domain_error() &&
            result.error_value().domain_error() == ZX_ERR_NOT_SUPPORTED) {
          ADR_LOG_OBJECT(kLogRingBufferFidlResponses)
              << "RingBuffer/SetActiveChannels: device does not support this method";
          ring_buffer.supports_set_active_channels = false;
          TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels response",
                        TRACE_SCOPE_PROCESS, "reason", "Device does not support SetActiveChannels",
                        "bitmask", channel_bitmask);
          callback(zx::error(ZX_ERR_NOT_SUPPORTED));
          return;
        }
        if (SetDeviceErrorOnFidlError(result, "RingBuffer/SetActiveChannels response")) {
          ring_buffer.supports_set_active_channels = false;
          // We need not invoke the callback: this device is in the process of being unwound.
          TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels response",
                        TRACE_SCOPE_PROCESS, "reason", "Driver error", "bitmask", channel_bitmask);
          return;
        }

        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/SetActiveChannels: success";
        TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels response", TRACE_SCOPE_PROCESS,
                      "reason", "Success", "bitmask", channel_bitmask);

        ring_buffer.supports_set_active_channels = true;
        ring_buffer.active_channels_bitmask = channel_bitmask;
        ring_buffer.set_active_channels_completed_at = zx::time(result->set_time());
        callback(zx::ok(*ring_buffer.set_active_channels_completed_at));
        LogActiveChannels(*ring_buffer.active_channels_bitmask,
                          *ring_buffer.set_active_channels_completed_at);

        // This completion can theoretically execute while the device is in the midst of error or
        // removal, so we only chronicle this SetActiveChannels call if inspect_instance exists.
        if (ring_buffer.inspect_instance) {
          ring_buffer.inspect_instance->RecordSetActiveChannelsCall(
              channel_bitmask, zx::clock::get_monotonic(),
              *ring_buffer.set_active_channels_completed_at);
        } else {
          ADR_WARN_OBJECT() << "cannot RecordSetActiveChannelsCall";
        }
      });
  TRACE_INSTANT("power-audio", "ADR::Device::SetActiveChannels exit", TRACE_SCOPE_PROCESS, "reason",
                "Waiting for async response", "bitmask", channel_bitmask);
  return true;
}

// Expects the caller to have already created the RingBuffer, and checked if it has already
// started or if a Start request is already pending.
void Device::StartRingBuffer(ElementId element_id,
                             fit::callback<void(zx::result<zx::time>)> start_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot StartRingBuffer";
    start_callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot StartRingBuffer";
    start_callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke start_callback: this device is in the process of being unwound. The
    // client has already received a HasError notification for this device.
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogRingBufferFidlCalls);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());

  SetCommandTimeout(kDefaultShortCmdTimeout, "RingBuffer::Start");
  ring_buffer.ring_buffer_client.value()->Start().Then(
      [this, &ring_buffer, element_id,
       callback = std::move(start_callback)](fidl::Result<fha::RingBuffer::Start>& result) mutable {
        ClearCommandTimeout();

        if (SetDeviceErrorOnFidlFrameworkError(result, "RingBuffer/Start response")) {
          // We need not invoke start_callback: this device is in the process of being unwound.
          return;
        }
        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/Start: success";

        ring_buffer.start_time = zx::time(result->start_time());
        callback(zx::ok(*ring_buffer.start_time));
        SetRingBufferState(element_id, RingBufferState::Started);

        // This completion can theoretically execute while the device is in the midst of error or
        // removal, so we only capture the 'started_at' timestamp if inspect_instance exists.
        if (ring_buffer.inspect_instance) {
          ring_buffer.inspect_instance->RecordStartTime(*ring_buffer.start_time);
        } else {
          ADR_WARN_OBJECT() << "cannot RecordStartTime";
        }
      });
}

// Expects the caller to have already created the RingBuffer, and checked if it is already
// stopped or if a Stop request is already pending.
void Device::StopRingBuffer(ElementId element_id, fit::callback<void(zx_status_t)> stop_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot StopRingBuffer";
    stop_callback(ZX_ERR_INTERNAL);
    return;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot StopRingBuffer";
    stop_callback(ZX_ERR_INTERNAL);
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke stop_callback: this device is in the process of being unwound. The
    // client has already received a HasError notification for this device.
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogRingBufferFidlCalls);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.ring_buffer_client.has_value() &&
           ring_buffer.ring_buffer_client->is_valid());

  SetCommandTimeout(kDefaultShortCmdTimeout, "RingBuffer::Stop");
  ring_buffer.ring_buffer_client.value()->Stop().Then(
      [this, &ring_buffer, element_id,
       callback = std::move(stop_callback)](fidl::Result<fha::RingBuffer::Stop>& result) mutable {
        ClearCommandTimeout();

        if (SetDeviceErrorOnFidlFrameworkError(result, "RingBuffer/Stop response")) {
          // We need not invoke the callback: this device is in the process of being unwound.
          return;
        }
        ADR_LOG_OBJECT(kLogRingBufferFidlResponses) << "RingBuffer/Stop: success";

        auto now = zx::clock::get_monotonic();
        ring_buffer.start_time.reset();
        callback(ZX_OK);
        SetRingBufferState(element_id, RingBufferState::Stopped);

        // This completion can theoretically execute while the device is in the midst of error or
        // removal, so we only capture the 'stopped_at' timestamp if inspect_instance exists.
        if (ring_buffer.inspect_instance) {
          ring_buffer.inspect_instance->RecordStopTime(now);
        } else {
          ADR_WARN_OBJECT() << "cannot RecordStopTime";
        }
      });
}

// Expects the caller to have already created the PacketStream, and checked if it has already
// started or if a Start request is already pending.
void Device::StartPacketStream(ElementId element_id,
                               fit::callback<void(zx::result<>)> start_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot StartPacketStream";
    start_callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot StartPacketStream";
    start_callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke start_callback: this device is in the process of being unwound. The
    // client has already received a HasError notification for this device.
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogPacketStreamFidlCalls);

  auto& packet_stream = packet_stream_map_.find(element_id)->second;
  FX_CHECK(packet_stream.packet_stream_client.has_value() &&
           packet_stream.packet_stream_client->is_valid());

  if (!packet_stream.configured_buffer_type.has_value()) {
    // Props must be present; supported buffer types must be present and include INLINE.
    if (!(packet_stream.packet_stream_properties.has_value() &&
          packet_stream.packet_stream_properties->supported_buffer_types().has_value() &&
          *packet_stream.packet_stream_properties->supported_buffer_types() &
              fha::BufferType::kInline)) {
      ADR_WARN_METHOD() << "PacketStream " << element_id
                        << " is not configured and does not support INLINE buffers.";
      start_callback(zx::error(ZX_ERR_BAD_STATE));
      return;
    }
  }

  SetCommandTimeout(kDefaultShortCmdTimeout, "PacketStream::Start");
  packet_stream.packet_stream_client.value()->Start().Then(
      [this, &packet_stream, element_id, callback = std::move(start_callback)](
          fidl::Result<fha::PacketStreamControl::Start>& result) mutable {
        ClearCommandTimeout();

        if (SetDeviceErrorOnFidlFrameworkError(result, "PacketStream/Start response")) {
          // We need not invoke start_callback: this device is in the process of being unwound.
          return;
        }

        if (result.is_error()) {
          // Domain error is zx_status_t
          ADR_WARN_OBJECT() << "PacketStreamControl/Start failed: " << result.error_value();
          if (result.error_value().is_domain_error()) {
            callback(zx::error(result.error_value().domain_error()));
          } else {
            callback(zx::error(ZX_ERR_INTERNAL));
          }
          return;
        }

        ADR_LOG_OBJECT(kLogPacketStreamFidlResponses) << "PacketStreamControl/Start: success";

        packet_stream.start_time = zx::clock::get_monotonic();
        // TODO(https://fxbug.dev/470157894): use a more accurate start time (e.g. from server)
        callback(zx::ok());
        SetPacketStreamState(element_id, PacketStreamState::Started);

        // This completion can theoretically execute while the device is in the midst of error or
        // removal, so we only capture the 'started_at' timestamp if inspect_instance exists.
        if (packet_stream.inspect_instance) {
          packet_stream.inspect_instance->RecordStartTime(*packet_stream.start_time);
        } else {
          ADR_WARN_OBJECT() << "cannot RecordStartTime";
        }
      });
}

// Expects the caller to have already created the PacketStream, and checked if it is already
// stopped or if a Stop request is already pending.
void Device::StopPacketStream(ElementId element_id,
                              fit::callback<void(zx_status_t)> stop_callback) {
  if (!GetControlNotify()) {
    ADR_WARN_METHOD() << "Device is not yet controlled: cannot StopPacketStream";
    stop_callback(ZX_ERR_INTERNAL);
    return;
  }

  if (!is_composite()) {
    ADR_WARN_METHOD() << "Incorrect device_type " << device_type_ << ": cannot StopPacketStream";
    stop_callback(ZX_ERR_INTERNAL);
    return;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device already has an error";
    // We need not invoke stop_callback: this device is in the process of being unwound. The
    // client has already received a HasError notification for this device.
    return;
  }
  FX_CHECK(is_operational());
  ADR_LOG_METHOD(kLogPacketStreamFidlCalls);

  auto& packet_stream = packet_stream_map_.find(element_id)->second;
  FX_CHECK(packet_stream.packet_stream_client.has_value() &&
           packet_stream.packet_stream_client->is_valid());

  SetCommandTimeout(kDefaultShortCmdTimeout, "PacketStream::Stop");
  // TODO(https://fxbug.dev/470157894): update start_time or related metrics as needed.
  packet_stream.packet_stream_client.value()->Stop().Then(
      [this, &packet_stream, element_id, callback = std::move(stop_callback)](
          fidl::Result<fha::PacketStreamControl::Stop>& result) mutable {
        ClearCommandTimeout();

        if (SetDeviceErrorOnFidlFrameworkError(result, "PacketStreamControl/Stop response")) {
          // We need not invoke the callback: this device is in the process of being unwound.
          return;
        }
        if (result.is_error()) {
          ADR_WARN_OBJECT() << "PacketStreamControl/Stop failed: " << result.error_value();
          if (result.error_value().is_domain_error()) {
            callback(result.error_value().domain_error());
          } else {
            callback(ZX_ERR_INTERNAL);
          }
          return;
        }

        ADR_LOG_OBJECT(kLogPacketStreamFidlResponses) << "PacketStreamControl/Stop: success";

        auto now = zx::clock::get_monotonic();
        if (packet_stream.start_time) {
          packet_stream.start_time.reset();
        } else {
          ADR_WARN_OBJECT() << "PacketStream Stop received but not started";
        }

        callback(ZX_OK);
        SetPacketStreamState(element_id, PacketStreamState::Stopped);

        if (packet_stream.inspect_instance) {
          packet_stream.inspect_instance->RecordStopTime(now);
        } else {
          ADR_WARN_OBJECT() << "cannot RecordStopTime";
        }
      });
}

// Expects the caller to have already created the PacketStream, checked if it is already
// configured (or started), and validated the input `setup_vmo_info` arguments.
void Device::SetPacketStreamBuffers(
    ElementId element_id, fuchsia_audio_device::PacketStreamSetupVmoInfo setup_vmo_info,
    fit::callback<void(fit::result<fuchsia_audio_device::PacketStreamSetBufferError,
                                   fuchsia_audio_device::PacketStreamBuffers>)>
        set_buffers_callback) {
  ADR_LOG_METHOD(kLogPacketStreamFidlCalls);

  if (setup_vmo_info.Which() != fad::PacketStreamSetupVmoInfo::Tag::kAllocateInfo &&
      setup_vmo_info.Which() != fad::PacketStreamSetupVmoInfo::Tag::kRegisterInfo) {
    ADR_WARN_METHOD() << "Unknown fad::PacketStreamSetupVmoInfo";
    set_buffers_callback(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
    return;
  }

  fha::BufferType buffer_type;
  if (setup_vmo_info.Which() == fad::PacketStreamSetupVmoInfo::Tag::kAllocateInfo) {
    buffer_type = fha::BufferType::kDriverOwned;
  } else if (setup_vmo_info.Which() == fad::PacketStreamSetupVmoInfo::Tag::kRegisterInfo) {
    buffer_type = fha::BufferType::kClientOwned;
  }

  if (has_error()) {
    ADR_WARN_METHOD() << "device has an error";
    set_buffers_callback(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
    return;
  }
  FX_CHECK(is_composite());

  auto ps_iter = packet_stream_map_.find(element_id);
  if (ps_iter == packet_stream_map_.end()) {
    ADR_WARN_METHOD() << "PacketStream " << element_id << " not found";
    set_buffers_callback(fit::error(fad::PacketStreamSetBufferError::kInvalidElementId));
    return;
  }
  auto& ps_record = ps_iter->second;

  // Check supported buffer type.
  // Props must be present; supported buffer types must be present and include the specified type.
  if (!(ps_record.packet_stream_properties.has_value() &&
        ps_record.packet_stream_properties->supported_buffer_types().has_value() &&
        *ps_record.packet_stream_properties->supported_buffer_types() & buffer_type)) {
    ADR_WARN_METHOD() << "Buffer type " << static_cast<uint64_t>(buffer_type) << " not supported";
    set_buffers_callback(fit::error(fad::PacketStreamSetBufferError::kBadStrategy));
    return;
  }

  // Determine whether the PacketStream element is "outgoing" or "incoming" to assess the correct
  // VMO rights.
  FX_CHECK(current_topology_id_.has_value());
  bool vmo_is_incoming =
      ElementHasIncomingEdges(sig_proc_topology_map_.at(*current_topology_id_), element_id);
  // Driver allocates VMOs: client must only read incoming buffers but must write outgoing ones.
  zx_rights_t allocate_required_rights =
      vmo_is_incoming ? kRequiredVmoRightsForRead : kRequiredVmoRightsForReadWrite;
  // Client allocates (driver registers) VMOs: driver must write incoming but only read outgoing.
  zx_rights_t register_required_rights =
      vmo_is_incoming ? kRequiredVmoRightsForReadWrite : kRequiredVmoRightsForRead;

  if (setup_vmo_info.Which() == fad::PacketStreamSetupVmoInfo::Tag::kAllocateInfo) {
    auto& allocate_info = setup_vmo_info.allocate_info().value();

    uint32_t expected_vmo_count = *allocate_info.vmo_count();
    uint64_t min_vmo_size = *allocate_info.min_vmo_size();

    SetCommandTimeout(kDefaultShortCmdTimeout, "AllocateVmos");
    ps_record.packet_stream_client.value()
        ->AllocateVmos(allocate_info)
        .Then([this, element_id, cb = std::move(set_buffers_callback), buffer_type,
               allocate_required_rights, expected_vmo_count,
               min_vmo_size](fidl::Result<fha::PacketStreamControl::AllocateVmos>& result) mutable {
          ClearCommandTimeout();

          if (has_error()) {
            return;
          }
          if (SetDeviceErrorOnFidlFrameworkError(result, "AllocateVmos response")) {
            cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
            return;
          }

          if (result.is_error()) {
            auto err = result.error_value();
            ADR_WARN_OBJECT() << "AllocateVmos failed: " << err;
            if (err.is_domain_error() && err.domain_error() == ZX_ERR_INVALID_ARGS) {
              cb(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
            } else if (err.is_domain_error() && err.domain_error() == ZX_ERR_BAD_STATE) {
              cb(fit::error(fad::PacketStreamSetBufferError::kAlreadyConfigured));
            } else {
              OnError(err.domain_error());
              cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
            }
            return;
          }

          if (result.value().vmos().size() != expected_vmo_count) {
            ADR_WARN_OBJECT() << "AllocateVmos returned " << result.value().vmos().size()
                              << " VMOs, expected " << expected_vmo_count;

            OnError(ZX_ERR_INTERNAL);
            cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
            return;
          }

          for (uint32_t i = 0; i < result.value().vmos().size(); ++i) {
            if (auto status = ValidatePacketStreamVmo(result.value().vmos()[i],
                                                      allocate_required_rights, min_vmo_size);
                status != ZX_OK) {
              ADR_WARN_OBJECT() << "AllocateVmos returned invalid VMO at index " << i << ": "
                                << status;
              OnError(status);
              cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
              return;
            }
          }

          // Success
          auto& ps = packet_stream_map_[element_id];
          ps.configured_buffer_type = buffer_type;

          if (ps.inspect_instance) {
            ps.inspect_instance->RecordBuffer(buffer_type, result.value().vmos());
          }

          fad::PacketStreamBuffers buffers;
          buffers.vmo_infos(std::move(result.value().vmos()));
          cb(fit::success(std::move(buffers)));
        });
  }

  if (setup_vmo_info.Which() == fad::PacketStreamSetupVmoInfo::Tag::kRegisterInfo) {
    auto& register_info = setup_vmo_info.register_info().value();
    for (uint32_t i = 0; i < register_info.vmo_infos()->size(); ++i) {
      auto& vmo_info = register_info.vmo_infos()->at(i);
      if (auto status = ValidatePacketStreamVmo(vmo_info, register_required_rights, 0);
          status != ZX_OK) {
        ADR_WARN_METHOD() << "SetupVmoInfo::RegisterInfo Config contains invalid VMO at index "
                          << i;
        auto error = fad::PacketStreamSetBufferError::kBadVmoConfig;
        if (status == ZX_ERR_ACCESS_DENIED) {
          error = fad::PacketStreamSetBufferError::kInsufficientRights;
        } else if (status == ZX_ERR_BUFFER_TOO_SMALL) {
          error = fad::PacketStreamSetBufferError::kBufferTooSmall;
        }
        set_buffers_callback(fit::error(error));
        return;
      }
      // Harden the handle before it crosses into driver_host: replace it with a
      // duplicate carrying only the rights the driver actually needs (no RESIZE,
      // no SET_PROPERTY), so the client cannot shrink/grow the VMO under the
      // driver's recorded mapping size.
      zx::vmo rights_reduced;
      if (auto s = vmo_info.vmo()->replace(register_required_rights | ZX_RIGHT_DUPLICATE,
                                           &rights_reduced);
          s != ZX_OK) {
        set_buffers_callback(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
        return;
      }
      vmo_info.vmo(std::move(rights_reduced));
    }

    auto vmo_infos_to_register = std::move(*register_info.vmo_infos());
    fha::RegisterVmosConfig config{{.vmo_infos = std::move(vmo_infos_to_register)}};
    // We need to capture the VMOs to record them in the callback.
    // However, we just moved them into `config`. So we must duplicate them if we want to keep a
    // copy.
    std::vector<fha::VmoInfo> vmo_infos_copy;
    for (const auto& vmo_info : *config.vmo_infos()) {
      zx::vmo duplicated_vmo;
      vmo_info.vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicated_vmo);
      vmo_infos_copy.push_back(
          fha::VmoInfo{{.id = vmo_info.id(), .vmo = std::move(duplicated_vmo)}});
    }

    SetCommandTimeout(kDefaultShortCmdTimeout, "RegisterVmos");
    ps_record.packet_stream_client.value()
        ->RegisterVmos(std::move(config))
        .Then([this, element_id, cb = std::move(set_buffers_callback), buffer_type,
               vmo_infos = std::move(vmo_infos_copy)](
                  fidl::Result<fha::PacketStreamControl::RegisterVmos>& result) mutable {
          ClearCommandTimeout();

          if (has_error()) {
            return;
          }
          if (SetDeviceErrorOnFidlFrameworkError(result, "RegisterVmos response")) {
            cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
            return;
          }

          if (result.is_error()) {
            auto err = result.error_value();
            ADR_WARN_OBJECT() << "RegisterVmos failed: " << err;
            if (err.is_domain_error() && err.domain_error() == ZX_ERR_INVALID_ARGS) {
              cb(fit::error(fad::PacketStreamSetBufferError::kBadVmoConfig));
            } else if (err.is_domain_error() && err.domain_error() == ZX_ERR_BAD_STATE) {
              cb(fit::error(fad::PacketStreamSetBufferError::kAlreadyConfigured));
            } else {
              OnError(err.domain_error());
              cb(fit::error(fad::PacketStreamSetBufferError::kDeviceError));
            }
            return;
          }

          // Success
          auto& ps = packet_stream_map_[element_id];
          ps.configured_buffer_type = buffer_type;

          if (ps.inspect_instance) {
            ps.inspect_instance->RecordBuffer(buffer_type, vmo_infos);
          }

          fad::PacketStreamBuffers buffers;
          buffers.vmo_infos(std::move(vmo_infos));
          cb(fit::success(std::move(buffers)));
        });
  }
}

// Uses the VMO format and ring buffer properties, to set bytes_per_frame,
// requested_ring_buffer_frames, ring_buffer_consumer_bytes and ring_buffer_producer_bytes.
void Device::CalculateRequiredRingBufferSizes(ElementId element_id) {
  ADR_LOG_METHOD(kLogRingBufferMethods);

  auto& ring_buffer = ring_buffer_map_.find(element_id)->second;
  FX_CHECK(ring_buffer.vmo_format.channel_count().has_value());
  FX_CHECK(ring_buffer.vmo_format.sample_type().has_value());
  FX_CHECK(ring_buffer.vmo_format.frames_per_second().has_value());
  FX_CHECK(ring_buffer.ring_buffer_properties.has_value());
  FX_CHECK(ring_buffer.ring_buffer_properties->driver_transfer_bytes().has_value());
  FX_CHECK(ring_buffer.requested_ring_buffer_bytes.has_value());
  FX_CHECK(*ring_buffer.requested_ring_buffer_bytes > 0);

  switch (*ring_buffer.vmo_format.sample_type()) {
    case fuchsia_audio::SampleType::kUint8:
      ring_buffer.bytes_per_frame = 1;
      break;
    case fuchsia_audio::SampleType::kInt16:
      ring_buffer.bytes_per_frame = 2;
      break;
    case fuchsia_audio::SampleType::kInt32:
    case fuchsia_audio::SampleType::kFloat32:
      ring_buffer.bytes_per_frame = 4;
      break;
    case fuchsia_audio::SampleType::kFloat64:
      ring_buffer.bytes_per_frame = 8;
      break;
    default:
      FX_LOGS(FATAL) << __func__ << ": unknown fuchsia_audio::SampleType";
      __UNREACHABLE;
  }
  ring_buffer.bytes_per_frame *= *ring_buffer.vmo_format.channel_count();

  ring_buffer.requested_ring_buffer_frames =
      media::TimelineRate{1, ring_buffer.bytes_per_frame}.Scale(
          *ring_buffer.requested_ring_buffer_bytes, media::TimelineRate::RoundingMode::Ceiling);

  // Determine whether the RingBuffer client is a Producer or a Consumer.
  // If the RingBuffer element is "outgoing" (if it is a source in Topology edges), then the client
  // indeed _produces_ the frames that populate the RingBuffer.
  bool element_is_outgoing = false, element_is_incoming = false;
  FX_CHECK(current_topology_id_.has_value());
  auto topology_match = sig_proc_topology_map_.find(*current_topology_id_);
  FX_CHECK(topology_match != sig_proc_topology_map_.end());
  element_is_outgoing = ElementHasOutgoingEdges(topology_match->second, element_id);
  element_is_incoming = ElementHasIncomingEdges(topology_match->second, element_id);
  FX_CHECK(element_is_outgoing != element_is_incoming);

  // We don't include driver transfer size in our VMO size request (requested_ring_buffer_frames_)
  // ... but we do communicate it in our description of ring buffer producer/consumer "zones".
  uint64_t driver_bytes = *ring_buffer.ring_buffer_properties->driver_transfer_bytes() +
                          ring_buffer.bytes_per_frame - 1;
  driver_bytes -= (driver_bytes % ring_buffer.bytes_per_frame);

  // The client memory zone is only an estimate; we'll update it based on what the driver returns.
  if (element_is_outgoing) {
    ring_buffer.ring_buffer_consumer_bytes = driver_bytes;
    ring_buffer.ring_buffer_producer_bytes =
        ring_buffer.requested_ring_buffer_frames * ring_buffer.bytes_per_frame;
  } else {
    ring_buffer.ring_buffer_producer_bytes = driver_bytes;
    ring_buffer.ring_buffer_consumer_bytes =
        ring_buffer.requested_ring_buffer_frames * ring_buffer.bytes_per_frame;
  }

  // TODO(https://fxbug.dev/42069012): validate this case; we don't surface an error to the caller.
  if (ring_buffer.requested_ring_buffer_frames > std::numeric_limits<uint32_t>::max()) {
    ADR_WARN_METHOD() << "requested_ring_buffer_frames cannot exceed uint32_t::max()";
    ring_buffer.requested_ring_buffer_frames = std::numeric_limits<uint32_t>::max();
  }
}

// TODO(https://fxbug.dev/42069013): implement, via RingBuffer/WatchClockRecoveryPositionInfo.
void Device::RecoverDeviceClockFromPositionInfo() { ADR_LOG_METHOD(kLogRingBufferMethods); }

// TODO(https://fxbug.dev/42069013): implement this.
void Device::StopDeviceClockRecovery() { ADR_LOG_METHOD(kLogRingBufferMethods); }

}  // namespace media_audio
